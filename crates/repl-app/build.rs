// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

fn main() {
    // 界面整体是深色 HUD；默认的 fluent 会跟随系统主题，系统为浅色时控件变成白底，
    // 与深色背景冲突。固定为 fluent-dark 保证任何系统主题下外观一致。
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("../../ui/main.slint", config)
        .expect("failed to compile Slint UI");
}
