// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Task/BattleHelper.cpp —— deploy_oper / fix_swipe_out_of_limit
//   src/MaaCore/Controller/Controller.cpp —— 滑动插值与 slope_in/slope_out 缓动
//   resource/tasks/tasks.json —— BattleSwipeOper / BattleUseOper 的参数
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 部署拖拽手势。
//!
//! 把"把某张卡片拖到某个格子并设定朝向"翻译成一串带时间戳的触点位置。
//! 参数全部来自 MAA 的实测值 —— 拖太快游戏收不到，拖太慢会被当成取消。
//!
//! 与 MAA 的唯一区别：**不做 swipe-with-pause**。MAA 在拖拽中途发 ESC 把游戏
//! 暂停，而我们在开始拖之前游戏就已经停在目标帧上了，再发一次 ESC 只会把它恢复运行。

use std::time::Duration;

use crate::geom::{Direction, Point, REF_HEIGHT};

/// `BattleSwipeOper.preDelay` —— 拖拽时长的距离系数。
/// `duration = 距离 / 1000 * 400`（毫秒）。
const DURATION_COEFF: f64 = 400.0;
/// `BattleSwipeOper.specialParams[4]` —— 拖拽时长下限。太短游戏认不到。
const MIN_DURATION_MS: u64 = 300;
/// `BattleSwipeOper.specialParams[0]` —— 设定朝向时的滑动距离（1280×720 下）。
const DIRECTION_DISTANCE_REF: f64 = 400.0;
/// `BattleSwipeOper.specialParams[1]` —— 朝向滑动越界时的补偿距离上限。
const DIRECTION_FIX_LIMIT: i32 = 100;
/// `BattleSwipeOper.postDelay` —— 朝向滑动的时长。
const DIRECTION_DURATION_MS: u64 = 150;
/// `BattleSwipeOper.specialParams[2]` —— 滑动初速度。
const SLOPE_IN: f64 = 2.0;
/// `BattleSwipeOper.specialParams[3]` —— 滑动末速度。
const SLOPE_OUT: f64 = 0.0;
/// `BattleUseOper.postDelay` —— 干员落地到设定朝向之间的等待。
pub const AFTER_DROP_MS: u64 = 200;
/// `BattleUseOper.preDelay` —— 朝向设定完之后的等待。
pub const AFTER_DIRECTION_MS: u64 = 150;
/// minitouch 的默认滑动步长间隔。
pub const SWIPE_STEP: Duration = Duration::from_millis(10);

/// 一段滑动：一串等间隔（[`SWIPE_STEP`]）的触点位置。
///
/// 第一个点是按下位置，最后一个是抬起位置。
#[derive(Clone, Debug, PartialEq)]
pub struct Swipe {
    pub points: Vec<Point>,
}

impl Swipe {
    pub fn duration(&self) -> Duration {
        SWIPE_STEP * (self.points.len().saturating_sub(1)) as u32
    }

    pub fn start(&self) -> Point {
        self.points[0]
    }

    pub fn end(&self) -> Point {
        *self
            .points
            .last()
            .expect("swipe always has at least one point")
    }
}

/// 一次完整的部署动作。
#[derive(Clone, Debug)]
pub struct DeployGesture {
    /// 从卡片拖到目标格子。
    pub drag: Swipe,
    /// 设定朝向的滑动。朝向为 [`Direction::None`] 时没有这一段。
    pub direction: Option<Swipe>,
}

/// 生成一次部署手势。
///
/// - `card_center` 待部署卡片的中心（客户区像素）
/// - `tile_pos` 目标格子在**侧视角**下的屏幕位置（客户区像素）
/// - `screen` 客户区尺寸，用于朝向滑动的越界纠偏
/// - `scale` 相对 1280×720 的缩放系数，用于换算朝向滑动距离
pub fn deploy(
    card_center: Point,
    tile_pos: Point,
    direction: Direction,
    screen: (i32, i32),
    scale: f64,
) -> DeployGesture {
    let distance = card_center.distance_to(tile_pos);
    let duration_ms = ((distance / 1000.0 * DURATION_COEFF).round() as u64).max(MIN_DURATION_MS);
    let drag = interpolate(
        card_center,
        tile_pos,
        Duration::from_millis(duration_ms),
        SLOPE_IN,
        SLOPE_OUT,
    );

    let direction = (direction != Direction::None).then(|| {
        // 朝向滑动距离按屏幕高度缩放（MAA：coeff * scale_height / 720）
        let coeff = (DIRECTION_DISTANCE_REF * scale * f64::from(REF_HEIGHT) / f64::from(REF_HEIGHT))
            .round() as i32;
        let mut start = tile_pos;
        let mut end = tile_pos + direction.unit() * coeff;
        fix_out_of_limit(
            &mut start,
            &mut end,
            screen.0,
            screen.1,
            DIRECTION_FIX_LIMIT,
        );
        interpolate(
            start,
            end,
            Duration::from_millis(DIRECTION_DURATION_MS),
            SLOPE_IN,
            SLOPE_OUT,
        )
    });

    DeployGesture { drag, direction }
}

