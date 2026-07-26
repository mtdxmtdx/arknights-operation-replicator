// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 脉冲时序参考 arknights-frame-assistant (GPL-3.0-only) 的
//   src/lib/hotkey_actions.ahk —— Action16ms / Action33ms / Action166ms
// 闭环校正部分是本项目新增的。详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 逐帧脉冲的参数整定。
//!
//! AFA 的过帧是**开环定时**：恢复运行固定毫秒数再暂停。
//!
//! 这里加了闭环校正：每发一次脉冲，从尺子读出实际推进了几帧，据此微调间隔。
//! 不对称调整是刻意的 ——
//!
//! - **过 0 帧完全无害**：恢复后立刻暂停，逻辑时间没走，重来一次就行。
//! - **过 2 帧不可挽回**：游戏没有倒带，只能中止整场复刻。
//!
//! # 实测发现：间隔本身几乎不起作用
//!
//! `step-sweep` 在 2560×1440、垂直同步开的机器上扫了 12 档参数（各 12 次）：
//!
//! ```text
//!  hold=8ms:  gap 12→32ms，平均帧数 0.42 / 0.58 / 0.42 / 0.42 / 0.50 / 0.42
//! ```
//!
//! **完全平坦。** 如果运行时长等于 `gap`，平均值应当从 0.36 线性涨到 0.96。
//! 实测没有任何斜率，说明游戏是按**渲染帧**轮询输入的：两个按键事件落在哪个
//! 渲染 tick 上决定一切，tick 内部的毫秒差不可见。
//!
//! 所以调大 `gap` 没有收益，只增加跨帧风险。整定策略据此改成：
//!
//! - **不因为单次空脉冲就加大间隔**（加了也没用，纯粹增加风险）
//! - 只有**连续多次**空脉冲才小幅加一点（那才说明间隔可能真的偏小）
//! - 一旦跨帧就果断减小

use std::time::Duration;

/// 1 倍速下单帧脉冲的默认间隔。实测最优档（58% 命中 +1、零跨帧）。
pub const DEFAULT_GAP_1X_MS: u32 = 16;
/// 脉冲间隔下限。
pub const MIN_GAP_MS: u32 = 12;
/// 脉冲间隔上限。实测推进量不随 gap 增长，所以没必要贴着一帧（33.3ms）。
pub const MAX_GAP_MS: u32 = 24;

/// 连续这么多次空脉冲之后才考虑加大间隔。
///
/// 单次空脉冲在实测分布下有约 42% 的概率，是正常现象，不该触发调整。
/// 连续 5 次的概率只有 `0.42^5 ≈ 1.3%`，那才真的说明间隔偏小。
const ZEROS_BEFORE_GROWING: u32 = 5;
/// 触发增长时的增量。小步试探，因为实测收益接近零。
const GROW_MS: u32 = 1;
/// 跨帧时的减量。刻意大于 [`GROW_MS`]：跨帧的代价远高于空转。
const SHRINK_MS: u32 = 3;

/// 脉冲间隔的闭环自适应器。
#[derive(Clone, Copy, Debug)]
pub struct GapTuner {
    gap_ms: u32,
    min_ms: u32,
    max_ms: u32,
    /// 总脉冲次数。
    pub pulses: u32,
    /// 其中推进了 0 帧的次数。
    pub zero_frame_pulses: u32,
    /// 其中推进了 ≥2 帧的次数。**验收要求这个必须是 0。**
    pub overshoot_pulses: u32,
    /// 当前连续空脉冲计数，用于判断是否真的需要加大间隔。
    consecutive_zeros: u32,
}

impl Default for GapTuner {
    fn default() -> Self {
        Self::new(DEFAULT_GAP_1X_MS)
    }
}

impl GapTuner {
    pub fn new(initial_ms: u32) -> Self {
        Self {
            gap_ms: initial_ms.clamp(MIN_GAP_MS, MAX_GAP_MS),
            min_ms: MIN_GAP_MS,
            max_ms: MAX_GAP_MS,
            pulses: 0,
            zero_frame_pulses: 0,
            overshoot_pulses: 0,
            consecutive_zeros: 0,
        }
    }

    pub fn gap(&self) -> Duration {
        Duration::from_millis(u64::from(self.gap_ms))
    }

    pub fn gap_ms(&self) -> u32 {
        self.gap_ms
    }

    /// 记录一次脉冲的实际推进帧数并调整间隔。
    pub fn observe(&mut self, advanced_frames: i64) {
        self.pulses += 1;
        match advanced_frames {
            // 倒退也按"没推进"处理：尺子偶发误判不该被当成冲过头而缩短间隔。
            i64::MIN..=0 => {
                self.zero_frame_pulses += 1;
                self.consecutive_zeros += 1;
                // 单次空脉冲是正常现象（实测约 42%），不动间隔；
                // 连续多次才说明间隔可能真的偏小。
                if self.consecutive_zeros >= ZEROS_BEFORE_GROWING {
                    self.consecutive_zeros = 0;
                    self.gap_ms = (self.gap_ms + GROW_MS).min(self.max_ms);
                }
            }
            1 => self.consecutive_zeros = 0,
            _ => {
                self.consecutive_zeros = 0;
                self.overshoot_pulses += 1;
                self.gap_ms = self.gap_ms.saturating_sub(SHRINK_MS).max(self.min_ms);
            }
        }
    }

    /// 空脉冲占比。验收时用来判断初值调得合不合适：偏高说明可以调大初值。
    pub fn zero_rate(&self) -> f64 {
        if self.pulses == 0 {
            0.0
        } else {
            f64::from(self.zero_frame_pulses) / f64::from(self.pulses)
        }
    }

