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
    ActionType, Copilot,
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
const PRE_ACTION_PAUSE: Duration = Duration::from_secs(2);

/// AFA 委托或部署动作发出后，等待尺子确认动作仍停在目标帧的上限。
const ACTION_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// AFA 自动开局暂停被尺子确认后，开始编队和后续动作前的稳定等待。
const OPENING_PAUSE_SETTLE: Duration = Duration::from_secs(1);

/// 编队绑定面板关闭后，等待用户把游戏恢复到前台的上限。
const GAME_FOCUS_TIMEOUT: Duration = Duration::from_secs(15);

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
                    log::info!(
                        "dispatching AFA runtime pause: cursor={} target={:?}",
                        self.machine.cursor(),
                        self.machine.target()
                    );
                    self.session.pause_battle()?;
                    self.machine.completed(Completion::PauseSent);
                }

                Command::Resume => {
                    log::info!(
                        "dispatching AFA runtime resume: cursor={} target={:?}",
                        self.machine.cursor(),
                        self.machine.target()
                    );
                    self.session.resume_battle()?;
                    self.machine.completed(Completion::ResumeSent);
                }

                Command::Pulse { gap } => {
                    // 逐帧推进阶段：每次脉冲前先等 SLOW_STEP_PAUSE。
                    // 这给游戏和尺子充足的稳定时间，避免在状态未稳定时解暂停
                    // （选中 UI 动画、费用条抖动等都会在这里安全地结束）。
                    if self.machine.phase() == Phase::Stepping {
                        precise_sleep(SLOW_STEP_PAUSE);
                    }
                    self.session.pulse(gap)?;
                    self.machine.completed(Completion::PulseSent);
                }

                Command::AwaitFrame { timeout } => {
                    let confirming_opening_pause = self.machine.phase() == Phase::ConfirmingZero;
                    self.await_frame(timeout)?;
                    if confirming_opening_pause && self.machine.phase() == Phase::BindingFormation {
                        self.note("AFA 开局暂停已确认，等待 1 秒后继续后续操作".into());
                        precise_sleep(OPENING_PAUSE_SETTLE);
                    }
                }

                Command::BindFormation => {
                    // NeedsBinding 事件由 binder 自己发（只有它知道扫出了哪些卡片）。
                    let done = (self.binder)(self.session)?;
                    if !done {
                        return Err(anyhow!("编队绑定未完成，复刻中止"));
                    }
                    self.restore_game_foreground()?;
                    // 用户刚在我们的窗口里点过"确认"，鼠标位置不可控 ——
                    // 停回安全区，防止它落在费用条上把尺子的分析冻住。
                    self.session.park_cursor();
                    self.machine.completed(Completion::FormationBound);
                }

                Command::Execute { index } => {
                    let action = self.machine.copilot().actions[index].clone();
                    // 到达目标帧后，在稳定等待期间持续读取尺子，而不是盲等后继续使用
                    // Machine 的旧游标。尺子是绝对帧的唯一真源；若它已经越过目标帧，
                    // 必须在任何鼠标或 AFA 输入发出前中止。
                    let min_frame_id = match wait_for_action_dispatch_ready(
                        self.frames,
                        &self.cancel,
                        action.frame,
                        self.machine.last_frame_id(),
                        PRE_ACTION_PAUSE,
                        |frame_id| self.machine.acknowledge_frame_id(frame_id),
                    ) {
                        Ok(frame_id) => frame_id,
                        Err(error) => {
                            self.machine.execution_failed(error.to_string());
                            return Err(error);
                        }
                    };
                    let _ = self.events.send(Progress::ActionStarted {
                        index,
                        frame: action.frame,
                    });
                    let mut confirmation_min_frame_id = min_frame_id;
                    let execution = {
                        let frames = self.frames;
                        let machine = &mut self.machine;
                        self.session.execute(&action, || {
                            confirmation_min_frame_id = verify_action_dispatch_now(
                                frames,
                                action.frame,
                                confirmation_min_frame_id,
                                |frame_id| machine.acknowledge_frame_id(frame_id),
                            )?;
                            Ok(())
                        })
                    };
                    match execution {
                        Ok(()) => {
                            if !matches!(action.kind, ActionType::Output) {
                                if let Err(error) =
                                    self.confirm_action(action.frame, confirmation_min_frame_id)
                                {
                                    self.machine.execution_failed(error.to_string());
                                    return Err(error);
                                }
                            }
                            let _ = self.events.send(Progress::ActionDone { index });
                            self.machine.completed(Completion::ActionExecuted);
                        }
                        Err(e) => {
                            self.machine.execution_failed(e.to_string());
                            return Err(e);
                        }
                    }
                }

                Command::Finish => {
                    self.note(format!(
                        "全部动作已确认（最后目标帧 {}），不再发送任何输入",
                        self.machine.cursor()
                    ));
                    return Ok(());
                }
            }
        }
    }

    /// 绑定面板会短暂夺走前台。这里只尝试一次系统前台切换；失败后等待用户手动
    /// 点击游戏，不在动作委托阶段偷偷抢焦点或重发热键。
    fn restore_game_foreground(&mut self) -> Result<()> {
        if self.session.window.is_foreground() {
            return Ok(());
        }
        let _ = self.session.window.focus();
        let deadline = Instant::now() + GAME_FOCUS_TIMEOUT;
        while Instant::now() < deadline {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("用户在等待游戏前台时中止复刻"));
            }
            if self.session.window.is_foreground() {
                return Ok(());
            }
            precise_sleep(Duration::from_millis(100));
        }
        Err(anyhow!(
            "编队绑定后游戏窗口仍不在前台；请点击游戏窗口后重新开始"
        ))
    }

    /// 确认一次真实输入已经收尾：只接受动作派发后的新样本，且样本必须显示暂停。
    ///
    /// 注意 `Snapshot::is_running` 是尺子的费用条识别管线状态，不是战斗是否运行；
    /// 这里用 `FrameView::paused == Some(false)` 判定游戏运行态。任何运行态样本
    /// 都直接失败，不能把它当作“短暂过渡”而继续等，否则可能已经越过目标帧。
    fn confirm_action(&mut self, action_frame: i64, min_frame_id: u64) -> Result<()> {
        let (frame_id, view) = wait_for_action_confirmation(
            self.frames,
            &self.cancel,
            action_frame,
            min_frame_id,
            |frame_id| self.machine.acknowledge_frame_id(frame_id),
        )?;
        self.note(format!(
            "动作确认通过：frame_id={frame_id} state={} elapsed={action_frame}",
            view.battle_state
        ));
        Ok(())
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

    /// 等待战斗开始。开局暂停完全由 AFA 的自动开局暂停负责；这里仅用尺子确认战斗与暂停状态，
    /// 绝不发送开局暂停输入。像素检测只用于进度和尺子健康提示。
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
                        self.note("检测到战斗画面，等待 AFA 自动开局暂停…".into());
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

            // 只检测尺子的第一条可信战斗内样本；暂停输入完全由 AFA 自动开局暂停负责。
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
                            "battle start detected by ruler; waiting for AFA automatic pause: state={} elapsed={} frame_id={}",
                            view.battle_state,
                            view.elapsed,
                            view.frame_id
                        );
                        self.machine.completed(Completion::OpeningPauseDelegated);
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

