// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 运行循环：把状态机的指令真正执行掉。
//!
//! 循环本身很薄 —— 所有判断逻辑都在 `repl-core::machine` 里（那部分可以离线回放测试），
//! 这里只负责"接指令 → 干活 → 回报"。

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use repl_core::{
    machine::{AbortReason, Command, Completion, FrameView, Machine, Phase},
    Copilot,
};
use repl_frames::{FrameError, FrameSource, Snapshot};
use repl_input::{precise_sleep, TimerResolution};

use crate::{config::Config, session::Session};

/// 编队绑定回调：拿到会话去做识别和交互，返回 `true` 表示绑定完成。
type Binder<'a> = Box<dyn FnMut(&mut Session) -> Result<bool> + 'a>;

/// 编队绑定面板需要展示的一张卡片。
#[derive(Clone, Debug)]
pub struct BindingCardInfo {
    pub index: usize,
    /// 职业中文名。
    pub role: String,
    pub available: bool,
    pub cooling: bool,
}

/// 运行期发给 UI 的进度事件。
#[derive(Clone, Debug)]
pub enum Progress {
    Phase {
        phase: Phase,
        cursor: i64,
        target: Option<i64>,
    },
    /// 需要用户完成编队绑定。带上识别出的卡片和待绑定的干员名 ——
    /// UI 的全部展示内容都来自这条事件，别的地方不维护绑定状态。
    NeedsBinding {
        cards: Vec<BindingCardInfo>,
        opers: Vec<String>,
    },
    ActionStarted {
        index: usize,
        frame: i64,
    },
    ActionDone {
        index: usize,
    },
    Log(String),
    Finished {
        pulses: u32,
        zero_pulses: u32,
        overshoots: u32,
    },
    Failed(String),
}

/// 等待开局的总超时。
const ARM_TIMEOUT: Duration = Duration::from_secs(300);

/// 逐帧推进阶段每次脉冲前的等待时长。
///
/// 目的是让游戏在下一次解暂停前有足够的稳定时间：选中 UI 动画完成、
/// 尺子读数稳定、费用条不再抖动。1000ms = 约 30 帧的缓冲，足够彻底稳定。
/// 代价：每次推进约 1060ms，FRAME_LEAD=8 时每个动作最多需要 ~9 秒对帧。
const SLOW_STEP_PAUSE: Duration = Duration::from_millis(1000);

/// 到达目标帧后、注入动作前的等待时长。
///
/// 给游戏时间确认"已暂停、已停在目标帧"的状态，再开始触控和键盘注入。
/// 对 Skill/Retreat：这段时间内尺子快照稳定，select_while_paused 后的
/// 选中状态也在这里确认；key_tap 在此之后发出，游戏必定已收到选中。
const PRE_ACTION_PAUSE: Duration = Duration::from_secs(2);

/// 一次复刻运行。
pub struct Runner<'a> {
    machine: Machine,
    session: &'a mut Session,
    frames: &'a dyn FrameSource,
    events: mpsc::Sender<Progress>,
    cancel: Arc<AtomicBool>,
    binder: Binder<'a>,
}

impl<'a> Runner<'a> {
    pub fn new(
        copilot: Copilot,
        config: &Config,
        session: &'a mut Session,
        frames: &'a dyn FrameSource,
        events: mpsc::Sender<Progress>,
        cancel: Arc<AtomicBool>,
        binder: Binder<'a>,
    ) -> Self {
        Self {
            machine: Machine::with_gap(copilot, config.initial_gap_ms),
            session,
            frames,
            events,
            cancel,
            binder,
        }
    }

    /// 跑完整场复刻。
    ///
    /// 整个过程持有 1ms 定时器精度 —— 这是进程级全局设置，会略微拉高整机功耗，
    /// 所以只在真正做帧操作的时段持有，跑完立刻释放。
    pub fn run(&mut self) -> Result<()> {
        let _timer = TimerResolution::acquire();
        self.machine.start();

        let result = self.pump();

        // 无论成败都把可能卡住的触点抬起来。
        self.session.release_inputs();

        match &result {
            Ok(()) => {
                let t = self.machine.tuner();
                let _ = self.events.send(Progress::Finished {
                    pulses: t.pulses,
                    zero_pulses: t.zero_frame_pulses,
                    overshoots: t.overshoot_pulses,
                });
            }
            Err(e) => {
                let _ = self.events.send(Progress::Failed(e.to_string()));
            }
        }
        result
    }

