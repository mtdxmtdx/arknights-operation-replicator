// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 等价于 OpenCV 的 TM_CCOEFF_NORMED；阈值与掩码参数取自
// MaaAssistantArknights (AGPL-3.0-only) 的 resource/tasks/tasks.json 与
// src/MaaCore/Vision/BestMatcher.cpp / MultiMatcher.cpp。
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 归一化互相关模板匹配。
//!
//! 为什么不用 OpenCV：整个项目只需要 `matchTemplate` 一个函数，模板都是 ≤60×60，
//! 搜索域都是窄条 ROI，而且只在游戏暂停时跑 —— 为此拖进一个几十 MB 的 C++ 依赖
//! 不划算。手写标量实现单次耗时在毫秒级，足够了。
//!
//! 公式与 OpenCV 的 `TM_CCOEFF_NORMED` 一致，三个颜色通道一起算
//! （MAA 的阈值就是在彩色图上标定的，改成灰度会让阈值失准）。

use repl_capture::Frame;
use repl_core::{Point, Rect};

/// 一个模板。构造时预计算均值和范数，匹配时就不用重复算了。
#[derive(Clone)]
pub struct Template {
    pub name: String,
    width: i32,
    height: i32,
    /// 去均值后的模板像素，按 BGR 三通道交错存放；被掩码排除的位置为 0。
    deviations: Vec<f64>,
    /// 每个像素是否参与计算。
    mask: Vec<bool>,
    /// 参与计算的像素数（三通道各算一次的话是 `valid * 3`）。
    valid: usize,
    /// `sqrt(Σ deviation²)`，归一化用。
    norm: f64,
}

impl Template {
    /// 从 BGRA 像素构造。
    ///
    /// `mask_range` 对应 MAA `tasks.json` 里的 `maskRange`：亮度落在该闭区间之外的
    /// 模板像素被排除。`BattleOpersFlag` 用 `[1, 255]`，作用是把纯黑的透明背景剔掉。
    pub fn from_bgra(
        name: impl Into<String>,
        width: u32,
        height: u32,
        bgra: &[u8],
        mask_range: Option<(u8, u8)>,
    ) -> Result<Self, TemplateError> {
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 {
            return Err(TemplateError::Empty);
        }
        if bgra.len() < w * h * 4 {
            return Err(TemplateError::TooSmall {
                expected: w * h * 4,
                actual: bgra.len(),
            });
        }

        let mut mask = vec![true; w * h];
        if let Some((lo, hi)) = mask_range {
            for (i, m) in mask.iter_mut().enumerate() {
                let px = &bgra[i * 4..i * 4 + 3];
                let luma = luma(px[2], px[1], px[0]);
                *m = luma >= lo && luma <= hi;
            }
        }
        let valid = mask.iter().filter(|m| **m).count();
        if valid == 0 {
            return Err(TemplateError::FullyMasked);
        }

        // 三通道各自的均值（只统计未被掩码的像素）
        let mut sums = [0.0_f64; 3];
        for (i, m) in mask.iter().enumerate() {
            if !*m {
                continue;
            }
            for c in 0..3 {
                sums[c] += f64::from(bgra[i * 4 + c]);
            }
        }
        let means = sums.map(|s| s / valid as f64);

        let mut deviations = vec![0.0_f64; w * h * 3];
        let mut norm_sq = 0.0_f64;
        for (i, m) in mask.iter().enumerate() {
            if !*m {
                continue;
            }
            for c in 0..3 {
                let d = f64::from(bgra[i * 4 + c]) - means[c];
                deviations[i * 3 + c] = d;
                norm_sq += d * d;
            }
        }
        if norm_sq <= f64::EPSILON {
            // 纯色模板没有任何结构，匹配任何东西都是 1.0，毫无意义。
            return Err(TemplateError::NoVariance);
        }

        Ok(Self {
            name: name.into(),
            width: width as i32,
            height: height as i32,
            deviations,
            mask,
            valid,
            norm: norm_sq.sqrt(),
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// 与另一张**同尺寸**模板直接算相关系数。
    ///
    /// 用于跨帧跟踪同一个干员的头像：两边都是已经裁好的固定尺寸小图，
    /// 不需要滑窗搜索。尺寸不同直接返回 `-1.0`（判为完全不匹配）。
    pub fn correlate_with(&self, other: &Self) -> f64 {
        if self.width != other.width || self.height != other.height {
            return -1.0;
        }
        let mut cov = 0.0_f64;
        let mut a_sq = 0.0_f64;
        let mut b_sq = 0.0_f64;
        for i in 0..self.mask.len() {
            // 任一侧被掩码排除的位置都跳过
            if !self.mask[i] || !other.mask[i] {
                continue;
            }
            for c in 0..3 {
                let da = self.deviations[i * 3 + c];
                let db = other.deviations[i * 3 + c];
                cov += da * db;
                a_sq += da * da;
                b_sq += db * db;
            }
        }
        if a_sq <= f64::EPSILON || b_sq <= f64::EPSILON {
            return 0.0;
        }
        cov / (a_sq.sqrt() * b_sq.sqrt())
    }
}

impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Template")
            .field("name", &self.name)
            .field("size", &(self.width, self.height))
            .field("valid_pixels", &self.valid)
            .finish()
    }
}

/// 一次匹配命中。
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// 命中区域（图像坐标）。
    pub rect: Rect,
    /// 相关系数，取值 `[-1, 1]`，越大越像。
    pub score: f64,
}