/// 在动作派发前用尺子完成稳定确认。
///
/// 与派发后的确认屏障不同，这里会覆盖完整的稳定等待时段：期间出现的每条新样本
/// 都必须仍然允许在目标帧派发。最终只有尺子最新状态为可信 `1x_paused` 且绝对帧
/// 等于目标帧时才返回；返回的 `frame_id` 同时成为派发后确认的新样本水位。
fn wait_for_action_dispatch_ready<F>(
    frames: &dyn FrameSource,
    cancel: &AtomicBool,
    action_frame: i64,
    min_frame_id: u64,
    settle: Duration,
    mut on_new_sample: F,
) -> Result<u64>
where
    F: FnMut(u64),
{
    let deadline = Instant::now() + settle;
    let mut after_frame_id = min_frame_id;
    let mut ready_frame_id = None;

    if let Some(snapshot) = frames.latest() {
        if let Some(frame_id) = snapshot.frame_id {
            if frame_id > after_frame_id {
                after_frame_id = frame_id;
                on_new_sample(frame_id);
            }
            if frame_id >= min_frame_id {
                let view = to_view(&snapshot);
                ready_frame_id = classify_action_dispatch_sample(&view, action_frame)?;
            }
        }
    }

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("用户在动作派发前停止复刻"));
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // 截止瞬间再读取一次 latest，避免最后一次 wait_next 超时边界上刚好到达的
            // 新样本没有参与派发判定。
            if let Some(snapshot) = frames.latest() {
                if let Some(frame_id) = snapshot.frame_id {
                    if frame_id > after_frame_id {
                        on_new_sample(frame_id);
                    }
                    if frame_id >= min_frame_id {
                        let view = to_view(&snapshot);
                        ready_frame_id = classify_action_dispatch_sample(&view, action_frame)?;
                    }
                }
            }
            return ready_frame_id.ok_or_else(|| {
                anyhow!(
                    "动作派发前尺子未确认目标帧 {action_frame} 的可信 1x_paused 状态（min_frame_id={min_frame_id}）"
                )
            });
        }

        match frames.wait_next(after_frame_id, remaining.min(Duration::from_millis(200))) {
            Ok(snapshot) => {
                let Some(frame_id) = snapshot.frame_id else {
                    ready_frame_id = None;
                    continue;
                };
                if frame_id <= after_frame_id {
                    continue;
                }
                after_frame_id = frame_id;
                on_new_sample(frame_id);
                let view = to_view(&snapshot);
                ready_frame_id = classify_action_dispatch_sample(&view, action_frame)?;
            }
            Err(FrameError::Timeout(_)) => continue,
            Err(error) => return Err(anyhow!("动作派发前读取尺子失败：{error}")),
        }
    }
}

