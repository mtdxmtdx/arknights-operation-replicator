// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件逐行移植自 MaaAssistantArknights (AGPL-3.0-only):
//   3rdparty/include/Arknights-Tile-Pos/TileCalc2.hpp
//   src/MaaCore/Config/Miscellaneous/TilePack.cpp —— calc_() 的多阶段判定
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 地图格子 → 屏幕坐标的投影。
//!
//! 这是对游戏里 Unity 相机的重建：用关卡数据里的相机位置和固定的欧拉角、FOV，
//! 拼出一个 4×4 的 MVP 矩阵，把格子的世界坐标投到 1280×720 的参考屏幕上。
//!
//! 两套视角必须分清楚：
//!
//! - **正常视角** (`side = false`)：点选场上已部署的干员用这套。
//! - **侧视角** (`side = true`)：拖拽部署时游戏会把镜头压低，落点要用这套。
//!
//! 用错视角的后果是干员放到隔壁格子，而且因为格子间距只有几十像素，
//! 肉眼看着"差不多"，实际全错。

use std::f64::consts::PI;

use crate::{geom::Point, level::Level};

const DEG: f64 = PI / 180.0;

/// 相机投影固定使用的参考分辨率。**不要**换成实际窗口尺寸 ——
/// 上游就是在 1280×720 下标定的，换了会整体偏移。
const PROJ_WIDTH: f64 = 1280.0;
const PROJ_HEIGHT: f64 = 720.0;

/// 撤退 / 技能按钮相对干员的偏移量（世界坐标）。数值来自上游标定。
const REL_POS_X: f64 = 1.313_338_684_082_031_2;
const REL_POS_Y: f64 = 1.314_337_134_361_267;
const REL_POS_Z: f64 = -0.396_787_405_014_038_1;

/// 4×4 行主序矩阵。只为这一处投影服务，没必要引入线性代数库。
#[derive(Clone, Copy, Debug)]
struct Mat4([[f64; 4]; 4]);

impl Mat4 {
    fn mul(self, rhs: Self) -> Self {
        let mut out = [[0.0; 4]; 4];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.0[i][k] * rhs.0[k][j]).sum();
            }
        }
        Self(out)
    }

    /// 乘一个齐次坐标为 1 的三维点，返回未做透视除法的四维结果。
    ///
    /// 对应 OpenCV 的 `Matx44d * Point3d`（会自动补 w = 1）。
    fn mul_point(self, p: [f64; 3]) -> [f64; 4] {
        let v = [p[0], p[1], p[2], 1.0];
        let mut out = [0.0; 4];
        for (i, cell) in out.iter_mut().enumerate() {
            *cell = (0..4).map(|k| self.0[i][k] * v[k]).sum();
        }
        out
    }
}

/// 相机在世界坐标里的位置。
///
/// 关卡数据给的是 16:9 下的相机位置；`TileCalc2` 会按当前宽高比做一次线性修正
/// （从 9:16 到 3:4 之间插值）。我们固定用 1280×720，所以 `t` 恒为 0，
/// 修正项恒为 0 —— 但仍然照抄公式，免得将来支持别的分辨率时忘了这一段。
fn camera_pos(level: &Level, side: bool, offset: [f64; 3]) -> [f64; 3] {
    let base = level.view[usize::from(side)];

    const FROM_RATIO: f64 = 9.0 / 16.0;
    const TO_RATIO: f64 = 3.0 / 4.0;
    let ratio = PROJ_HEIGHT / PROJ_WIDTH;
    let t = (FROM_RATIO - ratio) / (FROM_RATIO - TO_RATIO);

    [
        base[0] + (-1.4 * t) + offset[0],
        base[1] + (-2.8 * t) + offset[1],
        base[2] + offset[2],
    ]
}

/// 相机欧拉角（YXZ 序）。上游把它写死了，不随关卡变化。
fn camera_euler_yxz(side: bool) -> [f64; 3] {
    if side {
        [10.0 * DEG, 30.0 * DEG, 0.0]
    } else {
        [0.0, 30.0 * DEG, 0.0]
    }
}

