// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 鼠标停靠。
//!
//! 本项目不用鼠标点击（点击全走触控注入），但会做纯指针位移。这带来一个
//! 反面问题：触控注入不移动鼠标指针，所以指针会一直停在用户最后放的位置。
//! Arknights PC 自绘光标，一旦指针停在费用条或费用数字附近，尺子就会把每一帧
//! 标记 `cursorBlocked` 并冻结分析，我们就永远拿不到新的帧数了。
//!
//! 所以平时把指针**挪到安全区停好**；暂停中选中前会把它移到目标干员。
//! 两者都只是纯位移，不按键、不点击。

use repl_core::Point;

use crate::InputError;

/// 把系统鼠标指针移动到屏幕坐标。纯位移，不产生任何按键事件。
pub fn set_cursor_pos(screen: Point) -> Result<(), InputError> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
        // SAFETY: 只传屏幕坐标标量。
        unsafe { SetCursorPos(screen.x, screen.y) }
            .map_err(|e| InputError::CursorMove(e.to_string()))
    }
    #[cfg(not(windows))]
    {
        let _ = screen;
        Err(InputError::Unsupported)
    }
}
