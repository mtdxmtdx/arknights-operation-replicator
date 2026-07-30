// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 使用外部 AFA 的 `33ms` 热键执行 1 倍速单帧脉冲，并由尺子独立验收。
//!
//! ```text
//! cargo run --release -p repl-app --bin afa-step-test -- [次数]
//! ```
//!
//! 默认和最大次数都是 500。热键派发成功不代表 AFA 动作已经完成；每次脉冲仍须由
//! 尺子的 `running -> paused`，或宽限期后的连续暂停样本，证明最终落地。

use std::{
    collections::BTreeMap,
    io::Write,
    time::{Duration, Instant},
};

use repl_frames::{FrameSource, RulerClient};
use repl_input::{AfaAction, AfaController};

const DEFAULT_PULSES: u32 = 500;
const MAX_PULSES: u32 = 500;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const ASYNC_AFA_GRACE: Duration = Duration::from_millis(250);

fn main() {
    repl_app::init("warn");

    let pulses = match parse_pulses(std::env::args().nth(1).as_deref()) {
        Ok(value) => value,
        Err(reason) => {
            println!("参数错误：{reason}");
            println!("用法：afa-step-test.exe [1..={MAX_PULSES}]");
            return;
        }
    };

    println!("== AFA 单帧脉冲诊断（{pulses} 次）==\n");
    println!("准备：");
    println!("  1. AFA 已以管理员身份运行，且 `33ms` 热键已绑定");
    println!("  2. 尺子已运行并完成校准");
    println!("  3. 游戏已进入关卡、处于 1 倍速、已经暂停");
    println!("  4. 鼠标必须留在游戏客户区内，但不要遮挡费用条");
    println!("  5. 费用必须能正常回复；测试期间不要碰键鼠或切换窗口\n");

    let window = match repl_capture::GameWindow::find() {
        Ok(window) => window,
        Err(error) => {
            println!("找不到游戏窗口：{error}");
            return;
        }
    };

    if !focus_game(&window) {
        return;
    }

    let afa = match AfaController::discover() {
        Ok(controller) => controller,
        Err(error) => {
            println!("AFA 预检失败：{error}");
            return;
        }
    };
    let step_hotkey = afa.bindings().step_one_x;
    println!(
        "AFA 已就绪：过帧热键 = {step_hotkey}，配置 = {}",
        afa.settings_path().display()
    );

    let ruler = RulerClient::connect_default();
    println!("正在等待尺子……");
    let Some(baseline) = wait_for_baseline(&ruler) else {
        println!("拿不到可信的 1x_paused 帧数。请检查尺子、费用条和游戏暂停状态。");
        return;
    };
    let mut cursor = baseline.total_elapsed_frames;
    let mut last_frame_id = baseline.frame_id.unwrap_or(0);
    println!(
        "起始状态：{}，绝对帧 {cursor}，校准配置 {:?}\n",
        baseline.battle_state_str(),
        baseline.active_profile
    );
    println!("== 正式测试 ==");
    println!("每25次打印进度；任何跨帧都会保留统计，但不会提前掩盖后续样本。\n");

    let mut histogram: BTreeMap<i64, u32> = BTreeMap::new();
    let mut completed = 0_u32;
    let mut consecutive_zeros = 0_u32;

    for index in 1..=pulses {
        if !window.is_foreground() {
            println!("\n第 {index} 次之前检测到游戏失去前台，停止测试。");
            break;
        }

        if let Err(error) = afa.dispatch(AfaAction::StepOneX) {
            println!("\n第 {index} 次 AFA 过帧热键派发失败：{error}");
            break;
        }

        let delta = match wait_for_afa_pulse(&ruler, &mut last_frame_id, cursor) {
            Ok(delta) => delta,
            Err(error) => {
                println!("\n第 {index} 次 AFA 脉冲无法确认：{error}");
                break;
            }
        };

        cursor += delta;
        completed += 1;
        *histogram.entry(delta).or_default() += 1;
        consecutive_zeros = if delta == 0 { consecutive_zeros + 1 } else { 0 };

        if consecutive_zeros >= 10 {
            println!("\n连续10次没有推进。AFA 很可能因鼠标不在游戏客户区而静默拒绝，停止测试。");
            break;
        }

        if index % 25 == 0 || index == pulses {
            let overshoots: u32 = histogram
                .iter()
                .filter(|(delta, _)| **delta >= 2)
                .map(|(_, count)| *count)
                .sum();
            println!(
                "  {index:>3}/{pulses}  绝对帧 {cursor}  +1={}  跨帧={overshoots}",
                histogram.get(&1).copied().unwrap_or(0)
            );
        }
    }

    print_result(pulses, completed, &histogram);
}

fn parse_pulses(raw: Option<&str>) -> Result<u32, String> {
    let pulses = match raw {
        Some(raw) => raw
            .parse::<u32>()
            .map_err(|_| format!("次数不是正整数：{raw}"))?,
        None => DEFAULT_PULSES,
    };
    if !(1..=MAX_PULSES).contains(&pulses) {
        return Err(format!("次数必须在 1..={MAX_PULSES} 之间，实际 {pulses}"));
    }
    Ok(pulses)
}

