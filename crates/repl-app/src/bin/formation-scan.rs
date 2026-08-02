// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 战前编队 OCR 只读诊断。
//!
//! ```text
//! cargo run --release -p repl-app --bin formation-scan -- examples/test1.json
//! ```
//!
//! 运行前把游戏停在编队确认界面。本工具只截图，不发送任何游戏输入。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use repl_app::Config;
use repl_capture::{Frame, GameWindow, WindowCapturer};
use repl_core::{Copilot, Point, Rect, Viewport};
use repl_vision::{FormationResources, FormationScanner};

fn main() {
    repl_app::init("info,repl_vision=debug");
    if let Err(error) = run() {
        eprintln!("编队扫描失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> anyhow::Result<()> {
    let job_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("用法：formation-scan <作业.json>"))?;
    let job_text = fs::read_to_string(&job_path)?;
    let job = Copilot::parse(&job_text)?;
    let job_names = job.oper_names();
    if job_names.is_empty() {
        anyhow::bail!("作业 opers 为空，没有可用于确认的开局编队");
    }

    let config = Config::load();
    let resource_dir = config
        .resolve_maa_resource_dir()
        .ok_or_else(|| anyhow::anyhow!("找不到 MAA resource 目录"))?;
    println!("MAA 资源：{}", resource_dir.display());
    println!("作业编队：{}", job_names.join("、"));

    let window = GameWindow::find()?;
    let geometry = window.geometry()?;
    let ui_scaler = config
        .ui_scaler_override
        .or_else(|| repl_input::game_keys::read_ui_scaler().ok().flatten())
        .unwrap_or(repl_core::viewport::DEFAULT_UI_SCALER);
    let viewport = Viewport::new(geometry.width, geometry.height, ui_scaler);
    let mut capturer = WindowCapturer::new(window)?;
    let raw = capturer.grab()?;
    let mut reference = raw.resample(
        viewport.reference_source(),
        repl_core::REF_WIDTH as u32,
        repl_core::REF_HEIGHT as u32,
    );

    let started = std::time::Instant::now();
    let resources = FormationResources::load(&resource_dir)?;
    let mut scanner = FormationScanner::new(resources)?;
    let report = scanner.scan(&reference, &job_names)?;
    let elapsed = started.elapsed();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let output_dir = PathBuf::from("diagnostics");
    fs::create_dir_all(&output_dir)?;
    let image_path = output_dir.join(format!("formation-scan-{stamp}.png"));
    let json_path = output_dir.join(format!("formation-scan-{stamp}.json"));

    for row in &report.rows {
        outline(&mut reference, row.flag_rect, [255, 0, 0]);
        if let Some(rect) = row.name_rect {
            outline(&mut reference, rect, [0, 255, 0]);
        }
        if let Some(rect) = row.avatar_rect {
            outline(&mut reference, rect, [255, 200, 0]);
        }
    }
    save_png(&reference, &image_path)?;

    let rows = report
        .rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "slot": row.slot,
                "raw_text": row.raw_text,
                "confidence": row.confidence,
                "suggestion": row.suggestion.as_ref().map(|operator| &operator.display_name),
                "operator_id": row.suggestion.as_ref().map(|operator| &operator.id),
                "source": row.source.map(|source| source.zh()),
                "default_selected": row.default_selected,
                "role": row.suggestion.as_ref().map(|operator| operator.role.zh()),
                "avatar_available": row.avatar.is_some(),
            })
        })
        .collect::<Vec<_>>();
    let result = serde_json::json!({
        "job": job_path,
        "used_old_layout": report.used_old_layout,
        "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
        "missing_job_opers": report.missing_job_opers,
        "foreign_opers": report.foreign_opers,
        "rows": rows,
    });
    fs::write(&json_path, serde_json::to_vec_pretty(&result)?)?;

    println!(
        "识别 {} 个槽位，耗时 {:.1}ms{}",
        report.rows.len(),
        elapsed.as_secs_f64() * 1000.0,
        if report.used_old_layout {
            "（旧版姓名布局）"
        } else {
            ""
        }
    );
    for row in &report.rows {
        let suggestion = row
            .suggestion
            .as_ref()
            .map_or("未识别", |operator| operator.display_name.as_str());
        println!(
            "  #{:<2} OCR={:<12} conf={:.3} → {}{}",
            row.slot,
            row.raw_text,
            row.confidence,
            suggestion,
            row.source
                .map(|source| format!(" [{}]", source.zh()))
                .unwrap_or_default()
        );
    }
    if !report.missing_job_opers.is_empty() {
        println!("作业未识别：{}", report.missing_job_opers.join("、"));
    }
    if !report.foreign_opers.is_empty() {
        println!("非作业干员：{}", report.foreign_opers.join("、"));
    }
    println!("诊断图：{}", image_path.display());
    println!("诊断 JSON：{}", json_path.display());
    println!("红框=姓名锚点，绿框=实际 OCR 行，黄框=编队头像；文件仅保存在本机。");
    Ok(())
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
    let offset = (at.y as usize * frame.width as usize + at.x as usize) * 4;
    frame.pixels[offset] = rgb[2];
    frame.pixels[offset + 1] = rgb[1];
    frame.pixels[offset + 2] = rgb[0];
}

fn save_png(frame: &Frame, path: &Path) -> anyhow::Result<()> {
    let mut rgba = Vec::with_capacity(frame.pixels.len());
    for pixel in frame.pixels.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    image::RgbaImage::from_raw(frame.width, frame.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("像素缓冲区尺寸不符"))?
        .save(path)?;
    Ok(())
}
