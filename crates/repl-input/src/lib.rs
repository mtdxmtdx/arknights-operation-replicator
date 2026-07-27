// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本 crate 移植自 arknights-frame-assistant (GPL-3.0-only):
//   src/lib/touch_injection.ahk / src/lib/game_keys.ahk / src/lib/hotkey_actions.ahk
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! `repl-input` —— Windows 输入注入层。
//!
//! - [`touch`] 触控注入（`InjectTouchInput`）。**全程不移动鼠标指针**，
//!   否则 Arknights PC 的自绘光标会遮挡费用条，尺子就没法计帧了。
//! - [`keys`] 键盘注入（`SendInput`，同时带虚拟键码和扫描码）。
//! - [`mouse`] 指针停靠及 XButton / 滚轮 `SendInput` 注入。
//! - [`game_keys`] 从注册表读游戏内键位设置。
//! - [`stepper`] 暂停控制与逐帧脉冲，含闭环间隔自适应。
//! - [`clock`] 高精度延时与定时器精度 / 线程优先级的 RAII 包装。

pub mod afa;
pub mod clock;
pub mod game_keys;
pub mod keys;
pub mod mouse;
pub mod stepper;
pub mod touch;

pub use afa::{AfaAction, AfaBindings, AfaController, AfaError, AfaHotkey, AfaStatus};
pub use clock::{precise_sleep, TimeCriticalPriority, TimerResolution};
pub use game_keys::GameKeys;
pub use keys::{is_elevated, key_down, key_tap, key_up, KeyCode, DEFAULT_TAP_HOLD};
pub use stepper::{GapTuner, PauseController};
pub use touch::TouchInjector;

use repl_core::Point;

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("触控注入初始化失败：{0}")]
    TouchInit(String),
    #[error("在 {at} 注入触控事件失败：{reason}")]
    TouchInject { at: Point, reason: String },
    #[error("触点已经处于按下状态")]
    TouchAlreadyDown,
    #[error("触点当前未按下")]
    TouchNotDown,
    #[error("滑动路径为空")]
    EmptySwipePath,
    #[error("SendInput 未能送出输入 {0}（可能被更高完整性级别的窗口拦截，试试以管理员身份运行）")]
    SendInputBlocked(String),
    #[error("移动鼠标指针失败：{0}")]
    CursorMove(String),
    #[error("当前平台不支持输入注入")]
    Unsupported,
}
