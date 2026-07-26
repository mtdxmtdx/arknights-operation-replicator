// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Common/AsstTypes.h  —— Point / Rect / rectMove 语义
//   src/MaaCore/Common/AsstBattleDef.h —— DeployDirection
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 共享几何词汇表。
//!
//! 命名沿用 MAA 的约定（见 `MaaAssistantArknights/src/MaaCore/Common/AsstBattleDef.h` 顶部注释）：
//! - `loc` / `location`：**格子**坐标，例如 `[1, 1]`、`[5, 5]`
//! - `pos` / `position`：**像素**坐标，例如 `[1280, 720]`、`[500, 300]`
//!
//! 本 crate 里所有像素坐标默认位于 **1280×720 参考坐标系**（MAA 全部算法的工作坐标系，
//! 对应 `WindowWidthDefault` / `WindowHeightDefault`）。换算到真实客户区/屏幕像素是
//! `viewport` 模块的职责。

/// MAA 参考坐标系宽度（`asst::WindowWidthDefault`）。
pub const REF_WIDTH: i32 = 1280;
/// MAA 参考坐标系高度（`asst::WindowHeightDefault`）。
pub const REF_HEIGHT: i32 = 720;
/// 游戏渲染宽高比，客户区不是这个比例时会有黑边。
pub const REF_ASPECT: f64 = REF_WIDTH as f64 / REF_HEIGHT as f64;

/// 二维整数点。既用于格子坐标也用于像素坐标，靠变量名区分。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const ZERO: Self = Self::new(0, 0);
    pub const RIGHT: Self = Self::new(1, 0);
    pub const DOWN: Self = Self::new(0, 1);
    pub const LEFT: Self = Self::new(-1, 0);
    pub const UP: Self = Self::new(0, -1);

    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn distance_to(self, other: Self) -> f64 {
        let dx = f64::from(other.x - self.x);
        let dy = f64::from(other.y - self.y);
        dx.hypot(dy)
    }
}

impl std::ops::Add for Point {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<i32> for Point {
    type Output = Self;

    fn mul(self, rhs: i32) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// 左上角 + 宽高的矩形，语义与 `asst::Rect` 一致。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub const fn right(self) -> i32 {
        self.x + self.width
    }

    pub const fn bottom(self) -> i32 {
        self.y + self.height
    }

    pub const fn center(self) -> Point {
        Point::new(self.x + self.width / 2, self.y + self.height / 2)
    }

    pub fn contains(self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    /// MAA 的 `Rect::move`：把一个相对偏移矩形叠加到本矩形上。
    /// 对应 `tasks.json` 里的 `rectMove` 语义 —— 前两个分量是相对左上角的位移，
    /// 后两个分量**直接替换**宽高。
    pub const fn moved(self, delta: Rect) -> Self {
        Self {
            x: self.x + delta.x,
            y: self.y + delta.y,
            width: delta.width,
            height: delta.height,
        }
    }

    /// 裁剪到 `[0, width) × [0, height)`，越界的部分丢掉。
    /// 对应 MAA 的 `correct_rect`。
    pub fn clamped(self, width: i32, height: i32) -> Self {
        let left = self.x.clamp(0, width);
        let top = self.y.clamp(0, height);
        let right = self.right().clamp(0, width);
        let bottom = self.bottom().clamp(0, height);
        Self {
            x: left,
            y: top,
            width: (right - left).max(0),
            height: (bottom - top).max(0),
        }
    }

    pub fn intersects(self, other: Self) -> bool {
        self.x < other.right()
            && self.right() > other.x
            && self.y < other.bottom()
            && self.bottom() > other.y
    }
}

impl std::fmt::Display for Rect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}, {}, {}, {}]",
            self.x, self.y, self.width, self.height
        )
    }
}

