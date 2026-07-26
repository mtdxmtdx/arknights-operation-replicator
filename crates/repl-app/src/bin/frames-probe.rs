// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 阶段 1 的验收工具：实时打印尺子的帧数与战斗状态。
//!
//! ```text
//! cargo run -p repl-app --bin frames-probe
//! ```
//!
//! 期望：进入关卡后 `elapsed` 随游戏推进单调增长；按暂停后停住；
//! 退出结算时 `battle_state` 变成 `before_or_after_battle`。

use std::time::{Duration, Instant};

use repl_frames::{FrameError, FrameSource, RulerClient};

fn main() {
    repl_app::init("info");

    println!("== ArknightsCostBarRuler 帧源探针 ==");
    let ruler = RulerClient::connect_default();

    println!("正在连接尺子…");
    match ruler.probe(Duration::from_secs(5)) {
        Ok(snap) => println!(
            "已连接：apiVersion={} appVersion={} 校准配置={}",
            snap.api_version,
            snap.app_version,
            snap.active_profile.as_deref().unwrap_or("<未校准>")
        ),
        Err(e) => {
            println!("连不上尺子：{e}");
            println!("检查 ruler-app.exe 是否已启动。");
            return;
        }
    }

    let mut last_status = ruler.status();
    let mut cursor = 0_u64;
    let mut printed = 0_u64;
    let started = Instant::now();

    println!("开始跟踪，Ctrl-C 退出。");
    loop {
        let status = ruler.status();
        if status != last_status {
            println!("[连接] {last_status:?} -> {status:?}");
            last_status = status;
        }

        match ruler.wait_next(cursor, Duration::from_millis(1000)) {
            Ok(snap) => {
                cursor = snap.frame_id.unwrap_or(cursor);
                printed += 1;
                // 只有可信读数才值得刷屏；不可信的单独标注出来。
                let trust = if snap.frame_reading_is_trustworthy() {
                    " "
                } else {
                    "!"
                };
                println!(
                    "{trust}frame_id={:<8} elapsed={:<6} cycle={:>3}/{:<3} state={:<24} profile={:<12} time={}",
                    cursor,
                    snap.total_elapsed_frames,
                    snap.current_frame.map_or(-1, |v| v),
                    snap.total_frames_in_cycle,
                    snap.battle_state_str(),
                    snap.active_profile.as_deref().unwrap_or("<未校准>"),
                    snap.time,
                );
            }
            Err(FrameError::Timeout(_)) => {
                if status.is_connected() {
                    println!("(1s 内没有新的分析帧；游戏可能不在战斗中)");
                }
            }
            Err(e) => {
                println!("[错误] {e}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        if started.elapsed() > Duration::from_secs(600) {
            println!("跑满 10 分钟，共 {printed} 条样本，退出。");
            return;
        }
    }
}
