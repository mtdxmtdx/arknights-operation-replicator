// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 逐帧脉冲的传递函数实测。
//!
//! ```text
//! cargo run --release -p repl-app --bin step-sweep -- [每档次数]
//! ```
//!
//! `step-test` 的闭环整定假设"运行时长 ≈ gap"。实测发现这个假设不成立：
//! 间隔压到下限 14ms 仍然平均推进 1.3 帧，说明存在一段与 `gap` 无关的固定开销。
//!
//! 这个工具不做整定，只做**测量**：把 `gap` 和 `hold` 各扫一遍，
//! 统计每档实际推进多少帧。据此才能判断：
//!
//! - **固定开销来自 `hold`**（游戏在按键抬起时才暂停）→ 把 `hold` 调小就能解决
//! - **固定开销与两者都无关**（游戏按渲染帧轮询输入）→ 需要改游戏帧率设置
//! - **抖动本身就是 ±1 帧** → 当前方案在这台机器上达不到帧精度，是硬结论

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use repl_frames::{FrameSource, RulerClient};
use repl_input::{GameKeys, PauseController, TimerResolution};

/// 一档参数的测量结果。
struct Bucket {
    gap_ms: u32,
    hold_ms: u32,
    histogram: BTreeMap<i64, u32>,
}

impl Bucket {
    fn total(&self) -> u32 {
        self.histogram.values().sum()
    }

    fn mean(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        let sum: i64 = self.histogram.iter().map(|(d, n)| d * i64::from(*n)).sum();
        sum as f64 / f64::from(total)
    }

    fn share(&self, delta: i64) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        f64::from(*self.histogram.get(&delta).unwrap_or(&0)) / f64::from(total)
    }

    /// 推进 ≥2 帧的比例。这个必须是 0 才可用。
    fn overshoot_share(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        let over: u32 = self
            .histogram
            .iter()
            .filter(|(d, _)| **d >= 2)
            .map(|(_, n)| *n)
            .sum();
        f64::from(over) / f64::from(total)
    }
}

fn main() {
    repl_app::init("warn");
    let per_bucket: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(12);

    // hold 先扫两个极端，看固定开销是不是来自它。
    let holds = [8_u32, 50];
    let gaps = [12_u32, 16, 20, 24, 28, 32];
    let total_pulses = per_bucket * holds.len() as u32 * gaps.len() as u32;

    println!("== 逐帧脉冲传递函数扫描 ==\n");
    println!(
        "  将测量 {} 档参数 × {per_bucket} 次 = {total_pulses} 次脉冲，约需 {} 秒。",
        holds.len() * gaps.len(),
        total_pulses * 5 / 10
    );
    println!("  期间游戏会不断被推进，请找一个**费用能正常回复、且不会打输**的关卡。");
    println!("  建议直接找一关开局暂停后就跑本工具。\n");

    let Some(mut ctx) = Context::prepare() else {
        return;
    };
    let _timer = TimerResolution::acquire();

    let mut buckets = Vec::new();
    for hold_ms in holds {
        for gap_ms in gaps {
            print!("  hold={hold_ms:>2}ms gap={gap_ms:>2}ms … ");
            use std::io::Write;
            let _ = std::io::stdout().flush();

            let mut histogram: BTreeMap<i64, u32> = BTreeMap::new();
            for _ in 0..per_bucket {
                if !ctx.window.is_foreground() {
                    println!("\n游戏已不在前台，中止扫描。");
                    report(&buckets);
                    return;
                }
                let Some(delta) = ctx.pulse_and_measure(gap_ms, hold_ms) else {
                    println!("\n等不到尺子的新分析帧，中止扫描。");
                    report(&buckets);
                    return;
                };
                *histogram.entry(delta).or_default() += 1;
            }
            let bucket = Bucket {
                gap_ms,
                hold_ms,
                histogram,
            };
            println!(
                "平均 {:.2} 帧/次，跨帧 {:.0}%",
                bucket.mean(),
                bucket.overshoot_share() * 100.0
            );
            buckets.push(bucket);
        }
    }

    report(&buckets);
}

