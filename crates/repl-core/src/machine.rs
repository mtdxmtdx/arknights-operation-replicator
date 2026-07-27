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
    copilot::{ActionType, AfterLastAction, Copilot},
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
    /// 已发出开局暂停，等一条可信的暂停态样本来确认。
    ConfirmingZero,
    /// 已发出运行期暂停，等待尺子确认游戏已暂停。
    ConfirmingPause,
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
            Self::ConfirmingPause => "确认暂停",
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
    /// 武装开局触发器并等待战斗开始。
    ///
    /// 调用方应当同时跑两条路径：像素触发器（盯倍速按钮变白，最低延迟）
    /// 和尺子快照兜底。任一命中就立刻发暂停键，然后回报 [`Completion::PauseSent`]。
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
    /// 收尾。
    Finish { resume: bool },
    /// 中止。
    Abort(AbortReason),
    /// 已经结束，没有更多事要做。
    Idle,
}

/// 调用方对一条命令的回报。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    /// 已发出暂停键，等待尺子确认。
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
    /// 运行期暂停键已经发出，但还没有收到尺子的暂停确认。
    pause_pending: bool,
    /// 当前接近目标帧的暂停请求已经重试了多少次。
    pause_attempts: u8,
    /// 已消费到的最新 `frame_id`。
    last_frame_id: u64,
    /// 刚发过一次脉冲，等着看它推进了几帧。
    pulse_in_flight: bool,
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

/// 接近动作目标时的运行期暂停确认窗口要短于开场窗口，给重试留下安全帧余量。
const RUNTIME_PAUSE_PROBE_LIMIT: u32 = 4;