impl Match {
    pub fn center(&self) -> Point {
        self.rect.center()
    }
}

/// 在 `roi` 内搜索模板，返回**最佳**的一处命中。
pub fn best_match(frame: &Frame, roi: Rect, template: &Template) -> Option<Match> {
    let mut best: Option<Match> = None;
    for_each_candidate(frame, roi, template, |rect, score| {
        if best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Match { rect, score });
        }
    });
    best
}

/// 在 `roi` 内搜索模板，返回所有分数不低于 `threshold` 的命中（已做非极大值抑制）。
///
/// 对应 MAA 的 `MultiMatcher`：部署栏上每张卡片顶部都有一枚一样的小旗标，
/// 靠这个一次性把所有卡片位置找出来。
pub fn find_all(frame: &Frame, roi: Rect, template: &Template, threshold: f64) -> Vec<Match> {
    let mut hits = Vec::new();
    for_each_candidate(frame, roi, template, |rect, score| {
        if score >= threshold {
            hits.push(Match { rect, score });
        }
    });
    non_max_suppress(hits)
}

/// 遍历 ROI 内所有可能的模板位置，对每处算一次相关系数。
fn for_each_candidate(
    frame: &Frame,
    roi: Rect,
    template: &Template,
    mut visit: impl FnMut(Rect, f64),
) {
    let roi = roi.clamped(frame.width as i32, frame.height as i32);
    if roi.is_empty() || template.width > roi.width || template.height > roi.height {
        return;
    }
    let last_x = roi.right() - template.width;
    let last_y = roi.bottom() - template.height;

    for y in roi.y..=last_y {
        for x in roi.x..=last_x {
            let score = correlate_at(frame, template, x, y);
            visit(Rect::new(x, y, template.width, template.height), score);
        }
    }
}

/// 在 `(ox, oy)` 处计算一次归一化相关系数。
fn correlate_at(frame: &Frame, template: &Template, ox: i32, oy: i32) -> f64 {
    let stride = frame.width as usize * 4;
    let tw = template.width as usize;
    let th = template.height as usize;

    // 第一遍：窗口内（未被掩码的位置）三通道的均值
    let mut sums = [0.0_f64; 3];
    for ty in 0..th {
        let row = (oy as usize + ty) * stride;
        for tx in 0..tw {
            if !template.mask[ty * tw + tx] {
                continue;
            }
            let base = row + (ox as usize + tx) * 4;
            for (c, sum) in sums.iter_mut().enumerate() {
                *sum += f64::from(frame.pixels[base + c]);
            }
        }
    }
    let means = sums.map(|s| s / template.valid as f64);

    // 第二遍：分子（协方差）与窗口自身的范数
    let mut cov = 0.0_f64;
    let mut win_norm_sq = 0.0_f64;
    for ty in 0..th {
        let row = (oy as usize + ty) * stride;
        for tx in 0..tw {
            let idx = ty * tw + tx;
            if !template.mask[idx] {
                continue;
            }
            let base = row + (ox as usize + tx) * 4;
            for (c, mean) in means.iter().enumerate() {
                let d = f64::from(frame.pixels[base + c]) - mean;
                cov += d * template.deviations[idx * 3 + c];
                win_norm_sq += d * d;
            }
        }
    }
    if win_norm_sq <= f64::EPSILON {
        // 窗口是纯色：没有任何结构可比，判定为不匹配而不是完全匹配。
        return 0.0;
    }
    cov / (template.norm * win_norm_sq.sqrt())
}

