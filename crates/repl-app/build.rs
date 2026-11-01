// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

fn main() {
    // 界面整体是深色 HUD；默认的 fluent 会跟随系统主题，系统为浅色时控件变成白底，
    // 与深色背景冲突。固定为 fluent-dark 保证任何系统主题下外观一致。
    let mut config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    // 界面行为测试（Slint 测试后端按控件查找元素）需要调试信息；只在 debug 构建里打开，
    // release 发布版不带这部分元数据。
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        config = config.with_debug_info(true);
    }
    slint_build::compile_with_config("../../ui/main.slint", config)
        .expect("failed to compile Slint UI");
}
