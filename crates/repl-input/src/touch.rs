// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 移植自 arknights-frame-assistant (GPL-3.0-only):
//   src/lib/touch_injection.ahk —— TouchInjector 类
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! Windows 触控注入。
//!
//! 用 `InitializeTouchInjection` + `InjectTouchInput` 模拟真实触摸屏，而不是
//! 移动鼠标点击。这么做有两个决定性的好处：
//!
//! 1. 《明日方舟》PC 端对触摸的响应路径和鼠标不同，触控更接近手机端行为，
//!    MAA 那套拖拽部署的手势参数才对得上。
//! 2. **触控注入不会移动系统鼠标指针。** 而 Arknights PC 会自绘光标，一旦光标
//!    盖住费用条或费用数字，尺子就会把该帧标记 `cursorBlocked` 并冻结分析
//!    （见尺子的 `pc_cursor_guard.rs`）—— 那样我们就拿不到帧数了。
//!    所以全程只用触控、绝不移动鼠标，是这个项目能工作的前提之一。
//!
//! 坐标一律是**屏幕坐标**。客户区 → 屏幕的换算由调用方（`repl-capture` 的
//! `GameWindow`）负责。

use std::time::Duration;

use repl_core::Point;

use crate::{clock::precise_sleep, InputError};

/// 接触区半径（屏幕像素）。AFA 用 ±2，够让游戏认成一个手指点。
const CONTACT_RADIUS: i32 = 2;
/// AFA 用的压力值。MSDN 标称范围是 0–1024，但 AFA 长期用 32000 且工作正常，
/// 这里保持一致 —— 游戏只关心"有没有压力"。
const PRESSURE: u32 = 32000;
const ORIENTATION: u32 = 90;

/// 触控注入器。
///
/// 一个进程只需要 [`TouchInjector::initialize`] 一次；之后可以创建多个实例，
/// 但**同一时刻只能有一个处于按下状态**（我们只用单指）。
#[derive(Debug, Default)]
pub struct TouchInjector {
    down: bool,
    last: Point,
}

