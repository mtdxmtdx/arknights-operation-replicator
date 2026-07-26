// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 阶段 3 验收：把算出来的格子坐标画到真实截图上。
//!
//! ```text
//! cargo run --release -p repl-app --bin tile-preview -- <关卡> [输出文件]
//! # 例：cargo run --release -p repl-app --bin tile-preview -- 1-7
//! ```
//!
//! 判据：**每个绿点都落在对应格子的中心**，不能整体偏移或错位一格。
//! 这是比"和另一份实现对拍"更有意义的验证 —— 它验的是与游戏本身的一致性。
//!
//! 绿点 = 正常视角（点选场上干员用），青点 = 侧视角（拖拽部署的落点）。

use std::path::PathBuf;

use repl_app::config::Config;
use repl_capture::{Frame, GameWindow, WindowCapturer};
use repl_core::{LevelPack, Point, TileProjection, Viewport};

fn main() {
    repl_app::init("info");

    let mut args = std::env::args().skip(1);
    let Some(stage) = args.next() else {
        println!("用法：tile-preview <关卡> [输出文件]");
        println!("  关卡可以写 stageId / 关卡号 / 关卡名，例如 main_10-07、10-7、夜风");
        return;
    };
    let output = args.next().map_or_else(
        || PathBuf::from(format!("tile-preview-{}.png", sanitize(&stage))),
        PathBuf::from,
    );

    let config = Config::load();
    let Some(resource_dir) = config.resolve_maa_resource_dir() else {
        println!(
            "找不到 MAA 资源目录。设置环境变量 REPLICATOR_MAA_RESOURCE 指向 MAA 的 resource 目录，"
        );
        println!("或在 config.json 里填 maa_resource_dir。");
        return;
    };
    println!("MAA 资源目录：{}", resource_dir.display());

    let pack = match LevelPack::load(resource_dir.join("Arknights-Tile-Pos")) {
        Ok(p) => p,
        Err(e) => {
            println!("加载关卡索引失败：{e}");
            return;
        }
    };
    let level = match pack.load_level(&stage) {
        Ok(l) => l,
        Err(e) => {
            println!("{e}");
            let hits = pack.search(&stage, 8);
            if !hits.is_empty() {
                println!("你是不是想找：");
                for h in hits {
                    println!("  {} / {} / {}", h.key.stage_id, h.key.code, h.key.name);
                }
            }
            return;
        }
    };
    println!(
        "关卡 {} ({}) {}×{}",
        level.key.code,
        level.key.stage_id,
        level.width(),
        level.height()
    );

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
    println!(
        "客户区 {}×{}，内容区 {}，缩放 {:.3}，uiScaler {ui_scaler}",
        geometry.width,
        geometry.height,
        viewport.content_rect(),
        viewport.scale()
    );

    let mut capturer = match WindowCapturer::new(window) {
        Ok(c) => c,
        Err(e) => {
            println!("建立截图会话失败：{e}");
            return;
        }
    };
    let mut frame = match capturer.grab() {
        Ok(f) => f,
        Err(e) => {
            println!("截图失败：{e}");
            return;
        }
    };

    let projection = TileProjection::compute(&level, (0.0, 0.0));
    if projection.has_multi_stages {
        println!("注意：这张图被判定为多阶段地图，按钮位置按第一阶段计算。");
    }

    let mut drawn = 0;
    for loc in level.locations() {
        if let Some(p) = projection.normal_pos(loc) {
            if repl_core::tile::is_on_screen(p) {
                cross(&mut frame, viewport.field_to_client(p), [0, 255, 0], 6);
                drawn += 1;
            }
        }
        if let Some(p) = projection.side_pos(loc) {
            if repl_core::tile::is_on_screen(p) {
                cross(&mut frame, viewport.field_to_client(p), [255, 255, 0], 3);
            }
        }
    }
    // 撤退（品红）/ 技能（橙）按钮
    cross(
        &mut frame,
        viewport.field_to_client(projection.retreat_button),
        [255, 0, 255],
        8,
    );
    cross(
        &mut frame,
        viewport.field_to_client(projection.skill_button),
        [255, 128, 0],
        8,
    );

    match save_png(&frame, &output) {
        Ok(()) => {
            println!("\n已写出 {}（画了 {drawn} 个格子）", output.display());
            println!("\n请打开图片检查：");
            println!("  · 绿色十字必须落在每个格子的**中心**（正常视角）");
            println!("  · 黄色十字是侧视角落点，会整体偏下偏左，这是正常的");
            println!("  · 品红 = 撤退按钮，橙色 = 技能按钮（选中干员后出现的位置）");
            println!("  · 如果绿点整体偏移或错位一格，说明坐标映射有问题，不要继续用");
        }
        Err(e) => println!("写出图片失败：{e}"),
    }
}

/// 在图上画一个十字（BGRA 直接改像素）。
fn cross(frame: &mut Frame, at: Point, rgb: [u8; 3], arm: i32) {
    for d in -arm..=arm {
        put(frame, Point::new(at.x + d, at.y), rgb);
        put(frame, Point::new(at.x, at.y + d), rgb);
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
    let buffer = image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .ok_or_else(|| "像素缓冲区尺寸不符".to_owned())?;
    buffer.save(path).map_err(|e| e.to_string())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}
