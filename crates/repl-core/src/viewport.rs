// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// UI 边缘缩放模型参考自 ArknightsCostBarRuler (MIT) 的
//   crates/ruler-core/src/analysis/roi.rs::ui_edge_scale
// 与 crates/ruler-core/src/analysis/scanner/battle_state.rs 的 HUD 定位方式。
// 这里是按其公开行为独立实现的坐标换算，未复制上游代码。

//! 参考坐标系 ↔ 真实客户区像素。
//!
//! 三层坐标必须分清楚：
//!
//! 1. **格子坐标** `[x, y]` —— 作业里写的
//! 2. **1280×720 参考坐标** —— MAA 全部算法的工作坐标系（`tile` 模块的输出）
//! 3. **客户区像素** —— 实际窗口里的位置；再加上窗口原点就是屏幕坐标
//!
//! 而第 2 → 3 步又分两种，**不能混用**：
//!
//! - **战场投影**（格子、干员）：只受 16:9 内容区影响。相机投影本身就是按
//!   16:9 算的，游戏在非 16:9 窗口里加黑边，战场内容不会跟着 UI 缩放跑。
//! - **HUD 元素**（部署栏、暂停按钮）：还要额外乘游戏内的"UI 缩放"系数，
//!   而且位置是相对**最近的屏幕边缘**度量的 —— 这个设置的本意就是把 HUD
//!   往屏幕中间收，好让刘海屏不挡住按钮。

use crate::geom::{Point, Rect, REF_ASPECT, REF_HEIGHT, REF_WIDTH};

/// 游戏内 UI 缩放的默认值（注册表里读不到时用它）。
pub const DEFAULT_UI_SCALER: f64 = 1.0;

/// 把游戏内的 `uiScaler`（0.0–1.0）换算成边缘距离系数。
///
/// 与尺子一致：`0.9 + 0.1 * uiScaler`。默认值 1.0 时系数正好是 1.0，即不做调整。
pub fn ui_edge_scale(ui_scaler: f64) -> f64 {
    let s = if ui_scaler.is_finite() {
        ui_scaler.clamp(0.0, 1.0)
    } else {
        DEFAULT_UI_SCALER
    };
    0.9 + 0.1 * s
}

/// HUD 元素的水平锚定边。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HAnchor {
    Left,
    Right,
}

/// HUD 元素的垂直锚定边。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VAnchor {
    Top,
    Bottom,
}

/// 一个窗口的坐标映射。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    client_width: u32,
    client_height: u32,
    /// 客户区里那块 16:9 的实际渲染区域（黑边之内）。
    content: Rect,
    /// 参考坐标 → 内容区的线性系数。
    scale: f64,
    edge_scale: f64,
}

impl Viewport {
    pub fn new(client_width: u32, client_height: u32, ui_scaler: f64) -> Self {
        let cw = client_width.max(1) as f64;
        let ch = client_height.max(1) as f64;
        let aspect = cw / ch;

        // 窗口比 16:9 更宽 → 以高度为准，左右留黑边；更窄 → 以宽度为准，上下留黑边。
        let (content_w, content_h) = if aspect >= REF_ASPECT {
            (ch * REF_ASPECT, ch)
        } else {
            (cw, cw / REF_ASPECT)
        };
        let content = Rect::new(
            ((cw - content_w) / 2.0).round() as i32,
            ((ch - content_h) / 2.0).round() as i32,
            content_w.round() as i32,
            content_h.round() as i32,
        );

        Self {
            client_width,
            client_height,
            content,
            scale: content_w / f64::from(REF_WIDTH),
            edge_scale: ui_edge_scale(ui_scaler),
        }
    }

    pub fn client_width(&self) -> u32 {
        self.client_width
    }

    pub fn client_height(&self) -> u32 {
        self.client_height
    }

