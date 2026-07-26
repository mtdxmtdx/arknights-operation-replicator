// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! `repl-capture` —— 游戏窗口定位与按需截图。
//!
//! 帧数由尺子进程提供，本 crate 只在暂停态下按需抓单张图（开局像素触发、
//! 编队匹配、每次部署前刷新部署栏），所以不需要常驻截图管线。

pub mod wgc;
pub mod window;

pub use wgc::Frame;
#[cfg(windows)]
pub use wgc::WindowCapturer;
pub use window::{ClientGeometry, GameWindow, GAME_PROCESS};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("找不到 {0} 的窗口，请先启动游戏")]
    WindowNotFound(String),
    #[error("游戏窗口不可用：{0}")]
    WindowNotUsable(String),
    #[error("游戏窗口尺寸已改变，需要重新建立捕获会话并重算坐标映射")]
    WindowResized,
    #[error("等待截图帧超时")]
    FrameTimeout,
    #[error("截图后端错误：{0}")]
    Backend(String),
    #[error("当前系统不支持 Windows Graphics Capture（需要 Windows 10 1803 或更高）")]
    Unsupported,
}
