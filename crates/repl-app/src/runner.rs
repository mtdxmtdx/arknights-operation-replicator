// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 运行循环：把状态机的指令真正执行掉。
//!
//! 循环本身很薄 —— 所有判断逻辑都在 `repl-core::machine` 里（那部分可以离线回放测试），
//! 这里只负责"接指令 → 干活 → 回报"。

#![allow(clippy::empty_line_after_doc_comments)]

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use repl_core::{
    continuation::ContinuationPlan,
    machine::{AbortReason, Command, Completion, FrameView, Machine, Phase},
    ActionType, Copilot,
};
use repl_frames::{FrameError, FrameSource, Snapshot};
use repl_input::{precise_sleep, TimerResolution};

use crate::{config::Config, session::Session};

/// 编队绑定回调：拿到会话去做识别和交互，返回 `true` 表示绑定完成。
/// Whether the binding callback may ask the user to fill the binding panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingMode {
    AutomaticOnly,
    InteractiveAllowed,
}

/// A new execution session always starts in one of these two modes.
#[derive(Clone, Debug)]
pub enum RunMode {
    /// Normal, precise replication requested while the ruler is paused at frame 0.
    FromZero,
    /// Risk-accepted continuation from a frozen, user-confirmed cutoff.
    Continue(ContinuationPlan),
}

impl RunMode {
    pub fn assumed_action_count(&self) -> usize {
        match self {
            Self::FromZero => 0,
            Self::Continue(plan) => plan.assumed_action_count(),
        }
    }
}

/// 一次绑定请求。启动绑定允许部分完成；延迟绑定必须绑定当前 Deploy 目标。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BindingRequest {
    Startup { mode: BindingMode },
    Deferred { target: String, frame: i64 },
}

/// Binding callback. The cancellation flag is passed through so a waiting panel can stop
/// promptly when the user presses Stop.
type BindingHandler<'a> =
    Box<dyn FnMut(&mut Session, BindingRequest, &AtomicBool) -> Result<()> + 'a>;

/// Startup progress before the state machine receives its takeover sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupPhase {
    WaitingGameFocus,
    BindingFormation,
    WaitingFinalFocus,
    VerifyingTakeover,
}

impl StartupPhase {
    pub const fn zh(self) -> &'static str {
        match self {
            Self::WaitingGameFocus => "等待游戏回到前台",
            Self::BindingFormation => "准备编队绑定",
            Self::WaitingFinalFocus => "等待最终焦点交接",
            Self::VerifyingTakeover => "验证接管样本",
        }
    }
}

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
    Startup {
        phase: StartupPhase,
    },
    Phase {
        phase: Phase,
        cursor: i64,
        target: Option<i64>,
    },
    /// 需要用户完成编队绑定。带上识别出的卡片和待绑定的干员名 ——
    /// UI 的全部展示内容都来自这条事件，别的地方不维护绑定状态。
    NeedsBinding {
        cards: Vec<BindingCardInfo>,
        /// 下拉菜单允许选择的完整作业编队/当前延迟目标。
        options: Vec<String>,
        /// 仍未绑定的目标，只用于状态提示与确认门禁。
        opers: Vec<String>,
        assignments: Vec<(usize, String)>,
        allow_partial: bool,
        title: String,
        detail: String,
    },
    /// 工作线程已验证选择并把头像档案写入磁盘；此时 UI 才能关闭绑定面板。
    BindingCommitted {
        message: String,
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
        assumed_actions: usize,
        continued: bool,
    },
    Failed(String),
}

/// 等待开局的总超时。

/// 逐帧推进阶段每次脉冲前的等待时长。
///
/// 目的是让游戏在下一次解暂停前有足够的稳定时间：选中 UI 动画完成、
/// 尺子读数稳定、费用条不再抖动。500ms = 约 15 帧的墙钟缓冲。
const SLOW_STEP_PAUSE: Duration = Duration::from_millis(500);

/// 到达目标帧后、注入动作前的等待时长。
///
/// 给游戏时间确认"已暂停、已停在目标帧"的状态，再开始触控和键盘注入。
const PRE_ACTION_PAUSE: Duration = Duration::from_secs(1);