/// 单次暂停确认失败后允许重新发送的次数。
const PAUSE_RETRY_LIMIT: u8 = 2;

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
            pause_pending: false,
            pause_attempts: 0,
            last_frame_id: 0,
            pulse_in_flight: false,
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

    /// 返回最近一条可信尺子样本确认的暂停状态。
    pub fn is_paused(&self) -> bool {
        self.paused
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
            Phase::Cruising | Phase::Stepping | Phase::ConfirmingPause => {
                self.observe_running(view)
            }
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
            self.pause_pending = false;
            self.pause_attempts = 0;
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
                        self.fail(AbortReason::PauseIneffective);
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
        let previous = self.cursor;
        self.cursor = view.elapsed;

        if self.pulse_in_flight {
            self.tuner.observe(self.cursor - self.pulse_from);
            self.pulse_in_flight = false;
        }
        let _ = previous;

        if self.cursor > target {
            self.fail(AbortReason::OvershotTarget {
                target,
                actual: self.cursor,
            });
            return;
        }

        if self.pause_pending {
            if view.paused == Some(true) {
                self.pause_pending = false;
                self.pause_probe = None;
                self.pause_attempts = 0;
                self.paused = true;
                self.phase = Phase::Stepping;
                return;
            }

            self.paused = false;
            if let Some((from, count)) = self.pause_probe {
                if self.cursor > from {
                    let count = count + 1;
                    if count >= RUNTIME_PAUSE_PROBE_LIMIT {
                        if self.pause_attempts < PAUSE_RETRY_LIMIT {
                            self.pause_attempts += 1;
                            self.pause_pending = false;
                            self.pause_probe = None;
                            self.phase = Phase::Stepping;
                            log::warn!(
                                "pause confirmation failed; retrying ({}/{}) at frame {}",
                                self.pause_attempts,
                                PAUSE_RETRY_LIMIT,
                                self.cursor,
                            );
                        } else {
                            self.fail(AbortReason::PauseIneffective);
                        }
                    } else {
                        self.pause_probe = Some((self.cursor, count));
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
            Phase::ConfirmingPause => Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            },

            Phase::BindingFormation => Command::BindFormation,

            Phase::Cruising | Phase::Stepping | Phase::Injecting => self.next_running_command(),
        }
    }

    fn next_running_command(&mut self) -> Command {
        let Some(target) = self.target() else {
            self.phase = Phase::Finished;
            return Command::Finish {
                resume: self.copilot.meta.after_last_action == AfterLastAction::Resume,
            };
        };

        let remaining = target - self.cursor;

        if self.pause_pending {
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
        if self.pulse_in_flight {
            return Command::AwaitFrame {
                timeout: AWAIT_TIMEOUT,
            };
        }
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
            Completion::PauseSent => {
                if self.phase == Phase::WaitingBattle {
                    self.phase = Phase::ConfirmingZero;
                    self.pause_probe = None;
                } else {
                    // 运行期暂停必须等尺子确认，不能把按键发送成功当成游戏已暂停。
                    self.paused = false;
                    self.pause_pending = true;
                    self.pause_probe = Some((self.cursor, 0));
                    self.phase = Phase::ConfirmingPause;
                }
            }
            Completion::ResumeSent => {
                self.paused = false;
                self.pause_pending = false;
                self.pause_probe = None;
                self.pause_attempts = 0;
            }
            Completion::PulseSent => {
                self.pulse_in_flight = true;
                self.pulse_from = self.cursor;
            }
            // 这两个都回到推进逻辑，由 next_running_command 决定下一步：
            // 同帧还有动作就接着注入，帧号更大就巡航/推进，没动作了就收尾。
            // 注意**不要**在这里直接置 Finished —— 那样 Finish 命令就发不出去了，
            // 调用方也就不知道该恢复运行还是保持暂停。
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

    /// 走完"等开局 → 第零帧暂停 → 编队"，返回一台停在第 0 帧、绑定已完成的状态机。
    fn machine_at_zero() -> Machine {
        let mut m = machine();
        m.start();
        assert_eq!(m.next_command(), Command::ArmBattleStart);
        m.observe(&view(1, 0, false));
        m.completed(Completion::PauseSent);
        assert_eq!(m.phase(), Phase::ConfirmingZero);
        m.observe(&view(2, 0, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
        assert_eq!(m.next_command(), Command::BindFormation);
        m.completed(Completion::FormationBound);
        m
    }

    fn confirm_runtime_pause(m: &mut Machine, frame_id: u64, elapsed: i64) {
        assert_eq!(m.phase(), Phase::ConfirmingPause);
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
        assert!(m.observe(&view(frame_id, elapsed, true)));
        assert_eq!(m.phase(), Phase::Stepping);
        assert!(m.is_paused());
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

        // 之后就是逐帧脉冲
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
    }

    #[test]
    fn missed_runtime_pause_never_allows_a_blind_pulse() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 52, false));
        assert_eq!(m.next_command(), Command::Pause);
        m.completed(Completion::PauseSent);

        // A missed pause must not turn the one-second dwell into a blind pulse.
        assert!(matches!(m.next_command(), Command::AwaitFrame { .. }));
        for frame_id in 11_u64..=40 {
            m.observe(&view(frame_id, 52 + (frame_id - 10) as i64, false));
            if m.phase() == Phase::Aborted {
                break;
            }
            assert!(matches!(
                m.next_command(),
                Command::AwaitFrame { .. } | Command::Pause
            ));
        }
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::OvershotTarget { .. })
        ));
    }

    #[test]
    fn runtime_pause_retries_before_the_target_is_lost() {
        let mut m = machine_at_zero();
        m.completed(Completion::ActionExecuted);
        m.completed(Completion::ResumeSent);
        m.observe(&view(10, 52, false));
        m.completed(Completion::PauseSent);

        for frame_id in 11_u64..=14 {
            m.observe(&view(frame_id, 52 + (frame_id - 10) as i64, false));
        }
        assert_eq!(m.cursor(), 56);
        assert_eq!(m.phase(), Phase::Stepping);
        assert_eq!(m.next_command(), Command::Pause);

        m.completed(Completion::PauseSent);
        confirm_runtime_pause(&mut m, 15, 56);
        assert!(matches!(m.next_command(), Command::Pulse { .. }));
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
        m.observe(&view(12, 59, true)); // remaining = 1

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
            m.observe(&view(frame_id, elapsed, true));
            frame_id += 1;
        }
        assert_eq!(m.cursor(), 60);
        assert_eq!(m.next_command(), Command::Execute { index: 1 });
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
            m.observe(&view(frame_id, 58, true));
            frame_id += 1;
        }
        assert_eq!(m.phase(), Phase::Stepping, "空脉冲不应中止");
        assert_eq!(m.tuner().gap_ms(), gap_before, "零星空脉冲不该改变间隔");
        assert_eq!(m.tuner().zero_frame_pulses, 3);
        assert!(!m.tuner().had_overshoot());

        // 继续空转到第 5 次才小幅加大 —— 那才说明间隔可能真的偏小
        for _ in 0..2 {
            assert!(matches!(m.next_command(), Command::Pulse { .. }));
            m.completed(Completion::PulseSent);
            m.observe(&view(frame_id, 58, true));
            frame_id += 1;
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
        m.observe(&view(13, 56, true));
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
        m.completed(Completion::PauseSent);
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
        m.completed(Completion::PauseSent);
        m.observe(&view(2, 3, true));
        assert_eq!(m.phase(), Phase::BindingFormation);
        assert_eq!(m.origin(), 3);
        assert_eq!(m.cursor(), 3);
    }

    #[test]
    fn pause_that_never_takes_effect_aborts() {
        let mut m = machine();
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::PauseSent);
        // 发了暂停键但帧数一直在涨（代理指挥 / 托管）。
        // 要涨满 PAUSE_PROBE_LIMIT 条才判定 —— 前几条是按键与分析的固有延迟，
        // 帧数理应还在增长，不能急着判死刑。
        for i in 0..PAUSE_PROBE_LIMIT + 1 {
            let elapsed = i64::from(3 * (i + 1));
            m.observe(&view(10 + u64::from(i), elapsed, false));
        }
        assert_eq!(m.phase(), Phase::Aborted);
        assert!(matches!(
            m.abort_reason(),
            Some(AbortReason::PauseIneffective)
        ));
    }

    #[test]
    fn short_growth_after_pause_is_tolerated() {
        // 按下暂停键后头几条样本帧数仍在增长是正常的（按键生效有延迟）；
        // 只要在 PAUSE_PROBE_LIMIT 之内出现了暂停态样本，就应当正常进入编队阶段。
        // 首个动作放在第 10 帧 —— 停在第 5 帧不算错过开局。
        let job = JOB.replace(r#""frame":0"#, r#""frame":10"#);
        let mut m = Machine::new(Copilot::parse(&job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::PauseSent);
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
        m.completed(Completion::PauseSent);
        m.observe(&view(2, 0, true));
        m.completed(Completion::FormationBound);

        assert_eq!(m.next_command(), Command::Execute { index: 0 });
        m.completed(Completion::ActionExecuted);
        // 同一帧的第二个动作：不该插入任何脉冲或恢复
        assert_eq!(m.next_command(), Command::Execute { index: 1 });
        m.completed(Completion::ActionExecuted);
        assert_eq!(
            m.next_command(),
            Command::Finish { resume: true },
            "全部做完应当收尾"
        );
    }

    #[test]
    fn after_last_action_pause_keeps_the_game_frozen() {
        let job = JOB.replace(
            r#""after_last_action": "resume""#,
            r#""after_last_action": "pause""#,
        );
        let mut m = Machine::new(Copilot::parse(&job).unwrap());
        m.start();
        m.observe(&view(1, 0, false));
        m.completed(Completion::PauseSent);
        m.observe(&view(2, 0, true));
        m.completed(Completion::FormationBound);
        for _ in 0..3 {
            m.completed(Completion::ActionExecuted);
        }
        assert_eq!(m.next_command(), Command::Finish { resume: false });
        assert_eq!(m.phase(), Phase::Finished);
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
