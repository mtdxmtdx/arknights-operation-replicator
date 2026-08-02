// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Vision/Battle/BattlefieldMatcher.cpp —— deployment_analyze 及其子分析
//   src/MaaCore/Task/BattleHelper.cpp —— analyze_oper_with_cache（按头像跨帧跟踪）
//   resource/tasks/tasks.json —— 全部 ROI / rectMove / 阈值常量
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 待部署栏识别。
//!
//! 思路完全照搬 MAA：部署栏每张卡片顶部都有一枚**相同的小旗标**，先用模板匹配
//! 一次性把所有旗标找出来、按横坐标排序，然后从旗标位置按固定偏移量切出各个子区域
//! （点击区、职业图标、可用性、冷却条、头像）。
//!
//! 与 MAA 的差别是：本模块不在战斗部署栏内跑详情页 OCR 来**命名**卡片。干员名来自既有档案、
//! `formation` 模块的战前人工确认与 F0 头像桥接，或保守回退的人工绑定；得到真实部署栏头像后，
//! 全程用模板跟踪（这正是 MAA 的 `analyze_oper_with_cache` 快路径）。

use repl_capture::Frame;
use repl_core::{
    viewport::{HAnchor, VAnchor},
    Point, Rect, Role, Viewport,
};

use crate::{
    ncc::{self, Template},
    templates::TemplateSet,
};

// ————————————————————————————————————————————————————————————————
// 常量：全部来自 MAA 的 resource/tasks/tasks.json，坐标系是 1280×720
// ————————————————————————————————————————————————————————————————

/// 部署栏旗标的搜索带。
///
/// MAA 的 `BattleOpersFlag.roi` 是 `[35, 588, 1245, 18]`，那是**安卓端**的布局。
/// PC 端的待部署卡片是**右对齐**的，而且 UI 边距（`uiScaler`）会让整排卡片上下浮动，
/// 所以这里横向搜满整幅、纵向给足余量。代价是搜索面积大了几倍 ——
/// 但只在暂停时跑一次，实测仍在 10ms 量级，换来的是不会因为布局差异而整个失效。
const FLAG_ROI: Rect = Rect::new(0, 560, 1280, 80);
/// `BattleOpersFlag.templThreshold`
const FLAG_THRESHOLD: f64 = 0.65;

/// `BattleOperClickRange.rectMove` —— 卡片可点击区域（拖拽起点）。
const CLICK_MOVE: Rect = Rect::new(-45, 6, 75, 120);
/// `BattleOperRoleRange.rectMove` —— 职业图标。
const ROLE_MOVE: Rect = Rect::new(-41, 6, 31, 25);
/// `BattleOperAvailable.rectMove` —— 可用性采样块。
const AVAILABLE_MOVE: Rect = Rect::new(0, 0, 10, 10);
/// `BattleOperCooling.rectMove` —— 冷却条。
const COOLING_MOVE: Rect = Rect::new(-68, 124, 114, 4);
/// `BattleOperAvatar.rectMove` —— 头像裁图（相对点击区）。
const AVATAR_MOVE: Rect = Rect::new(7, 32, 60, 60);

/// 职业识别的绝对下限。
///
/// MAA 用的是 `BattleOperRole.templThreshold = 0.65`，那是在安卓端接近原生 720p
/// 的截图上标定的。PC 端要先把高分辨率截图重采样到 1280×720，双线性会抹平一点边缘，
/// 实测同一枚图标只能拿到 0.60 左右 —— 卡在 0.65 上，整张卡就被丢掉了。
///
/// 但九选一本来就不该用绝对阈值判：实测第一名 0.604、第二名 0.171，
/// 3.5 倍的差距，赢得毫无悬念。所以改成"绝对下限 + 相对领先"双条件，
/// 这比任何单一绝对阈值都更能扛住不同分辨率带来的分数漂移。
const ROLE_MIN_SCORE: f64 = 0.40;
/// 最高分至少要是第二名的这个倍数，才算认出来。
const ROLE_MIN_MARGIN: f64 = 1.5;
/// `BattleOperAvailable.specialParams[0]` —— HSV 的 V 通道均值阈值。
const AVAILABLE_V_MIN: f64 = 100.0;
/// `BattleOperCooling.specialParams[0]` —— 落在冷却色域内的像素数阈值。
const COOLING_PIXEL_MIN: u32 = 300;
/// `BattleOperCooling.colorScales` —— HSV 下界 / 上界。
const COOLING_HSV_LO: [f64; 3] = [0.0, 100.0, 0.0];
const COOLING_HSV_HI: [f64; 3] = [20.0, 255.0, 150.0];