/// AFA 委托或部署动作发出后，等待尺子确认动作仍停在目标帧的上限。
const ACTION_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// 部署及朝向手势确认后，在允许下一条动作或 Resume 前保持目标帧暂停的时间。
///
/// 实机日志表明 150ms 的手势尾延迟后立即发送 ReleasePause 会被间歇性吞掉；500ms 与逐帧阶段
/// 已验证的 UI 稳定窗口一致。等待期间仍持续读取尺子，绝不以盲等掩盖错帧或意外运行。
const POST_DEPLOY_SETTLE: Duration = Duration::from_millis(500);

/// AFA 自动开局暂停被尺子确认后，开始编队和后续动作前的稳定等待。

/// 编队绑定面板关闭后，等待用户把游戏恢复到前台的上限。
const GAME_FOCUS_TIMEOUT: Duration = Duration::from_secs(15);

/// After focus is observed, require a newer authoritative ruler sample before takeover.
const POST_FOCUS_SAMPLE_TIMEOUT: Duration = Duration::from_secs(2);

/// 一次复刻运行。
pub struct Runner<'a> {
    machine: Machine,
    run_mode: RunMode,
    session: &'a mut Session,
    frames: &'a dyn FrameSource,
    events: mpsc::Sender<Progress>,
    cancel: Arc<AtomicBool>,
    binder: BindingHandler<'a>,
}