impl TouchInjector {
    /// 初始化触控注入子系统。重复调用是安全的（内部只做一次）。
    ///
    /// `max_contacts` 是同时可注入的触点数，我们只用 1，但和 AFA 一样申请 3。
    pub fn initialize() -> Result<(), InputError> {
        #[cfg(windows)]
        {
            use std::sync::OnceLock;
            use windows::Win32::UI::Input::Pointer::{
                InitializeTouchInjection, TOUCH_FEEDBACK_DEFAULT,
            };

            static INIT: OnceLock<Result<(), String>> = OnceLock::new();
            let result = INIT.get_or_init(|| {
                // SAFETY: 只传标量参数。
                unsafe { InitializeTouchInjection(3, TOUCH_FEEDBACK_DEFAULT) }
                    .map_err(|e| e.to_string())
            });
            result.clone().map_err(InputError::TouchInit)
        }
        #[cfg(not(windows))]
        {
            Err(InputError::Unsupported)
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_down(&self) -> bool {
        self.down
    }

    /// 按下。已经按下时返回 [`InputError::TouchAlreadyDown`]。
    pub fn down(&mut self, screen: Point) -> Result<(), InputError> {
        if self.down {
            return Err(InputError::TouchAlreadyDown);
        }
        self.last = screen;
        self.inject(screen, Flags::Down)?;
        self.down = true;
        Ok(())
    }

    /// 保持按下并移动到新位置。未按下时返回 [`InputError::TouchNotDown`]。
    pub fn update(&mut self, screen: Point) -> Result<(), InputError> {
        if !self.down {
            return Err(InputError::TouchNotDown);
        }
        self.last = screen;
        self.inject(screen, Flags::Update)
    }

    /// 抬起。
    ///
    /// 先发一条 UPDATE 再发 UP —— 和 AFA 一致。少了那条 UPDATE，
    /// 游戏偶尔会把抬起点当成按下点，滑动方向就废了。
    pub fn up(&mut self, screen: Point) -> Result<(), InputError> {
        if !self.down {
            return Err(InputError::TouchNotDown);
        }
        self.last = screen;
        self.inject(screen, Flags::Update)?;
        self.inject(screen, Flags::Up)?;
        self.down = false;
        Ok(())
    }

    /// 点一下。
    pub fn tap(&mut self, screen: Point) -> Result<(), InputError> {
        self.down(screen)?;
        self.up(screen)
    }

    /// 沿给定路径滑动：按下起点 → 依次经过中间点 → 在终点抬起。
    ///
    /// `step` 是相邻两点之间的间隔，MAA 的 minitouch 用的是 10ms。
    /// 路径的生成（缓动、时长）在 `repl-core::gesture` 里，这里只负责发出去。
    pub fn swipe_path(&mut self, path: &[Point], step: Duration) -> Result<(), InputError> {
        let Some((&first, rest)) = path.split_first() else {
            return Err(InputError::EmptySwipePath);
        };
        self.down(first)?;
        let mut last = first;
        for &point in rest {
            precise_sleep(step);
            self.update(point)?;
            last = point;
        }
        self.up(last)
    }

    /// 出错时尽力把触点抬起来，免得游戏一直以为手指还按着。
    pub fn release_if_down(&mut self) {
        if self.down {
            let last = self.last;
            if let Err(e) = self.up(last) {
                log::warn!("failed to release stuck touch contact at {last}: {e}");
                self.down = false;
            }
        }
    }

    #[cfg(windows)]
    fn inject(&self, screen: Point, flags: Flags) -> Result<(), InputError> {
        use windows::Win32::Foundation::{POINT, RECT};
        use windows::Win32::UI::Input::Pointer::{
            InjectTouchInput, POINTER_FLAGS, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT,
            POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_TOUCH_INFO,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            PT_TOUCH, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_ORIENTATION, TOUCH_MASK_PRESSURE,
        };

        let pointer_flags: POINTER_FLAGS = match flags {
            Flags::Down => POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_DOWN,
            Flags::Update => POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_UPDATE,
            Flags::Up => POINTER_FLAG_UP,
        };

        let mut info = POINTER_TOUCH_INFO::default();
        info.pointerInfo.pointerType = PT_TOUCH;
        info.pointerInfo.pointerId = 0;
        info.pointerInfo.pointerFlags = pointer_flags;
        info.pointerInfo.ptPixelLocation = POINT {
            x: screen.x,
            y: screen.y,
        };
        info.touchMask = TOUCH_MASK_CONTACTAREA | TOUCH_MASK_ORIENTATION | TOUCH_MASK_PRESSURE;
        info.rcContact = RECT {
            left: screen.x - CONTACT_RADIUS,
            top: screen.y - CONTACT_RADIUS,
            right: screen.x + CONTACT_RADIUS,
            bottom: screen.y + CONTACT_RADIUS,
        };
        info.orientation = ORIENTATION;
        info.pressure = PRESSURE;

        // SAFETY: 传入一个完整初始化的 POINTER_TOUCH_INFO 切片。
        unsafe { InjectTouchInput(&[info]) }.map_err(|e| InputError::TouchInject {
            at: screen,
            reason: e.to_string(),
        })
    }

    #[cfg(not(windows))]
    fn inject(&self, _screen: Point, _flags: Flags) -> Result<(), InputError> {
        Err(InputError::Unsupported)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flags {
    Down,
    Update,
    Up,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_rejects_illegal_transitions() {
        let mut t = TouchInjector::new();
        assert!(!t.is_down());
        // 没按下就更新 / 抬起 —— 必须报错而不是发出畸形事件
        assert!(matches!(
            t.update(Point::new(1, 1)),
            Err(InputError::TouchNotDown)
        ));
        assert!(matches!(
            t.up(Point::new(1, 1)),
            Err(InputError::TouchNotDown)
        ));
    }

    #[test]
    fn empty_swipe_path_is_rejected() {
        let mut t = TouchInjector::new();
        assert!(matches!(
            t.swipe_path(&[], Duration::from_millis(10)),
            Err(InputError::EmptySwipePath)
        ));
    }

    #[test]
    fn release_if_down_is_a_noop_when_up() {
        let mut t = TouchInjector::new();
        t.release_if_down(); // 不应 panic
        assert!(!t.is_down());
    }
}