fn classify_action_dispatch_sample(view: &FrameView, action_frame: i64) -> Result<Option<u64>> {
    match classify_action_sample(view, action_frame) {
        ActionSampleDecision::Ignore => Ok(None),
        ActionSampleDecision::Confirm => Ok(Some(view.frame_id)),
        ActionSampleDecision::Reject(reason) => Err(anyhow!(
            "动作派发前尺子状态不满足目标：{reason} frame_id={} state={} elapsed={}",
            view.frame_id,
            view.battle_state,
            view.elapsed
        )),
    }
}

/// Session 完成识别、坐标计算等准备工作后，在首个实际动作输入前再次读取尺子。
/// 这封住动作前稳定屏障与部署拖拽/AFA 热键之间的准备时间竞态。
fn verify_action_dispatch_now<F>(
    frames: &dyn FrameSource,
    action_frame: i64,
    min_frame_id: u64,
    mut on_new_sample: F,
) -> Result<u64>
where
    F: FnMut(u64),
{
    let snapshot = frames
        .latest()
        .ok_or_else(|| anyhow!("动作输入前尺子没有可用快照"))?;
    let frame_id = snapshot
        .frame_id
        .ok_or_else(|| anyhow!("动作输入前尺子快照缺少 frame_id"))?;
    if frame_id < min_frame_id {
        return Err(anyhow!(
            "动作输入前尺子快照倒退：frame_id={frame_id} min_frame_id={min_frame_id}"
        ));
    }
    if frame_id > min_frame_id {
        on_new_sample(frame_id);
    }

    let view = to_view(&snapshot);
    classify_action_dispatch_sample(&view, action_frame)?.ok_or_else(|| {
        anyhow!(
            "动作输入前尺子尚未确认目标帧 {action_frame} 的可信 1x_paused 状态：frame_id={frame_id} state={} elapsed={}",
            view.battle_state,
            view.elapsed
        )
    })
}