impl<'a> Runner<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        copilot: Copilot,
        _config: &Config,
        run_mode: RunMode,
        session: &'a mut Session,
        frames: &'a dyn FrameSource,
        events: mpsc::Sender<Progress>,
        cancel: Arc<AtomicBool>,
        binder: BindingHandler<'a>,
    ) -> Self {
        let machine = match &run_mode {
            RunMode::FromZero => Machine::new(copilot),
            RunMode::Continue(plan) => Machine::with_continuation(copilot, plan),
        };
        Self {
            machine,
            run_mode,
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
        let result = self.prepare_takeover().and_then(|takeover| {
            if !self.machine.take_over(&takeover) {
                let reason = self.machine.abort_reason().cloned().unwrap_or_else(|| {
                    AbortReason::ExecutionFailed("接管样本未能初始化状态机".into())
                });
                return Err(anyhow!("{reason}"));
            }

            // Timer resolution is only needed once real frame work begins. Startup waits
            // deliberately do not change the process-wide timer setting.
            let _timer = TimerResolution::acquire();
            self.pump()
        });

        // 无论成功还是失败，都抬起可能卡住的触控输入。
        self.session.release_inputs();

        match &result {
            Ok(()) => {
                let t = self.machine.tuner();
                let _ = self.events.send(Progress::Finished {
                    pulses: t.pulses,
                    zero_pulses: t.zero_frame_pulses,
                    overshoots: t.overshoot_pulses,
                    assumed_actions: self.run_mode.assumed_action_count(),
                    continued: matches!(&self.run_mode, RunMode::Continue(_)),
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

                Command::Pulse => {
                    // 逐帧推进阶段：每次脉冲前先等 SLOW_STEP_PAUSE。
                    // 这给游戏和尺子充足的稳定时间，避免在状态未稳定时解暂停
                    // （选中 UI 动画、费用条抖动等都会在这里安全地结束）。
                    if self.machine.phase() == Phase::Stepping {
                        precise_sleep(SLOW_STEP_PAUSE);
                    }
                    self.session.pulse()?;
                    self.machine.completed(Completion::PulseSent);
                }

                Command::AwaitFrame { timeout } => self.await_frame(timeout)?,

                Command::Execute { index } => {
                    let action = self.machine.copilot().actions[index].clone();
                    // 到达目标帧后，在稳定等待期间持续读取尺子，而不是盲等后继续使用
                    // Machine 的旧游标。尺子是绝对帧的唯一真源；若它已经越过目标帧，
                    // 必须在任何鼠标或 AFA 输入发出前中止。
                    let mut min_frame_id = match wait_for_action_dispatch_ready(
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
                    if action.kind == ActionType::Deploy {
                        min_frame_id = match self.ensure_deploy_binding(
                            &action.name,
                            action.frame,
                            min_frame_id,
                        ) {
                            Ok(frame_id) => frame_id,
                            Err(error) => {
                                self.machine.execution_failed(error.to_string());
                                return Err(error);
                            }
                        };
                    }
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
                            if action.kind == ActionType::Deploy
                                && index + 1 < self.machine.copilot().actions.len()
                            {
                                let settle_min_frame_id = self.machine.last_frame_id();
                                let settle_result = {
                                    let frames = self.frames;
                                    let machine = &mut self.machine;
                                    wait_for_post_deploy_settle(
                                        frames,
                                        &self.cancel,
                                        action.frame,
                                        settle_min_frame_id,
                                        POST_DEPLOY_SETTLE,
                                        |frame_id| machine.acknowledge_frame_id(frame_id),
                                    )
                                };
                                if let Err(error) = settle_result {
                                    self.machine.execution_failed(error.to_string());
                                    return Err(error);
                                }
                                self.note(format!(
                                    "部署后稳定确认通过：frame_id={} elapsed={}",
                                    self.machine.last_frame_id(),
                                    action.frame
                                ));
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

    /// 在动作目标帧确认部署头像。已有档案且唯一命中时零交互通过；否则弹出
    /// 延迟绑定，等待用户把游戏切回前台，再要求一条新的同目标帧暂停样本。
    fn ensure_deploy_binding(
        &mut self,
        target: &str,
        action_frame: i64,
        min_frame_id: u64,
    ) -> Result<u64> {
        if self.session.deploy_target_visible(target)? {
            return Ok(min_frame_id);
        }

        self.note(format!(
            "第 {action_frame} 帧的部署目标「{target}」没有可用头像匹配，等待延迟人工绑定"
        ));
        (self.binder)(
            self.session,
            BindingRequest::Deferred {
                target: target.to_owned(),
                frame: action_frame,
            },
            &self.cancel,
        )?;

        let focus_watermark = self.latest_frame_id().max(min_frame_id);
        self.note(format!(
            "延迟绑定完成；请将游戏切回前台（目标 F{action_frame}，ruler watermark={focus_watermark}）"
        ));
        self.wait_for_game_focus()?;
        self.session.park_cursor();
        let frame_id = self.wait_for_action_focus_sample(focus_watermark, action_frame)?;
        self.machine.acknowledge_frame_id(frame_id);

        if !self.session.deploy_target_visible(target)? {
            return Err(anyhow!(
                "延迟绑定后仍无法在部署栏唯一识别「{target}」，未发送部署输入"
            ));
        }
        Ok(frame_id)
    }

    fn wait_for_action_focus_sample(
        &mut self,
        min_frame_id: u64,
        action_frame: i64,
    ) -> Result<u64> {
        let deadline = Instant::now() + POST_FOCUS_SAMPLE_TIMEOUT;
        let mut after_frame_id = min_frame_id;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("用户在延迟绑定焦点交接期间中止复刻"));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!(
                    "延迟绑定后 {} 秒内没有收到目标帧 {action_frame} 的新可信 1x_paused 样本",
                    POST_FOCUS_SAMPLE_TIMEOUT.as_secs()
                ));
            }
            match self
                .frames
                .wait_next(after_frame_id, remaining.min(Duration::from_millis(200)))
            {
                Ok(snapshot) => {
                    let Some(frame_id) = snapshot.frame_id else {
                        continue;
                    };
                    if frame_id <= after_frame_id {
                        continue;
                    }
                    after_frame_id = frame_id;
                    let view = to_view(&snapshot);
                    match classify_action_sample(&view, action_frame) {
                        ActionSampleDecision::Ignore => continue,
                        ActionSampleDecision::Confirm => return Ok(frame_id),
                        ActionSampleDecision::Reject(reason) => {
                            return Err(anyhow!(
                                "延迟绑定后焦点交接失败：{reason} frame_id={frame_id} state={} elapsed={}",
                                view.battle_state,
                                view.elapsed
                            ));
                        }
                    }
                }
                Err(FrameError::Timeout(_)) => continue,
                Err(error) => return Err(anyhow!("延迟绑定后读取尺子失败：{error}")),
            }
        }
    }

    /// 绑定面板会短暂夺走前台。这里只尝试一次系统前台切换；失败后等待用户手动
    fn prepare_takeover(&mut self) -> Result<FrameView> {
        let next_action = self
            .machine
            .target()
            .ok_or_else(|| anyhow!("作业没有可执行动作"))?;

        let initial = self
            .frames
            .latest()
            .ok_or_else(|| anyhow!("开始前没有收到尺子样本，请确认尺子已连接"))?;
        let initial_view = to_view(&initial);
        if initial.frame_id.is_none() {
            return Err(anyhow!(
                "开始请求被拒绝：尺子首样本缺少 frame_id，无法建立焦点交接水位"
            ));
        }
        match &self.run_mode {
            RunMode::FromZero if !start_sample_ready(&initial_view, next_action) => {
                return Err(anyhow!(
                    "开始请求被拒绝：需要可信、战斗内、1x_paused 且 elapsed=0，当前 state={} trustworthy={} paused={:?} elapsed={}",
                    initial_view.battle_state,
                    initial_view.trustworthy,
                    initial_view.paused,
                    initial_view.elapsed,
                ));
            }
            RunMode::Continue(plan)
                if !continuation_sample_ready(&initial_view)
                    || initial_view.elapsed < plan.cutoff_frame
                    || initial_view.elapsed > next_action =>
            {
                return Err(anyhow!(
                    "继续请求被拒绝：冻结截止帧={}，下一动作帧={}，当前 state={} trustworthy={} paused={:?} elapsed={}",
                    plan.cutoff_frame,
                    next_action,
                    initial_view.battle_state,
                    initial_view.trustworthy,
                    initial_view.paused,
                    initial_view.elapsed,
                ));
            }
            _ => {}
        }

        let focus_watermark = initial_view.frame_id;
        self.emit_startup(StartupPhase::WaitingGameFocus);
        self.note(format!(
            "开始请求已接受；等待用户手动将游戏切回前台（ruler watermark={focus_watermark}）"
        ));
        self.wait_for_game_focus()?;
        self.session.park_cursor();

        let first_focus_sample = self.wait_for_takeover_sample(focus_watermark, next_action)?;
        let binding_mode = if first_focus_sample.paused == Some(true) {
            BindingMode::InteractiveAllowed
        } else {
            self.note("焦点后的第一条可信样本为 1x_running；本轮只允许自动恢复编队绑定".into());
            BindingMode::AutomaticOnly
        };

        self.emit_startup(StartupPhase::BindingFormation);
        (self.binder)(
            self.session,
            BindingRequest::Startup { mode: binding_mode },
            &self.cancel,
        )?;
        self.session.park_cursor();

        self.emit_startup(StartupPhase::WaitingFinalFocus);
        self.note("编队准备完成；请再次将游戏切回前台".into());
        let final_focus_watermark = self.latest_frame_id();
        self.wait_for_game_focus()?;
        self.session.park_cursor();

        self.emit_startup(StartupPhase::VerifyingTakeover);
        self.wait_for_takeover_sample(final_focus_watermark, next_action)
    }

    fn latest_frame_id(&self) -> u64 {
        self.frames
            .latest()
            .and_then(|snapshot| snapshot.frame_id)
            .unwrap_or(0)
    }

    fn wait_for_game_focus(&mut self) -> Result<()> {
        let deadline = Instant::now() + GAME_FOCUS_TIMEOUT;
        while Instant::now() < deadline {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("{}", AbortReason::UserAborted));
            }
            if self.session.window.is_foreground() {
                self.note("检测到游戏窗口已回到前台".into());
                return Ok(());
            }
            precise_sleep(Duration::from_millis(100));
        }
        Err(anyhow!(
            "等待游戏窗口回到前台超时（{} 秒）；复刻器没有自动抢回游戏焦点",
            GAME_FOCUS_TIMEOUT.as_secs()
        ))
    }

    fn wait_for_takeover_sample(
        &mut self,
        min_frame_id: u64,
        next_action: i64,
    ) -> Result<FrameView> {
        let deadline = Instant::now() + POST_FOCUS_SAMPLE_TIMEOUT;
        let mut after_frame_id = min_frame_id;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("{}", AbortReason::UserAborted));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                let latest = self.frames.latest();
                return Err(anyhow!(
                    "焦点交接后 {} 秒内没有收到新的可信 1x 样本（min_frame_id={}，当前 state={} elapsed={})",
                    POST_FOCUS_SAMPLE_TIMEOUT.as_secs(),
                    min_frame_id,
                    latest
                        .as_ref()
                        .map_or("<无快照>".to_owned(), |snapshot| snapshot.battle_state_str().to_owned()),
                    latest.as_ref().map_or(-1, |snapshot| snapshot.total_elapsed_frames),
                ));
            }

            match self
                .frames
                .wait_next(after_frame_id, remaining.min(Duration::from_millis(200)))
            {
                Ok(snapshot) => {
                    let Some(frame_id) = snapshot.frame_id else {
                        continue;
                    };
                    if frame_id <= after_frame_id {
                        continue;
                    }
                    after_frame_id = frame_id;
                    let view = to_view(&snapshot);
                    if !view.trustworthy {
                        log::debug!(
                            "takeover sample ignored: frame_id={} state={} is untrustworthy",
                            view.frame_id,
                            view.battle_state
                        );
                        continue;
                    }
                    if !view.in_battle {
                        return Err(anyhow!(
                            "{}",
                            AbortReason::LeftBattle {
                                state: view.battle_state.clone()
                            }
                        ));
                    }
                    if !view.one_x {
                        return Err(anyhow!(
                            "{}",
                            AbortReason::NotOneX {
                                state: view.battle_state.clone()
                            }
                        ));
                    }
                    if view.paused.is_none() {
                        log::debug!(
                            "takeover sample ignored: frame_id={} state={} pause state unknown",
                            view.frame_id,
                            view.battle_state
                        );
                        continue;
                    }
                    let minimum = self.machine.continuation_cutoff().unwrap_or(0);
                    if view.elapsed < minimum {
                        return Err(anyhow!(
                            "继续接管样本倒退到冻结截止帧之前：当前第 {} 帧，冻结截止帧 {}",
                            view.elapsed,
                            minimum,
                        ));
                    }
                    if view.elapsed > next_action {
                        return Err(anyhow!(
                            "{}",
                            AbortReason::MissedStart {
                                takeover_frame: view.elapsed,
                                first_action: next_action,
                            }
                        ));
                    }
                    self.note(format!(
                        "接管样本通过：frame_id={} state={} elapsed={} paused={:?} watermark={min_frame_id}",
                        view.frame_id, view.battle_state, view.elapsed, view.paused,
                    ));
                    return Ok(view);
                }
                Err(FrameError::Timeout(_)) => continue,
                Err(error) => return Err(anyhow!("焦点交接后读取尺子失败：{error}")),
            }
        }
    }

    fn emit_startup(&self, phase: StartupPhase) {
        log::info!("startup phase: {}", phase.zh());
        let _ = self.events.send(Progress::Startup { phase });
    }

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

    /// 运行期等待尺子样本。开局接管已经在 [`Self::prepare_takeover`] 完成；
    /// 这里仅处理巡航、暂停确认和脉冲收尾，绝不发送开局暂停输入。
    ///
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