/// `BattleAvatarData.templThreshold` —— 正常头像跟踪阈值。
pub const AVATAR_THRESHOLD: f64 = 0.8;
/// `BattleAvatarCoolingData.templThreshold` —— 冷却态头像被压暗，阈值要放宽。
pub const AVATAR_COOLING_THRESHOLD: f64 = 0.5;
/// `BattleAvatarCoolingData.maskRange`
pub const AVATAR_COOLING_MASK: (u8, u8) = (0, 50);
/// `BattleDroneAvatarData.templThreshold`
pub const AVATAR_DRONE_THRESHOLD: f64 = 0.7;

// ————————————————————————————————————————————————————————————————

/// 部署栏上的一张卡片。
#[derive(Clone)]
pub struct Card {
    /// 从左到右的序号。**不要缓存**：干员上场或进入冷却后卡片会重排。
    pub index: usize,
    pub role: Role,
    /// 费用够不够、能不能点。
    pub available: bool,
    /// 是否处于再部署冷却中。
    pub cooling: bool,
    /// 可点击区域（客户区像素）。拖拽从这里的中心开始。
    pub click_rect: Rect,
    /// 头像区域（客户区像素）。
    pub avatar_rect: Rect,
    /// 头像裁图（BGRA），用于跨帧跟踪同一个干员。
    pub avatar: Vec<u8>,
    pub avatar_width: u32,
    pub avatar_height: u32,
}

impl Card {
    /// 拖拽起点。
    pub fn drag_origin(&self) -> Point {
        self.click_rect.center()
    }

    /// 把头像裁图做成可用于跟踪的模板。
    pub fn avatar_template(&self, name: impl Into<String>) -> Option<Template> {
        let mask = self.cooling.then_some(AVATAR_COOLING_MASK);
        Template::from_bgra(
            name,
            self.avatar_width,
            self.avatar_height,
            &self.avatar,
            mask,
        )
        .ok()
    }

    /// 跟踪时该用的阈值。冷却态的卡片被整体压暗，用正常阈值会全部匹配失败。
    pub fn tracking_threshold(&self) -> f64 {
        if self.cooling {
            AVATAR_COOLING_THRESHOLD
        } else if self.role == Role::Drone {
            AVATAR_DRONE_THRESHOLD
        } else {
            AVATAR_THRESHOLD
        }
    }
}

impl std::fmt::Debug for Card {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Card")
            .field("index", &self.index)
            .field("role", &self.role)
            .field("available", &self.available)
            .field("cooling", &self.cooling)
            .field("click_rect", &self.click_rect)
            .finish()
    }
}