    fn pump(&mut self) -> Result<()> {
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                self.machine.user_abort();
            }
            self.report_phase();

            match self.machine.next_command() {
                Command::Idle => return Ok(()),

                Command::Abort(reason) => return Err(anyhow!("{reason}")),

                Command::ArmBattleStart => self.arm_battle_start()?,

                Command::Pause => {
                    self.session.pause.pause()?;
                    self.machine.completed(Completion::PauseSent);
                }

                Command::Resume => {
                    self.session.pause.resume()?;
                    self.machine.completed(Completion::ResumeSent);
                }

                Command::Pulse { gap } => {
                    // 逐帧推进阶段：每次脉冲前先等 SLOW_STEP_PAUSE。
                    // 这给游戏和尺子充足的稳定时间，避免在状态未稳定时解暂停
                    // （选中 UI 动画、费用条抖动等都会在这里安全地结束）。
                    if self.machine.phase() == Phase::Stepping {
                        precise_sleep(SLOW_STEP_PAUSE);
                    }
                    self.session.pause.pulse(gap)?;
                    self.machine.completed(Completion::PulseSent);
                }

                Command::AwaitFrame { timeout } => self.await_frame(timeout)?,

                Command::BindFormation => {
                    // NeedsBinding 事件由 binder 自己发（只有它知道扫出了哪些卡片）。
                    let done = (self.binder)(self.session)?;
                    if !done {
                        return Err(anyhow!("编队绑定未完成，复刻中止"));
                    }
                    // 用户刚在我们的窗口里点过"确认"，鼠标位置不可控 ——
                    // 停回安全区，防止它落在费用条上把尺子的分析冻住。
                    self.session.park_cursor();
                    self.machine.completed(Completion::FormationBound);
                }

                Command::Execute { index } => {
                    // 到达目标帧后，先稳定等待再注入，让游戏确认暂停状态。
                    precise_sleep(PRE_ACTION_PAUSE);
                    let action = self.machine.copilot().actions[index].clone();
                    let _ = self.events.send(Progress::ActionStarted {
                        index,
                        frame: action.frame,
                    });
                    match self.session.execute(&action) {
                        Ok(()) => {
                            let _ = self.events.send(Progress::ActionDone { index });
                            self.machine.completed(Completion::ActionExecuted);
                        }
                        Err(e) => {
                            self.machine.execution_failed(e.to_string());
                            return Err(e);
                        }
                    }
                }

                Command::Finish { resume } => {
                    if resume {
                        self.session.pause.resume()?;
                    }
                    let _ = self.events.send(Progress::Log(if resume {
                        "全部动作已注入，已恢复运行".into()
                    } else {
                        "全部动作已注入，游戏保持暂停".into()
                    }));
                    return Ok(());
                }
            }
        }
    }

    fn report_phase(&self) {
        let _ = self.events.send(Progress::Phase {
            phase: self.machine.phase(),
            cursor: self.machine.cursor(),
            target: self.machine.target(),
        });
    }

    /// 发一条既进 UI 日志面板、也进日志文件的诊断信息。
    ///
    /// GUI 的 release 构建没有控制台，UI 面板的内容又没法事后取证 ——
    /// 关键诊断必须两边都写。
    fn note(&self, message: String) {
        log::info!("{message}");
        let _ = self.events.send(Progress::Log(message));
    }

    /// 等待战斗开始，并在合适的时机发暂停。
    ///
    /// **暂停的唯一触发条件：尺子给出第一条可信的战斗内样本** ——
    /// 即费用条已经被真正读到了。这一条同时排除了三类踩过的坑：
    ///
    /// 1. 像素触发在准备界面 / 加载画面上误开火（ESC 按了个寂寞，
    ///    后面等"暂停态样本"全部落空 —— 实测发生过）；
    /// 2. HUD 刚出现就暂停，但开局标题卡还盖着费用条：暂停把渐隐动画一起冻住，
    ///    尺子永远读不到费用条；
    /// 3. 尺子那头因为任何原因还没就绪，我们却已经把游戏停住了。
    ///
    /// 代价是暂停比 HUD 出现晚 2–4 个逻辑帧（尺子分析 + 我们的轮询延迟）——
    /// 状态机本来就接受 `origin <= 首个动作帧`，示例作业首个动作在第 10 帧，兜得住。
    ///
    /// 像素检测（倍速按钮区变白）降级为纯进度提示，不再驱动任何按键。
    fn arm_battle_start(&mut self) -> Result<()> {
        // 防呆：如果此刻已经身处战斗中（比如刚跑完 step-test 没退出来），
        // 后续的"开局暂停"语义就全错了。明确拒绝，让用户从关卡准备界面开始。
        if let Some(snapshot) = self.frames.latest() {
            let view = to_view(&snapshot);
            if view.trustworthy && view.in_battle {
                return Err(anyhow!(
                    "看起来已经在战斗中（{}，第 {} 帧）。请先退出本场战斗，\
                     从关卡准备界面点「开始复刻」再进关",
                    view.battle_state,
                    view.elapsed
                ));
            }
        }

        self.note("等待开局中，请进入关卡…".into());
        let deadline = Instant::now() + ARM_TIMEOUT;
        // 我们自己看到战斗画面的时刻 + 当时尺子的分析帧号。
        // 两者配合是尺子健康度的试金石：战斗画面在动，尺子的 frameId 就必须在涨。
        let mut hud_seen: Option<(Instant, u64)> = None;
        /// 看到战斗画面后给尺子的宽限期。
        const RULER_GRACE: Duration = Duration::from_secs(10);

        while Instant::now() < deadline {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("{}", AbortReason::UserAborted));
            }

            // 进度提示：看到战斗画面的迹象就告诉用户"快了"。
            // 只做提示，不驱动按键；截图失败（静止画面上 WGC 不产帧）也无所谓。
            if hud_seen.is_none() {
                if let Ok(frame) = self.session.capture() {
                    if speed_box_lit(&frame) {
                        let ruler_id = self.frames.latest().and_then(|s| s.frame_id).unwrap_or(0);
                        hud_seen = Some((Instant::now(), ruler_id));
                        self.note("检测到战斗画面，等待尺子读到费用条后暂停…".into());
                    }
                }
            }

            // 尺子健康检查：战斗画面已经出现（屏幕在动、WGC 在产帧），
            // 尺子却迟迟给不出可信的战斗内样本 —— 按帧号是否在涨分两种病，
            // 各给一条能直接照做的处置指引，而不是笼统的"等待超时"。
            if let Some((since, id_at_hud)) = hud_seen {
                if since.elapsed() > RULER_GRACE {
                    let latest = self.frames.latest();
                    let now_id = latest.as_ref().and_then(|s| s.frame_id).unwrap_or(0);
                    let state = latest.as_ref().map_or("<无快照>".to_owned(), |s| {
                        s.battle_state_str().to_owned()
                    });
                    if now_id == id_at_hud {
                        return Err(anyhow!(
                            "战斗画面已出现 {} 秒，但尺子的分析帧号一直卡在 {now_id} 不动 \
                             —— 它的截图管线已经停滞（通常是游戏窗口重建后尺子还盯着旧窗口）。\
                             请重启尺子（保持游戏开着），确认其悬浮窗的帧数在战斗中会走动，再重试",
                            RULER_GRACE.as_secs()
                        ));
                    }
                    return Err(anyhow!(
                        "战斗画面已出现 {} 秒，尺子在分析（帧号 {id_at_hud} → {now_id}）\
                         但始终没有给出可信的战斗内样本（当前 state={state}）。\
                         检查尺子的捕获目标是否是游戏窗口、校准配置是否匹配当前分辨率",
                        RULER_GRACE.as_secs()
                    ));
                }
            }

            // 唯一的暂停触发：尺子的第一条可信战斗内样本。
            // 用 wait_next（而不是 latest()）让尺子客户端进入主动轮询模式，
            // 不依赖"状态变化才推送"的语义。
            match self
                .frames
                .wait_next(self.machine.last_frame_id(), Duration::from_millis(100))
            {
                Ok(snapshot) => {
                    let view = to_view(&snapshot);
                    let accepted = self.machine.observe(&view);
                    if self.machine.phase() == Phase::Aborted {
                        return Err(anyhow!("{}", self.machine.abort_reason().unwrap()));
                    }
                    if accepted && view.in_battle {
                        log::info!(
                            "battle start confirmed by ruler: state={} elapsed={} frame_id={}",
                            view.battle_state,
                            view.elapsed,
                            view.frame_id
                        );
                        self.session.pause.pause()?;
                        self.machine.completed(Completion::PauseSent);
                        // 用户刚点过"开始行动"，鼠标还停在按钮那儿；从现在起帧数
                        // 完全依赖尺子读费用条，必须立刻把鼠标挪去安全区。
                        self.session.park_cursor();
                        return Ok(());
                    }
                }
                Err(FrameError::Timeout(_)) => {}
                Err(e) => return Err(anyhow!("{e}")),
            }
        }
        Err(anyhow!("等待开局超时（5 分钟）"))
    }

    /// 等一条新的、可信的尺子样本。
    fn await_frame(&mut self, timeout: Duration) -> Result<()> {
        let mut deadline = Instant::now() + timeout;
        // 鼠标遮挡费用条导致的等待失败可以自愈一次：挪开鼠标、重置期限重等。
        let mut parked_for_cursor = false;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                self.machine.user_abort();
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                // 超时诊断：把尺子最新快照的关键字段原样交给用户。
                // GUI 没有控制台，这是用户唯一能看到"当时到底发生了什么"的通道。
                match self.frames.latest() {
                    Some(s) => {
                        self.note(format!(
                            "等待帧超时诊断：尺子最新快照 frame_id={:?} state={} \
                             isRunning={} currentFrame={:?} cursorBlocked={} elapsed={}；\
                             状态机游标 frame_id={} 绝对帧={}（阶段 {}）",
                            s.frame_id,
                            s.battle_state_str(),
                            s.is_running,
                            s.current_frame,
                            s.cursor_blocked,
                            s.total_elapsed_frames,
                            self.machine.last_frame_id(),
                            self.machine.cursor(),
                            self.machine.phase().zh(),
                        ));
                        if s.cursor_blocked && !parked_for_cursor {
                            parked_for_cursor = true;
                            self.session.park_cursor();
                            self.note("检测到鼠标正遮挡费用条，已自动移到安全区并重试".into());
                            deadline = Instant::now() + timeout;
                            continue;
                        }
                    }
                    None => {
                        self.note(format!(
                            "等待帧超时：尚未从尺子收到任何快照（连接状态 {:?}）",
                            self.frames.status()
                        ));
                    }
                }
                self.machine.frame_timeout();
                return Ok(());
            }
            // 必须传状态机的帧游标，不能传 0：传 0 时缓存旧快照立刻满足条件，
            // wait_next 永不阻塞，尺子客户端也就永远不进入主动轮询模式。
            // 空脉冲（解暂停→暂停发生在同一渲染 tick 内）不产生任何状态变化、
            // 尺子不会推送 —— 没有轮询就只能干等到超时中止。
            match self.frames.wait_next(
                self.machine.last_frame_id(),
                remaining.min(Duration::from_millis(200)),
            ) {
                Ok(snapshot) => {
                    let view = to_view(&snapshot);
                    let accepted = self.machine.observe(&view);
                    if self.machine.phase() == Phase::Aborted {
                        return Ok(()); // 下一轮 next_command 会返回 Abort
                    }
                    if accepted {
                        return Ok(());
                    }
                    // 样本被丢弃（重复或不可信），接着等。写进日志文件（debug 级），
                    // 让"2 秒里到底拒了什么"事后有据可查。
                    log::debug!(
                        "sample rejected: frame_id={} state={} trustworthy={} \
                         elapsed={} (machine cursor frame_id={})",
                        view.frame_id,
                        view.battle_state,
                        view.trustworthy,
                        view.elapsed,
                        self.machine.last_frame_id(),
                    );
                }
                Err(FrameError::Timeout(_)) => continue,
                Err(e) => return Err(anyhow!("{e}")),
            }
        }
    }
}