/// 等待一条动作派发后的可信暂停样本。
///
/// `on_new_sample` 在每条新 `frame_id` 被分类前调用，让 Runner 继续推进 Machine 的
/// 帧游标；传入回调也让这个循环可以在没有 Session/AFA 的单元测试中使用假帧源。
fn wait_for_action_confirmation<F>(
    frames: &dyn FrameSource,
    cancel: &AtomicBool,
    action_frame: i64,
    min_frame_id: u64,
    mut on_new_sample: F,
) -> Result<(u64, FrameView)>
where
    F: FnMut(u64),
{
    let deadline = Instant::now() + ACTION_CONFIRM_TIMEOUT;
    let mut after_frame_id = min_frame_id;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("用户在动作确认期间中止复刻"));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!(
                "动作确认超时：目标帧 {action_frame} 在派发后未收到新的暂停样本（min_frame_id={min_frame_id}）"
            ));
        }
        match frames.wait_next(after_frame_id, remaining.min(Duration::from_millis(200))) {
            Ok(snapshot) => {
                let Some(frame_id) = snapshot.frame_id else {
                    continue;
                };
                if frame_id <= after_frame_id {
                    continue;
                }
                after_frame_id = frame_id;
                on_new_sample(frame_id);
                let view = to_view(&snapshot);

                match classify_action_sample(&view, action_frame) {
                    ActionSampleDecision::Ignore => continue,
                    ActionSampleDecision::Confirm => return Ok((frame_id, view)),
                    ActionSampleDecision::Reject(reason) => {
                        return Err(anyhow!(
                            "动作确认失败：{reason} frame_id={frame_id} state={} elapsed={}",
                            view.battle_state,
                            view.elapsed
                        ));
                    }
                }
            }
            Err(FrameError::Timeout(_)) => continue,
            Err(error) => return Err(anyhow!("动作确认读取尺子失败：{error}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionSampleDecision {
    Ignore,
    Confirm,
    Reject(ActionRejectReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActionRejectReason {
    Running,
    LeftBattle,
    WrongFrame,
    NotOneX,
}

impl std::fmt::Display for ActionRejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Running => "观察到运行态",
            Self::LeftBattle => "已离开战斗",
            Self::WrongFrame => "绝对帧与目标不一致",
            Self::NotOneX => "倍速不是 1x",
        })
    }
}

