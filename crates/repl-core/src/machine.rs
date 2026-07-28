// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 主循环结构参考 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Task/Miscellaneous/BattleProcessTask.cpp
// 其中 wait_condition() 的"每步视觉条件等待"被整体替换为"等到绝对逻辑帧"。
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 帧复刻状态机。
//!
//! 纯逻辑：吃尺子快照，吐注入指令。不碰任何 IO，所以可以拿录制的快照序列
//! 离线回放，把"巡航 / 提前暂停 / 逐帧推进"的切换点全部测到。
//!
//! ```text
//! 等待开局 → 第零帧暂停 → 编队匹配 → 正常运行 → 提前暂停 → 逐帧推进
//!          → 到目标帧暂停 → 注入动作 → 下一动作 → … → 收尾
//! ```

use std::time::Duration;

use crate::{
    copilot::{ActionType, Copilot},
    stepping::GapTuner,
};

/// 提前多少帧从"正常运行"切到"逐帧推进"。
///
/// 必须大于"尺子采样 + 分析 + 我们注入暂停"的往返延迟。1 倍速下一帧 33.3ms，
/// WGC 采样约 16.7ms，加上分析和注入约 30–50ms ≈ 1–2 帧。取 8 帧（≈267ms）
/// 留足余量：宁可多几步逐帧推进（每步约 60ms），也不能冲过目标帧。
pub const FRAME_LEAD: i64 = 8;

/// 等一条新分析帧的超时。
pub const AWAIT_TIMEOUT: Duration = Duration::from_millis(2000);

/// 从尺子快照里提炼出状态机真正关心的部分。
///
/// 这样 `repl-core` 不必依赖 `repl-frames`，测试里也能手工构造。
#[derive(Clone, Debug, PartialEq)]
pub struct FrameView {
    /// 截图管线的单调帧号。用来判断"这是不是一条新样本"。
    pub frame_id: u64,
    /// 本场战斗累计逻辑帧（`totalElapsedFrames`）。
    pub elapsed: i64,
    /// 是否处于战斗中（含暂停、部署慢放）。
    pub in_battle: bool,
    /// 是否暂停。`None` 表示尺子看不出来（部署慢放时暂停按钮被遮挡）。
    pub paused: Option<bool>,
    /// 是否 1 倍速。首版硬性要求。
    pub one_x: bool,
    /// 这条读数可不可信（`isRunning && currentFrame.is_some() && !cursorBlocked`）。
    ///
    /// **不可信的样本里 `elapsed` 保持的是上一次的值**，把它当成"帧数没涨"会误判，
    /// 所以必须整条丢弃。
    pub trustworthy: bool,
    /// 原始状态字符串，只用于日志和 UI 显示。
    pub battle_state: String,
}

/// 状态机当前所处的阶段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// 还没开始。
    Idle,
    /// 等待战斗开始。
    WaitingBattle,
    /// 已检测到战斗，等待 AFA 自动开局暂停产生可信的暂停态样本。
    ConfirmingZero,
    /// 编队匹配（用户把作业干员和部署栏卡片对上）。
    BindingFormation,
    /// 正常运行，等着接近目标帧。
    Cruising,
    /// 已提前暂停，正在逐帧推进。
    Stepping,
    /// 已停在目标帧，准备注入动作。
    Injecting,
    /// 全部动作执行完。
    Finished,
    /// 出错中止。
    Aborted,
}

impl Phase {
    pub fn zh(self) -> &'static str {
        match self {
            Self::Idle => "空闲",
            Self::WaitingBattle => "等待开局",
            Self::ConfirmingZero => "确认开局暂停",
            Self::BindingFormation => "编队匹配",
            Self::Cruising => "正常运行",
            Self::Stepping => "逐帧推进",
            Self::Injecting => "注入动作",
            Self::Finished => "已完成",
            Self::Aborted => "已中止",
        }
    }
}

/// 状态机让调用方去做的事。
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// 等待战斗开始；检测到后由 AFA 的自动开局暂停负责停住游戏。
    ArmBattleStart,
    /// 发暂停键。
    Pause,
    /// 发恢复键。
    Resume,
    /// 发一次逐帧脉冲。
    Pulse { gap: Duration },
    /// 抓图并让用户完成编队绑定。
    BindFormation,
    /// 执行第 `index` 个动作。
    Execute { index: usize },
    /// 等下一条尺子样本。
    AwaitFrame { timeout: Duration },
    /// 最后动作已执行且没有后续动作；直接收尾，不再发送任何输入。
    Finish,
    /// 中止。
    Abort(AbortReason),
    /// 已经结束，没有更多事要做。
    Idle,
}

/// 调用方对一条命令的回报。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    /// 已检测到战斗，开局暂停完全委托给 AFA；本完成事件本身不代表发送了输入。
    OpeningPauseDelegated,
    /// 已发出一次运行期暂停键。
    PauseSent,
    /// 已发出恢复键。
    ResumeSent,
    /// 已发出一次逐帧脉冲。
    PulseSent,
    /// 编队绑定完成。
    FormationBound,
    /// 动作执行完毕。
    ActionExecuted,
}

/// 中止原因。每一条都对应一个用户能看懂、能采取行动的说明。
#[derive(Clone, Debug, PartialEq)]
pub enum AbortReason {
    /// 开局暂停没赶上，第一个动作的帧号已经过去了。
    MissedStart { origin: i64, first_action: i64 },
    /// 倍速不是 1x。
    NotOneX { state: String },
    /// 战斗已经结束或退出。
    LeftBattle { state: String },
    /// 尺子报告的帧数倒退了。
    FrameWentBackwards { from: i64, to: i64 },
    /// 冲过了目标帧 —— 游戏没有倒带，只能中止。
    OvershotTarget { target: i64, actual: i64 },
    /// 等不到新的分析帧。
    FrameTimeout,
    /// 暂停键没生效（多半是代理指挥 / 托管）。
    PauseIneffective,
    /// AFA 没有在开局自动暂停。
    OpeningPauseIneffective,
    /// 脉冲已经发出，但尺子长时间没有看到它回到暂停态。
    PulseDidNotSettle { state: String },
    /// 执行动作时出错。
    ExecutionFailed(String),
    /// 用户手动中止。
    UserAborted,
}