/// 在两点之间插值出一串触点。
///
/// 缓动模型来自 MAA：`slope_in` 是初速度、`slope_out` 是末速度，
/// 平均速度固定，所以 `2.0 / 0.0` 表示"起步快、结束前减速到停" ——
/// 这样干员落点更稳，不会因为惯性滑到隔壁格子。
fn interpolate(from: Point, to: Point, duration: Duration, slope_in: f64, slope_out: f64) -> Swipe {
    let steps = (duration.as_millis() / SWIPE_STEP.as_millis()).max(1) as usize;
    let mut points = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        let eased = ease(t, slope_in, slope_out);
        points.push(Point::new(
            from.x + ((to.x - from.x) as f64 * eased).round() as i32,
            from.y + ((to.y - from.y) as f64 * eased).round() as i32,
        ));
    }
    Swipe { points }
}

/// 缓动函数。
///
/// 用一条三次 Hermite 曲线，端点导数分别是 `slope_in` / `slope_out`。
/// `slope_in = slope_out = 1` 时退化成线性。
fn ease(t: f64, slope_in: f64, slope_out: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    // h00*p0 + h10*m0 + h01*p1 + h11*m1，其中 p0=0, p1=1
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    (h01 + h10 * slope_in + h11 * slope_out).clamp(0.0, 1.0)
}