/// 识别当前部署栏上的所有卡片。
///
/// **`frame` 必须是已经重采样到 1280×720 的参考帧**（见 `Frame::resample`），
/// 不是原始客户区截图。原因：MAA 的模板是在 1280×720 下裁的，`BattleOpersFlag.png`
/// 只有 10×11 像素，而归一化互相关没有尺度不变性 —— 拿它去 2560×1440 的图上搜
/// 一个都匹配不到。MAA 自己也是先缩图再识别。
///
/// 返回的全部矩形都在**参考坐标系**里，调用方用 `Viewport::field_to_client` 映回客户区。
///
/// 部署栏贴着**左下**边缘，所以 HUD 映射用 `(Left, Bottom)`。
pub fn analyze(frame: &Frame, viewport: &Viewport, templates: &TemplateSet) -> Vec<Card> {
    let roi = flag_search_roi(viewport);
    let mut flags = ncc::find_all(frame, roi, &templates.opers_flag, FLAG_THRESHOLD);
    if flags.is_empty() {
        log::debug!(
            "no deployment card flags found in {roi} of a {}×{} reference frame",
            frame.width,
            frame.height
        );
        return Vec::new();
    }
    ncc::sort_by_horizontal(&mut flags);

    // 已经在参考坐标系里了，rectMove 直接相加（与 MAA 完全一致）。
    let bounds = frame.bounds();

    let mut cards = Vec::with_capacity(flags.len());
    for flag in &flags {
        let click_rect = flag
            .rect
            .moved(CLICK_MOVE)
            .clamped(bounds.width, bounds.height);
        let role_rect = flag
            .rect
            .moved(ROLE_MOVE)
            .clamped(bounds.width, bounds.height);
        let avail_rect = flag
            .rect
            .moved(AVAILABLE_MOVE)
            .clamped(bounds.width, bounds.height);
        let cooling_rect = flag
            .rect
            .moved(COOLING_MOVE)
            .clamped(bounds.width, bounds.height);
        let avatar_rect = click_rect
            .moved(AVATAR_MOVE)
            .clamped(bounds.width, bounds.height);

        // MAA 会在职业识别失败时丢掉整张卡，因为它后续需要职业来命名干员。
        // 本程序由用户手工绑定名字，职业只作提示，旗标已经足以证明这里是一张卡。
        // PC 客户端的职业图标与安卓模板存在差异时，保留为“未知”比让绑定面板漏卡安全。
        let role = classify_role(frame, role_rect, templates).unwrap_or_else(|| {
            log::debug!("keeping flagged card at {} with unknown role", flag.rect);
            Role::Unknown
        });
        if avatar_rect.is_empty() || click_rect.is_empty() {
            continue;
        }

        let cooling = is_cooling(frame, cooling_rect);
        let available = is_available(frame, avail_rect);
        if cooling && available {
            // MAA 遇到这种组合会 log error；说明两个采样区之一被遮挡了。
            log::warn!("card at {} reads as both cooling and available", flag.rect);
        }

        cards.push(Card {
            index: cards.len(),
            role,
            available,
            cooling,
            click_rect,
            avatar_rect,
            avatar: crop_bgra(frame, avatar_rect),
            avatar_width: avatar_rect.width as u32,
            avatar_height: avatar_rect.height as u32,
        });
    }
    log::debug!("deployment bar: {} cards", cards.len());
    cards
}

/// MAA `BattlePauseCancelCheck.roi` —— 战斗 HUD 右上角暂停按钮的确认区域。
const PAUSE_BUTTON_ROI: Rect = Rect::new(1165, 20, 75, 65);
/// 暂停图标模板的判定阈值。MAA 默认 0.8；我们的截图经过重采样会损失一点边缘，
/// 参照职业图标的实测衰减（0.65 → 0.60）放宽到 0.6。
const HUD_THRESHOLD: f64 = 0.6;

/// 战斗 HUD 是否已经出现（右上角能匹配到暂停按钮图标）。
///
/// 这是开局触发的**硬确认**：倍速按钮区域出现白色只是粗筛 —— 准备界面、
/// 加载画面上的任何白色元素都可能骗过它；而 `BattleOfficiallyBegin.png`
/// 是暂停按钮本体的模板，只有真正进入战斗 HUD 才会匹配上。
/// MAA 的 `wait_for_frame_start` 在发暂停键前做的正是这个检查。
///
/// `frame` 必须是 1280×720 参考帧。
pub fn battle_hud_visible(frame: &Frame, viewport: &Viewport, templates: &TemplateSet) -> bool {
    let roi = viewport.hud_rect_ref(PAUSE_BUTTON_ROI, HAnchor::Right, VAnchor::Top);
    match ncc::best_match(frame, roi, &templates.officially_begin) {
        Some(m) => {
            log::debug!(
                "battle HUD check: pause icon scored {:.3} in {roi}",
                m.score
            );
            m.score >= HUD_THRESHOLD
        }
        None => false,
    }
}

/// 旗标搜索带（参考坐标系）。
///
/// **纵向**按 UI 边距（`uiScaler`）调整 —— 整排卡片会随这个设置上下浮动。
/// **横向搜满整幅，不做任何缩放** —— 卡片是右对齐的，而且数量随编队变化，
/// 位置无法预先算准。早先横向也乘了 `edge_scale`，结果搜索带只到 x=1152，
/// 把贴着右边缘的那张卡整个漏掉了。
pub fn flag_search_roi(viewport: &Viewport) -> Rect {
    let band = viewport.hud_rect_ref(FLAG_ROI, HAnchor::Left, VAnchor::Bottom);
    Rect::new(0, band.y, repl_core::REF_WIDTH, band.height)
}