/// 非极大值抑制：同一个目标会在相邻若干像素上都超过阈值，只保留局部最高分。
///
/// 重叠判据用的是"中心点落在对方矩形内"，比 IoU 简单且对这里的场景够用 ——
/// 部署栏上相邻卡片的旗标间距远大于旗标本身。
fn non_max_suppress(mut hits: Vec<Match>) -> Vec<Match> {
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Match> = Vec::new();
    for hit in hits {
        let overlaps = kept
            .iter()
            .any(|k| k.rect.contains(hit.center()) || hit.rect.contains(k.center()));
        if !overlaps {
            kept.push(hit);
        }
    }
    kept
}

/// 按横坐标从左到右排序。MAA 的 `sort_by_horizontal_`：部署栏卡片必须按
/// 屏幕顺序编号，否则 index 与实际卡片对不上。
pub fn sort_by_horizontal(hits: &mut [Match]) {
    hits.sort_by_key(|m| m.rect.x);
}

#[inline]
fn luma(r: u8, g: u8, b: u8) -> u8 {
    // 与 OpenCV 的 COLOR_BGR2GRAY 一致的整数近似
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("模板尺寸为空")]
    Empty,
    #[error("模板像素数据不足：需要 {expected} 字节，实际 {actual}")]
    TooSmall { expected: usize, actual: usize },
    #[error("模板被掩码完全排除，检查 maskRange 是否设反了")]
    FullyMasked,
    #[error("模板是纯色，没有可匹配的结构")]
    NoVariance,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张纯色图，再往里画一个方块。
    fn canvas(w: u32, h: u32, bg: [u8; 3]) -> Frame {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            pixels.extend_from_slice(&[bg[2], bg[1], bg[0], 255]);
        }
        Frame::new(w, h, pixels)
    }

    /// 画一个**带内部结构**的方块。
    ///
    /// 刻意不画纯色：归一化互相关对纯色区域是无定义的（方差为 0），
    /// 拿纯色当模板既不合法也不像真实的游戏图标。这里叠一个棋盘格，
    /// 既保证有方差，又便于人工核对匹配位置。
    fn draw(frame: &mut Frame, rect: Rect, color: [u8; 3]) {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let i = (y as usize * frame.width as usize + x as usize) * 4;
                let checker = if (x - rect.x + y - rect.y) % 2 == 0 {
                    0
                } else {
                    40
                };
                frame.pixels[i] = color[2].saturating_sub(checker);
                frame.pixels[i + 1] = color[1].saturating_sub(checker);
                frame.pixels[i + 2] = color[0].saturating_sub(checker);
            }
        }
    }

    fn crop_template(frame: &Frame, rect: Rect, name: &str) -> Template {
        let mut bgra = Vec::new();
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let i = (y as usize * frame.width as usize + x as usize) * 4;
                bgra.extend_from_slice(&frame.pixels[i..i + 4]);
            }
        }
        Template::from_bgra(name, rect.width as u32, rect.height as u32, &bgra, None).unwrap()
    }

    #[test]
    fn perfect_match_scores_one() {
        let mut img = canvas(60, 40, [20, 20, 20]);
        draw(&mut img, Rect::new(10, 5, 8, 6), [200, 30, 40]);
        let templ = crop_template(&img, Rect::new(10, 5, 8, 6), "block");
        let m = best_match(&img, img.bounds(), &templ).unwrap();
        assert_eq!(m.rect, Rect::new(10, 5, 8, 6));
        assert!(
            (m.score - 1.0).abs() < 1e-9,
            "完全一致应当得 1.0，实际 {}",
            m.score
        );
    }

    #[test]
    fn finds_every_repeated_instance() {
        let mut img = canvas(200, 40, [15, 15, 15]);
        for x in [10, 60, 110, 160] {
            draw(&mut img, Rect::new(x, 8, 10, 10), [220, 220, 40]);
        }
        let templ = crop_template(&img, Rect::new(10, 8, 10, 10), "flag");
        let mut hits = find_all(&img, img.bounds(), &templ, 0.8);
        assert_eq!(hits.len(), 4, "四个方块应当各命中一次，实际 {hits:?}");
        sort_by_horizontal(&mut hits);
        let xs: Vec<_> = hits.iter().map(|h| h.rect.x).collect();
        assert_eq!(xs, vec![10, 60, 110, 160]);
    }

    #[test]
    fn non_max_suppression_collapses_neighbours() {
        // 不做 NMS 的话，一个目标周围一圈位置都会超过阈值。
        let mut img = canvas(40, 40, [0, 0, 0]);
        draw(&mut img, Rect::new(15, 15, 6, 6), [255, 255, 255]);
        let templ = crop_template(&img, Rect::new(15, 15, 6, 6), "dot");
        let hits = find_all(&img, img.bounds(), &templ, 0.5);
        assert_eq!(hits.len(), 1, "NMS 后应当只剩一个，实际 {hits:?}");
    }

    #[test]
    fn brightness_shift_does_not_break_normalized_correlation() {
        // CCOEFF_NORMED 对整体亮度/对比度变化不敏感 —— 这正是我们要的，
        // 游戏里同一张卡片在不同背景下亮度会有差异。
        let mut img = canvas(40, 30, [10, 10, 10]);
        draw(&mut img, Rect::new(5, 5, 8, 8), [100, 100, 100]);
        let templ = crop_template(&img, Rect::new(5, 5, 8, 8), "grey");

        let mut brighter = canvas(40, 30, [60, 60, 60]);
        draw(&mut brighter, Rect::new(20, 10, 8, 8), [200, 200, 200]);
        let m = best_match(&brighter, brighter.bounds(), &templ).unwrap();
        assert_eq!(m.rect.x, 20);
        assert!(m.score > 0.95, "亮度整体抬高后仍应高分，实际 {}", m.score);
    }

    #[test]
    fn mask_range_excludes_black_background() {
        // 模拟 BattleOpersFlag 的 maskRange [1,255]：纯黑背景不参与匹配。
        let mut src = canvas(20, 20, [0, 0, 0]);
        draw(&mut src, Rect::new(4, 4, 6, 6), [200, 180, 60]);
        let mut bgra = Vec::new();
        for y in 0..12 {
            for x in 0..12 {
                let i = (y as usize * src.width as usize + x as usize) * 4;
                bgra.extend_from_slice(&src.pixels[i..i + 4]);
            }
        }
        let masked = Template::from_bgra("flag", 12, 12, &bgra, Some((1, 255))).unwrap();
        let unmasked = Template::from_bgra("flag", 12, 12, &bgra, None).unwrap();
        // 掩码后参与计算的像素应当明显更少：只剩 6×6 方块里非纯黑的那些。
        // （棋盘格让方块内一半像素偏暗，但仍远离 0，所以整块都保留）
        assert!(masked.valid < unmasked.valid);
        assert_eq!(masked.valid, 36);
        assert_eq!(unmasked.valid, 144);
    }

    #[test]
    fn uniform_template_is_rejected() {
        let flat = vec![128u8; 8 * 8 * 4];
        assert!(matches!(
            Template::from_bgra("flat", 8, 8, &flat, None),
            Err(TemplateError::NoVariance)
        ));
    }

    #[test]
    fn fully_masked_template_is_rejected() {
        let black = vec![0u8; 8 * 8 * 4];
        assert!(matches!(
            Template::from_bgra("black", 8, 8, &black, Some((1, 255))),
            Err(TemplateError::FullyMasked)
        ));
    }

    #[test]
    fn template_larger_than_roi_yields_nothing() {
        let img = canvas(10, 10, [30, 40, 50]);
        let mut big = canvas(20, 20, [0, 0, 0]);
        draw(&mut big, Rect::new(2, 2, 10, 10), [255, 0, 0]);
        let templ = crop_template(&big, Rect::new(0, 0, 20, 20), "big");
        assert!(best_match(&img, img.bounds(), &templ).is_none());
        assert!(find_all(&img, img.bounds(), &templ, 0.5).is_empty());
    }

    #[test]
    fn roi_limits_the_search() {
        let mut img = canvas(100, 20, [10, 10, 10]);
        draw(&mut img, Rect::new(5, 5, 6, 6), [255, 255, 255]);
        draw(&mut img, Rect::new(80, 5, 6, 6), [255, 255, 255]);
        let templ = crop_template(&img, Rect::new(5, 5, 6, 6), "dot");
        // 只在右半边搜，应当只找到右边那个
        let hits = find_all(&img, Rect::new(50, 0, 50, 20), &templ, 0.9);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].rect.x, 80);
    }
}