    /// 是否出现过跨帧。**只要出现过一次，这次复刻的精度就不可信。**
    pub fn had_overshoot(&self) -> bool {
        self.overshoot_pulses > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Speed;

    #[test]
    fn single_empty_pulses_do_not_move_the_gap() {
        // 实测空脉冲率约 42%，是正常现象。为此就加大间隔只会来回震荡，
        // 而且实测加大间隔对推进量毫无帮助 —— 纯粹增加跨帧风险。
        let mut t = GapTuner::new(16);
        for _ in 0..(ZEROS_BEFORE_GROWING - 1) {
            t.observe(0);
        }
        assert_eq!(t.gap_ms(), 16, "零星空脉冲不该改变间隔");
        // 中间夹一次成功就重新计数
        t.observe(1);
        for _ in 0..(ZEROS_BEFORE_GROWING - 1) {
            t.observe(0);
        }
        assert_eq!(t.gap_ms(), 16, "计数应当被成功推进重置");
    }

    #[test]
    fn a_long_run_of_zeros_nudges_the_gap_up() {
        let mut t = GapTuner::new(16);
        for _ in 0..ZEROS_BEFORE_GROWING {
            t.observe(0);
        }
        assert_eq!(t.gap_ms(), 16 + GROW_MS, "连续空脉冲才小幅加大");
    }

    #[test]
    fn overshoot_shrinks_immediately_and_more_than_it_grows() {
        let mut t = GapTuner::new(20);
        t.observe(2);
        assert_eq!(t.gap_ms(), 20 - SHRINK_MS, "跨帧必须立刻缩小");

        // 一次跨帧的缩小量，必须超过连续空脉冲攒出来的增长量 ——
        // 否则在"偶尔跨帧 + 大量空脉冲"的输入下会被慢慢推回危险区间。
        let mut grower = GapTuner::new(20);
        for _ in 0..ZEROS_BEFORE_GROWING {
            grower.observe(0);
        }
        let grew = grower.gap_ms() - 20;
        assert!(
            grew < SHRINK_MS,
            "一次跨帧的缩量({SHRINK_MS})必须盖过一轮增量({grew})"
        );
    }

    #[test]
    fn respects_bounds() {
        let mut t = GapTuner::new(MAX_GAP_MS);
        for _ in 0..100 {
            t.observe(0);
        }
        assert_eq!(t.gap_ms(), MAX_GAP_MS, "不能超过上限");

        let mut t = GapTuner::new(MIN_GAP_MS);
        for _ in 0..20 {
            t.observe(5);
        }
        assert_eq!(t.gap_ms(), MIN_GAP_MS, "不能低于下限");
    }

    #[test]
    fn initial_gap_is_clamped() {
        assert_eq!(GapTuner::new(1).gap_ms(), MIN_GAP_MS);
        assert_eq!(GapTuner::new(999).gap_ms(), MAX_GAP_MS);
    }

    #[test]
    fn tracks_statistics() {
        let mut t = GapTuner::new(22);
        t.observe(0);
        t.observe(1);
        t.observe(1);
        t.observe(3);
        assert_eq!(t.pulses, 4);
        assert_eq!(t.zero_frame_pulses, 1);
        assert_eq!(t.overshoot_pulses, 1);
        assert!(t.had_overshoot());
        assert!((t.zero_rate() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn negative_frame_delta_is_treated_as_no_progress() {
        let mut t = GapTuner::new(16);
        t.observe(-1);
        assert_eq!(t.zero_frame_pulses, 1);
        assert!(!t.had_overshoot(), "倒退不算跨帧");
    }

    #[test]
    fn default_gap_is_safely_below_one_frame() {
        // 1 倍速一帧 33.3ms；默认间隔必须明显小于它，留出抖动余量。
        let frame_ms = Speed::One.frame_millis();
        assert!(f64::from(DEFAULT_GAP_1X_MS) < frame_ms * 0.75);
        assert!(f64::from(MAX_GAP_MS) < frame_ms * 0.8);
    }

    #[test]
    fn converges_away_from_overshoot() {
        // 模拟一个"20ms 及以上就会跨帧"的机器：整定器应当收敛到安全区间并稳住。
        let mut t = GapTuner::new(MAX_GAP_MS);
        for _ in 0..40 {
            let advanced = if t.gap_ms() >= 20 { 2 } else { 1 };
            t.observe(advanced);
        }
        assert!(
            t.gap_ms() < 20,
            "应当收敛到不跨帧的区间，实际 {}",
            t.gap_ms()
        );
        let before = t.overshoot_pulses;
        for _ in 0..20 {
            let advanced = if t.gap_ms() >= 20 { 2 } else { 1 };
            t.observe(advanced);
        }
        assert_eq!(t.overshoot_pulses, before, "收敛后不该再跨帧");
    }

    /// 用实测到的分布回放：42% 空脉冲、58% 推进一帧、零跨帧。
    /// 整定器在这种输入下必须**稳住不动**，而不是被空脉冲带着往上爬。
    #[test]
    fn stays_put_under_the_measured_distribution() {
        let mut t = GapTuner::new(DEFAULT_GAP_1X_MS);
        // 交替出现，模拟"偶尔连续几次空脉冲"但不会长跑
        let pattern = [0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 1];
        for _ in 0..10 {
            for d in pattern {
                t.observe(d);
            }
        }
        assert_eq!(
            t.gap_ms(),
            DEFAULT_GAP_1X_MS,
            "实测分布下间隔不该漂移，实际 {}",
            t.gap_ms()
        );
        assert!(!t.had_overshoot());
    }
}