/// 部署动作已经确认后，再保持一段完整的目标帧暂停窗口。
///
/// 复用动作派发前的严格分类规则：窗口内出现 running、错帧、非 1x 或离战都立即失败；只有截止
/// 时最新样本仍是目标帧可信 `1x_paused` 才允许 Runner 进入下一条动作或发送 Resume。
fn wait_for_post_deploy_settle<F>(
    frames: &dyn FrameSource,
    cancel: &AtomicBool,
    action_frame: i64,
    min_frame_id: u64,
    settle: Duration,
    on_new_sample: F,
) -> Result<u64>
where
    F: FnMut(u64),
{
    let frame_id = wait_for_action_dispatch_ready(
        frames,
        cancel,
        action_frame,
        min_frame_id,
        settle,
        on_new_sample,
    )
    .map_err(|error| anyhow!("部署完成后稳定确认失败：{error}"))?;
    if frame_id <= min_frame_id {
        return Err(anyhow!(
            "部署完成后稳定确认失败：{}ms 窗口内没有收到更新的目标帧暂停样本（min_frame_id={min_frame_id}）",
            settle.as_millis()
        ));
    }
    Ok(frame_id)
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
/// Whether the Start button's live precondition is satisfied.
pub fn start_sample_ready(view: &FrameView, _first_action: i64) -> bool {
    view.trustworthy
        && view.in_battle
        && view.one_x
        && view.paused == Some(true)
        && view.elapsed == 0
}

/// UI/启动入口的完整预检；没有 `frame_id` 就不能建立“焦点后新样本”的水位。
pub fn start_snapshot_ready(snapshot: &Snapshot, first_action: i64) -> bool {
    snapshot.frame_id.is_some() && start_sample_ready(&to_view(snapshot), first_action)
}

/// Continue may be proposed at any positive authoritative paused frame. Whether an action
/// remains is a property of the job and is checked by [`ContinuationPlan::build`].
pub fn continuation_sample_ready(view: &FrameView) -> bool {
    view.trustworthy
        && view.in_battle
        && view.one_x
        && view.paused == Some(true)
        && view.elapsed > 0
}

pub fn continuation_snapshot_plan(
    snapshot: &Snapshot,
    copilot: &Copilot,
) -> Option<ContinuationPlan> {
    snapshot.frame_id?;
    let view = to_view(snapshot);
    continuation_sample_ready(&view)
        .then(|| ContinuationPlan::build(copilot, view.elapsed).ok())
        .flatten()
}

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
    fn start_sample_requires_authoritative_paused_battle_at_zero() {
        let paused = to_view(&snapshot_json("1x_paused", 0, 7, true));
        assert!(start_sample_ready(&paused, 10));

        let running = to_view(&snapshot_json("1x_running", 0, 7, true));
        assert!(!start_sample_ready(&running, 10));

        let late = to_view(&snapshot_json("1x_paused", 1, 7, true));
        assert!(!start_sample_ready(&late, 10));
    }

    #[test]
    fn continuation_requires_positive_authoritative_paused_frame_and_remaining_action() {
        let job = Copilot::parse(include_str!("../../../examples/test1.json")).unwrap();
        let frame_one = snapshot_json("1x_paused", 1, 7, true);
        let plan = continuation_snapshot_plan(&frame_one, &job).unwrap();
        assert_eq!(plan.next_action_index, 0);

        let frame_eleven = snapshot_json("1x_paused", 11, 8, true);
        let plan = continuation_snapshot_plan(&frame_eleven, &job).unwrap();
        assert_eq!(plan.next_action_index, 1);

        assert!(
            continuation_snapshot_plan(&snapshot_json("1x_running", 11, 9, true), &job).is_none()
        );
        assert!(
            continuation_snapshot_plan(&snapshot_json("1x_paused", 249, 10, true), &job).is_none()
        );
    }

    #[test]
    fn start_snapshot_requires_frame_id_for_focus_watermark() {
        let mut snapshot = snapshot_json("1x_paused", 10, 7, true);
        snapshot.frame_id = None;
        assert!(!start_snapshot_ready(&snapshot, 10));
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

    #[test]
    fn post_deploy_settle_holds_the_target_pause_before_resume() {
        let frames = FakeFrameSource::new([
            snapshot_json("1x_paused", 60, 9, true),
            snapshot_json("1x_paused", 60, 10, true),
        ]);
        frames
            .wait_next(0, Duration::from_millis(1))
            .expect("prime the post-deploy paused sample");

        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let frame_id =
            wait_for_post_deploy_settle(&frames, &cancel, 60, 9, Duration::from_millis(20), |_| {})
                .expect("a stable target-frame pause should survive the deployment settle window");

        assert_eq!(frame_id, 10);
        assert!(started.elapsed() >= Duration::from_millis(15));
    }

    #[test]
    fn post_deploy_settle_rejects_running_before_resume() {
        let frames = FakeFrameSource::new([
            snapshot_json("1x_paused", 60, 9, true),
            snapshot_json("1x_running", 60, 10, true),
        ]);
        frames
            .wait_next(0, Duration::from_millis(1))
            .expect("prime the post-deploy paused sample");

        let error = wait_for_post_deploy_settle(
            &frames,
            &AtomicBool::new(false),
            60,
            9,
            Duration::from_millis(20),
            |_| {},
        )
        .expect_err("deployment settle must fail closed if the game starts running");

        assert!(error.to_string().contains("部署完成后"));
        assert!(error.to_string().contains("观察到运行态"));
    }
}