impl std::fmt::Display for AbortReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissedStart {
                origin,
                first_action,
            } => write!(
                f,
                "错过了开局：暂停时已经是第 {origin} 帧，但第一个动作在第 {first_action} 帧。\
                 请退出重进关卡再试"
            ),
            Self::NotOneX { state } => write!(
                f,
                "倍速不是 1x（当前 {state}）。帧复刻首版只支持 1 倍速，请把游戏调回 1x"
            ),
            Self::LeftBattle { state } => {
                write!(f, "已经不在战斗中（{state}），复刻中止")
            }
            Self::FrameWentBackwards { from, to } => write!(
                f,
                "尺子报告的帧数从 {from} 倒退到 {to}。多半是费用条被遮挡导致误判，\
                 检查一下鼠标是不是停在了费用条上"
            ),
            Self::OvershotTarget { target, actual } => write!(
                f,
                "冲过了目标帧：想停在第 {target} 帧，实际到了第 {actual} 帧。\
                 游戏没法倒带，只能重来。可以把脉冲间隔调小一点再试"
            ),
            Self::FrameTimeout => write!(
                f,
                "等不到尺子的新分析帧。按可能性排查：\
                 ① 鼠标是否停在费用条附近（PC 自绘光标会遮挡费用条，日志里 cursorBlocked=true 即是）；\
                 ② 尺子是否还在运行、是否已校准；\
                 ③ 费用满 / 费用回复被锁 / 剿灭作战都会让尺子停止计帧。\
                 详细诊断见日志面板和 replicator.log"
            ),
            Self::PauseIneffective => write!(
                f,
                "发了暂停键但游戏仍在推进。如果开着代理指挥或托管，请先关掉"
            ),
            Self::OpeningPauseIneffective => write!(
                f,
                "AFA 没有在开局自动暂停。请在 AFA 中启用自动开局暂停，并确认 AFA 正在运行"
            ),
            Self::PulseDidNotSettle { state } => write!(
                f,
                "逐帧脉冲后迟迟没有恢复暂停（当前 {state}）。为避免重复切换导致超跑，复刻已中止"
            ),
            Self::ExecutionFailed(what) => write!(f, "执行动作失败：{what}"),
            Self::UserAborted => write!(f, "用户中止"),
        }
    }
}

/// 帧复刻状态机。
pub struct Machine {
    copilot: Copilot,
    phase: Phase,
    /// 当前已确认的绝对帧。
    cursor: i64,
    /// 第零帧暂停时实际停在的帧号。
    origin: i64,
    /// 下一个要执行的动作下标。
    action_index: usize,
    /// 游戏当前是否被我们停住了。
    paused: bool,
    /// 运行期 Resume 已发送，但还没有收到尺子的运行态确认。
    resume_in_flight: bool,
    /// 运行期 Pause 已发送，但还没有收到尺子的暂停确认。
    pause_in_flight: bool,
    /// 已消费到的最新 `frame_id`。
    last_frame_id: u64,
    /// 刚发过一次脉冲，等着看它推进了几帧。
    pulse_in_flight: bool,
    /// 当前脉冲尚未回到暂停态时已经看到的新样本数。
    pulse_settle_samples: u32,
    /// 当前脉冲是否已经观察到真实的运行中间态。
    pulse_seen_running: bool,
    /// 尚未看到运行中间态时，连续收到的暂停候选样本数。
    pulse_paused_samples: u32,
    /// 发脉冲之前的游标，用来算实际推进量。
    pulse_from: i64,
    /// 已发暂停键、还没确认生效时，记录当时的帧数，用于检测暂停无效。
    pause_probe: Option<(i64, u32)>,
    /// 最近一个输入动作完成后已经看到的 0.2x 样本数；`None` 表示未开启宽限。
    point_two_samples: Option<u32>,
    tuner: GapTuner,
    abort: Option<AbortReason>,
}

/// 暂停后连续多少条**帧数仍在增长**的可信样本就判定暂停无效。
///
/// 不能取太小：从我们按下暂停键到尺子看见"帧数冻结"之间有一段固有延迟
/// （按键被游戏下一个渲染 tick 处理 + 尺子按截图节奏分析，约 2–4 条样本），
/// 这期间帧数**理应**还在增长。8 条 ≈ 130–260ms，既不会误杀正常暂停，
/// 也能在半秒内识破真正的暂停失效（代理指挥 / 托管会一直涨下去）。
const PAUSE_PROBE_LIMIT: u32 = 8;

/// 接近目标帧时，显式 Pause 发出后最多允许游戏继续推进的逻辑帧数。
/// 6 帧约 200ms，覆盖 AFA 的 50ms ESC 时序、游戏输入处理和尺子分析延迟；
/// FRAME_LEAD=8 仍保留至少 2 帧余量。重复分析同一 elapsed 不消耗这个窗口。
const RUNTIME_PAUSE_MAX_ADVANCE_FRAMES: i64 = 6;
const _: () = assert!(FRAME_LEAD - RUNTIME_PAUSE_MAX_ADVANCE_FRAMES >= 2);

/// 脉冲中间态最多容忍的新样本数；按约 60Hz 分析相当于约 267ms。
const PULSE_SETTLE_SAMPLE_LIMIT: u32 = 16;

/// 未观察到 running 中间态时，至少连续几条新 paused 样本才把脉冲视为已收尾。
/// 单条 paused 可能是输入发出前已进入尺子管线的旧画面。
const PULSE_PAUSED_CONFIRM_SAMPLES: u32 = 2;

/// 动作注入完成后，允许尺子短暂报告 0.2x 的新样本数。
///
/// Skill / Retreat 的暂停态选中流程会让游戏经过一小段 0.2x_running /
/// 0.2x_paused 过渡。动作执行期间主循环不会消费尺子样本，因此这些状态通常
/// 会在动作返回后的第一批样本里集中出现。32 条样本约覆盖半秒（按 60Hz
/// 截图分析估算），足以覆盖慢速机器上的确认延迟；超过上限仍视为用户误开
/// 子弹时间。0.2x 样本本身不会更新帧游标，避免把非 1x 读数带入帧精度计算。
const POINT_TWO_GRACE_LIMIT: u32 = 32;