/// 倍速按钮区域是否出现近白像素（AFA `SpeedButtonPositionColor` 的采样区）。
///
/// 只是**粗筛**：准备界面 / 加载画面上的白色元素也会命中，必须再过
/// [`repl_vision::battle_hud_visible`] 的硬确认才能当作开局。
fn speed_box_lit(frame: &repl_capture::Frame) -> bool {
    const LEFT: f64 = 0.8450;
    const RIGHT: f64 = 0.8807;
    const TOP: f64 = 0.0713;
    const BOTTOM: f64 = 0.0870;
    /// AFA 的 PixelSearch 容差。
    const TOLERANCE: i32 = 10;

    let w = f64::from(frame.width);
    let h = f64::from(frame.height);
    let (x0, x1) = ((w * LEFT) as i32, (w * RIGHT) as i32);
    let (y0, y1) = ((h * TOP) as i32, (h * BOTTOM) as i32);

    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            if let Some([r, g, b]) = frame.rgb(x, y) {
                if i32::from(r) >= 255 - TOLERANCE
                    && i32::from(g) >= 255 - TOLERANCE
                    && i32::from(b) >= 255 - TOLERANCE
                {
                    return true;
                }
            }
        }
    }
    false
}

/// 尺子快照 → 状态机视图。
pub fn to_view(snapshot: &Snapshot) -> FrameView {
    let state = snapshot.battle_state.as_ref();
    FrameView {
        frame_id: snapshot.frame_id.unwrap_or(0),
        elapsed: snapshot.total_elapsed_frames,
        in_battle: state.is_some_and(repl_frames::BattleState::is_in_battle),
        paused: state.and_then(repl_frames::BattleState::is_paused),
        one_x: state.is_some_and(repl_frames::BattleState::is_one_x),
        trustworthy: snapshot.frame_reading_is_trustworthy(),
        battle_state: snapshot.battle_state_str().to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_json(state: &str, elapsed: i64, frame_id: u64, running: bool) -> Snapshot {
        serde_json::from_str(&format!(
            r#"{{"battleState":"{state}","totalElapsedFrames":{elapsed},"frameId":{frame_id},
                 "isRunning":{running},"currentFrame":3,"cursorBlocked":false}}"#
        ))
        .unwrap()
    }

    #[test]
    fn snapshot_maps_to_view() {
        let v = to_view(&snapshot_json("1x_paused", 42, 7, true));
        assert_eq!(v.elapsed, 42);
        assert_eq!(v.frame_id, 7);
        assert!(v.in_battle);
        assert_eq!(v.paused, Some(true));
        assert!(v.one_x);
        assert!(v.trustworthy);
    }

    #[test]
    fn two_x_is_flagged() {
        let v = to_view(&snapshot_json("2x_running", 10, 1, true));
        assert!(v.in_battle);
        assert!(!v.one_x, "2 倍速必须被识别出来");
        assert_eq!(v.paused, Some(false));
    }

    #[test]
    fn deploying_state_has_unknown_pause() {
        // 部署慢放时暂停按钮被遮挡，尺子给不出暂停与否
        let v = to_view(&snapshot_json("deploying_operator", 10, 1, true));
        assert!(v.in_battle);
        assert_eq!(v.paused, None);
        assert!(!v.one_x);
    }

    #[test]
    fn not_running_snapshot_is_untrustworthy() {
        let v = to_view(&snapshot_json("1x_running", 10, 1, false));
        assert!(!v.trustworthy, "isRunning=false 的读数不可信");
    }

    #[test]
    fn cursor_blocked_snapshot_is_untrustworthy() {
        let s: Snapshot = serde_json::from_str(
            r#"{"battleState":"1x_running","totalElapsedFrames":10,"frameId":1,
                "isRunning":true,"currentFrame":3,"cursorBlocked":true}"#,
        )
        .unwrap();
        assert!(!to_view(&s).trustworthy);
    }

    #[test]
    fn missing_battle_state_is_not_in_battle() {
        let s: Snapshot = serde_json::from_str(r#"{"totalElapsedFrames":0}"#).unwrap();
        let v = to_view(&s);
        assert!(!v.in_battle);
        assert!(!v.one_x);
        assert_eq!(v.paused, None);
    }
}
