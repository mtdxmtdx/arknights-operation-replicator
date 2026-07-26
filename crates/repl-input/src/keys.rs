// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 键盘注入。
//!
//! 用 `SendInput` 同时带上虚拟键码和扫描码（AutoHotkey 的 `Send` 默认也是这么做的），
//! 这样无论游戏读的是 Windows 消息还是 Raw Input 都能收到。
//!
//! 注意：`SendInput` 发给**前台窗口**，所以调用前必须确认游戏在前台。

use std::time::Duration;

use crate::{clock::precise_sleep, InputError};

/// 虚拟键码。数值即 Win32 的 `VIRTUAL_KEY`。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct KeyCode(pub u16);

impl KeyCode {
    pub const ESCAPE: Self = Self(0x1B);
    pub const SPACE: Self = Self(0x20);
    pub const TAB: Self = Self(0x09);
    pub const ENTER: Self = Self(0x0D);
    pub const BACKSPACE: Self = Self(0x08);
    pub const SHIFT: Self = Self(0x10);
    pub const CONTROL: Self = Self(0x11);
    pub const ALT: Self = Self(0x12);

    /// 字母 / 数字键。Windows 的 VK 码里 `A`–`Z` 就是 ASCII 大写，`0`–`9` 同理。
    pub fn from_ascii(c: char) -> Option<Self> {
        match c {
            'a'..='z' => Some(Self(c.to_ascii_uppercase() as u16)),
            'A'..='Z' | '0'..='9' => Some(Self(c as u16)),
            _ => None,
        }
    }

    /// F1–F24。
    pub fn function(n: u8) -> Option<Self> {
        (1..=24).contains(&n).then(|| Self(0x6F + u16::from(n)))
    }

    /// 扩展键需要在 `SendInput` 里带 `KEYEVENTF_EXTENDEDKEY`，否则游戏可能收错键。
    fn is_extended(self) -> bool {
        matches!(
            self.0,
            0x21..=0x28 // PgUp PgDn End Home ← ↑ → ↓
                | 0x2D  // Insert
                | 0x2E  // Delete
                | 0x5B  // LWin
                | 0x5C  // RWin
                | 0x5D  // Apps
                | 0x6F  // Numpad /
                | 0x90  // NumLock
                | 0xA3  // RControl
                | 0xA5 // RMenu
        )
    }
}

impl std::fmt::Display for KeyCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0x1B => f.write_str("Esc"),
            0x20 => f.write_str("Space"),
            0x09 => f.write_str("Tab"),
            0x0D => f.write_str("Enter"),
            v @ 0x30..=0x39 => write!(f, "{}", (v as u8) as char),
            v @ 0x41..=0x5A => write!(f, "{}", (v as u8) as char),
            v @ 0x70..=0x87 => write!(f, "F{}", v - 0x6F),
            v => write!(f, "VK_{v:#04X}"),
        }
    }
}

/// 按下一个键（不抬起）。
pub fn key_down(key: KeyCode) -> Result<(), InputError> {
    send_key(key, false)
}

/// 抬起一个键。
pub fn key_up(key: KeyCode) -> Result<(), InputError> {
    send_key(key, true)
}

/// 按下 → 保持 `hold` → 抬起。
///
/// `hold` 不能太短：Unity 是逐帧轮询输入的，按下和抬起落在同一渲染帧里会被吞掉。
/// AFA 用的是 50ms。
pub fn key_tap(key: KeyCode, hold: Duration) -> Result<(), InputError> {
    key_down(key)?;
    precise_sleep(hold);
    key_up(key)
}

/// AFA 在所有单键点击里用的按下时长。
pub const DEFAULT_TAP_HOLD: Duration = Duration::from_millis(50);

/// 当前进程是否以管理员权限运行。
///
/// **这件事很重要且无法从 `SendInput` 的返回值看出来。** Windows 的 UIPI
/// （用户界面特权隔离）会把低完整性级别进程发往高完整性级别窗口的输入**静默丢弃**：
/// `SendInput` 照样返回成功，输入却根本到不了游戏。表现就是"什么都没发生、
/// 也没有任何报错"。
///
/// 明日方舟 PC 端通常以管理员权限运行，所以本工具也必须提权。
/// AFA 的 FAQ 第一条同样是"确认程序以管理员权限运行"。
pub fn is_elevated() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::Security::{
            GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
        };
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

        // SAFETY: 只查询本进程令牌，句柄用完立即关闭。
        unsafe {
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION::default();
            let mut size = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                Some(std::ptr::from_mut(&mut elevation).cast()),
                u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).unwrap_or(0),
                &mut size,
            )
            .is_ok();
            let _ = CloseHandle(token);
            ok && elevation.TokenIsElevated != 0
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
fn send_key(key: KeyCode, up: bool) -> Result<(), InputError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
    };

    // SAFETY: MapVirtualKeyW 只做查表，没有指针参数。
    let scan = unsafe { MapVirtualKeyW(u32::from(key.0), MAPVK_VK_TO_VSC) } as u16;

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    if key.is_extended() {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key.0),
                // 同时给出扫描码：读 Raw Input / DirectInput 的游戏认扫描码。
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    // SAFETY: 传入一个合法初始化的 INPUT 切片，cbsize 与结构体大小一致。
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent == 1 {
        Ok(())
    } else {
        Err(InputError::SendInputBlocked(key.to_string()))
    }
}

#[cfg(not(windows))]
fn send_key(_key: KeyCode, _up: bool) -> Result<(), InputError> {
    Err(InputError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_keycodes() {
        assert_eq!(KeyCode::from_ascii('e'), Some(KeyCode(0x45)));
        assert_eq!(KeyCode::from_ascii('E'), Some(KeyCode(0x45)));
        assert_eq!(KeyCode::from_ascii('q'), Some(KeyCode(0x51)));
        assert_eq!(KeyCode::from_ascii('0'), Some(KeyCode(0x30)));
        assert_eq!(KeyCode::from_ascii('-'), None);
    }

    #[test]
    fn function_keys() {
        assert_eq!(KeyCode::function(1), Some(KeyCode(0x70)));
        assert_eq!(KeyCode::function(12), Some(KeyCode(0x7B)));
        assert_eq!(KeyCode::function(0), None);
        assert_eq!(KeyCode::function(25), None);
    }

    #[test]
    fn extended_flag_only_for_extended_keys() {
        assert!(!KeyCode::ESCAPE.is_extended());
        assert!(!KeyCode::SPACE.is_extended());
        assert!(!KeyCode::from_ascii('e').unwrap().is_extended());
        assert!(KeyCode(0x25).is_extended()); // ←
        assert!(KeyCode(0x2E).is_extended()); // Delete
    }

    #[test]
    fn display_is_human_readable() {
        assert_eq!(KeyCode::ESCAPE.to_string(), "Esc");
        assert_eq!(KeyCode::SPACE.to_string(), "Space");
        assert_eq!(KeyCode::from_ascii('f').unwrap().to_string(), "F");
        assert_eq!(KeyCode::function(3).unwrap().to_string(), "F3");
    }
}