/// 拼出 `proj * rotX * rotY * translate`。
///
/// 注意 `matrix_x` 第三行是 `[0, -sin_x, -cos_x, 0]` —— 第三个分量是**负**的，
/// 这不是笔误，是上游为了对齐 Unity 左手坐标系刻意写成这样的。照抄，别"修正"。
fn camera_matrix(pos: [f64; 3], euler: [f64; 3], ratio: f64) -> Mat4 {
    const FOV_2_Y: f64 = 20.0 * DEG;
    const FAR: f64 = 1000.0;
    const NEAR: f64 = 0.3;

    let (cos_y, sin_y) = (euler[0].cos(), euler[0].sin());
    let (cos_x, sin_x) = (euler[1].cos(), euler[1].sin());
    let tan_f = FOV_2_Y.tan();

    let translate = Mat4([
        [1.0, 0.0, 0.0, -pos[0]],
        [0.0, 1.0, 0.0, -pos[1]],
        [0.0, 0.0, 1.0, -pos[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let matrix_y = Mat4([
        [cos_y, 0.0, sin_y, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [-sin_y, 0.0, cos_y, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let matrix_x = Mat4([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cos_x, -sin_x, 0.0],
        [0.0, -sin_x, -cos_x, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let proj = Mat4([
        [ratio / tan_f, 0.0, 0.0, 0.0],
        [0.0, 1.0 / tan_f, 0.0, 0.0],
        [
            0.0,
            0.0,
            -(FAR + NEAR) / (FAR - NEAR),
            -(FAR * NEAR * 2.0) / (FAR - NEAR),
        ],
        [0.0, 0.0, -1.0, 0.0],
    ]);

    proj.mul(matrix_x).mul(matrix_y).mul(translate)
}

/// 世界坐标 → 1280×720 参考屏幕坐标。
pub fn world_to_screen(level: &Level, world: [f64; 3], side: bool, offset: [f64; 3]) -> Point {
    let pos = camera_pos(level, side, offset);
    let euler = camera_euler_yxz(side);
    let matrix = camera_matrix(pos, euler, PROJ_HEIGHT / PROJ_WIDTH);

    let mut v = matrix.mul_point(world);
    // 透视除法
    let w = v[3];
    for c in &mut v {
        *c /= w;
    }
    // NDC [-1, 1] → [0, 1]
    for c in &mut v {
        *c = (*c + 1.0) / 2.0;
    }
    Point::new(
        (v[0] * PROJ_WIDTH).round() as i32,
        ((1.0 - v[1]) * PROJ_HEIGHT).round() as i32,
    )
}

/// 格子 → 世界坐标。
///
/// 地图以中心为原点：x 向右、y 向上（注意格子的 y 是向下的，所以要取反），
/// z 由高度类型决定（高台 0 → 0，地面 1 → -0.4）。
pub fn tile_world_pos(level: &Level, loc: Point) -> Option<[f64; 3]> {
    let tile = level.tile(loc)?;
    let w = f64::from(level.width());
    let h = f64::from(level.height());
    Some([
        f64::from(loc.x) - (w - 1.0) / 2.0,
        (h - 1.0) / 2.0 - f64::from(loc.y),
        tile.height_type as f64 * -0.4,
    ])
}

/// 一张关卡算好的全部投影结果。
#[derive(Clone, Debug)]
pub struct TileProjection {
    /// 正常视角下每个格子的屏幕位置（点选场上干员用）。
    pub normal: std::collections::HashMap<Point, Point>,
    /// 侧视角下每个格子的屏幕位置（拖拽部署的落点用）。
    pub side: std::collections::HashMap<Point, Point>,
    /// 撤退按钮位置（侧视角）。
    pub retreat_button: Point,
    /// 技能按钮位置（侧视角）。
    pub skill_button: Point,
    /// 地图是否有多个阶段（蓝门/红门投到了屏幕外）。
    pub has_multi_stages: bool,
}

impl TileProjection {
    /// 计算整张地图的投影。
    ///
    /// `shift` 是镜头偏移（引航者试炼那种可以移动镜头的关卡才用得上），
    /// 常规关卡传 `(0.0, 0.0)`。符号与 MAA 的 `calc_` 一致：`offset = (-shift_x, shift_y, 0)`。
    pub fn compute(level: &Level, shift: (f64, f64)) -> Self {
        let offset = [-shift.0, shift.1, 0.0];
        let mut normal = std::collections::HashMap::new();
        let mut side = std::collections::HashMap::new();
        let mut has_multi_stages = false;

        for loc in level.locations() {
            let Some(world) = tile_world_pos(level, loc) else {
                continue;
            };
            let n = world_to_screen(level, world, false, offset);
            let s = world_to_screen(level, world, true, offset);

            // 蓝门 / 红门被投到屏幕外 => 这张图有多个阶段，撤退/技能按钮要按
            // 第一阶段的相机位置重新算（见 get_retreat_screen_pos 的 has_multi_stages）。
            if level.tile(loc).is_some_and(|t| t.key.is_gate()) && !within_tolerance(n) {
                has_multi_stages = true;
            }
            normal.insert(loc, n);
            side.insert(loc, s);
        }

        let stage_shift = if has_multi_stages {
            level.view[0][0]
        } else {
            0.0
        };
        let retreat_button = world_to_screen(
            level,
            [-REL_POS_X + stage_shift, REL_POS_Y, REL_POS_Z],
            true,
            [0.0; 3],
        );
        let skill_button = world_to_screen(
            level,
            [REL_POS_X + stage_shift, -REL_POS_Y, REL_POS_Z],
            true,
            [0.0; 3],
        );

        Self {
            normal,
            side,
            retreat_button,
            skill_button,
            has_multi_stages,
        }
    }

    /// 正常视角下的格子屏幕位置。
    pub fn normal_pos(&self, loc: Point) -> Option<Point> {
        self.normal.get(&loc).copied()
    }

    /// 侧视角（部署态）下的格子屏幕位置。
    pub fn side_pos(&self, loc: Point) -> Option<Point> {
        self.side.get(&loc).copied()
    }
}

/// MAA 判定多阶段用的容差：参考屏幕范围外扩 5%。
fn within_tolerance(p: Point) -> bool {
    const TOL_X: f64 = PROJ_WIDTH * 0.05;
    const TOL_Y: f64 = PROJ_HEIGHT * 0.05;
    (-TOL_X..=PROJ_WIDTH + TOL_X).contains(&f64::from(p.x))
        && (-TOL_Y..=PROJ_HEIGHT + TOL_Y).contains(&f64::from(p.y))
}

/// 判断某个格子在正常视角下是否落在屏幕里。
///
/// MAA 在 `is_skill_ready` 里做过同样的检查：投到屏幕外的格子点了也没用。
pub fn is_on_screen(p: Point) -> bool {
    p.x >= 0 && p.y >= 0 && f64::from(p.x) < PROJ_WIDTH && f64::from(p.y) < PROJ_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::Level;

    /// 一张 5×3 的地图。
    ///
    /// `view` 取自真实关卡数据（`main_00-01` 等绝大多数关卡都是这两组值）。
    /// **z 是负的** —— 相机在地图前方朝里看；早期把它写成正数会让整张图翻到屏幕外，
    /// 所以这里固定用真实值，不要"顺手改成好看的数"。
    ///
    /// `heightType` 也用真实语义：**`tile_road` = 0（地面）、`tile_wall` = 1（高台）**。
    /// 跨 40 张主线地图统计过：529 个 `tile_wall` 全是 1，906 个 `tile_road` 全是 0。
    /// MAA 的 `HeightType` 枚举名（`Highland = 0, Floor = 1`）与数据相反，别照抄。
    const MAP: &str = r#"{
        "stageId": "unit_test",
        "code": "UT-1",
        "levelId": "unit/test",
        "name": "单测图",
        "width": 5,
        "height": 3,
        "view": [[0.0, -4.81, -7.76], [0.5975098586953793, -5.31, -8.642108163374733]],
        "tiles": [
            [{"heightType":1,"buildableType":2,"tileKey":"tile_wall"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":1,"buildableType":2,"tileKey":"tile_wall"}],
            [{"heightType":0,"buildableType":0,"tileKey":"tile_start"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":0,"tileKey":"tile_end"}],
            [{"heightType":1,"buildableType":2,"tileKey":"tile_wall"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":1,"buildableType":2,"tileKey":"tile_wall"}]
        ]
    }"#;

    fn map() -> Level {
        Level::from_json(MAP).unwrap()
    }

    #[test]
    fn matrix_multiplication_is_row_major_correct() {
        let identity = Mat4([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let m = Mat4([
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ]);
        let p = identity.mul(m);
        for i in 0..4 {
            for j in 0..4 {
                assert!((p.0[i][j] - m.0[i][j]).abs() < 1e-12);
            }
        }
        // 平移矩阵作用在点上：应当直接加偏移
        let translate = Mat4([
            [1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 20.0],
            [0.0, 0.0, 1.0, 30.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        let v = translate.mul_point([1.0, 2.0, 3.0]);
        assert!((v[0] - 11.0).abs() < 1e-12);
        assert!((v[1] - 22.0).abs() < 1e-12);
        assert!((v[2] - 33.0).abs() < 1e-12);
        assert!((v[3] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn tile_world_pos_is_centred_and_flips_y() {
        let level = map();
        // 5 宽 3 高 => 中心格是 (2, 1)
        let centre = tile_world_pos(&level, Point::new(2, 1)).unwrap();
        assert!((centre[0] - 0.0).abs() < 1e-12, "中心格 x 应为 0");
        assert!((centre[1] - 0.0).abs() < 1e-12, "中心格 y 应为 0");

        // 格子 y 向下、世界 y 向上
        let top = tile_world_pos(&level, Point::new(2, 0)).unwrap();
        let bottom = tile_world_pos(&level, Point::new(2, 2)).unwrap();
        assert!(top[1] > bottom[1], "屏幕上方的格子世界 y 更大");

        // 实测语义：地面 tile_road 的 heightType=0 => z=0；
        //           高台 tile_wall 的 heightType=1 => z=-0.4（更"高"）。
        // MAA 的 HeightType 枚举名（Highland=0/Floor=1）与数据相反，别照着改。
        let wall = tile_world_pos(&level, Point::new(0, 0)).unwrap();
        let road = tile_world_pos(&level, Point::new(1, 0)).unwrap();
        assert!((wall[2] + 0.4).abs() < 1e-12, "高台 z 应为 -0.4");
        assert!((road[2] - 0.0).abs() < 1e-12, "地面 z 应为 0");

        assert!(tile_world_pos(&level, Point::new(9, 9)).is_none());
    }

    #[test]
    fn projection_puts_the_map_on_screen_and_preserves_ordering() {
        let level = map();
        let proj = TileProjection::compute(&level, (0.0, 0.0));

        assert_eq!(proj.normal.len(), 15);
        assert_eq!(proj.side.len(), 15);

        // 全部格子都应落在参考屏幕内
        for loc in level.locations() {
            let p = proj.normal_pos(loc).unwrap();
            assert!(
                is_on_screen(p),
                "格子 {loc} 投到了屏幕外 {p}（相机参数或矩阵写错了）"
            );
        }

        // x 递增 => 屏幕 x 递增；y 递增（往屏幕下方）=> 屏幕 y 递增
        let row: Vec<_> = (0..5)
            .map(|x| proj.normal_pos(Point::new(x, 1)).unwrap().x)
            .collect();
        assert!(
            row.windows(2).all(|w| w[0] < w[1]),
            "同一行 x 必须单调递增：{row:?}"
        );
        let col: Vec<_> = (0..3)
            .map(|y| proj.normal_pos(Point::new(2, y)).unwrap().y)
            .collect();
        assert!(
            col.windows(2).all(|w| w[0] < w[1]),
            "同一列 y 必须单调递增：{col:?}"
        );
    }

    #[test]
    fn side_view_differs_from_normal_view() {
        let level = map();
        let proj = TileProjection::compute(&level, (0.0, 0.0));
        let loc = Point::new(2, 1);
        let n = proj.normal_pos(loc).unwrap();
        let s = proj.side_pos(loc).unwrap();
        // 侧视角相机被压低且左移，同一个格子必然投到不同位置。
        // 这条断言的意义：如果哪天两套坐标算成一样了，说明 side 参数没生效，
        // 部署会全部落到错误的格子上。
        assert_ne!(n, s, "侧视角与正常视角不应相同");
    }

    #[test]
    fn perspective_makes_far_rows_denser() {
        // 透视投影下，远处（屏幕上方）的行间距应当小于近处。
        let level = map();
        let proj = TileProjection::compute(&level, (0.0, 0.0));
        let y0 = proj.normal_pos(Point::new(2, 0)).unwrap().y;
        let y1 = proj.normal_pos(Point::new(2, 1)).unwrap().y;
        let y2 = proj.normal_pos(Point::new(2, 2)).unwrap().y;
        assert!(
            (y1 - y0) < (y2 - y1),
            "近处行距应当更大：{} vs {}",
            y1 - y0,
            y2 - y1
        );
    }

    #[test]
    fn buttons_sit_on_the_selection_diagonal() {
        let level = map();
        let proj = TileProjection::compute(&level, (0.0, 0.0));
        // 选中干员后游戏会把镜头对到干员上，撤退出现在**左上**、技能在**右下**，
        // 二者关于选中点对称。上游的偏移量就是这么定的：
        //   retreat = {-x, +y, z}，skill = {+x, -y, z}（世界 y 向上）。
        assert!(
            proj.retreat_button.x < proj.skill_button.x,
            "撤退应在技能左边：{} / {}",
            proj.retreat_button,
            proj.skill_button
        );
        assert!(
            proj.retreat_button.y < proj.skill_button.y,
            "撤退应在技能上方：{} / {}",
            proj.retreat_button,
            proj.skill_button
        );
        assert!(
            is_on_screen(proj.retreat_button) && is_on_screen(proj.skill_button),
            "两个按钮都应落在屏幕内：{} / {}",
            proj.retreat_button,
            proj.skill_button
        );
    }

    /// 拿真实的 MAA 关卡数据跑一遍，比人造小地图更能暴露相机参数写错。
    ///
    /// 需要用户本机有 MAA 资源目录；找不到就跳过（本仓库不分发这些数据）。
    #[test]
    fn real_maa_levels_project_onto_the_screen() {
        let Some(dir) = crate::level::locate_tile_pos_dir() else {
            eprintln!("跳过：未找到 MAA 的 Arknights-Tile-Pos 目录");
            return;
        };
        let pack = match crate::level::LevelPack::load(&dir) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("跳过：加载关卡索引失败 {e}");
                return;
            }
        };

        // 挑一批覆盖面广的主线关：不同尺寸、不同高低台布局。
        let mut checked = 0;
        for key in [
            "0-1", "1-7", "2-8", "4-4", "6-4", "7-4", "9-17", "10-7", "12-17", "14-19",
        ] {
            let Ok(level) = pack.load_level(key) else {
                continue;
            };
            let proj = TileProjection::compute(&level, (0.0, 0.0));
            for loc in level.locations() {
                let tile = level.tile(loc).unwrap();
                let p = proj.normal_pos(loc).unwrap();
                let s = proj.side_pos(loc).unwrap();

                // **可部署**的格子必须严格在屏幕内 —— 我们要往上面点/拖。
                // 不可部署的格子（禁区、纯装饰）可以略微出界：地图比可视区宽时
                // 边角确实会被裁掉，MAA 的 `is_skill_ready` 也是这么处理的
                // （用 screen_rect.include 挡掉屏幕外的点）。
                if matches!(
                    tile.buildable,
                    crate::level::Buildable::Melee
                        | crate::level::Buildable::Ranged
                        | crate::level::Buildable::All
                ) {
                    assert!(
                        is_on_screen(p),
                        "关卡 {key} 的可部署格子 {loc} 投到了屏幕外 {p}"
                    );
                    assert!(
                        is_on_screen(s),
                        "关卡 {key} 的可部署格子 {loc} 侧视角投到了屏幕外 {s}"
                    );
                }
                // 不可部署的边角格子出界是正常的（透视下近处最宽，地图比可视区宽时
                // 底部两角会被裁掉）；这里只做一个松散的兜底，防止矩阵整体写错。
                assert!(
                    p.x > -400 && p.x < 1680 && p.y > -400 && p.y < 1120,
                    "关卡 {key} 的格子 {loc} 投得离谱 {p}，相机矩阵可能写错了"
                );
            }
            // 同一行必须横向单调
            let mid_row = level.height() / 2;
            let xs: Vec<_> = (0..level.width())
                .map(|x| proj.normal_pos(Point::new(x, mid_row)).unwrap().x)
                .collect();
            assert!(
                xs.windows(2).all(|w| w[0] < w[1]),
                "关卡 {key} 第 {mid_row} 行的 x 不单调：{xs:?}"
            );
            checked += 1;
        }
        assert!(checked > 0, "一个关卡都没验到，检查 MAA 资源目录是否完整");
        eprintln!("已用 {checked} 个真实关卡验证投影");
    }

    #[test]
    fn small_map_is_single_stage() {
        let level = map();
        let proj = TileProjection::compute(&level, (0.0, 0.0));
        assert!(!proj.has_multi_stages);
    }

    #[test]
    fn camera_shift_moves_the_projection() {
        let level = map();
        let base = TileProjection::compute(&level, (0.0, 0.0));
        let shifted = TileProjection::compute(&level, (1.0, 0.0));
        let loc = Point::new(2, 1);
        // shift_x 为正 => offset[0] 为负 => 相机左移 => 内容右移
        assert!(
            shifted.normal_pos(loc).unwrap().x > base.normal_pos(loc).unwrap().x,
            "镜头偏移没有生效"
        );
    }

    #[test]
    fn on_screen_predicate() {
        assert!(is_on_screen(Point::new(0, 0)));
        assert!(is_on_screen(Point::new(1279, 719)));
        assert!(!is_on_screen(Point::new(1280, 0)));
        assert!(!is_on_screen(Point::new(0, 720)));
        assert!(!is_on_screen(Point::new(-1, 0)));
    }
}