fn focus_game(window: &repl_capture::GameWindow) -> bool {
    if window.is_foreground() {
        return true;
    }

    let _ = window.focus();
    if !window.is_foreground() {
        println!("请在倒计时结束前点一下游戏窗口，并把鼠标留在客户区内：");
        for remaining in (1..=5).rev() {
            print!("  {remaining}… ");
            let _ = std::io::stdout().flush();
            std::thread::sleep(Duration::from_secs(1));
            if window.is_foreground() {
                break;
            }
        }
        println!();
    }

    if !window.is_foreground() {
        println!("游戏窗口仍不在前台，退出。请点一下游戏窗口后重新运行。");
        return false;
    }

    println!("游戏已在前台，3秒后开始；请勿再操作键鼠。");
    std::thread::sleep(Duration::from_secs(3));
    true
}

fn wait_for_baseline(ruler: &RulerClient) -> Option<repl_frames::Snapshot> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let Ok(sample) = ruler.wait_next(0, Duration::from_millis(500)) else {
            continue;
        };
        if sample.frame_reading_is_trustworthy()
            && sample.battle_state_str().starts_with("1x")
            && sample
                .battle_state
                .as_ref()
                .and_then(|state| state.is_paused())
                == Some(true)
        {
            return Some(sample);
        }
    }
    None
}

fn wait_for_afa_pulse(
    ruler: &RulerClient,
    last_frame_id: &mut u64,
    cursor: i64,
) -> Result<i64, String> {
    let started = Instant::now();
    let deadline = started + SETTLE_TIMEOUT;
    let mut seen_running = false;
    let mut paused_after_grace = 0_u8;

    while Instant::now() < deadline {
        let Ok(sample) = ruler.wait_next(*last_frame_id, Duration::from_millis(300)) else {
            continue;
        };
        let Some(frame_id) = sample.frame_id else {
            continue;
        };
        if frame_id <= *last_frame_id {
            continue;
        }
        *last_frame_id = frame_id;
        if !sample.frame_reading_is_trustworthy() {
            continue;
        }
        if !sample.battle_state_str().starts_with("1x") {
            return Err(format!(
                "脉冲期间不再是1倍速：{}",
                sample.battle_state_str()
            ));
        }

        match sample
            .battle_state
            .as_ref()
            .and_then(|state| state.is_paused())
        {
            Some(false) => {
                seen_running = true;
                paused_after_grace = 0;
            }
            Some(true) if seen_running => {
                return Ok(sample.total_elapsed_frames - cursor);
            }
            Some(true) if started.elapsed() >= ASYNC_AFA_GRACE => {
                paused_after_grace += 1;
                if paused_after_grace >= 2 {
                    return Ok(sample.total_elapsed_frames - cursor);
                }
            }
            _ => {}
        }
    }

    Err(format!(
        "{}ms 内没有取得稳定暂停回执",
        SETTLE_TIMEOUT.as_millis()
    ))
}

fn print_result(pulses: u32, completed: u32, histogram: &BTreeMap<i64, u32>) {
    println!("\n== 结果 ==");
    for (delta, count) in histogram {
        let bar = "#".repeat((*count as usize).min(60));
        println!("  推进 {delta:>+2} 帧 : {count:>4} 次  {bar}");
    }

    let zero = histogram.get(&0).copied().unwrap_or(0);
    let one = histogram.get(&1).copied().unwrap_or(0);
    let negative: u32 = histogram
        .iter()
        .filter(|(delta, _)| **delta < 0)
        .map(|(_, count)| *count)
        .sum();
    let overshoots: u32 = histogram
        .iter()
        .filter(|(delta, _)| **delta >= 2)
        .map(|(_, count)| *count)
        .sum();
    let zero_rate = if completed == 0 {
        0.0
    } else {
        f64::from(zero) / f64::from(completed) * 100.0
    };

    println!("\n  计划脉冲    {pulses}");
    println!("  完成脉冲    {completed}");
    println!("  空脉冲      {zero} ({zero_rate:.0}%)");
    println!("  单帧推进    {one}");
    println!("  跨帧        {overshoots}");
    println!("  负数推进    {negative}");

    println!("\n== 判定 ==");
    if completed != pulses {
        println!("  ✗ 不通过：只完成 {completed}/{pulses} 次，测试中途停止。");
    } else if overshoots > 0 {
        println!("  ✗ 不通过：本轮长跑中出现 {overshoots} 次跨帧。");
    } else if negative > 0 {
        println!("  ✗ 不通过：出现 {negative} 次帧数倒退，尺子读数不稳定。");
    } else if one * 4 < completed {
        println!("  △ 不通过：单帧命中 {one}/{completed}，低于25%。");
    } else {
        println!("  ✓ 通过：{completed} 次全部完成，跨帧0、负数推进0，单帧命中率达标。");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_count_defaults_to_and_caps_at_five_hundred() {
        assert_eq!(parse_pulses(None), Ok(500));
        assert_eq!(parse_pulses(Some("1")), Ok(1));
        assert_eq!(parse_pulses(Some("500")), Ok(500));
        assert!(parse_pulses(Some("0")).is_err());
        assert!(parse_pulses(Some("501")).is_err());
        assert!(parse_pulses(Some("abc")).is_err());
    }
}