/// 诊断用：不设阈值地报告最像旗标的若干处位置。
///
/// 识别不出卡片时用它判断到底是"模板压根不匹配"还是"搜索区域找错地方"。
pub fn probe_flags(
    frame: &Frame,
    viewport: &Viewport,
    templates: &TemplateSet,
    top_n: usize,
) -> Vec<crate::ncc::Match> {
    let roi = flag_search_roi(viewport);
    let mut hits = ncc::find_all(frame, roi, &templates.opers_flag, 0.0);
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(top_n);
    hits
}

/// 从一处旗标位置推导出各个子区域，供诊断工具画框。
pub fn subrects(flag: Rect, bounds: Rect) -> CardRects {
    let click = flag.moved(CLICK_MOVE).clamped(bounds.width, bounds.height);
    CardRects {
        click,
        role: flag.moved(ROLE_MOVE).clamped(bounds.width, bounds.height),
        available: flag
            .moved(AVAILABLE_MOVE)
            .clamped(bounds.width, bounds.height),
        cooling: flag
            .moved(COOLING_MOVE)
            .clamped(bounds.width, bounds.height),
        avatar: click
            .moved(AVATAR_MOVE)
            .clamped(bounds.width, bounds.height),
    }
}

/// [`subrects`] 的结果。
#[derive(Clone, Copy, Debug)]
pub struct CardRects {
    pub click: Rect,
    pub role: Rect,
    pub available: Rect,
    pub cooling: Rect,
    pub avatar: Rect,
}

/// 诊断用：报告某个区域内九个职业模板各自的匹配分数。
pub fn probe_roles(frame: &Frame, roi: Rect, templates: &TemplateSet) -> Vec<(Role, f64)> {
    let mut scores: Vec<(Role, f64)> = templates
        .roles
        .iter()
        .map(|(role, t)| {
            let score = ncc::best_match(frame, roi, t).map_or(f64::NAN, |m| m.score);
            (*role, score)
        })
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    scores
}

/// 九选一识别职业图标。
fn classify_role(frame: &Frame, roi: Rect, templates: &TemplateSet) -> Option<Role> {
    if roi.is_empty() {
        return None;
    }
    let mut best: Option<(Role, f64)> = None;
    let mut runner_up = f64::NEG_INFINITY;
    for (role, template) in &templates.roles {
        let Some(m) = ncc::best_match(frame, roi, template) else {
            continue;
        };
        match best {
            Some((_, s)) if m.score <= s => runner_up = runner_up.max(m.score),
            _ => {
                if let Some((_, s)) = best {
                    runner_up = runner_up.max(s);
                }
                best = Some((*role, m.score));
            }
        }
    }

    let (role, score) = best?;
    if score < ROLE_MIN_SCORE {
        log::trace!("best role {role:?} scored {score:.3}, below floor {ROLE_MIN_SCORE}");
        return None;
    }
    // 第二名可能是负数（完全不像）；那种情况直接算领先足够。
    let clear_winner = runner_up <= 0.0 || score >= runner_up * ROLE_MIN_MARGIN;
    if !clear_winner {
        log::trace!(
            "role {role:?} scored {score:.3} but runner-up got {runner_up:.3}; too close to call"
        );
        return None;
    }
    Some(role)
}

/// 可用性：采样块的 HSV 明度均值超过阈值就是"可部署"。
///
/// 不可用（费用不够）时游戏会把整张卡压暗，所以看明度就够了。
fn is_available(frame: &Frame, roi: Rect) -> bool {
    if roi.is_empty() {
        return false;
    }
    let mut sum = 0.0_f64;
    let mut count = 0_u32;
    for y in roi.y..roi.bottom() {
        for x in roi.x..roi.right() {
            if let Some([b, g, r, _]) = frame.pixel(x, y) {
                // HSV 的 V 就是 max(r, g, b)
                sum += f64::from(r.max(g).max(b));
                count += 1;
            }
        }
    }
    count > 0 && sum / f64::from(count) > AVAILABLE_V_MIN
}

/// 冷却：冷却条区域里落在指定 HSV 色域内的像素数超过阈值。
fn is_cooling(frame: &Frame, roi: Rect) -> bool {
    if roi.is_empty() {
        return false;
    }
    let mut hits = 0_u32;
    for y in roi.y..roi.bottom() {
        for x in roi.x..roi.right() {
            let Some([b, g, r, _]) = frame.pixel(x, y) else {
                continue;
            };
            let hsv = rgb_to_hsv(r, g, b);
            let inside = (0..3).all(|i| hsv[i] >= COOLING_HSV_LO[i] && hsv[i] <= COOLING_HSV_HI[i]);
            if inside {
                hits += 1;
            }
        }
    }
    hits >= COOLING_PIXEL_MIN
}

