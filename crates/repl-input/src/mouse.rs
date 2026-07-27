// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 鼠标停靠与通过 `SendInput` 注入的鼠标按钮/滚轮事件。
//!
//! 本项目不用鼠标点击（点击全走触控注入），但会做纯指针位移。这带来一个
//! 反面问题：触控注入不移动鼠标指针，所以指针会一直停在用户最后放的位置。
//! Arknights PC 自绘光标，一旦指针停在费用条或费用数字附近，尺子就会把每一帧
//! 标记 `cursorBlocked` 并冻结分析，我们就永远拿不到新的帧数了。
//!
//! 所以平时把指针**挪到安全区停好**；暂停中选中前会把它移到目标干员。
//! 两者都只是纯位移，不按键、不点击。
//!
//! AFA 的 `XButton1`/`XButton2` 与滚轮热键也通过这里注入。发送到游戏前，
//! 调用方必须像键盘注入一样确认游戏窗口在前台；`SendInput` 本身不会选择目标窗口。

use std::time::Duration;

use repl_core::Point;

use crate::{clock::precise_sleep, InputError};

/// 鼠标侧键。
///
/// Windows 的 `MOUSEINPUT.mouseData` 要求使用 `XBUTTON1`/`XBUTTON2` 的数值，
/// 而不是键盘虚拟键码 `VK_XBUTTON1`/`VK_XBUTTON2`；这个枚举把两者的区别
/// 封装在输入层内，调用方不需要依赖 Win32 常量。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MouseButton {
    XButton1,
    XButton2,
}

/// `MouseButton` 的语义别名，方便按 AFA 配置中的名称调用。
pub type XButton = MouseButton;

impl MouseButton {
    #[cfg(windows)]
    fn mouse_data(self) -> u32 {
        use windows::Win32::UI::WindowsAndMessaging::{XBUTTON1, XBUTTON2};

        match self {
            Self::XButton1 => u32::from(XBUTTON1),
            Self::XButton2 => u32::from(XBUTTON2),
        }
    }
}

impl std::fmt::Display for MouseButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::XButton1 => "XButton1",
            Self::XButton2 => "XButton2",
        })
    }
}

/// 一次垂直或水平滚轮刻度的方向。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Wheel {
    Up,
    Down,
    Left,
    Right,
}

/// `Wheel` 的语义别名，方便按配置解析出的 AFA 名称调用。
pub type WheelDirection = Wheel;

impl Wheel {
    /// Windows `MOUSEINPUT.mouseData` 对应的 `WHEEL_DELTA` 值。
    fn delta(self) -> i32 {
        const WHEEL_DELTA: i32 = 120;

        match self {
            Self::Up | Self::Right => WHEEL_DELTA,
            Self::Down | Self::Left => -WHEEL_DELTA,
        }
    }

    /// 水平滚轮使用 `MOUSEEVENTF_HWHEEL`，其余方向使用 `MOUSEEVENTF_WHEEL`。
    #[cfg(windows)]
    fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

impl std::fmt::Display for Wheel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Up => "WheelUp",
            Self::Down => "WheelDown",
            Self::Left => "WheelLeft",
            Self::Right => "WheelRight",
        })
    }
}

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

/// 按下一个 XButton（不抬起）。
pub fn xbutton_down(button: MouseButton) -> Result<(), InputError> {
    send_xbutton(button, false)
}

/// 抬起一个 XButton。
pub fn xbutton_up(button: MouseButton) -> Result<(), InputError> {
    send_xbutton(button, true)
}

/// 按下 → 保持 `hold` → 抬起一个 XButton。
///
/// 即使按下报告失败，也尝试发送一次抬起，避免底层只接收到了部分输入时
/// 让侧键永久保持按下。`hold` 的默认值由 AFA 适配器传入；底层不写死时序。
pub fn xbutton_tap(button: MouseButton, hold: Duration) -> Result<(), InputError> {
    match xbutton_down(button) {
        Ok(()) => {
            precise_sleep(hold);
            xbutton_up(button)
        }
        Err(error) => {
            let _ = xbutton_up(button);
            Err(error)
        }
    }
}

