// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! `repl-app` —— 编排层：把帧源、输入注入、识别、状态机接到一起。

pub mod config;
pub mod editor;
pub mod runner;
pub mod session;

pub use config::{Binding, BindingProfile, BindingStore, Config};
pub use editor::EditorState;
pub use runner::{Progress, Runner};
pub use session::Session;

/// 统一的进程启动初始化：控制台编码 + 日志（写到 stderr，给命令行工具用）。
///
/// 中文 Windows 的控制台默认代码页是 936(GBK)，而 Rust 始终往 stdout 写 UTF-8，
/// 不改代码页的话所有中文输出都是乱码。
pub fn init(default_log_filter: &str) {
    set_console_utf8();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_log_filter))
        .init();
}

/// GUI 版的日志初始化：写到文件。
///
/// release 构建的 GUI 是 `windows` 子系统，**stderr 不存在** —— 用 [`init`] 的话
/// 所有日志都会静默消失，出了问题只能盲猜。所以 GUI 一律把日志写到可执行文件
/// 旁边的 `replicator.log`（每次启动覆盖，避免无限增长）。
/// 打不开文件时退回 stderr（开发时 `cargo run` 有控制台，照样能看）。
pub fn init_to_file(default_log_filter: &str, path: &std::path::Path) {
    set_console_utf8();
    let builder = || {
        env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or(default_log_filter),
        )
    };
    match std::fs::File::create(path) {
        Ok(file) => builder()
            .target(env_logger::Target::Pipe(Box::new(file)))
            .init(),
        Err(_) => builder().init(),
    }
    log::info!(
        "Arknights Operation Replicator {} started, logging to {}",
        env!("CARGO_PKG_VERSION"),
        path.display()
    );
}

#[cfg(windows)]
fn set_console_utf8() {
    use windows::Win32::Globalization::CP_UTF8;
    use windows::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};

    // 进程没有控制台（GUI 子系统）时这两个调用会失败，无所谓。
    // SAFETY: 只改本进程控制台的代码页。
    unsafe {
        let _ = SetConsoleOutputCP(CP_UTF8);
        let _ = SetConsoleCP(CP_UTF8);
    }
}

#[cfg(not(windows))]
fn set_console_utf8() {}

/// 免责声明。首次启动必须让用户确认一次。
pub const DISCLAIMER: &str = "\
本工具通过操作系统的公开接口（屏幕捕获 + 输入注入）操作《明日方舟》PC 客户端，
不注入游戏进程、不读写游戏内存、不修改游戏文件、不与游戏服务器通信。

即便如此，使用自动化工具仍可能违反游戏的用户协议，由此产生的一切后果由使用者自行承担。

《明日方舟》著作权归上海鹰角网络科技有限公司所有，本项目与其无任何隶属或授权关系。

本程序以 AGPL-3.0-only 授权，分发时必须同时提供完整源代码。";