/// 修正超出屏幕的滑动终点。
///
/// 直接移植 MAA 的 `fix_swipe_out_of_limit`：终点越界时，把**整段**滑动
/// 沿反方向平移回来（而不是把终点截断），这样滑动的方向和长度都不变，
/// 游戏才能正确识别出朝向。
fn fix_out_of_limit(p1: &mut Point, p2: &mut Point, width: i32, height: i32, max_distance: i32) {
    let (direct, distance) = if p2.y > height {
        (Point::UP, p2.y - height)
    } else if p2.x > width {
        (Point::LEFT, p2.x - width)
    } else if p2.y < 0 {
        (Point::DOWN, -p2.y)
    } else if p2.x < 0 {
        (Point::RIGHT, -p2.x)
    } else {
        return;
    };
    let distance = distance.min(max_distance);
    let shift = direct * distance;
    *p1 = *p1 + shift;
    *p2 = *p2 + shift;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_hits_both_endpoints() {
        assert!((ease(0.0, SLOPE_IN, SLOPE_OUT) - 0.0).abs() < 1e-12);
        assert!((ease(1.0, SLOPE_IN, SLOPE_OUT) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn ease_is_monotonic_and_decelerating() {
        let mut previous = 0.0;
        let mut first_half = 0.0;
        let mut second_half = 0.0;
        for i in 1..=100 {
            let t = f64::from(i) / 100.0;
            let v = ease(t, SLOPE_IN, SLOPE_OUT);
            assert!(v >= previous - 1e-12, "缓动必须单调不减：t={t} v={v}");
            if i <= 50 {
                first_half = v;
            } else {
                second_half = v - first_half;
            }
            previous = v;
        }
        // 初速度 2、末速度 0 => 前半段走的距离应当超过一半
        assert!(first_half > 0.5, "起步应当更快，前半段只走了 {first_half}");
        assert!(second_half < 0.5);
    }

    #[test]
    fn linear_slopes_give_linear_motion() {
        for t in [0.25, 0.5, 0.75] {
            assert!(
                (ease(t, 1.0, 1.0) - t).abs() < 1e-12,
                "slope 1/1 应当是线性"
            );
        }
    }

    #[test]
    fn drag_respects_minimum_duration() {
        // 很短的距离也不能低于 300ms，否则干员放不上去
        let g = deploy(
            Point::new(100, 600),
            Point::new(110, 590),
            Direction::Right,
            (1280, 720),
            1.0,
        );
        assert!(
            g.drag.duration() >= Duration::from_millis(MIN_DURATION_MS),
            "实际 {:?}",
            g.drag.duration()
        );
    }

    #[test]
    fn drag_duration_scales_with_distance() {
        let short = deploy(
            Point::new(0, 0),
            Point::new(800, 0),
            Direction::None,
            (1280, 720),
            1.0,
        );
        let long = deploy(
            Point::new(0, 0),
            Point::new(2000, 0),
            Direction::None,
            (1280, 720),
            1.0,
        );
        assert!(long.drag.duration() > short.drag.duration());
        // 2000px => 2000/1000*400 = 800ms
        assert_eq!(long.drag.duration(), Duration::from_millis(800));
    }

    #[test]
    fn drag_starts_at_card_and_ends_at_tile() {
        let card = Point::new(200, 640);
        let tile = Point::new(700, 300);
        let g = deploy(card, tile, Direction::Right, (1280, 720), 1.0);
        assert_eq!(g.drag.start(), card);
        assert_eq!(g.drag.end(), tile, "终点必须精确落在目标格子上");
    }

    #[test]
    fn direction_swipe_goes_the_right_way() {
        let tile = Point::new(640, 360);
        for (dir, check) in [
            (Direction::Right, (1_i32, 0_i32)),
            (Direction::Left, (-1, 0)),
            (Direction::Up, (0, -1)),
            (Direction::Down, (0, 1)),
        ] {
            let g = deploy(tile, tile, dir, (1280, 720), 1.0);
            let swipe = g.direction.expect("应当有朝向滑动");
            let dx = swipe.end().x - swipe.start().x;
            let dy = swipe.end().y - swipe.start().y;
            assert_eq!(dx.signum(), check.0, "{dir:?} 的 x 方向不对");
            assert_eq!(dy.signum(), check.1, "{dir:?} 的 y 方向不对");
        }
    }

    #[test]
    fn direction_none_skips_the_second_swipe() {
        let g = deploy(
            Point::new(100, 600),
            Point::new(400, 300),
            Direction::None,
            (1280, 720),
            1.0,
        );
        assert!(g.direction.is_none(), "无朝向的干员不该有第二段滑动");
    }

    #[test]
    fn out_of_limit_shifts_the_whole_swipe_not_just_the_end() {
        // 在右边缘往右拖，终点会超出屏幕
        let tile = Point::new(1200, 360);
        let g = deploy(tile, tile, Direction::Right, (1280, 720), 1.0);
        let swipe = g.direction.unwrap();
        // 整段被左移，所以起点不再等于格子位置，但方向和长度保持
        assert!(swipe.start().x < tile.x, "整段应当被平移回来");
        let length = swipe.end().x - swipe.start().x;
        assert_eq!(length, 400, "平移不该改变滑动长度");
    }

    #[test]
    fn fix_out_of_limit_matches_maa_behaviour() {
        // 下边界超限
        let mut p1 = Point::new(100, 700);
        let mut p2 = Point::new(100, 800);
        fix_out_of_limit(&mut p1, &mut p2, 1280, 720, 100);
        assert_eq!(p1, Point::new(100, 620));
        assert_eq!(p2, Point::new(100, 720));

        // 补偿距离有上限：超出 300 但最多只补 100
        let mut p1 = Point::new(100, 700);
        let mut p2 = Point::new(100, 1020);
        fix_out_of_limit(&mut p1, &mut p2, 1280, 720, 100);
        assert_eq!(p1, Point::new(100, 600));
        assert_eq!(p2, Point::new(100, 920));

        // 没越界就不动
        let mut p1 = Point::new(100, 100);
        let mut p2 = Point::new(200, 200);
        fix_out_of_limit(&mut p1, &mut p2, 1280, 720, 100);
        assert_eq!(p1, Point::new(100, 100));
        assert_eq!(p2, Point::new(200, 200));
    }

    #[test]
    fn swipe_points_are_step_aligned() {
        let g = deploy(
            Point::new(0, 0),
            Point::new(1000, 0),
            Direction::None,
            (1280, 720),
            1.0,
        );
        // 1000px => 400ms => 40 步 => 41 个点
        assert_eq!(g.drag.points.len(), 41);
        assert_eq!(g.drag.duration(), Duration::from_millis(400));
    }

    #[test]
    fn constants_match_maa_tasks_json() {
        assert!((DURATION_COEFF - 400.0).abs() < 1e-9);
        assert_eq!(MIN_DURATION_MS, 300);
        assert!((DIRECTION_DISTANCE_REF - 400.0).abs() < 1e-9);
        assert_eq!(DIRECTION_FIX_LIMIT, 100);
        assert!((SLOPE_IN - 2.0).abs() < 1e-9);
        assert!((SLOPE_OUT - 0.0).abs() < 1e-9);
        assert_eq!(AFTER_DROP_MS, 200);
    }
}
