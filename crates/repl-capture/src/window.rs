// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 定位《明日方舟》PC 客户端窗口，并做客户区 ↔ 屏幕坐标换算。
//!
//! DPI 处理沿用 AFA 的做法：调用前把线程切到 `PER_MONITOR_AWARE_V2`
//! （`SetThreadDpiAwarenessContext(-3)`），否则在缩放显示器上拿到的是被系统
//! 虚拟化过的坐标，触控注入会打偏。

use repl_core::{Point, Rect};

use crate::CaptureError;

/// 游戏进程名。
pub const GAME_PROCESS: &str = "Arknights.exe";

/// 一个已定位到的游戏窗口。
///
/// 只持有 `HWND`。窗口可能随时被关闭，所以每次使用前都重新查询几何信息，
/// 不缓存尺寸。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameWindow {
    hwnd: isize,
}

impl GameWindow {
    /// 按进程名查找可见的主窗口。
    pub fn find() -> Result<Self, CaptureError> {
        Self::find_by_process(GAME_PROCESS)
    }

    pub fn raw_handle(self) -> isize {
        self.hwnd
    }

    /// 从已知 HWND 构造（调试 / 测试用）。
    pub fn from_raw(hwnd: isize) -> Self {
        Self { hwnd }
    }
}

/// 客户区几何信息。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientGeometry {
    /// 客户区尺寸（物理像素）。
    pub width: u32,
    pub height: u32,
    /// 客户区左上角在屏幕上的位置（物理像素）。
    pub origin: Point,
}

impl ClientGeometry {
    pub fn client_to_screen(self, client: Point) -> Point {
        Point::new(self.origin.x + client.x, self.origin.y + client.y)
    }

    pub fn contains(self, client: Point) -> bool {
        client.x >= 0
            && client.y >= 0
            && client.x < self.width as i32
            && client.y < self.height as i32
    }

    pub fn as_rect(self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, MAX_PATH, POINT, RECT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetForegroundWindow, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, SetForegroundWindow,
    };

    /// RAII：把当前线程切到 per-monitor-aware v2，作用域结束后还原。
    ///
    /// 不这么做的话，在 125%/150% 缩放的显示器上 `GetClientRect` 返回的是被系统
    /// 缩放过的逻辑像素，而触控注入吃的是物理像素，坐标会整体偏移。
    pub struct DpiScope(isize);

    impl DpiScope {
        pub fn enter() -> Self {
            // SAFETY: 只切换本线程的 DPI 感知上下文。
            let previous =
                unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
            Self(previous.0 as isize)
        }
    }

