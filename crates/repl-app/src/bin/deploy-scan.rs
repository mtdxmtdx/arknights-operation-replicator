// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 阶段 4 验收：部署栏识别自检。
//!
//! ```text
//! cargo run --release -p repl-app --bin deploy-scan
//! ```
//!
//! 判据：**识别出的卡片数量与屏幕上一致，职业全部正确，头像框正好框住头像。**

use std::path::PathBuf;

use repl_app::config::{perceptual_hash, Config};
use repl_capture::{Frame, GameWindow, WindowCapturer};
use repl_core::{Point, Rect, Viewport};
use repl_vision::TemplateSet;

fn main() {
    repl_app::init("info");
    let output = PathBuf::from("deploy-scan.png");

    let config = Config::load();
    let Some(resource_dir) = config.resolve_maa_resource_dir() else {
        println!("找不到 MAA 资源目录。设置 REPLICATOR_MAA_RESOURCE 或在 config.json 里填 maa_resource_dir。");
        return;
    };
    let templates = match TemplateSet::load(&resource_dir) {
        Ok(t) => t,
        Err(e) => {
            println!("加载模板失败：{e}");
            return;
        }
    };

    let window = match GameWindow::find() {
        Ok(w) => w,
        Err(e) => {
            println!("找不到游戏窗口：{e}");
            return;
        }
    };
    let geometry = match window.geometry() {
        Ok(g) => g,
        Err(e) => {
            println!("读取窗口几何失败：{e}");
            return;
        }
    };
    let ui_scaler = config
        .ui_scaler_override
        .or_else(|| repl_input::game_keys::read_ui_scaler().ok().flatten())
        .unwrap_or(repl_core::viewport::DEFAULT_UI_SCALER);
    let viewport = Viewport::new(geometry.width, geometry.height, ui_scaler);

    let mut capturer = match WindowCapturer::new(window) {
        Ok(c) => c,
        Err(e) => {
            println!("建立截图会话失败：{e}");
            return;
        }
    };
    let raw = match capturer.grab() {
        Ok(f) => f,
        Err(e) => {
            println!("截图失败：{e}");
            return;
        }
    };
    // 识别必须在 1280×720 参考坐标系里做 —— 模板就是那个尺度裁的。
    let mut frame = raw.resample(
        viewport.reference_source(),
        repl_core::REF_WIDTH as u32,
        repl_core::REF_HEIGHT as u32,
    );

    let started = std::time::Instant::now();
    let cards = repl_vision::analyze(&frame, &viewport, &templates);
    let elapsed = started.elapsed();

    println!(
        "客户区 {}×{} → 参考帧 {}×{}，uiScaler {ui_scaler}，识别耗时 {:.1}ms\n",
        geometry.width,
        geometry.height,
        frame.width,
        frame.height,
        elapsed.as_secs_f64() * 1000.0
    );

    // 无论成败都打印旗标候选：卡片漏检时这是唯一能看出"漏在哪"的信息。
    let probes = repl_vision::deployment::probe_flags(&frame, &viewport, &templates, 16);
    println!("旗标候选（阈值 {:.2}，按分数降序）：", 0.65);
    for m in &probes {
        let taken = m.score >= 0.65;
        println!(
            "  {:<24} {:.3} {}",
            format!("{}", m.rect),
            m.score,
            if taken { "← 采纳" } else { "" }
        );
    }
    println!();

    if cards.is_empty() {
        // 即使失败也把图写出来，并报告"最像旗标的位置"，
        // 用来区分"模板压根不匹配"和"搜索区域找错地方"。
        println!("一张卡片都没识别到。诊断信息：\n");
        if probes.is_empty() {
            println!("  搜索带内没有任何候选位置（带子可能是空的）。");
        } else {
            for m in &probes {
                outline(&mut frame, m.rect, [255, 0, 0]);
            }
            // 拿最高分那处推导出子区域，看职业识别为什么失败
            let best = &probes[0];
            let bounds = frame.bounds();
            let r = repl_vision::deployment::subrects(best.rect, bounds);
            println!("\n  以最高分位置 {} 推导的子区域：", best.rect);
            println!("    点击区 {}", r.click);
            println!("    职业   {}", r.role);
            println!("    头像   {}", r.avatar);
            println!("\n  该职业区域的九个模板得分（阈值 0.65）：");
            for (role, score) in repl_vision::deployment::probe_roles(&frame, r.role, &templates) {
                println!("    {:<8} {:.3}", role.zh(), score);
            }
            outline(&mut frame, r.click, [0, 255, 0]);
            outline(&mut frame, r.role, [255, 0, 255]);
            outline(&mut frame, r.avatar, [255, 200, 0]);
        }
        let roi = repl_vision::deployment::flag_search_roi(&viewport);
        outline(&mut frame, roi, [0, 160, 255]);
        let _ = save_png(&frame, &output);
        println!(
            "\n  已写出 {}（蓝框 = 搜索带，红框 = 候选位置）",
            output.display()
        );
        println!("\n  怎么看：");
        println!("    · 最高分 > 0.6 但没到阈值 → 模板基本对，微调阈值即可");
        println!("    · 最高分 < 0.4 → PC 端的卡片外观和 MAA 的安卓模板不一样，需要换识别方案");
        println!("    · 红框位置和真实卡片对不上 → 搜索带找错地方了");
        return;
    }

    println!(
        "{:<4} {:<8} {:<10} {:<24} {:<18}",
        "序号", "职业", "状态", "点击区", "头像哈希"
    );
    for card in &cards {
        let state = if card.cooling {
            "冷却"
        } else if card.available {
            "可用"
        } else {
            "费用不足"
        };
        let hash = perceptual_hash(&card.avatar, card.avatar_width, card.avatar_height);
        println!(
            "{:<4} {:<8} {:<10} {:<24} {:016x}",
            card.index,
            card.role.zh(),
            state,
            format!("{}", card.click_rect),
            hash
        );
        outline(&mut frame, card.click_rect, [0, 255, 0]);
        outline(&mut frame, card.avatar_rect, [255, 200, 0]);
    }

    // 部署栏搜索带
    outline(
        &mut frame,
        repl_vision::deployment::flag_search_roi(&viewport),
        [0, 160, 255],
    );

    match save_png(&frame, &output) {
        Ok(()) => {
            println!("\n共 {} 张卡片，已写出 {}", cards.len(), output.display());
            println!("\n请打开图片检查：");
            println!("  · 蓝框 = 旗标搜索带，必须横跨整个部署栏");
            println!("  · 绿框 = 卡片点击区（拖拽起点），必须覆盖整张卡");
            println!("  · 黄框 = 头像裁图，必须正好框住干员头像，不能带进边框或职业图标");
            println!("  · 卡片数量、职业、可用/冷却状态都要和屏幕上一致");
        }
        Err(e) => println!("写出图片失败：{e}"),
    }
}

fn outline(frame: &mut Frame, rect: Rect, rgb: [u8; 3]) {
    for x in rect.x..rect.right() {
        put(frame, Point::new(x, rect.y), rgb);
        put(frame, Point::new(x, rect.bottom() - 1), rgb);
    }
    for y in rect.y..rect.bottom() {
        put(frame, Point::new(rect.x, y), rgb);
        put(frame, Point::new(rect.right() - 1, y), rgb);
    }
}

fn put(frame: &mut Frame, at: Point, rgb: [u8; 3]) {
    if at.x < 0 || at.y < 0 || at.x >= frame.width as i32 || at.y >= frame.height as i32 {
        return;
    }
    let i = (at.y as usize * frame.width as usize + at.x as usize) * 4;
    frame.pixels[i] = rgb[2];
    frame.pixels[i + 1] = rgb[1];
    frame.pixels[i + 2] = rgb[0];
}

fn save_png(frame: &Frame, path: &PathBuf) -> Result<(), String> {
    let mut rgba = Vec::with_capacity(frame.pixels.len());
    for px in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .ok_or_else(|| "像素缓冲区尺寸不符".to_owned())?
        .save(path)
        .map_err(|e| e.to_string())
}
