// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 移植自 arknights-frame-assistant (GPL-3.0-only):
//   src/lib/hotkey_actions.ahk —— ActionPressPause / ActionReleasePause /
//                                  Action16ms / Action33ms / Action166ms
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 暂停控制与逐帧脉冲。
//!
//! # 原理
//!
//! 《明日方舟》的暂停是"冻结时间但 UI 仍可交互"的软暂停 —— 暂停时仍然可以从
//! 待部署栏把干员拖到场上。这是整个帧级操作的物理基础。
//!
//! ESC 和游戏内暂停键（默认空格）**都是暂停切换键**。AFA 的做法是两者交替使用：
//! 用 ESC 进入暂停、用游戏键退出暂停，脉冲里则是 ESC 抬起暂停、游戏键重新按下。
//! 交替而不是连按同一个键，可以避开游戏对同键快速重复的吞键处理。
//!
//! 逐帧推进就是一个精确定时的"恢复 → 暂停"脉冲：
//!
//! ```text
//! t=0     ESC ↓            游戏恢复运行
//! t=gap   pauseBattle ↓    游戏重新暂停
//! t=gap+hold  两个键一起 ↑
//! ```
//!
//! 游戏在这 `gap` 毫秒里推进的逻辑帧数 = `gap / 每帧毫秒数`。1 倍速下一帧 33.3ms。
//!
//! # 为什么脉冲间隔要取得比"一帧"短
//!
//! 这是开环定时，受调度抖动影响。**过 0 帧完全无害**（恢复后立刻暂停，逻辑时间
//! 没走），**过 2 帧则不可挽回**（游戏没有倒带）。所以默认间隔取 22ms 而不是
//! AFA 的 30ms：宁可多来几次空脉冲，也绝不冒跨两帧的风险。真正的收敛由上层
//! 状态机的闭环校正完成（每次脉冲后读尺子，看实际走了几帧再调整间隔）。

use std::time::Duration;

use repl_core::Speed;

use crate::{
    clock::TimeCriticalPriority,
    game_keys::{func, GameKeys},
    keys::{key_down, key_tap, key_up, KeyCode, DEFAULT_TAP_HOLD},
    InputError,
};

/// 脉冲里两个键都按下之后、一起抬起之前的保持时长。
///
/// **AFA 用的是 50ms，这里刻意用 8ms。** 实测（2560×1440、垂直同步开、
/// `step-sweep` 各档 12 次）：
///
/// ```text
///  hold   gap   平均帧   +0    +1   ≥+2
///     8    16    0.58   42%   58%    0%
///    50    16    0.92   25%   58%   17%
///    50    32    1.42   33%    8%   58%
/// ```
///
/// 对 `hold=50ms` 那组做线性外推，`gap=0` 处的截距是 **1.0 帧（34ms）** ——
/// 也就是说这 50ms 几乎原封不动地变成了额外的运行时间。原因是游戏在**按键抬起**
/// 时才真正响应暂停，那么实际运行时长是 `gap + hold` 而不是 `gap`。
///
/// 8ms 足够让按键被识别（实测 58% 命中 +1），又不会白送一整帧。
const PULSE_HOLD: Duration = Duration::from_millis(8);

/// 1 倍速下单帧脉冲的默认间隔。
///
/// 实测最优档（58% 命中 +1、零跨帧）。
pub const DEFAULT_GAP_1X_MS: u32 = 16;
/// 脉冲间隔下限。
pub const MIN_GAP_MS: u32 = 12;
/// 脉冲间隔上限。
///
/// 实测表明推进量**几乎不随 gap 变化**（12→32ms 平均帧数一直在 0.42–0.58），
/// 所以调大 gap 没有收益，只会增加跨帧风险。上限压到 24 而不是贴着一帧的 31。
pub const MAX_GAP_MS: u32 = 24;

/// 暂停控制器。
#[derive(Clone, Debug)]
pub struct PauseController {
    /// 用来**进入**暂停的键。AFA 的"按下暂停"，固定是 ESC。
    enter_key: KeyCode,
    /// 用来**退出**暂停的键。AFA 的"松开暂停"，取游戏内的 `pauseBattle` 绑定。
    exit_key: KeyCode,
}

impl PauseController {
    pub fn new(game_keys: &GameKeys) -> Self {
        Self {
            enter_key: KeyCode::ESCAPE,
            exit_key: game_keys.get_or(func::PAUSE_BATTLE, KeyCode::SPACE),
        }
    }