    /// 客户区里的 16:9 渲染区域。非 16:9 窗口时它比客户区小。
    pub fn content_rect(&self) -> Rect {
        self.content
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn edge_scale(&self) -> f64 {
        self.edge_scale
    }

    /// **战场坐标**：1280×720 参考像素 → 客户区像素。
    ///
    /// 用于把 `tile` 模块算出的格子屏幕位置落到真实窗口上。不带 UI 缩放。
    pub fn field_to_client(&self, reference: Point) -> Point {
        Point::new(
            self.content.x + (f64::from(reference.x) * self.scale).round() as i32,
            self.content.y + (f64::from(reference.y) * self.scale).round() as i32,
        )
    }

    /// **HUD 坐标**：1280×720 参考矩形 → 客户区矩形，带 UI 缩放且相对锚定边度量。
    ///
    /// `h` / `v` 指明这个元素贴着哪条边。贴哪条边取决于游戏 UI 的布局，不能瞎猜：
    /// 部署栏贴左下，暂停按钮贴右上。
    pub fn hud_rect(&self, reference: Rect, h: HAnchor, v: VAnchor) -> Rect {
        let s = self.scale * self.edge_scale;
        let w = (f64::from(reference.width) * s).round() as i32;
        let ht = (f64::from(reference.height) * s).round() as i32;

        let x = match h {
            HAnchor::Left => self.content.x + (f64::from(reference.x) * s).round() as i32,
            HAnchor::Right => {
                let from_right = f64::from(REF_WIDTH - reference.right());
                self.content.right() - (from_right * s).round() as i32 - w
            }
        };
        let y = match v {
            VAnchor::Top => self.content.y + (f64::from(reference.y) * s).round() as i32,
            VAnchor::Bottom => {
                let from_bottom = f64::from(REF_HEIGHT - reference.bottom());
                self.content.bottom() - (from_bottom * s).round() as i32 - ht
            }
        };
        Rect::new(x, y, w, ht)
    }

    /// [`Self::hud_rect`] 的点版本，取矩形中心。
    pub fn hud_point(&self, reference: Rect, h: HAnchor, v: VAnchor) -> Point {
        self.hud_rect(reference, h, v).center()
    }

    /// **HUD 矩形，但结果仍留在 1280×720 参考坐标系里** —— 只应用 UI 边缘缩放，
    /// 不做分辨率缩放。
    ///
    /// 识别流程用这个：截图先被重采样到 1280×720（见 `Frame::resample`），
    /// 之后所有搜索、模板匹配、rectMove 都在参考坐标系里做，
    /// MAA 的常量和模板尺寸才对得上。识别完再用 [`Self::field_to_client`]
    /// 把结果映回真实客户区像素。
    pub fn hud_rect_ref(&self, reference: Rect, h: HAnchor, v: VAnchor) -> Rect {
        let e = self.edge_scale;
        let w = (f64::from(reference.width) * e).round() as i32;
        let ht = (f64::from(reference.height) * e).round() as i32;
        let x = match h {
            HAnchor::Left => (f64::from(reference.x) * e).round() as i32,
            HAnchor::Right => {
                REF_WIDTH - (f64::from(REF_WIDTH - reference.right()) * e).round() as i32 - w
            }
        };
        let y = match v {
            VAnchor::Top => (f64::from(reference.y) * e).round() as i32,
            VAnchor::Bottom => {
                REF_HEIGHT - (f64::from(REF_HEIGHT - reference.bottom()) * e).round() as i32 - ht
            }
        };
        Rect::new(x, y, w, ht)
    }

    /// 供识别使用的重采样目标区域：客户区里的 16:9 内容矩形。
    pub fn reference_source(&self) -> Rect {
        self.content
    }

    /// 客户区像素 → 参考坐标（战场口径）。识别结果回读时用。
    pub fn client_to_field(&self, client: Point) -> Point {
        Point::new(
            ((f64::from(client.x - self.content.x)) / self.scale).round() as i32,
            ((f64::from(client.y - self.content.y)) / self.scale).round() as i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_16_9_has_no_letterbox() {
        let vp = Viewport::new(1920, 1080, 1.0);
        assert_eq!(vp.content_rect(), Rect::new(0, 0, 1920, 1080));
        assert!((vp.scale() - 1.5).abs() < 1e-9);
        // 参考坐标原点和右下角都要落在客户区角上
        assert_eq!(vp.field_to_client(Point::new(0, 0)), Point::new(0, 0));
        assert_eq!(
            vp.field_to_client(Point::new(1280, 720)),
            Point::new(1920, 1080)
        );
    }

    #[test]
    fn reference_resolution_is_identity() {
        let vp = Viewport::new(1280, 720, 1.0);
        assert!((vp.scale() - 1.0).abs() < 1e-9);
        for p in [
            Point::new(0, 0),
            Point::new(640, 360),
            Point::new(1279, 719),
        ] {
            assert_eq!(vp.field_to_client(p), p);
        }
    }

    #[test]
    fn wider_than_16_9_letterboxes_left_and_right() {
        // 21:9 超宽屏
        let vp = Viewport::new(2560, 1080, 1.0);
        let content = vp.content_rect();
        assert_eq!(content.height, 1080);
        assert_eq!(content.width, 1920);
        assert_eq!(content.x, 320);
        assert_eq!(content.y, 0);
    }

    #[test]
    fn narrower_than_16_9_letterboxes_top_and_bottom() {
        // 4:3
        let vp = Viewport::new(1280, 960, 1.0);
        let content = vp.content_rect();
        assert_eq!(content.width, 1280);
        assert_eq!(content.height, 720);
        assert_eq!(content.x, 0);
        assert_eq!(content.y, 120);
    }

    #[test]
    fn field_round_trip() {
        let vp = Viewport::new(2560, 1440, 1.0);
        for p in [
            Point::new(0, 0),
            Point::new(100, 200),
            Point::new(1279, 719),
        ] {
            assert_eq!(vp.client_to_field(vp.field_to_client(p)), p);
        }
    }

    #[test]
    fn ui_edge_scale_matches_ruler_formula() {
        assert!((ui_edge_scale(1.0) - 1.0).abs() < 1e-12);
        assert!((ui_edge_scale(0.0) - 0.9).abs() < 1e-12);
        assert!((ui_edge_scale(0.5) - 0.95).abs() < 1e-12);
        // 越界和 NaN 都要有确定行为
        assert!((ui_edge_scale(2.0) - 1.0).abs() < 1e-12);
        assert!((ui_edge_scale(f64::NAN) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn hud_default_ui_scale_is_plain_linear_mapping() {
        // uiScaler = 1.0 => edge_scale = 1.0 => HUD 映射退化成普通线性缩放
        let vp = Viewport::new(1280, 720, 1.0);
        let pause = Rect::new(1170, 20, 60, 60); // MAA 的 BattlePause specificRect
        assert_eq!(
            vp.hud_rect(pause, HAnchor::Right, VAnchor::Top),
            Rect::new(1170, 20, 60, 60)
        );
        let bar = Rect::new(35, 588, 1245, 18); // MAA 的 BattleOpersFlag roi
        assert_eq!(
            vp.hud_rect(bar, HAnchor::Left, VAnchor::Bottom),
            Rect::new(35, 588, 1245, 18)
        );
    }

    #[test]
    fn hud_shrinking_ui_scale_pulls_elements_off_the_edges() {
        // uiScaler = 0 => edge_scale = 0.9：元素相对边缘的距离缩到 90%，
        // 也就是往屏幕中间靠。
        let vp = Viewport::new(1280, 720, 0.0);
        let pause = Rect::new(1170, 20, 60, 60);
        let mapped = vp.hud_rect(pause, HAnchor::Right, VAnchor::Top);
        // 右上角的按钮：离上边更近、离右边更近
        assert!(mapped.y < 20, "顶部距离应当缩小，实际 {}", mapped.y);
        assert!(
            mapped.right() > 1230,
            "右侧距离应当缩小，实际右边界 {}",
            mapped.right()
        );
        assert!(mapped.width < 60, "尺寸也跟着缩");
    }

    #[test]
    fn hud_respects_letterbox_offset() {
        // 4:3 窗口：HUD 也应当落在 16:9 内容区里，而不是整个客户区
        let vp = Viewport::new(1280, 960, 1.0);
        let bar = Rect::new(35, 588, 1245, 18);
        let mapped = vp.hud_rect(bar, HAnchor::Left, VAnchor::Bottom);
        assert_eq!(mapped.x, 35);
        // 内容区底边在 y = 120 + 720 = 840
        assert_eq!(mapped.bottom(), 840 - (720 - 606));
    }

    #[test]
    fn hud_rect_ref_stays_in_reference_space() {
        // 分辨率再高，参考空间里的结果都一样 —— 这正是重采样识别流程需要的性质。
        let a = Viewport::new(1280, 720, 1.0);
        let b = Viewport::new(2560, 1440, 1.0);
        let bar = Rect::new(35, 588, 1245, 18);
        assert_eq!(
            a.hud_rect_ref(bar, HAnchor::Left, VAnchor::Bottom),
            b.hud_rect_ref(bar, HAnchor::Left, VAnchor::Bottom)
        );
        // uiScaler = 1 时是恒等映射
        assert_eq!(a.hud_rect_ref(bar, HAnchor::Left, VAnchor::Bottom), bar);
    }

    #[test]
    fn hud_rect_ref_applies_ui_scaler() {
        // 用户实测环境：uiScaler = 0 => edge_scale = 0.9，HUD 往屏幕中间收
        let vp = Viewport::new(2560, 1440, 0.0);
        let bar = Rect::new(35, 588, 1245, 18);
        let r = vp.hud_rect_ref(bar, HAnchor::Left, VAnchor::Bottom);
        assert!(r.x < bar.x, "左边距应当缩小");
        assert!(r.bottom() > bar.bottom(), "底边距应当缩小（更贴近底部）");
        assert!(r.width < bar.width, "尺寸也跟着缩");
        // 仍然在参考画布内
        assert!(r.x >= 0 && r.bottom() <= REF_HEIGHT);
    }

    #[test]
    fn reference_source_is_the_content_rect() {
        let vp = Viewport::new(2560, 1080, 1.0); // 超宽，有左右黑边
        assert_eq!(vp.reference_source(), vp.content_rect());
        assert!(vp.reference_source().x > 0, "超宽屏应当有左黑边");
    }

    #[test]
    fn degenerate_client_size_does_not_panic() {
        let vp = Viewport::new(0, 0, 1.0);
        assert!(vp.scale() > 0.0);
        let _ = vp.field_to_client(Point::new(10, 10));
    }
}