fn classify_action_sample(view: &FrameView, action_frame: i64) -> ActionSampleDecision {
    // 运行态是不可恢复的确认失败：即使尺子当前读数不可信，也不能假设游戏没有推进。
    if view.paused == Some(false) {
        return ActionSampleDecision::Reject(ActionRejectReason::Running);
    }
    if !view.in_battle {
        return ActionSampleDecision::Reject(ActionRejectReason::LeftBattle);
    }
    // deploying_operator / adjusting_operator_facing 的暂停状态未知，不能确认，
    // 也不把它当成运行态；继续等待可判定的暂停样本。
    if view.paused != Some(true) || !view.trustworthy {
        return ActionSampleDecision::Ignore;
    }
    if view.elapsed != action_frame {
        return ActionSampleDecision::Reject(ActionRejectReason::WrongFrame);
    }
    if view.battle_state == "0.2x_paused" {
        return ActionSampleDecision::Ignore;
    }
    if !view.one_x {
        return ActionSampleDecision::Reject(ActionRejectReason::NotOneX);
    }
    ActionSampleDecision::Confirm
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
    use repl_frames::source::fake::FakeFrameSource;

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

    #[test]
    fn action_confirmation_rejects_running_state_even_when_sample_is_untrusted() {
        let view = to_view(&snapshot_json("1x_running", 60, 8, false));
        assert_eq!(
            classify_action_sample(&view, 60),
            ActionSampleDecision::Reject(ActionRejectReason::Running)
        );
    }

    #[test]
    fn action_confirmation_tolerates_only_paused_point_two_transition() {
        let view = to_view(&snapshot_json("0.2x_paused", 60, 8, true));
        assert_eq!(
            classify_action_sample(&view, 60),
            ActionSampleDecision::Ignore
        );
    }

    #[test]
    fn action_confirmation_accepts_new_one_x_paused_target_sample() {
        let view = to_view(&snapshot_json("1x_paused", 60, 8, true));
        assert_eq!(
            classify_action_sample(&view, 60),
            ActionSampleDecision::Confirm
        );
    }

    #[test]
    fn action_confirmation_rejects_wrong_target_frame() {
        let view = to_view(&snapshot_json("1x_paused", 61, 8, true));
        assert_eq!(
            classify_action_sample(&view, 60),
            ActionSampleDecision::Reject(ActionRejectReason::WrongFrame)
        );
    }

    #[test]
    fn confirmation_loop_requires_new_sample_and_waits_through_point_two() {
        let frames = FakeFrameSource::new([
            // 与 min_frame_id 相同的样本必须被丢弃，不能成为确认依据。
            snapshot_json("1x_paused", 60, 8, true),
            snapshot_json("0.2x_paused", 60, 9, true),
            snapshot_json("1x_paused", 60, 10, true),
        ]);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let (frame_id, view) =
            wait_for_action_confirmation(&frames, &cancel, 60, 8, |id| seen.push(id))
                .expect("new 1x_paused target sample should confirm");

        assert_eq!(seen, vec![9, 10]);
        assert_eq!(frame_id, 10);
        assert_eq!(view.battle_state, "1x_paused");
    }

    #[test]
    fn confirmation_loop_rejects_running_sample_without_waiting() {
        let frames = FakeFrameSource::new([snapshot_json("1x_running", 60, 9, true)]);
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let error = wait_for_action_confirmation(&frames, &cancel, 60, 8, |id| seen.push(id))
            .expect_err("running state must fail closed");

        assert!(error.to_string().contains("观察到运行态"));
        assert_eq!(seen, vec![9]);
    }

    #[test]
    fn action_dispatch_preflight_rejects_ruler_frame_ahead_of_machine_cursor() {
        let frames = FakeFrameSource::new([
            snapshot_json("1x_paused", 10, 100, true),
            snapshot_json("1x_paused", 11, 101, true),
        ]);
        let baseline = frames
            .wait_next(0, Duration::from_millis(1))
            .expect("prime the ruler snapshot at the machine cursor");
        assert_eq!(baseline.total_elapsed_frames, 10);

        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        let error = wait_for_action_dispatch_ready(
            &frames,
            &cancel,
            10,
            100,
            Duration::from_millis(20),
            |id| seen.push(id),
        )
        .expect_err("a newer authoritative ruler frame must block action dispatch");

        assert!(error.to_string().contains("动作派发前"));
        assert!(error.to_string().contains("elapsed=11"));
        assert_eq!(seen, vec![101]);
    }

    #[test]
    fn action_dispatch_preflight_accepts_current_authoritative_target_sample() {
        let frames = FakeFrameSource::new([snapshot_json("1x_paused", 10, 100, true)]);
        frames
            .wait_next(0, Duration::from_millis(1))
            .expect("prime the current ruler snapshot");

        let cancel = AtomicBool::new(false);
        let frame_id =
            wait_for_action_dispatch_ready(&frames, &cancel, 10, 100, Duration::ZERO, |_| {})
                .expect("the current authoritative target sample should permit dispatch");

        assert_eq!(frame_id, 100);
    }

    #[test]
    fn final_input_guard_rejects_frame_that_advanced_during_action_preparation() {
        let frames = FakeFrameSource::new([
            snapshot_json("1x_paused", 10, 100, true),
            snapshot_json("1x_paused", 11, 101, true),
        ]);
        frames
            .wait_next(0, Duration::from_millis(1))
            .expect("prime the preflight target sample");
        let preflight_frame_id = wait_for_action_dispatch_ready(
            &frames,
            &AtomicBool::new(false),
            10,
            100,
            Duration::ZERO,
            |_| {},
        )
        .expect("preflight should initially pass at frame 10");
        frames
            .wait_next(preflight_frame_id, Duration::from_millis(1))
            .expect("advance the ruler while Session prepares the action");

        let mut seen = Vec::new();
        let error = verify_action_dispatch_now(&frames, 10, preflight_frame_id, |id| seen.push(id))
            .expect_err("the last responsible input boundary must observe frame 11");

        assert!(error.to_string().contains("elapsed=11"));
        assert_eq!(seen, vec![101]);
    }
}