impl Machine {
    pub fn new(copilot: Copilot) -> Self {
        Self {
            copilot,
            phase: Phase::Idle,
            cursor: 0,
            origin: 0,
            action_index: 0,
            paused: false,
            resume_in_flight: false,
            pause_in_flight: false,
            last_frame_id: 0,
            pulse_in_flight: false,
            pulse_settle_samples: 0,
            pulse_seen_running: false,
            pulse_paused_samples: 0,
            pulse_from: 0,
            pause_probe: None,
            point_two_samples: None,
            tuner: GapTuner::default(),
            abort: None,
        }
    }

    /// 用指定的初始脉冲间隔构造（调参 / 测试用）。
    pub fn with_gap(copilot: Copilot, initial_gap_ms: u32) -> Self {
        let mut m = Self::new(copilot);
        m.tuner = GapTuner::new(initial_gap_ms);
        m
    }

    pub fn start(&mut self) {
        self.phase = Phase::WaitingBattle;
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn cursor(&self) -> i64 {
        self.cursor
    }

    pub fn origin(&self) -> i64 {
        self.origin
    }

    pub fn tuner(&self) -> &GapTuner {
        &self.tuner
    }

    pub fn copilot(&self) -> &Copilot {
        &self.copilot
    }

    pub fn abort_reason(&self) -> Option<&AbortReason> {
        self.abort.as_ref()
    }

    /// 已消费到的最新尺子 `frame_id`。
    ///
    /// 运行循环等待新样本时**必须**把这个值传给 `FrameSource::wait_next` ——
    /// 传 0 的话缓存里的旧快照会立刻满足条件，调用永不阻塞，尺子客户端也就
    /// 永远不会进入主动轮询模式；此时只剩"状态变化时的推送"一条路，而空脉冲
    /// 恰恰不产生任何状态变化，等待就会一直超时。
    ///
    /// 不可信样本（光标遮挡等）也会推进这个游标（见 [`Self::observe`]），
    /// 所以用它等待不会重复收到已经被拒绝过的样本。
    pub fn last_frame_id(&self) -> u64 {
        self.last_frame_id
    }

    /// 上层在动作注入期间自行检查过一条尺子样本时，同步等待游标，避免后续阶段
    /// 重新消费动作前的旧快照。这里只推进 `frame_id`，不改变逻辑帧或暂停结论。
    pub fn acknowledge_frame_id(&mut self, frame_id: u64) {
        self.last_frame_id = self.last_frame_id.max(frame_id);
    }

    /// 已完成 / 总动作数。
    pub fn progress(&self) -> (usize, usize) {
        (self.action_index, self.copilot.actions.len())
    }

    /// 下一个动作的目标帧。全部执行完则为 `None`。
    pub fn target(&self) -> Option<i64> {
        self.copilot.actions.get(self.action_index).map(|a| a.frame)
    }

    /// 用户主动中止。
    pub fn user_abort(&mut self) {
        self.fail(AbortReason::UserAborted);
    }

    fn fail(&mut self, reason: AbortReason) {
        if self.phase != Phase::Aborted {
            log::error!("frame replicator aborting: {reason}");
            self.abort = Some(reason);
            self.phase = Phase::Aborted;
        }
    }

    /// 喂一条尺子样本。
    ///
    /// 返回 `true` 表示这条样本被采纳（新的 `frame_id` 且读数可信）。
    /// 不可信或重复的样本会被静默丢弃 —— 这是正确性的关键：
    /// 光标遮挡时 `elapsed` 保持旧值，当成"没推进"会让脉冲整定器越调越大。
    pub fn observe(&mut self, view: &FrameView) -> bool {
        if self.phase == Phase::Aborted || self.phase == Phase::Finished {
            return false;
        }
        // 旧样本：尺子在暂停时会反复推同一份快照。
        if view.frame_id <= self.last_frame_id {
            return false;
        }
        self.last_frame_id = view.frame_id;

        if !view.trustworthy {
            log::trace!(
                "dropping untrustworthy sample at frame_id {} ({})",
                view.frame_id,
                view.battle_state
            );
            return false;
        }

        // 开局之前不做战斗态校验 —— 那会儿本来就不在战斗里。
        if !matches!(self.phase, Phase::Idle | Phase::WaitingBattle) {
            if !view.in_battle {
                self.fail(AbortReason::LeftBattle {
                    state: view.battle_state.clone(),
                });
                return false;
            }
            if view.one_x {
                // 一旦回到可信的 1x，动作过渡宽限立即失效；后续再出现 0.2x
                // 就必须按真实变速处理。
                self.point_two_samples = None;
            }
            if !view.one_x {
                if view.battle_state.starts_with("0.2x_") {
                    if let Some(samples) = self.point_two_samples.as_mut() {
                        *samples += 1;
                        if *samples < POINT_TWO_GRACE_LIMIT {
                            log::trace!(
                                "tolerating transient 0.2x sample {}/{} after action: state={} frame_id={}",
                                *samples,
                                POINT_TWO_GRACE_LIMIT,
                                view.battle_state,
                                view.frame_id,
                            );
                            return false;
                        }
                        // 第 LIMIT 条仍未恢复 1x：让下面的统一错误路径生成可读原因。
                    }
                }
                self.fail(AbortReason::NotOneX {
                    state: view.battle_state.clone(),
                });
                return false;
            }
            // Resume 发出后，尺子管线里可能还积压着派发前拍到的 paused 画面。
            // 它们虽然拥有新的 frame_id，却不能证明游戏仍处于暂停态，也不能
            // 更新逻辑游标；否则状态机会在同一运行区间连续重发 ReleasePause。
            if matches!(self.phase, Phase::Cruising | Phase::Stepping) && self.resume_in_flight {
                if view.paused == Some(false) {
                    self.resume_in_flight = false;
                    log::info!(
                        "runtime resume confirmed: frame_id={} elapsed={}",
                        view.frame_id,
                        view.elapsed
                    );
                } else {
                    log::debug!(
                        "runtime resume waiting for running state: frame_id={} state={} elapsed={}",
                        view.frame_id,
                        view.battle_state,
                        view.elapsed
                    );
                    return false;
                }
            }
            if view.elapsed < self.cursor {
                self.fail(AbortReason::FrameWentBackwards {
                    from: self.cursor,
                    to: view.elapsed,
                });
                return false;
            }
        }

        match self.phase {
            Phase::WaitingBattle => self.observe_waiting(view),
            Phase::ConfirmingZero => self.observe_confirming(view),
            Phase::Cruising | Phase::Stepping => self.observe_running(view),
            _ => {}
        }
        true
    }

    fn observe_waiting(&mut self, view: &FrameView) {
        if view.in_battle {
            if !view.one_x {
                self.fail(AbortReason::NotOneX {
                    state: view.battle_state.clone(),
                });
                return;
            }
            self.cursor = view.elapsed;
        }
    }

    fn observe_confirming(&mut self, view: &FrameView) {
        self.cursor = view.elapsed;
        if view.paused == Some(true) {
            self.pause_probe = None;
            self.paused = true;
            self.origin = view.elapsed;

            let first = self.copilot.actions[0].frame;
            if self.origin > first {
                self.fail(AbortReason::MissedStart {
                    origin: self.origin,
                    first_action: first,
                });
                return;
            }
            if self.origin != 0 {
                // 不强求正好是第 0 帧。真正的不变量是"停在一个已知的绝对帧，
                // 且不晚于第一个动作"—— 尺子的计数本来就是从开局算起的。
                log::warn!(
                    "battle-start pause landed on frame {} instead of 0 (still fine)",
                    self.origin
                );
            }
            self.phase = Phase::BindingFormation;
            return;
        }

        // 还在跑：看看暂停键是不是压根没生效。
        match self.pause_probe {
            None => self.pause_probe = Some((view.elapsed, 0)),
            Some((from, count)) => {
                if view.elapsed > from {
                    let count = count + 1;
                    if count >= PAUSE_PROBE_LIMIT {
                        self.fail(AbortReason::OpeningPauseIneffective);
                    } else {
                        self.pause_probe = Some((view.elapsed, count));
                    }
                }
            }
        }
    }

    fn observe_running(&mut self, view: &FrameView) {
        let Some(target) = self.target() else {
            return;
        };
        self.cursor = view.elapsed;

        // 尺子既可能正常看到 running → paused，也可能先吐出一条脉冲前已进入
        // 分析管线的旧 paused。后者不能立即结算，否则下一条真实 running 到来时
        // 状态机已经进入动作阶段，尚未收尾的脉冲还会继续推进游戏。
        let pulse_can_settle = if self.pulse_in_flight {
            match view.paused {
                Some(false) => {
                    self.paused = false;
                    if !self.pulse_seen_running {
                        log::debug!(
                            "pulse observed running transition: frame_id={} elapsed={}",
                            view.frame_id,
                            view.elapsed
                        );
                    }
                    self.pulse_seen_running = true;
                    self.pulse_paused_samples = 0;
                    false
                }
                Some(true) => {
                    self.paused = true;
                    self.pulse_paused_samples += 1;
                    let confirmed = self.pulse_seen_running
                        || self.pulse_paused_samples >= PULSE_PAUSED_CONFIRM_SAMPLES;
                    if !confirmed {
                        log::debug!(
                            "pulse deferred single paused candidate: frame_id={} elapsed={}",
                            view.frame_id,
                            view.elapsed
                        );
                    }
                    confirmed
                }
                None => {
                    self.pulse_paused_samples = 0;
                    false
                }
            }
        } else {
            false
        };

        if self.cursor > target {
            self.fail(AbortReason::OvershotTarget {
                target,
                actual: self.cursor,
            });
            return;
        }

        if pulse_can_settle {
            let delta = self.cursor - self.pulse_from;
            log::debug!(
                "pulse settled: frame_id={} elapsed={} delta={} evidence={}",
                view.frame_id,
                view.elapsed,
                delta,
                if self.pulse_seen_running {
                    "running_then_paused"
                } else {
                    "two_paused_samples"
                }
            );
            self.tuner.observe(delta);
            self.pulse_in_flight = false;
            self.pulse_settle_samples = 0;
            self.pulse_seen_running = false;
            self.pulse_paused_samples = 0;
        }

        if self.pulse_in_flight {
            self.pulse_settle_samples += 1;
            if self.pulse_settle_samples >= PULSE_SETTLE_SAMPLE_LIMIT {
                self.fail(AbortReason::PulseDidNotSettle {
                    state: view.battle_state.clone(),
                });
            }
            return;
        }

        if self.pause_in_flight {
            if view.paused == Some(true) {
                let advanced = self
                    .pause_probe
                    .map_or(0, |(from, _)| self.cursor.saturating_sub(from));
                log::info!(
                    "runtime pause confirmed: frame_id={} elapsed={} advanced={}",
                    view.frame_id,
                    view.elapsed,
                    advanced
                );
                self.pause_in_flight = false;
                self.pause_probe = None;
                self.paused = true;
            } else {
                self.paused = false;
                if let Some((from, _)) = self.pause_probe {
                    let advanced = self.cursor.saturating_sub(from);
                    log::debug!(
                        "runtime pause pending: frame_id={} elapsed={} from={} advanced={}",
                        view.frame_id,
                        view.elapsed,
                        from,
                        advanced
                    );
                    if advanced >= RUNTIME_PAUSE_MAX_ADVANCE_FRAMES {
                        self.fail(AbortReason::PauseIneffective);
                    } else {
                        self.pause_probe = Some((from, advanced as u32));
                    }
                }
            }
            return;
        }
        if let Some(paused) = view.paused {
            self.paused = paused;
        }
    }

    /// 问下一步该做什么。
    ///
    /// 这个函数只读状态、不改状态（除了阶段推进这种确定性转移），
    /// 调用方执行完命令后要用 [`Self::completed`] 回报。
    pub fn next_command(&mut self) -> Command {
        match self.phase {
            Phase::Idle => Command::Idle,
            Phase::Aborted => Command::Abort(
                self.abort
                    .clone()
                    .unwrap_or(AbortReason::ExecutionFailed("未知原因".into())),
            ),
            Phase::Finished => Command::Idle,

            Phase::WaitingBattle => Command::ArmBattleStart,

            Phase::ConfirmingZero => Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            },

            Phase::BindingFormation => Command::BindFormation,

            Phase::Cruising | Phase::Stepping | Phase::Injecting => self.next_running_command(),
        }
    }