/// RGB → HSV，取值范围与 OpenCV 的 8 位实现一致：H ∈ [0,180)、S/V ∈ [0,255]。
///
/// 必须和 OpenCV 对齐，否则 MAA 的 `colorScales` 阈值全部失效。
fn rgb_to_hsv(r: u8, g: u8, b: u8) -> [f64; 3] {
    let (rf, gf, bf) = (f64::from(r), f64::from(g), f64::from(b));
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    let h = if delta <= f64::EPSILON {
        0.0
    } else if (max - rf).abs() < f64::EPSILON {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if (max - gf).abs() < f64::EPSILON {
        60.0 * ((bf - rf) / delta + 2.0)
    } else {
        60.0 * ((rf - gf) / delta + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max <= f64::EPSILON {
        0.0
    } else {
        delta / max * 255.0
    };
    // OpenCV 的 8 位 HSV 把色相压到 [0,180)
    [h / 2.0, s, max]
}

fn crop_bgra(frame: &Frame, rect: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity((rect.width * rect.height * 4).max(0) as usize);
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            match frame.pixel(x, y) {
                Some(px) => out.extend_from_slice(&px),
                None => out.extend_from_slice(&[0, 0, 0, 255]),
            }
        }
    }
    out
}

/// 按已知头像在当前卡片里找出对应的那张。
///
/// 这是 MAA `analyze_oper_with_cache` 的快路径：拿绑定时存下的头像裁图当模板，
/// 挨个卡片比一遍，取分数最高且过阈值的。
pub fn track<'a>(cards: &'a [Card], known_avatar: &Template, threshold: f64) -> Option<&'a Card> {
    let mut best: Option<(&Card, f64)> = None;
    for card in cards {
        if card.avatar.is_empty() {
            continue;
        }
        let Ok(candidate) = Template::from_bgra(
            "candidate",
            card.avatar_width,
            card.avatar_height,
            &card.avatar,
            None,
        ) else {
            continue;
        };
        let score = compare(known_avatar, &candidate);
        if best.as_ref().is_none_or(|(_, s)| score > *s) {
            best = Some((card, score));
        }
    }
    match best {
        Some((card, score)) if score >= threshold => Some(card),
        Some((_, score)) => {
            log::debug!("best avatar match scored {score:.3}, below {threshold}");
            None
        }
        None => None,
    }
}