/// [`xbutton_tap`] 的动词优先别名，便于与键盘模块的 `key_tap` 对照调用。
pub fn tap_xbutton(button: MouseButton, hold: Duration) -> Result<(), InputError> {
    xbutton_tap(button, hold)
}

/// 发送一个滚轮刻度（`WHEEL_DELTA`）。
pub fn wheel_tick(direction: Wheel) -> Result<(), InputError> {
    send_wheel(direction)
}

/// 发送多个滚轮刻度。
///
/// `ticks == 0` 是无操作并返回 `Ok(())`；每个刻度都是独立的 `SendInput` 事件，
/// 这样与真实滚轮热键触发的离散事件一致。
pub fn scroll(direction: Wheel, ticks: u32) -> Result<(), InputError> {
    for _ in 0..ticks {
        wheel_tick(direction)?;
    }
    Ok(())
}

#[cfg(windows)]
fn send_xbutton(button: MouseButton, up: bool) -> Result<(), InputError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
        MOUSE_EVENT_FLAGS,
    };

    let flags: MOUSE_EVENT_FLAGS = if up {
        MOUSEEVENTF_XUP
    } else {
        MOUSEEVENTF_XDOWN
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: button.mouse_data(),
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
        Err(InputError::SendInputBlocked(format!(
            "{} {}",
            button,
            if up { "up" } else { "down" }
        )))
    }
}

#[cfg(not(windows))]
fn send_xbutton(_button: MouseButton, _up: bool) -> Result<(), InputError> {
    Err(InputError::Unsupported)
}

#[cfg(windows)]
fn send_wheel(direction: Wheel) -> Result<(), InputError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_WHEEL, MOUSEINPUT,
        MOUSE_EVENT_FLAGS,
    };

    let flags: MOUSE_EVENT_FLAGS = if direction.is_horizontal() {
        MOUSEEVENTF_HWHEEL
    } else {
        MOUSEEVENTF_WHEEL
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: direction.delta() as u32,
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
        Err(InputError::SendInputBlocked(direction.to_string()))
    }
}

#[cfg(not(windows))]
fn send_wheel(_direction: Wheel) -> Result<(), InputError> {
    Err(InputError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbutton_values_and_display_are_stable() {
        assert_eq!(MouseButton::XButton1.to_string(), "XButton1");
        assert_eq!(MouseButton::XButton2.to_string(), "XButton2");
        assert_eq!(XButton::XButton1, MouseButton::XButton1);
    }

    #[test]
    fn wheel_directions_use_standard_delta() {
        assert_eq!(Wheel::Up.delta(), 120);
        assert_eq!(Wheel::Right.delta(), 120);
        assert_eq!(Wheel::Down.delta(), -120);
        assert_eq!(Wheel::Left.delta(), -120);
        assert_eq!(WheelDirection::Up, Wheel::Up);
    }

    #[test]
    fn wheel_display_matches_afa_names() {
        assert_eq!(Wheel::Up.to_string(), "WheelUp");
        assert_eq!(Wheel::Down.to_string(), "WheelDown");
        assert_eq!(Wheel::Left.to_string(), "WheelLeft");
        assert_eq!(Wheel::Right.to_string(), "WheelRight");
    }

    #[cfg(windows)]
    #[test]
    fn horizontal_detection_matches_win32_flags() {
        assert!(!Wheel::Up.is_horizontal());
        assert!(!Wheel::Down.is_horizontal());
        assert!(Wheel::Left.is_horizontal());
        assert!(Wheel::Right.is_horizontal());
    }

    #[cfg(windows)]
    #[test]
    fn xbutton_data_matches_win32_constants() {
        assert_eq!(MouseButton::XButton1.mouse_data(), 1);
        assert_eq!(MouseButton::XButton2.mouse_data(), 2);
    }
}