    fn next_running_command(&mut self) -> Command {
        let Some(target) = self.target() else {
            self.phase = Phase::Finished;
            return Command::Finish;
        };

        let remaining = target - self.cursor;

        // Resume 是一个需要尺子 running 回执的事务。回执到来之前，即使缓存中
        // 仍有新的 paused 样本，也只能继续等待，不能重发 Resume 或插入其他输入。
        if self.resume_in_flight {
            self.phase = Phase::Cruising;
            return Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            };
        }

        // 脉冲本身已经包含重暂停边沿；稳定前禁止补发 Pause，否则额外切换会把游戏重新放开。
        if self.pulse_in_flight {
            self.phase = Phase::Stepping;
            return Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            };
        }
        if self.pause_in_flight {
            self.phase = Phase::Stepping;
            return Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            };
        }

        // 已经到点了：确保处于暂停态，然后注入。
        if remaining == 0 {
            if !self.paused {
                self.phase = Phase::Stepping;
                return Command::Pause;
            }
            self.phase = Phase::Injecting;
            return Command::Execute {
                index: self.action_index,
            };
        }

        // 还远：恢复运行，巡航过去。
        if remaining > FRAME_LEAD {
            if self.paused {
                self.phase = Phase::Cruising;
                return Command::Resume;
            }
            self.phase = Phase::Cruising;
            return Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            };
        }

        // 进入提前暂停区间。
        if !self.paused {
            self.phase = Phase::Stepping;
            return Command::Pause;
        }
        // 已暂停且还没到点：发一次脉冲，然后等结果。
        self.phase = Phase::Stepping;
        // 最后一步是唯一致命的一步：这里 +2 就直接越过目标、整场作废
        // （更早的 +2 只是离目标更近，无害；remaining==2 时 +2 甚至正好落在目标上）。
        // 实测推进量几乎不随 gap 变化，但脉冲的解暂停窗口 ∝ gap ——
        // 所以最后一步固定用最短 gap，把暴露窗口压到最小，宁可多空转几次。
        let gap = if remaining == 1 {
            Duration::from_millis(u64::from(crate::stepping::MIN_GAP_MS))
        } else {
            self.tuner.gap()
        };
        Command::Pulse { gap }
    }

    /// 回报一条命令已经执行完。
    pub fn completed(&mut self, completion: Completion) {
        match completion {
            Completion::OpeningPauseDelegated => {
                if self.phase == Phase::WaitingBattle {
                    self.phase = Phase::ConfirmingZero;
                    self.pause_probe = None;
                }
            }
            Completion::PauseSent => {
                // 运行期 Pause 只表示按键已发送；必须等尺子确认后才能进入慢速等待。
                self.paused = false;
                self.resume_in_flight = false;
                self.pause_in_flight = true;
                self.pause_probe = Some((self.cursor, 0));
            }
            Completion::ResumeSent => {
                self.paused = false;
                self.resume_in_flight = true;
                self.pause_in_flight = false;
                self.pause_probe = None;
                self.pulse_settle_samples = 0;
                self.pulse_seen_running = false;
                self.pulse_paused_samples = 0;
            }
            Completion::PulseSent => {
                self.resume_in_flight = false;
                self.pulse_in_flight = true;
                self.pulse_settle_samples = 0;
                self.pulse_seen_running = false;
                self.pulse_paused_samples = 0;
                self.pulse_from = self.cursor;
            }
            // 这两个都回到推进逻辑，由 next_running_command 决定下一步：
            // 同帧还有动作就接着注入，帧号更大就巡航/推进，没动作了就收尾。
            // 注意**不要**在这里直接置 Finished —— Runner 仍需收到无输入的 Finish
            // 命令，以便记录完成状态并退出循环。
            Completion::FormationBound => self.phase = Phase::Stepping,
            Completion::ActionExecuted => {
                // 只有真正会向游戏发输入的动作才可能触发选中/拖拽慢放。
                // Output 不改变游戏状态，不应打开 0.2x 宽限窗口。
                if self
                    .copilot
                    .actions
                    .get(self.action_index)
                    .is_some_and(|action| {
                        matches!(
                            action.kind,
                            ActionType::Deploy | ActionType::UseSkill | ActionType::Retreat
                        )
                    })
                {
                    self.point_two_samples = Some(0);
                } else {
                    self.point_two_samples = None;
                }
                self.action_index += 1;
                self.phase = Phase::Stepping;
            }
        }
    }

    /// 执行动作时出错。
    pub fn execution_failed(&mut self, what: impl Into<String>) {
        self.fail(AbortReason::ExecutionFailed(what.into()));
    }

    /// 等帧超时。
    pub fn frame_timeout(&mut self) {
        self.fail(AbortReason::FrameTimeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::Copilot;

    const JOB: &str = r#"{
        "stage_name": "1-7",
        "frame_replicator": { "after_last_action": "resume" },
        "opers": [{"name": "山"}],
        "actions": [
            {"type":"Deploy","frame":0,"name":"山","location":[3,2]},
            {"type":"Skill","frame":60,"name":"山"},
            {"type":"Retreat","frame":200,"name":"山"}
        ]
    }"#;

    fn machine() -> Machine {
        Machine::new(Copilot::parse(JOB).unwrap())
    }

    fn view(frame_id: u64, elapsed: i64, paused: bool) -> FrameView {
        FrameView {
            frame_id,
            elapsed,
            in_battle: true,
            paused: Some(paused),
            one_x: true,
            trustworthy: true,
            battle_state: if paused { "1x_paused" } else { "1x_running" }.into(),
        }
    }

    /// 走完"等开局 → AFA 自动暂停 → 编队"，返回一台停在第 0 帧、绑定已完成的状态机。
    fn machine_at_zero() -> Machine {
        let mut m = machine();
        m.start();
        assert_eq!(m.next_command(), Command::ArmBattleStart);
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        assert_eq!(m.phase(), Phase::ConfirmingZero);
        m.observe(&view(2, 0, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
        assert_eq!(m.next_command(), Command::BindFormation);
        m.completed(Completion::FormationBound);
        m
    }

    fn confirm_runtime_pause(m: &mut Machine, frame_id: u64, elapsed: i64) {
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
        assert!(m.observe(&view(frame_id, elapsed, true)));
        assert!(!m.pause_in_flight);
        assert!(m.paused);
    }

    fn confirm_pulse_without_running(m: &mut Machine, frame_id: &mut u64, elapsed: i64) {
        for _ in 0..PULSE_PAUSED_CONFIRM_SAMPLES {
            assert!(m.observe(&view(*frame_id, elapsed, true)));
            *frame_id += 1;
        }
        assert!(!m.pulse_in_flight);
    }

    #[test]
    fn happy_path_reaches_frame_zero_and_injects_immediately() {
        let mut m = machine_at_zero();
        assert_eq!(m.origin(), 0);
        assert_eq!(m.cursor(), 0);
        // 第一个动作就在第 0 帧，应当立刻注入
        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        assert_eq!(m.progress(), (1, 3));
        assert_eq!(m.target(), Some(60));
    }

    #[test]
    fn opening_pause_is_delegated_to_afa_without_a_pause_command() {
        let mut m = machine();
        m.start();
        assert_eq!(m.next_command(), Command::ArmBattleStart);

        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        assert_eq!(m.phase(), Phase::ConfirmingZero);
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));

        m.observe(&view(2, 0, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
    }

    #[test]
    fn far_target_cruises_then_pauses_early_then_steps() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted); // 第 0 帧的部署做完，目标变成 60

        // 60 - 0 = 60 > FRAME_LEAD，且当前是暂停态 => 先恢复运行
        assert_eq!(m.next_command(), Command::Resume);
        m.completed(Completion::ResumeSent);
        assert_eq!(m.phase(), Phase::Cruising);

        // 巡航：只等帧，不做别的
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
        m.observe(&view(10, 30, false));
        assert_eq!(m.cursor(), 30);
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));

        // 进入 FRAME_LEAD 区间 => 提前暂停
        m.observe(&view(11, 60 - FRAME_LEAD, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 12, 60 - FRAME_LEAD);
        assert_eq!(m.phase(), Phase::Stepping);

        // 之后就是逐帧脉冲
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
    }

    #[test]
    fn resume_waits_for_running_and_ignores_new_paused_backlog() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted); // 第 0 帧动作完成，下一目标为 60

        assert_eq!(m.next_command(), Command::Resume);
        m.completed(Completion::ResumeSent);

        // PRE_ACTION_PAUSE 期间已经进入尺子管线的暂停画面仍会带着新的 frame_id
        // 陆续到达。它们不能证明本次 Resume 失败，也不能触发重复 Resume。
        assert!(!m.observe(&view(3, 0, true)));
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
        assert!(!m.observe(&view(4, 0, true)));
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));

        // 只有第一条可信运行态样本才确认 Resume 已经落地。
        assert!(m.observe(&view(5, 2, false)));
        assert!(!m.paused);

        // 此时仍离目标很远，只能继续巡航等待，不能再次 Resume。
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
    }

    #[test]
    fn final_step_uses_the_minimum_gap() {
        // 最后一步（remaining == 1）必须用最短 gap：那是唯一 +2 会致命的位置，
        // 解暂停窗口越短越安全。之前的步子照常用整定器的 gap。
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted); // 目标变成 60
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 58, false)); // remaining = 2
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 58);

        let Command::Pulse { gap } = m.next_command() else {
            panic!("应当发脉冲");
        };
        assert_eq!(
            gap.as_millis() as u32,
            m.tuner().gap_ms(),
            "remaining=2 时用整定器的 gap"
        );
        m.completed(Completion::PulseSent);
        let mut frame_id = 12;
        confirm_pulse_without_running(&mut m, &mut frame_id, 59); // remaining = 1

        let Command::Pulse { gap } = m.next_command() else {
            panic!("应当发脉冲");
        };
        assert_eq!(
            gap.as_millis() as u32,
            crate::stepping::MIN_GAP_MS,
            "remaining=1 时必须用最短 gap"
        );
    }

    #[test]
    fn stepping_advances_one_frame_at_a_time_until_target() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 55, false)); // 距目标 5 帧，进入提前暂停区间
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 55);

        let mut frame_id = 12;
        for elapsed in 56..=60 {
            let cmd = m.next_command();
            assert!(
                matches!(cmd, Command::Pulse { .. }),
                "第 {elapsed} 帧前应当发脉冲，实际 {cmd:?}"
            );
            m.completed(Completion::PulseSent);
            confirm_pulse_without_running(&mut m, &mut frame_id, elapsed);
        }
        assert_eq!(m.cursor(), 60);
        assert_eq!(m.next_command(), Command::Execute { index: 1 });
    }

    #[test]
    fn pulse_running_transition_waits_for_the_final_paused_sample() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 55, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 55);

        assert!(matches!(m.next_command(), Command::Pulse { .. }));
        m.completed(Completion::PulseSent);

        // 脉冲顺序是 ESC → pauseBattle，尺子可能先看到前半段的运行态。
        m.observe(&view(12, 56, false));
        assert_eq!(m.tuner().pulses, 0, "中间态不能提前结算脉冲");
        assert!(
            matches!(m.next_command(), Command::AwaitFrame { .. }),
            "脉冲中间态不能补发 Pause，否则会把游戏再次切回运行"
        );

        m.observe(&view(13, 56, true));
        assert_eq!(m.tuner().pulses, 1, "最终暂停态才结算一次脉冲");
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
    }

    #[test]
    fn pulse_does_not_settle_on_a_single_stale_paused_sample() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 59, false));
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 59);

        assert!(matches!(m.next_command(), Command::Pulse { .. }));
        m.completed(Completion::PulseSent);

        // 实机复现顺序：脉冲后先冒出一条旧 paused，随后才出现真实 running。
        // 第一条 paused 不能让状态机进入 Execute，否则动作前会发现 running，
        // 而这发尚未收尾的脉冲还会继续把游戏推到下一帧。
        m.observe(&view(12, 60, true));
        assert_eq!(m.tuner().pulses, 0, "单条旧 paused 不能结算脉冲");
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));

        m.observe(&view(13, 60, false));
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));

        m.observe(&view(14, 60, true));
        assert_eq!(m.tuner().pulses, 1);
        assert_eq!(m.next_command(), Command::Execute { index: 1 });
    }

    #[test]
    fn runtime_pause_must_be_confirmed_before_the_first_pulse() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 55, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);

        assert!(
            matches!(m.next_command(), Command::AwaitFrame { .. }),
            "发送 Pause 不等于游戏已经暂停，不能直接进入 1 秒等待和脉冲"
        );
        m.observe(&view(11, 55, true));
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
    }

    #[test]
    fn runtime_pause_probe_ignores_repeated_running_samples_at_the_same_frame() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 52, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);

        // AFA 的 ESC 保持和尺子分析期间会连续产生多个新 frame_id，但逻辑帧可能
        // 仍停在原地。这些重复样本不是“暂停无效”的证据，不能消耗安全窗口。
        for frame_id in 11_u64..=14 {
            m.observe(&view(frame_id, 52, false));
        }
        assert_ne!(m.phase(), Phase::Aborted);
        assert!(m.pause_in_flight);

        m.observe(&view(15, 52, true));
        assert!(!m.pause_in_flight);
        assert!(m.paused);
    }

    #[test]
    fn missed_runtime_pause_aborts_before_the_one_second_dwell() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 52, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);

        for advanced in 1..=RUNTIME_PAUSE_MAX_ADVANCE_FRAMES {
            m.observe(&view(10 + advanced as u64, 52 + advanced, false));
        }
        assert_eq!(m.cursor(), 58, "应在目标前保留 2 帧安全余量");
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::PauseIneffective)
        ));
    }

    #[test]
    fn pulse_that_never_returns_to_paused_aborts_boundedly() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 55, false));
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 55);
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
        m.completed(Completion::PulseSent);

        for i in 0..PULSE_SETTLE_SAMPLE_LIMIT {
            m.observe(&view(12 + u64::from(i), 56, false));
        }
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::PulseDidNotSettle { .. })
        ));
    }

    #[test]
    fn empty_pulses_are_harmless_and_retried() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 58, false));
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 58);

        // 连发三次脉冲但帧数纹丝不动 —— 必须继续重试，而不是中止。
        // 实测空脉冲率约 42%，连着几次是常态；间隔也不该因此漂移
        // （实测证明加大间隔对推进量没有帮助，只增加跨帧风险）。
        let gap_before = m.tuner().gap_ms();
        let mut frame_id = 12;
        for _ in 0..3 {
            assert!(matches!(m.next_command(), Command::Pulse { .. }));
            m.completed(Completion::PulseSent);
            confirm_pulse_without_running(&mut m, &mut frame_id, 58);
        }
        assert_eq!(m.phase(), Phase::Stepping, "空脉冲不应中止");
        assert_eq!(m.tuner().gap_ms(), gap_before, "零星空脉冲不该改变间隔");
        assert_eq!(m.tuner().zero_frame_pulses, 3);
        assert!(!m.tuner().had_overshoot());

        // 继续空转到第 5 次才小幅加大 —— 那才说明间隔可能真的偏小
        for _ in 0..2 {
            assert!(matches!(m.next_command(), Command::Pulse { .. }));
            m.completed(Completion::PulseSent);
            confirm_pulse_without_running(&mut m, &mut frame_id, 58);
        }
        assert!(
            m.tuner().gap_ms() > gap_before,
            "连续 5 次空脉冲后应当试探性加大"
        );
    }

    #[test]
    fn overshooting_the_target_aborts() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 59, false));
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 59);
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
        m.completed(Completion::PulseSent);

        // 一发脉冲跨了两帧，直接越过 60
        m.observe(&view(12, 61, true));
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::OvershotTarget {
                target: 60,
                actual: 61
            })
        ));
        // 报错要说清楚为什么没法补救
        assert!(m.abort_reason().unwrap().to_string().contains("倒带"));
    }

    #[test]
    fn untrustworthy_samples_are_dropped_entirely() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 55, false));
        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 11, 55);
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
        m.completed(Completion::PulseSent);

        // 光标遮挡：elapsed 停在旧值且不可信。若被当成"没推进"，
        // 整定器会误加间隔，最终导致跨帧。
        let mut blocked = view(12, 55, true);
        blocked.trustworthy = false;
        assert!(!m.observe(&blocked), "不可信样本必须被丢弃");
        assert_eq!(m.tuner().pulses, 0, "不可信样本不该喂给整定器");
        assert_eq!(m.cursor(), 55);

        // 恢复正常后才计数
        let mut frame_id = 13;
        confirm_pulse_without_running(&mut m, &mut frame_id, 56);
        assert_eq!(m.tuner().pulses, 1);
        assert_eq!(m.cursor(), 56);
    }

    #[test]
    fn frame_cursor_advances_even_for_rejected_samples() {
        // 运行循环拿 last_frame_id() 去 wait_next；如果被拒绝的样本不推进游标，
        // 同一条不可信样本会被反复递送，等待循环就死锁在原地。
        let mut m = machine_at_zero();
        assert_eq!(m.last_frame_id(), 2);

        let mut blocked = view(7, 0, true);
        blocked.trustworthy = false;
        assert!(!m.observe(&blocked), "不可信样本应被拒绝");
        assert_eq!(m.last_frame_id(), 7, "但游标必须照样前进");
    }

    #[test]
    fn stale_frame_ids_are_ignored() {
        let mut m = machine_at_zero();
        assert!(!m.observe(&view(2, 0, true)), "重复的 frame_id 应当被忽略");
        assert!(!m.observe(&view(1, 0, true)), "更旧的 frame_id 也要忽略");
    }

    #[test]
    fn missing_the_start_aborts_with_a_clear_message() {
        let job = JOB.replace(r#""frame":0"#, r#""frame":5"#);
        let mut m = Machine::new(Copilot::parse(&job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        // 实际停在第 9 帧，已经晚于第一个动作（第 5 帧）
        m.observe(&view(2, 9, true));
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::MissedStart {
                origin: 9,
                first_action: 5
            })
        ));
    }

    #[test]
    fn landing_a_few_frames_late_is_accepted() {
        // 停在第 3 帧、第一个动作在第 5 帧 => 可以接受，正常往下走
        let job = JOB.replace(r#""frame":0"#, r#""frame":5"#);
        let mut m = Machine::new(Copilot::parse(&job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        m.observe(&view(2, 3, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
        assert_eq!(m.origin(), 3);
        assert_eq!(m.cursor(), 3);
    }

    #[test]
    fn afa_opening_pause_that_never_takes_effect_aborts() {
        let mut m = machine();
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        // AFA 没有自动暂停，帧数一直在涨。
        // 要涨满 PAUSE_PROBE_LIMIT 条才判定 —— 前几条是按键与分析的固有延迟，
        // 帧数理应还在增长，不能急着判死刑。
        for i in 0..PAUSE_PROBE_LIMIT + 1 {
            let elapsed = i64::from(3 * (i + 1));
            m.observe(&view(10 + u64::from(i), elapsed, false));
        }
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::OpeningPauseIneffective)
        ));
    }

    #[test]
    fn short_growth_before_afa_opening_pause_is_tolerated() {
        // AFA 自动暂停生效前头几条样本帧数仍在增长是正常的；
        // 只要在 PAUSE_PROBE_LIMIT 之内出现了暂停态样本，就应当正常进入编队阶段。
        // 首个动作放在第 10 帧 —— 停在第 5 帧不算错过开局。
        let job = JOB.replace(r#""frame":0"#, r#""frame":10"#);
        let mut m = Machine::new(Copilot::parse(&job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        // 4 条仍在增长的样本（少于上限）
        for i in 0..4_u64 {
            m.observe(&view(10 + i, 1 + i as i64, false));
        }
        assert_eq!(m.phase(), Phase::ConfirmingZero, "尚未到上限，不该中止");
        // 暂停终于生效，停在第 5 帧
        m.observe(&view(20, 5, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
        assert_eq!(m.origin(), 5);
    }

    #[test]
    fn leaving_battle_aborts() {
        let mut m = machine_at_zero();
        let mut gone = view(20, 0, true);
        gone.in_battle = false;
        gone.battle_state = "before_or_after_battle".into();
        m.observe(&gone);
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::LeftBattle { .. })
        ));
    }

    #[test]
    fn speed_change_aborts() {
        let mut m = machine_at_zero();
        let mut fast = view(20, 0, false);
        fast.one_x = false;
        fast.battle_state = "2x_running".into();
        m.observe(&fast);
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::NotOneX { .. })
        ));
        assert!(m.abort_reason().unwrap().to_string().contains("1x"));
    }

    #[test]
    fn transient_point_two_speed_during_action_is_tolerated() {
        let mut m = machine_at_zero();
        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        let mut slow = view(10, 0, true);
        slow.one_x = false;
        slow.battle_state = "0.2x_running".into();

        // 选中干员时可能短暂经过 0.2x_running / 0.2x_paused；
        // 只要随后恢复 1x，就不应把这段过渡判成用户误开变速。
        for frame_id in 10..17 {
            slow.frame_id = frame_id;
            assert!(!m.observe(&slow));
            assert_ne!(m.phase(), Phase::Aborted);
        }

        slow.frame_id = 17;
        slow.battle_state = "1x_paused".into();
        slow.one_x = true;
        assert!(m.observe(&slow));
        assert_ne!(m.phase(), Phase::Aborted);
    }

    #[test]
    fn persistent_point_two_speed_still_aborts() {
        let mut m = machine_at_zero();
        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        let mut slow = view(10, 0, true);
        slow.one_x = false;
        slow.battle_state = "0.2x_running".into();

        for frame_id in 10..(10 + u64::from(POINT_TWO_GRACE_LIMIT)) {
            slow.frame_id = frame_id;
            m.observe(&slow);
        }

        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::NotOneX { .. })
        ));
    }

    #[test]
    fn point_two_before_action_still_aborts() {
        let mut m = machine_at_zero();
        let mut slow = view(10, 0, true);
        slow.one_x = false;
        slow.battle_state = "0.2x_running".into();

        m.observe(&slow);

        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::NotOneX { .. })
        ));
    }

    #[test]
    fn backwards_frames_abort() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 40, false));
        m.observe(&view(11, 30, false));
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::FrameWentBackwards { from: 40, to: 30 })
        ));
    }

    #[test]
    fn same_frame_actions_execute_back_to_back() {
        let job = r#"{
            "stage_name": "1-7",
            "frame_replicator": {},
            "opers": [{"name":"山"},{"name":"能天使"}],
            "actions": [
                {"type":"Deploy","frame":0,"name":"山","location":[3,2]},
                {"type":"Deploy","frame":0,"name":"能天使","location":[4,1]}
            ]
        }"#;
        let mut m = Machine::new(Copilot::parse(job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        m.observe(&view(2, 0, true));
        m.completed(Completion::FormationBound);

        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        // 同一帧的第二个动作：不该插入任何脉冲或恢复
        assert_eq!(m.next_command(), Command::Execute { index: 1 });
        m.completed(Completion::ActionExecuted);
        assert_eq!(m.next_command(), Command::Finish);
    }

    #[test]
    fn final_action_finishes_immediately_without_any_follow_up_input() {
        let job = r#"{
            "stage_name": "1-7",
            "frame_replicator": {"after_last_action":"resume"},
            "opers": [{"name":"山"}],
            "actions": [
                {"type":"Deploy","frame":0,"name":"山","location":[3,2]}
            ]
        }"#;
        let mut m = Machine::new(Copilot::parse(job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::OpeningPauseDelegated);
        m.observe(&view(2, 0, true));
        m.completed(Completion::FormationBound);

        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        assert_eq!(
            m.next_command(),
            Command::Finish,
            "最后一个动作已经给出最终输入，之后必须直接结束，不能再等帧、暂停或恢复"
        );
    }

    #[test]
    fn user_abort_is_terminal() {
        let mut m = machine_at_zero();
        m.user_abort();
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.next_command(),
            Command::Abort(AbortReason::UserAborted)
        ));
        // 中止后不再接受任何样本
        assert!(!m.observe(&view(99, 100, true)));
    }

    #[test]
    fn frame_lead_covers_the_observed_round_trip() {
        // FRAME_LEAD 必须大于"尺子采样 + 分析 + 我们注入暂停"的往返延迟，
        // 否则从"发现接近目标"到"暂停真正生效"之间游戏就已经冲过去了。
        // 用一个具体的时间预算把这个常量钉住：改动它时这条测试会拦下来。
        let frame_ms = crate::Speed::One.frame_millis();
        let worst_round_trip_ms = 16.7 /* WGC 采样 */ + 5.0 /* 分析 */ + 5.0 /* 注入 */;
        let needed_frames = (worst_round_trip_ms / frame_ms).ceil() as i64;
        assert!(
            FRAME_LEAD >= needed_frames * 2,
            "FRAME_LEAD={FRAME_LEAD} 不足以覆盖 {needed_frames} 帧的往返延迟（还要留一倍余量）"
        );
        // 每步逐帧推进约 60ms，余量太大会明显拖慢每个动作
        let worst_case_ms = FRAME_LEAD as f64 * 60.0;
        assert!(
            worst_case_ms <= 600.0,
            "每个动作最坏要花 {worst_case_ms}ms 对帧，太慢了"
        );
    }
}