    pub fn enter_key(&self) -> KeyCode {
        self.enter_key
    }

    pub fn exit_key(&self) -> KeyCode {
        self.exit_key
    }

    /// 诊断工具直接进入暂停。
    ///
    /// 主复刻路径的普通暂停必须走 [`crate::AfaController`]；这个入口只给
    /// `step-test` 等测量工具使用，避免把 Rust 时序重新带回运行期。
    pub fn diagnostic_pause(&self) -> Result<(), InputError> {
        key_tap(self.enter_key, DEFAULT_TAP_HOLD)
    }

    /// 诊断工具直接退出暂停。
    pub fn diagnostic_resume(&self) -> Result<(), InputError> {
        key_tap(self.exit_key, DEFAULT_TAP_HOLD)
    }

    /// 发一次逐帧脉冲：暂停态 → 运行 `gap` → 回到暂停态。
    ///
    /// 调用前游戏**必须已经处于暂停**，否则这一发会把游戏从运行切成暂停，
    /// 语义完全不同。上层状态机负责保证这个前提。
    ///
    /// 整个脉冲期间把线程提到 TIME_CRITICAL：两个按键边沿之间被抢占的话，
    /// 游戏就会多跑几帧。
    pub fn pulse(&self, gap: Duration) -> Result<(), InputError> {
        self.pulse_with_hold(gap, PULSE_HOLD)
    }

    /// 同 [`Self::pulse`]，但可以指定两个键一起按住的时长。
    ///
    /// `hold` 是调参用的：如果游戏是在**按键抬起**时才响应暂停，
    /// 那么实际运行时长就是 `gap + hold` 而不是 `gap`，`hold` 会直接变成
    /// 一段与 `gap` 无关的固定开销 —— 这正是"间隔压到下限仍然跨帧"的典型症状。
    /// 用 `step-test sweep` 可以把这条传递函数实测出来。
    pub fn pulse_with_hold(&self, gap: Duration, hold: Duration) -> Result<(), InputError> {
        let _priority = TimeCriticalPriority::acquire();

        // 两个按键边沿之间用**纯自旋**，不碰 Sleep。
        // precise_sleep 的 Sleep(1) 在负载下可能一觉多睡十几毫秒 —— 那正好是
        // 一个渲染 tick，足以让这一发脉冲多过一帧。实测 100 次里的那 1 次 +2
        // 最可能就是这么来的。gap+hold ≤ ~35ms，自旋的 CPU 代价可以忽略。
        key_down(self.enter_key)?; // 恢复运行
        crate::clock::spin_sleep(gap);
        let pause_result = key_down(self.exit_key); // 重新暂停
        crate::clock::spin_sleep(hold);

        // 无论中间出没出错，两个键都必须抬起来，否则游戏会一直认为键按着。
        let up_enter = key_up(self.enter_key);
        let up_exit = key_up(self.exit_key);

        pause_result.and(up_enter).and(up_exit)
    }

    /// 按当前倍速发一个"标称一帧"的脉冲。
    ///
    /// 这是 AFA 三个按钮（16ms / 33ms / 166ms）的等价物，仅供手动调试用；
    /// 复刻流程走的是带闭环校正的 [`GapTuner`]。
    pub fn pulse_one_frame(&self, speed: Speed) -> Result<(), InputError> {
        // 取标称帧长的 2/3，理由见模块文档：宁可空脉冲也不跨两帧。
        let gap_ms = (speed.frame_millis() * 2.0 / 3.0).round().max(1.0) as u64;
        self.pulse(Duration::from_millis(gap_ms))
    }
}

/// 脉冲间隔的闭环自适应器。
///
/// 纯逻辑，定义在 `repl-core::stepping` 里，这里只是重新导出，方便调用方从
/// 一个地方拿到脉冲相关的全部东西。
pub use repl_core::stepping::GapTuner;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_controller_uses_esc_to_enter_and_game_key_to_exit() {
        let keys = GameKeys::defaults_only();
        let pc = PauseController::new(&keys);
        assert_eq!(pc.enter_key(), KeyCode::ESCAPE);
        assert_eq!(pc.exit_key(), KeyCode::SPACE);
        // 交替使用，绝不连按同一个键
        assert_ne!(pc.enter_key(), pc.exit_key());
    }
}
