// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 移植自 arknights-frame-assistant (GPL-3.0-only):
//   src/lib/hotkey_actions.ahk —— USleep()
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 高精度延时。
//!
//! 逐帧脉冲的成败取决于"恢复"和"暂停"两个按键边沿之间的间隔是否准确。
//! 1 倍速下一个逻辑帧是 33.3ms，我们用的脉冲间隔在 14–31ms 之间，
//! 误差 3ms 就可能多过一帧 —— `thread::sleep` 在默认 15.6ms 的系统定时器精度下
//! 完全不够用。

use std::time::{Duration, Instant};

/// 剩余时间超过这个值时先让出 CPU，低于它就纯自旋。
///
/// 4ms 是 AFA 用的阈值：`Sleep(1)` 在 `timeBeginPeriod(1)` 下实际会睡 1–2ms，
/// 留 4ms 余量足够吸收抖动又不至于白烧太多 CPU。
const SPIN_THRESHOLD: Duration = Duration::from_millis(4);

/// 高精度等待。先粗睡再自旋，误差通常 < 0.1ms。
///
/// 调用方应当先持有一个 [`TimerResolution`]，否则粗睡阶段的精度会退化到 15.6ms。
pub fn precise_sleep(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let deadline = Instant::now() + duration;
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return;
        };
        if remaining > SPIN_THRESHOLD {
            std::thread::sleep(Duration::from_millis(1));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// 纯自旋等待，全程不调用 `Sleep`。
///
/// [`precise_sleep`] 在剩余 >4ms 时会 `Sleep(1)` 让出 CPU —— 省电，但每次
/// `Sleep(1)` 实际睡多久取决于调度器，负载高时可能一觉睡出去十几毫秒。
/// 对普通延时无所谓；对**脉冲的两个按键边沿之间**却是灾难：多睡 17ms 就是
/// 游戏里多跑一个渲染 tick，可能直接多过一帧。
///
/// 脉冲的 gap ≤ 24ms、hold = 8ms，纯自旋一共 ≤ ~35ms，CPU 代价可以忽略。
/// 只给脉冲用，别拿它做长时间等待。
pub fn spin_sleep(duration: Duration) {
    if duration.is_zero() {
        return;
    }
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

/// RAII 包装 `timeBeginPeriod(1)`。
///
/// 把系统定时器精度从默认的 15.6ms 提到 1ms。这是进程级全局设置，
/// 所以只在真正要做帧操作的时段持有，别常驻 —— 常驻会拉高整机功耗。
#[derive(Debug)]
pub struct TimerResolution {
    period_ms: u32,
}

impl TimerResolution {
    /// 请求 1ms 定时器精度。失败时返回 `None`（不致命，只是精度差些）。
    pub fn acquire() -> Option<Self> {
        Self::acquire_ms(1)
    }

    pub fn acquire_ms(period_ms: u32) -> Option<Self> {
        #[cfg(windows)]
        {
            use windows::Win32::Media::{timeBeginPeriod, TIMERR_NOERROR};
            // SAFETY: timeBeginPeriod 只接受一个整数周期，无内存安全影响。
            let ok = unsafe { timeBeginPeriod(period_ms) } == TIMERR_NOERROR;
            if !ok {
                log::warn!(
                    "timeBeginPeriod({period_ms}) failed; frame pulses will be less precise"
                );
                return None;
            }
        }
        #[cfg(not(windows))]
        let _ = period_ms;
        Some(Self { period_ms })
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::Media::timeEndPeriod;
            // SAFETY: 与 acquire 里的 timeBeginPeriod 配对。
            unsafe {
                let _ = timeEndPeriod(self.period_ms);
            }
        }
    }
}

/// RAII 把当前线程提到 `THREAD_PRIORITY_TIME_CRITICAL`。
///
/// 只在发脉冲的那一小段时间里持有：如果注入线程在两个按键边沿之间被调度器抢占，
/// 游戏就会多跑几帧。
#[derive(Debug)]
pub struct TimeCriticalPriority {
    #[cfg(windows)]
    previous: i32,
}

impl TimeCriticalPriority {
    pub fn acquire() -> Option<Self> {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, GetThreadPriority, SetThreadPriority,
                THREAD_PRIORITY_TIME_CRITICAL,
            };
            // SAFETY: GetCurrentThread 返回伪句柄，无需释放。
            unsafe {
                let thread = GetCurrentThread();
                let previous = GetThreadPriority(thread);
                if SetThreadPriority(thread, THREAD_PRIORITY_TIME_CRITICAL).is_err() {
                    log::warn!("SetThreadPriority(TIME_CRITICAL) failed; pulses may jitter");
                    return None;
                }
                Some(Self { previous })
            }
        }
        #[cfg(not(windows))]
        Some(Self {})
    }
}

impl Drop for TimeCriticalPriority {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY,
            };
            // SAFETY: 恢复 acquire 时记录的原优先级。
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY(self.previous));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precise_sleep_is_accurate_enough_for_frame_pulses() {
        let _res = TimerResolution::acquire();
        // 22ms 是 1 倍速逐帧脉冲的默认间隔。
        for _ in 0..5 {
            let target = Duration::from_millis(22);
            let started = Instant::now();
            precise_sleep(target);
            let actual = started.elapsed();
            assert!(actual >= target, "睡短了: {actual:?} < {target:?}");
            // 一个逻辑帧 33ms，超时 5ms 就有多过一帧的风险。
            assert!(
                actual < target + Duration::from_millis(5),
                "睡长了: {actual:?} 超出 {target:?} 太多"
            );
        }
    }

    #[test]
    fn zero_duration_returns_immediately() {
        let started = Instant::now();
        precise_sleep(Duration::ZERO);
        assert!(started.elapsed() < Duration::from_millis(2));
    }
}