/// 部署朝向。数值与 MAA `battle::DeployDirection` 一致。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Direction {
    #[default]
    Right = 0,
    Down = 1,
    Left = 2,
    Up = 3,
    /// 无朝向，通常是无人机之类。
    None = 4,
}

impl Direction {
    /// 朝向对应的单位向量（屏幕坐标系，y 向下）。
    pub const fn unit(self) -> Point {
        match self {
            Self::Right => Point::RIGHT,
            Self::Down => Point::DOWN,
            Self::Left => Point::LEFT,
            Self::Up => Point::UP,
            Self::None => Point::ZERO,
        }
    }
}

/// 干员职业。顺序与 MAA `battle::Role` 一致。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Role {
    #[default]
    Unknown,
    /// 术师
    Caster,
    /// 医疗
    Medic,
    /// 先锋
    Pioneer,
    /// 狙击
    Sniper,
    /// 特种
    Special,
    /// 辅助
    Support,
    /// 重装
    Tank,
    /// 近卫
    Warrior,
    /// 无人机 / 召唤物
    Drone,
}

impl Role {
    pub const ALL: [Self; 9] = [
        Self::Caster,
        Self::Medic,
        Self::Pioneer,
        Self::Sniper,
        Self::Special,
        Self::Support,
        Self::Tank,
        Self::Warrior,
        Self::Drone,
    ];

    /// 该职业**通常**的部署位类型。用于在编队绑定界面里做预匹配提示；
    /// 地面辅助、高台先锋这类特例会不符，所以只是提示，不作判定依据。
    pub fn usual_placement(self) -> Placement {
        match self {
            Self::Warrior | Self::Pioneer | Self::Tank | Self::Special | Self::Drone => {
                Placement::Melee
            }
            Self::Medic | Self::Sniper | Self::Caster | Self::Support => Placement::Ranged,
            Self::Unknown => Placement::Unknown,
        }
    }

    pub fn zh(self) -> &'static str {
        match self {
            Self::Caster => "术师",
            Self::Medic => "医疗",
            Self::Pioneer => "先锋",
            Self::Sniper => "狙击",
            Self::Special => "特种",
            Self::Support => "辅助",
            Self::Tank => "重装",
            Self::Warrior => "近卫",
            Self::Drone => "无人机",
            Self::Unknown => "未知",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.zh())
    }
}

/// 部署位类型。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Placement {
    #[default]
    Unknown,
    /// 地面
    Melee,
    /// 高台
    Ranged,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_cover_all_nine_classes() {
        assert_eq!(Role::ALL.len(), 9);
        assert!(!Role::ALL.contains(&Role::Unknown));
        // 九个职业的中文名不能重复，UI 里要靠它区分
        let mut names: Vec<_> = Role::ALL.iter().map(|r| r.zh()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 9);
    }

    #[test]
    fn usual_placement_splits_ground_and_highground() {
        assert_eq!(Role::Warrior.usual_placement(), Placement::Melee);
        assert_eq!(Role::Sniper.usual_placement(), Placement::Ranged);
        assert_eq!(Role::Unknown.usual_placement(), Placement::Unknown);
    }

    #[test]
    fn rect_moved_matches_maa_semantics() {
        // BattleOperClickRange 的 rectMove 是 [-45, 6, 75, 120]：
        // 位移叠加，宽高替换。
        let flag = Rect::new(100, 588, 12, 12);
        let click = flag.moved(Rect::new(-45, 6, 75, 120));
        assert_eq!(click, Rect::new(55, 594, 75, 120));
    }

    #[test]
    fn rect_clamped_drops_out_of_bounds() {
        let r = Rect::new(-10, 700, 40, 40).clamped(REF_WIDTH, REF_HEIGHT);
        assert_eq!(r, Rect::new(0, 700, 30, 20));
    }

    #[test]
    fn point_distance() {
        assert!((Point::new(0, 0).distance_to(Point::new(3, 4)) - 5.0).abs() < 1e-9);
    }
}
