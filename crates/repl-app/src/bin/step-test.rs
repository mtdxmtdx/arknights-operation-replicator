// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 阶段 2 验收：单帧脉冲精度实测。
//!
//! ```text
//! cargo run --release -p repl-app --bin step-test -- [次数] [固定间隔ms]
//! ```
//!
//! 判据：**每次推进必须是 +1 或 +0，一次 +2 都不能有。**
//! +0 无害（恢复后立刻暂停，逻辑时间没走），+2 则不可挽回。
//!
//! 注意：`step-sweep` 实测表明推进量几乎不随间隔变化（游戏按渲染帧轮询输入），
//! 空脉冲多只影响速度、不影响正确性 —— 不要为了压低空脉冲率去调大间隔。
//! 第二个参数可以固定间隔（关闭闭环整定），用来单独验证某一档是否稳定。

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use repl_core::stepping::{GapTuner, DEFAULT_GAP_1X_MS};
use repl_frames::{FrameSource, RulerClient};
use repl_input::{GameKeys, PauseController, TimerResolution};

fn main() {
    repl_app::init("warn");
    let mut args = std::env::args().skip(1);
    let pulses: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(100);
    // 第二个参数：固定间隔，关闭闭环整定。用来单独验证某一档是否稳定。
    let fixed_gap: Option<u32> = args.next().and_then(|s| s.parse().ok());

    println!("== 单帧脉冲精度测试（{pulses} 次）==\n");
    if let Some(gap) = fixed_gap {
        println!("固定间隔 {gap}ms（已关闭闭环整定）\n");
    }
    println!("准备：");
    println!("  1. 尺子已运行且完成校准");
    println!("  2. 游戏已进入关卡、处于 1 倍速、**已经暂停**");
    println!("  3. 游戏窗口在前台，鼠标不要停在费用条上");
    println!("  4. 关卡的费用要能正常回复（费用满 / 剿灭作战不行）\n");

    let window = match repl_capture::GameWindow::find() {
        Ok(w) => w,
        Err(e) => {
            println!("找不到游戏窗口：{e}");
            return;
        }
    };

    // 你是在终端里敲的命令，此刻前台窗口必然是终端而不是游戏。
    // 先尝试把游戏拉到前台；Windows 对跨进程抢焦点有限制，失败就倒计时让你手动切。
    if !window.is_foreground() {
        let _ = window.focus();
        if !window.is_foreground() {
            println!("请在倒计时结束前**点一下游戏窗口**（按键注入只会发给前台窗口）：");
            for remaining in (1..=5).rev() {
                print!("  {remaining}… ");
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
            println!("游戏窗口仍然不在前台，退出。请点一下游戏窗口后重新运行。");
            return;
        }
    }
    println!("游戏已在前台，3 秒后开始，期间不要碰键盘鼠标。");
    std::thread::sleep(Duration::from_secs(3));

    let ruler = RulerClient::connect_default();
    println!("正在等待尺子…");
    let mut baseline = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(s) = ruler.wait_next(0, Duration::from_millis(500)) {
            if s.frame_reading_is_trustworthy() {
                baseline = Some(s);
                break;
            }
        }
    }
    let Some(baseline) = baseline else {
        println!("拿不到可信的帧数读数。检查尺子是否已校准、费用条是否可见。");
        return;
    };
    let state = baseline.battle_state_str().to_owned();
    println!(
        "起始状态：{state}，绝对帧 {}，校准配置 {:?}\n",
        baseline.total_elapsed_frames, baseline.active_profile
    );
    if !state.starts_with("1x") {
        println!("必须是 1 倍速（当前 {state}），退出。");
        return;
    }

    let keys = GameKeys::load();
    if !keys.from_registry() {
        println!("提示：没能从注册表读到游戏内键位，正在使用默认键。");
    }
    let pause = PauseController::new(&keys);
    println!(
        "按键：进入暂停 = {}，退出暂停 = {}\n",
        pause.enter_key(),
        pause.exit_key()
    );

    let _timer = TimerResolution::acquire();

    // ——— 预检：先确认按键真的能到游戏里 ———
    //
    // 不做这一步的话，"按键被 UIPI 静默丢弃"和"脉冲间隔太短"会表现得一模一样
    // （都是 100% 空脉冲），根本没法区分。这里单独试每个键，用足够长的时间
    // 让效果明显。
    println!("== 预检：确认按键能到达游戏 ==");
    if !repl_input::is_elevated() {
        println!("  ⚠ 本程序**没有**以管理员权限运行。");
        println!("    明日方舟 PC 端通常以管理员权限运行，此时 Windows 的 UIPI 会把");
        println!("    低权限进程的按键**静默丢弃** —— SendInput 照样返回成功，但游戏收不到。");
        println!("    如果下面的预检失败，请以管理员身份重开终端再试。\n");
    }

    let probe_ms = 300_u64;
    let expect = (probe_ms as f64 / 1000.0 * 30.0).round() as i64; // 1 倍速 30 帧/秒
    let mut cursor = baseline.total_elapsed_frames;
    let mut last_frame_id = baseline.frame_id.unwrap_or(0);

    let mut measure = |label: &str, key: repl_input::KeyCode| -> i64 {
        if repl_input::key_tap(key, repl_input::keys::DEFAULT_TAP_HOLD).is_err() {
            println!("  {label}（{key}）：发送失败");
            return 0;
        }
        std::thread::sleep(Duration::from_millis(probe_ms));
        let mut advanced = 0;
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            let Ok(s) = ruler.wait_next(last_frame_id, Duration::from_millis(300)) else {
                continue;
            };
            let Some(id) = s.frame_id else { continue };
            if id <= last_frame_id || !s.frame_reading_is_trustworthy() {
                continue;
            }
            last_frame_id = id;
            advanced = s.total_elapsed_frames - cursor;
            cursor = s.total_elapsed_frames;
            break;
        }
        println!(
            "  按一下 {label}（{key}）后等 {probe_ms}ms：帧数推进 {advanced}（预期 0 或 ≈{expect}）"
        );
        advanced
    };

    let esc_advance = measure("进入暂停键", pause.enter_key());
    let space_advance = measure("退出暂停键", pause.exit_key());

    // 不管刚才把游戏搞成什么状态，都先停回暂停态。
    if let Ok(s) = ruler.wait_next(0, Duration::from_millis(600)) {
        if s.battle_state.as_ref().and_then(|b| b.is_paused()) == Some(false) {
            let _ = pause.pause();
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    if esc_advance == 0 && space_advance == 0 {
        println!("\n  ✗ 预检失败：两个键按下去游戏都毫无反应。");
        if repl_input::is_elevated() {
            println!("    程序已经是管理员权限，所以不是 UIPI 的问题。请检查：");
            println!("      · 游戏窗口确实在前台（预检期间不要切窗口）");
            println!(
                "      · 游戏内键位设置里「暂停」确实绑在 {} 上",
                pause.exit_key()
            );
            println!("      · 没有第三方按键拦截软件（AHK 脚本、手柄映射工具等）在抢");
        } else {
            println!("    **请以管理员身份重开终端再跑一次。** 这是最可能的原因。");
        }
        println!("\n  预检不过就不用往下测了，100 次脉冲只会得到 100 个空脉冲。");
        return;
    }
    println!("  ✓ 按键能到达游戏。\n");

    // 顺带确认按键语义和 AFA 一致：应当是"进入暂停键把游戏放开、退出暂停键把它停住"。
    if esc_advance == 0 && space_advance > 0 {
        println!(
            "  注意：推进游戏的是「{}」而不是「{}」，与 AFA 的约定相反。",
            pause.exit_key(),
            pause.enter_key()
        );
        println!("  脉冲时序按 AFA 的顺序发，可能需要调整。\n");
    }

    let mut tuner = GapTuner::new(DEFAULT_GAP_1X_MS);
    let mut histogram: BTreeMap<i64, u32> = BTreeMap::new();

    println!("== 正式测试：{pulses} 次单帧脉冲 ==");
    println!("开始。每 10 次打印一行进度。\n");
    for i in 1..=pulses {
        // 中途切走窗口的话，后面的按键会打到别的程序上 —— 立刻停下比继续危险。
        if !window.is_foreground() {
            println!("\n第 {i} 次之前检测到游戏已不在前台，停止测试。");
            println!("（已完成 {} 次，结果见下。）", i - 1);
            break;
        }
        // 固定间隔模式下不走闭环整定 —— 用来单独验证某一档是否稳定。
        let gap = fixed_gap.map_or_else(|| tuner.gap(), |ms| Duration::from_millis(u64::from(ms)));
        if let Err(e) = pause.pulse(gap) {
            println!("第 {i} 次脉冲失败：{e}");
            return;
        }

        // 等一条新的、可信的样本
        let mut delta = None;
        let deadline = Instant::now() + Duration::from_millis(2000);
        while Instant::now() < deadline {
            match ruler.wait_next(last_frame_id, Duration::from_millis(300)) {
                Ok(s) => {
                    let Some(id) = s.frame_id else { continue };
                    if id <= last_frame_id || !s.frame_reading_is_trustworthy() {
                        continue;
                    }
                    last_frame_id = id;
                    delta = Some(s.total_elapsed_frames - cursor);
                    cursor = s.total_elapsed_frames;
                    break;
                }
                Err(_) => continue,
            }
        }
        let Some(delta) = delta else {
            println!("第 {i} 次脉冲后等不到新的分析帧，中止。");
            break;
        };

        *histogram.entry(delta).or_default() += 1;
        tuner.observe(delta);

        if i % 10 == 0 {
            println!(
                "  {i:>4}/{pulses}  当前间隔 {}ms  绝对帧 {cursor}",
                fixed_gap.unwrap_or_else(|| tuner.gap_ms())
            );
        }
    }

    println!("\n== 结果 ==");
    for (delta, count) in &histogram {
        let bar = "#".repeat((*count as usize).min(60));
        println!("  推进 {delta:>+2} 帧 : {count:>4} 次  {bar}");
    }
    println!("\n  总脉冲      {}", tuner.pulses);
    println!(
        "  空脉冲      {} ({:.0}%)",
        tuner.zero_frame_pulses,
        tuner.zero_rate() * 100.0
    );
    println!("  跨帧        {}", tuner.overshoot_pulses);
    println!("  最终间隔    {}ms", tuner.gap_ms());

    let advanced = *histogram.get(&1).unwrap_or(&0);

    println!("\n== 判定 ==");
    if tuner.had_overshoot() {
        let total = f64::from(tuner.pulses);
        let p_over = f64::from(tuner.overshoot_pulses) / total;
        let p_hit = f64::from(advanced) / total;
        // 跨帧只有发生在**最后一步**（游标 = 目标 − 1）时才致命；
        // 更早的跨帧只是让游标离目标更近，状态机不会中止。
        // 所以每个动作的失败概率 = 在最后一格上"先跨帧、后命中"的概率。
        let per_action = if p_hit + p_over > 0.0 {
            p_over / (p_hit + p_over)
        } else {
            1.0
        };
        println!(
            "  ✗ 不通过：{} 次跨帧 / {} 次脉冲（{:.0}%）。",
            tuner.overshoot_pulses,
            tuner.pulses,
            p_over * 100.0
        );
        println!("\n  这意味着什么：");
        println!("    跨帧只有发生在最后一步（离目标还剩 1 帧）时才致命，更早跨帧无害。");
        println!(
            "    按当前分布，每个动作约有 {:.1}% 的概率因跨帧而中止；",
            per_action * 100.0
        );
        for actions in [5_u32, 10, 20] {
            let ok = (1.0 - per_action).powi(actions as i32);
            println!(
                "      {actions:>2} 个动作的作业能一次跑完的概率约 {:.0}%",
                ok * 100.0
            );
        }
        println!("\n  怎么解决（按有效性排序）：");
        println!("    1. **把游戏帧率锁到 30**（设置 → 画面 → 帧率），并关掉垂直同步。");
        println!("       逻辑帧就是 30/秒，渲染帧也锁 30 的话两者 1:1 对齐，");
        println!("       「解暂停 → 下一个 tick → 暂停」就正好是一帧，量化误差直接消失。");
        println!("       这是唯一能从根上解决问题的办法，其余都是碰运气。");
        println!("    2. 关掉后台占 CPU 的程序（浏览器、录屏、杀毒扫描）。");
        println!("    3. 用固定间隔多测几次找稳定档：step-test.exe 100 <间隔ms>");
        println!("\n  改完帧率后请重跑本工具确认。");
    } else if histogram.keys().any(|d| *d < 0) {
        println!("  ✗ 不通过：出现了帧数倒退，说明尺子读数不稳定。");
        println!("    检查鼠标是否停在费用条上（PC 自绘光标会遮挡），或重新校准尺子。");
    } else if advanced == 0 {
        // 全是空脉冲 = 逐帧推进这个机制压根没工作。这绝不是"通过"。
        println!(
            "  ✗ 不通过：{} 次脉冲一帧都没推进，逐帧机制没有生效。",
            tuner.pulses
        );
        println!(
            "    间隔已经自适应到上限 {}ms 仍然推不动，说明不是间隔的问题。",
            tuner.gap_ms()
        );
        println!("    预检虽然过了（按键能到游戏），但脉冲那个「解暂停→再暂停」的时序没起作用。");
        println!("    请把完整输出发我 —— 需要按你这边的实际按键语义调整脉冲顺序。");
    } else if advanced < tuner.pulses / 4 {
        println!(
            "  △ 勉强可用：{advanced}/{} 次成功推进，空脉冲占比 {:.0}% 偏高。",
            tuner.pulses,
            tuner.zero_rate() * 100.0
        );
        println!("    正确性没问题（零跨帧），只是对帧偏慢：平均每前进一帧要发");
        println!(
            "    {:.1} 次脉冲。实测表明调大间隔换不来更高命中率，若嫌慢可尝试",
            f64::from(tuner.pulses) / f64::from(advanced.max(1))
        );
        println!("    把游戏帧率锁到 30（渲染帧与逻辑帧 1:1，命中率会大幅上升）。");
    } else {
        println!("  ✓ 通过：{advanced} 次成功推进，没有任何一次跨帧。");
        if tuner.zero_rate() > 0.30 {
            println!(
                "    空脉冲占比 {:.0}%，平均每前进一帧要发 {:.1} 次脉冲。",
                tuner.zero_rate() * 100.0,
                f64::from(tuner.pulses) / f64::from(advanced.max(1))
            );
            println!("    这只影响速度、不影响正确性（实测调 gap 换不来更高命中率）。");
            println!("    若想提速可把游戏帧率锁到 30 —— 渲染帧与逻辑帧 1:1 后命中率会大幅上升。");
        }
    }
}