/// 两张同尺寸头像的相关系数。尺寸不同直接判为不匹配。
fn compare(a: &Template, b: &Template) -> f64 {
    if a.width() != b.width() || a.height() != b.height() {
        return -1.0;
    }
    a.correlate_with(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_moves_are_applied_verbatim_in_reference_space() {
        // 识别全程在 1280×720 参考坐标系里做，rectMove 直接相加，与 MAA 完全一致。
        let flag = Rect::new(100, 588, 10, 11);
        assert_eq!(flag.moved(CLICK_MOVE), Rect::new(55, 594, 75, 120));
        assert_eq!(flag.moved(ROLE_MOVE), Rect::new(59, 594, 31, 25));
        assert_eq!(
            flag.moved(CLICK_MOVE).moved(AVATAR_MOVE),
            Rect::new(62, 626, 60, 60)
        );
    }

    #[test]
    fn hsv_matches_opencv_ranges() {
        // 纯红：H=0, S=255, V=255
        let hsv = rgb_to_hsv(255, 0, 0);
        assert!((hsv[0] - 0.0).abs() < 1e-6);
        assert!((hsv[1] - 255.0).abs() < 1e-6);
        assert!((hsv[2] - 255.0).abs() < 1e-6);

        // 纯绿：OpenCV 里 H = 120/2 = 60
        let hsv = rgb_to_hsv(0, 255, 0);
        assert!((hsv[0] - 60.0).abs() < 1e-6, "H 应当是 60，实际 {}", hsv[0]);

        // 纯蓝：H = 240/2 = 120
        let hsv = rgb_to_hsv(0, 0, 255);
        assert!((hsv[0] - 120.0).abs() < 1e-6);

        // 灰：S = 0
        let hsv = rgb_to_hsv(128, 128, 128);
        assert!((hsv[1] - 0.0).abs() < 1e-6);
        assert!((hsv[2] - 128.0).abs() < 1e-6);

        // 黑：全 0，且不能除零 panic
        let hsv = rgb_to_hsv(0, 0, 0);
        assert_eq!(hsv, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn cooling_colour_range_accepts_the_orange_bar() {
        // 冷却条是橙红色。HSV 下界 [0,100,0]、上界 [20,255,150]
        // => 色相偏红、饱和度高、明度中等偏暗。
        let orange = rgb_to_hsv(140, 60, 20);
        assert!(orange[0] >= COOLING_HSV_LO[0] && orange[0] <= COOLING_HSV_HI[0]);
        assert!(orange[1] >= COOLING_HSV_LO[1] && orange[1] <= COOLING_HSV_HI[1]);
        assert!(orange[2] >= COOLING_HSV_LO[2] && orange[2] <= COOLING_HSV_HI[2]);

        // 亮白不应被判成冷却
        let white = rgb_to_hsv(250, 250, 250);
        assert!(white[2] > COOLING_HSV_HI[2]);
    }

    fn frame_of(width: u32, height: u32, bgr: [u8; 3]) -> Frame {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            pixels.extend_from_slice(&[bgr[0], bgr[1], bgr[2], 255]);
        }
        Frame::new(width, height, pixels)
    }

    fn draw_checker(frame: &mut Frame, rect: Rect, bgr: [u8; 3]) {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let i = (y as usize * frame.width as usize + x as usize) * 4;
                let delta = if (x - rect.x + y - rect.y) % 2 == 0 {
                    0
                } else {
                    40
                };
                frame.pixels[i] = bgr[0].saturating_sub(delta);
                frame.pixels[i + 1] = bgr[1].saturating_sub(delta);
                frame.pixels[i + 2] = bgr[2].saturating_sub(delta);
            }
        }
    }

    fn crop_template(frame: &Frame, rect: Rect, name: &str) -> Template {
        Template::from_bgra(
            name,
            rect.width as u32,
            rect.height as u32,
            &crop_bgra(frame, rect),
            None,
        )
        .unwrap()
    }

    #[test]
    fn binding_keeps_a_flagged_card_when_role_is_unrecognized() {
        let viewport = Viewport::new(1280, 720, 0.0);
        let mut frame = frame_of(1280, 720, [80, 80, 80]);
        let band = flag_search_roi(&viewport);
        let flag_rect = Rect::new(300, band.y + 8, 10, 11);
        draw_checker(&mut frame, flag_rect, [220, 170, 40]);
        let opers_flag = crop_template(&frame, flag_rect, "flag");

        // Keep every role template structurally valid but absent from the role ROI. This
        // reproduces the binding failure mode: the deployment flag is certain, while a
        // profession icon changed or scores below the classifier threshold.
        let mut role_source = frame_of(29, 24, [10, 10, 10]);
        draw_checker(&mut role_source, Rect::new(0, 0, 29, 24), [20, 30, 230]);
        let roles = Role::ALL
            .into_iter()
            .map(|role| {
                (
                    role,
                    crop_template(&role_source, role_source.bounds(), role.zh()),
                )
            })
            .collect();
        let templates = TemplateSet {
            officially_begin: crop_template(&role_source, role_source.bounds(), "hud"),
            opers_flag,
            roles,
        };

        let cards = analyze(&frame, &viewport, &templates);
        assert_eq!(cards.len(), 1, "职业识别失败不应让绑定面板丢掉整张卡");
        assert_eq!(cards[0].role, Role::Unknown);
    }

    #[test]
    fn availability_uses_brightness() {
        let bright = frame_of(20, 20, [200, 200, 200]);
        assert!(is_available(&bright, Rect::new(0, 0, 10, 10)));
        let dim = frame_of(20, 20, [40, 40, 40]);
        assert!(!is_available(&dim, Rect::new(0, 0, 10, 10)));
        // 空矩形不能判成可用
        assert!(!is_available(&bright, Rect::new(0, 0, 0, 0)));
    }

    #[test]
    fn cooling_needs_enough_matching_pixels() {
        // 整块都是冷却色但面积不够 => 不算冷却
        let small = frame_of(10, 10, [20, 60, 140]); // BGR，对应 RGB(140,60,20)
        assert!(!is_cooling(&small, Rect::new(0, 0, 10, 10)));
        // 面积够了 => 算
        let big = frame_of(40, 20, [20, 60, 140]);
        assert!(is_cooling(&big, Rect::new(0, 0, 40, 20)));
    }

    #[test]
    fn tracking_threshold_depends_on_card_state() {
        let base = Card {
            index: 0,
            role: Role::Warrior,
            available: true,
            cooling: false,
            click_rect: Rect::new(0, 0, 10, 10),
            avatar_rect: Rect::new(0, 0, 10, 10),
            avatar: vec![0; 400],
            avatar_width: 10,
            avatar_height: 10,
        };
        assert!((base.tracking_threshold() - AVATAR_THRESHOLD).abs() < 1e-9);

        let cooling = Card {
            cooling: true,
            ..base.clone()
        };
        assert!((cooling.tracking_threshold() - AVATAR_COOLING_THRESHOLD).abs() < 1e-9);

        let drone = Card {
            role: Role::Drone,
            ..base.clone()
        };
        assert!((drone.tracking_threshold() - AVATAR_DRONE_THRESHOLD).abs() < 1e-9);
    }

    #[test]
    fn flag_search_band_always_spans_the_full_width() {
        // 待部署卡片是右对齐的，贴着右边缘那张必须在搜索范围内。
        // 早先横向也乘了 edge_scale，带子只到 x=1152，最右边那张卡整个漏掉。
        for ui_scaler in [0.0, 0.5, 1.0] {
            let vp = Viewport::new(2560, 1440, ui_scaler);
            let roi = flag_search_roi(&vp);
            assert_eq!(roi.x, 0, "uiScaler={ui_scaler} 时左边界不是 0");
            assert_eq!(
                roi.right(),
                repl_core::REF_WIDTH,
                "uiScaler={ui_scaler} 时右边界没到 1280"
            );
            // 纵向仍然要跟着 UI 边距走，并且落在画面下方
            assert!(roi.y > 500 && roi.bottom() <= repl_core::REF_HEIGHT);
        }
        // UI 边距不同，纵向位置应当确实不同
        let tight = flag_search_roi(&Viewport::new(2560, 1440, 0.0));
        let loose = flag_search_roi(&Viewport::new(2560, 1440, 1.0));
        assert_ne!(tight.y, loose.y, "纵向应当随 uiScaler 变化");
    }

    #[test]
    fn rect_moves_match_maa_tasks_json() {
        // 这些偏移量直接决定识别成败，改动前先去 MAA 的 tasks.json 对一遍。
        assert_eq!(CLICK_MOVE, Rect::new(-45, 6, 75, 120));
        assert_eq!(ROLE_MOVE, Rect::new(-41, 6, 31, 25));
        assert_eq!(AVAILABLE_MOVE, Rect::new(0, 0, 10, 10));
        assert_eq!(COOLING_MOVE, Rect::new(-68, 124, 114, 4));
        assert_eq!(AVATAR_MOVE, Rect::new(7, 32, 60, 60));
        assert!((FLAG_THRESHOLD - 0.65).abs() < 1e-9);
        assert_eq!(COOLING_PIXEL_MIN, 300);
    }

    /// 复现 `classify_role` 的判定条件，用来对着实测数据做回归。
    fn role_accepted(best: f64, runner_up: f64) -> bool {
        best >= ROLE_MIN_SCORE && (runner_up <= 0.0 || best >= runner_up * ROLE_MIN_MARGIN)
    }

    #[test]
    fn role_criteria_are_margin_based_not_a_hard_threshold() {
        // 2560×1440 实测：先锋 0.604、第二名重装 0.171。
        // 绝对分数没到 MAA 的 0.65（重采样抹平了边缘），但领先 3.5 倍，必须判定为认出。
        assert!(role_accepted(0.604, 0.171), "实测数据必须能被认出来");
        // 两个职业咬得很近时应当拒绝，而不是硬选一个 —— 认错职业比认不出更糟
        assert!(!role_accepted(0.55, 0.50), "0.55 vs 0.50 太接近，不该判定");
        // 绝对下限仍要挡住噪声级分数
        assert!(!role_accepted(0.30, 0.02), "0.30 属于噪声，必须挡掉");
        // 第二名为负 = 完全不像，此时只看绝对下限
        assert!(role_accepted(0.45, -0.10), "第二名为负时应当直接采纳");
    }
}