fn report(buckets: &[Bucket]) {
    if buckets.is_empty() {
        return;
    }
    println!("\n== 结果 ==\n");
    println!(
        "  {:>5} {:>5} {:>8} {:>7} {:>7} {:>7} {:>8}",
        "hold", "gap", "平均帧", "+0", "+1", "≥+2", "可用?"
    );
    for b in buckets {
        let usable = b.overshoot_share() == 0.0 && b.share(1) > 0.0;
        println!(
            "  {:>5} {:>5} {:>8.2} {:>6.0}% {:>6.0}% {:>6.0}% {:>8}",
            b.hold_ms,
            b.gap_ms,
            b.mean(),
            b.share(0) * 100.0,
            b.share(1) * 100.0,
            b.overshoot_share() * 100.0,
            if usable { "✓" } else { "" }
        );
    }

    println!("\n== 怎么读这张表 ==\n");

    // 固定开销：拿同一 hold 下 gap 最小和最大两档的平均帧数做线性外推。
    for hold in [8_u32, 50] {
        let row: Vec<&Bucket> = buckets.iter().filter(|b| b.hold_ms == hold).collect();
        if row.len() < 2 {
            continue;
        }
        let (first, last) = (row[0], row[row.len() - 1]);
        let dgap = f64::from(last.gap_ms) - f64::from(first.gap_ms);
        let dframes = last.mean() - first.mean();
        if dgap > 0.0 && dframes.abs() > 1e-9 {
            let ms_per_frame = 1000.0 / 30.0;
            let slope = dframes / dgap; // 帧 / ms
            let intercept_frames = first.mean() - slope * f64::from(first.gap_ms);
            println!(
                "  hold={hold}ms：斜率 {:.3} 帧/ms（理论值 {:.3}），gap=0 时的固定开销约 {:.1} 帧 = {:.0}ms",
                slope,
                1.0 / ms_per_frame,
                intercept_frames,
                intercept_frames * ms_per_frame
            );
        }
    }

    let best = buckets
        .iter()
        .filter(|b| b.overshoot_share() == 0.0)
        .max_by(|a, b| a.share(1).total_cmp(&b.share(1)));

    match best {
        Some(b) if b.share(1) > 0.0 => {
            println!(
                "\n  ✓ 最佳参数：hold={}ms gap={}ms（{:.0}% 命中 +1，零跨帧）",
                b.hold_ms,
                b.gap_ms,
                b.share(1) * 100.0
            );
            println!("    把 config.json 的 initial_gap_ms 设成 {}。", b.gap_ms);
            if b.hold_ms != 50 {
                println!(
                    "    另外 hold 需要从默认的 50ms 改成 {}ms —— 说明游戏是在**按键抬起**时",
                    b.hold_ms
                );
                println!("    才响应暂停的，那 50ms 全都变成了额外的运行时间。");
            }
        }
        _ => {
            println!("\n  ✗ 没有任何一档做到零跨帧。");
            println!("    说明抖动本身就 ≥1 帧，靠调参解决不了。可以试的方向：");
            println!("      · 游戏内把帧率锁到 30（渲染帧与逻辑帧 1:1，输入轮询就不会错位）");
            println!("      · 关掉垂直同步");
            println!("      · 关掉后台占 CPU 的程序");
            println!("    如果都没用，那当前这套「按键脉冲」方案在这台机器上达不到帧精度。");
        }
    }
}

/// 扫描过程中需要反复用到的东西。
struct Context {
    ruler: RulerClient,
    pause: PauseController,
    window: repl_capture::GameWindow,
    cursor: i64,
    last_frame_id: u64,
}

impl Context {
    fn prepare() -> Option<Self> {
        let window = match repl_capture::GameWindow::find() {
            Ok(w) => w,
            Err(e) => {
                println!("找不到游戏窗口：{e}");
                return None;
            }
        };
        if !repl_input::is_elevated() {
            println!("本程序没有以管理员身份运行，按键会被 Windows 静默丢弃。请提权后重试。");
            return None;
        }
        if !window.is_foreground() {
            let _ = window.focus();
            if !window.is_foreground() {
                println!("请在 5 秒内点一下游戏窗口：");
                for i in (1..=5).rev() {
                    print!("  {i}… ");
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    std::thread::sleep(Duration::from_secs(1));
                    if window.is_foreground() {
                        break;
                    }
                }
                println!();
            }
            if !window.is_foreground() {
                println!("游戏窗口仍不在前台，退出。");
                return None;
            }
        }

        let ruler = RulerClient::connect_default();
        let Ok(baseline) = ruler.probe(Duration::from_secs(5)) else {
            println!("连不上尺子。");
            return None;
        };
        if !baseline.frame_reading_is_trustworthy() {
            println!("尺子读数不可信，检查校准和费用条是否可见。");
            return None;
        }
        let state = baseline.battle_state_str().to_owned();
        if !state.starts_with("1x") {
            println!("必须是 1 倍速（当前 {state}）。");
            return None;
        }
        println!("起始：{state}，绝对帧 {}\n", baseline.total_elapsed_frames);
        println!("3 秒后开始，期间不要碰键盘鼠标。\n");
        std::thread::sleep(Duration::from_secs(3));

        Some(Self {
            cursor: baseline.total_elapsed_frames,
            last_frame_id: baseline.frame_id.unwrap_or(0),
            ruler,
            pause: PauseController::new(&GameKeys::load()),
            window,
        })
    }

    /// 发一次脉冲并测量实际推进了几帧。
    fn pulse_and_measure(&mut self, gap_ms: u32, hold_ms: u32) -> Option<i64> {
        if self
            .pause
            .pulse_with_hold(
                Duration::from_millis(u64::from(gap_ms)),
                Duration::from_millis(u64::from(hold_ms)),
            )
            .is_err()
        {
            return None;
        }
        let deadline = Instant::now() + Duration::from_millis(2500);
        while Instant::now() < deadline {
            let Ok(s) = self
                .ruler
                .wait_next(self.last_frame_id, Duration::from_millis(300))
            else {
                continue;
            };
            let Some(id) = s.frame_id else { continue };
            if id <= self.last_frame_id || !s.frame_reading_is_trustworthy() {
                continue;
            }
            self.last_frame_id = id;
            let delta = s.total_elapsed_frames - self.cursor;
            self.cursor = s.total_elapsed_frames;
            return Some(delta);
        }
        None
    }
}