    impl Drop for DpiScope {
        fn drop(&mut self) {
            use windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT;
            if self.0 != 0 {
                // SAFETY: 还原 enter() 里保存的上下文。
                unsafe {
                    let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT(
                        self.0 as *mut core::ffi::c_void,
                    ));
                }
            }
        }
    }

    struct SearchState {
        wanted: String,
        found: isize,
    }

    impl GameWindow {
        pub fn find_by_process(process_name: &str) -> Result<Self, CaptureError> {
            let mut state = SearchState {
                wanted: process_name.to_ascii_lowercase(),
                found: 0,
            };
            // SAFETY: 回调只在 EnumWindows 期间被调用，state 在此期间保持有效。
            unsafe {
                let _ = EnumWindows(
                    Some(enum_proc),
                    LPARAM(&mut state as *mut SearchState as isize),
                );
            }
            if state.found == 0 {
                Err(CaptureError::WindowNotFound(process_name.to_owned()))
            } else {
                Ok(Self { hwnd: state.found })
            }
        }

        /// 客户区几何。窗口已关闭 / 最小化时返回错误。
        pub fn geometry(self) -> Result<ClientGeometry, CaptureError> {
            let _dpi = DpiScope::enter();
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);

            // SAFETY: hwnd 来自 EnumWindows；失效时这些调用会返回错误而非 UB。
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() {
                    return Err(CaptureError::WindowNotUsable("窗口不可见".into()));
                }
                if IsIconic(hwnd).as_bool() {
                    return Err(CaptureError::WindowNotUsable("窗口已最小化".into()));
                }
                let mut rect = RECT::default();
                GetClientRect(hwnd, &mut rect)
                    .map_err(|e| CaptureError::WindowNotUsable(e.to_string()))?;
                let width = (rect.right - rect.left).max(0) as u32;
                let height = (rect.bottom - rect.top).max(0) as u32;
                if width == 0 || height == 0 {
                    return Err(CaptureError::WindowNotUsable("客户区为空".into()));
                }
                let mut origin = POINT { x: 0, y: 0 };
                if !ClientToScreen(hwnd, &mut origin).as_bool() {
                    return Err(CaptureError::WindowNotUsable("ClientToScreen 失败".into()));
                }
                Ok(ClientGeometry {
                    width,
                    height,
                    origin: Point::new(origin.x, origin.y),
                })
            }
        }

        /// 游戏是否在前台。键盘注入必须满足这个条件。
        pub fn is_foreground(self) -> bool {
            // SAFETY: 只读取前台窗口句柄。
            let fg = unsafe { GetForegroundWindow() };
            fg.0 as isize == self.hwnd
        }

        /// 把游戏拉到前台。
        ///
        /// Windows 对跨进程抢焦点有限制，失败是正常的 —— 调用方应当提示用户手动点一下，
        /// 而不是反复重试。
        pub fn focus(self) -> Result<(), CaptureError> {
            let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
            // SAFETY: 只请求前台化。
            let ok = unsafe { SetForegroundWindow(hwnd) }.as_bool();
            if ok {
                Ok(())
            } else {
                Err(CaptureError::WindowNotUsable(
                    "无法把游戏窗口切到前台，请手动点击游戏窗口".into(),
                ))
            }
        }
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let state = &mut *(lparam.0 as *mut SearchState);

        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return true.into();
        }
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return true.into();
        };
        let mut buffer = [0u16; MAX_PATH as usize];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return true.into();
        }
        let path = String::from_utf16_lossy(&buffer[..len as usize]).to_ascii_lowercase();
        let matches = path
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|file| file == state.wanted);
        if matches {
            // 游戏可能有隐藏的辅助窗口；只取有实际客户区的那个。
            let mut rect = RECT::default();
            if GetClientRect(hwnd, &mut rect).is_ok()
                && rect.right - rect.left > 200
                && rect.bottom - rect.top > 200
            {
                state.found = hwnd.0 as isize;
                return false.into(); // 找到了，停止枚举
            }
        }
        true.into()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    impl GameWindow {
        pub fn find_by_process(process_name: &str) -> Result<Self, CaptureError> {
            Err(CaptureError::WindowNotFound(process_name.to_owned()))
        }
        pub fn geometry(self) -> Result<ClientGeometry, CaptureError> {
            Err(CaptureError::Unsupported)
        }
        pub fn is_foreground(self) -> bool {
            false
        }
        pub fn focus(self) -> Result<(), CaptureError> {
            Err(CaptureError::Unsupported)
        }
    }
}

#[cfg(windows)]
pub use imp::DpiScope;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_to_screen_offsets_by_origin() {
        let g = ClientGeometry {
            width: 1600,
            height: 900,
            origin: Point::new(100, 50),
        };
        assert_eq!(g.client_to_screen(Point::new(0, 0)), Point::new(100, 50));
        assert_eq!(g.client_to_screen(Point::new(10, 20)), Point::new(110, 70));
    }

    #[test]
    fn contains_rejects_out_of_bounds() {
        let g = ClientGeometry {
            width: 100,
            height: 50,
            origin: Point::ZERO,
        };
        assert!(g.contains(Point::new(0, 0)));
        assert!(g.contains(Point::new(99, 49)));
        assert!(!g.contains(Point::new(100, 49)));
        assert!(!g.contains(Point::new(99, 50)));
        assert!(!g.contains(Point::new(-1, 0)));
    }

    #[test]
    fn finding_a_nonexistent_process_is_an_error_not_a_panic() {
        let result = GameWindow::find_by_process("definitely-not-running-12345.exe");
        assert!(matches!(result, Err(CaptureError::WindowNotFound(_))));
    }
}
