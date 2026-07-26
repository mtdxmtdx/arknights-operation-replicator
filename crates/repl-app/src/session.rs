// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 动作执行时序移植自 MaaAssistantArknights (AGPL-3.0-only) 的
//   src/MaaCore/Task/BattleHelper.cpp (deploy_oper / use_skill / retreat_oper)
// 与 arknights-frame-assistant (GPL-3.0-only) 的
//   src/lib/hotkey_actions.ahk (ActionPauseSelect / ActionPauseSkill / ActionPauseRetreat)
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 一次复刻会话：把状态机的抽象指令翻译成真实的触控和按键。

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use repl_capture::{ClientGeometry, Frame, GameWindow, WindowCapturer};
use repl_core::{
    copilot::{Action, ActionType},
    gesture, Direction, Level, LevelPack, Point, Rect, TileProjection, Viewport,
};
use repl_input::{
    game_keys::func, key_tap, keys::DEFAULT_TAP_HOLD, precise_sleep, GameKeys, PauseController,
    TouchInjector,
};
use repl_vision::{Card, Template, TemplateSet};

use crate::config::{Binding, Config};

/// 暂停按钮左半 / 右半的客户区比例。来自 AFA 的 `PauseButtonPositionLeft/Right`。
///
/// 为什么要分左右两个点：暂停态下无法选中场上干员，必须"瞬间解暂停 → 点选 → 再暂停"，
/// 而两次点击落在**同一像素**会被游戏当成双击吞掉。错开一点就没事。
const PAUSE_BTN_LEFT: (f64, f64) = (0.9400, 0.0700);
const PAUSE_BTN_RIGHT: (f64, f64) = (0.9650, 0.0700);

/// 暂停时选中干员后，等待游戏确认选中状态的时长。
///
/// 三次 Tap 完成重暂停后，游戏需要一帧左右才能确认"已选中"状态；
/// 在此之前发技能键或撤退键，游戏尚未进入"已选中"模式，按键会被吞掉。
/// 1 秒远超必要时间，配合 runner 里的 PRE_ACTION_PAUSE 给整个注入阶段
/// 提供充分的稳定缓冲。
const POST_SELECT_PAUSE: Duration = Duration::from_secs(1);

/// MAA 的 `BattleCancelSelection.specificRect`（1280×720 参考坐标）。
const CANCEL_SELECTION_REF: Rect = Rect::new(630, 0, 20, 20);

/// 一次复刻会话持有的全部运行期资源。
pub struct Session {
    pub window: GameWindow,
    capturer: WindowCapturer,
    pub geometry: ClientGeometry,
    pub viewport: Viewport,
    pub level: Level,
    pub projection: TileProjection,
    pub templates: TemplateSet,
    touch: TouchInjector,
    pub pause: PauseController,
    pub game_keys: GameKeys,
    /// 干员名 → 头像模板，用于在部署栏里认出这张卡。
    avatars: HashMap<String, Template>,
    /// 干员名 → 场上格子坐标。Deploy 时写入，Retreat 时移除。
    battlefield: HashMap<String, Point>,
}

impl Session {
    /// 建立会话：定位窗口、加载关卡与模板、初始化输入。
    pub fn open(config: &Config, stage_name: &str) -> Result<Self> {
        let window = GameWindow::find().context("找不到明日方舟窗口，请先启动游戏")?;
        let geometry = window.geometry().context("读取游戏窗口几何失败")?;

        let resource_dir = config
            .resolve_maa_resource_dir()
            .ok_or_else(|| anyhow!("找不到 MAA 资源目录，请在设置里指定"))?;
        let templates = TemplateSet::load(&resource_dir).context("加载识别模板失败")?;
        let pack =
            LevelPack::load(resource_dir.join("Arknights-Tile-Pos")).context("加载关卡索引失败")?;
        let level = pack
            .load_level(stage_name)
            .with_context(|| format!("加载关卡「{stage_name}」失败"))?;
        let projection = TileProjection::compute(&level, (0.0, 0.0));

        let ui_scaler = config
            .ui_scaler_override
            .or_else(|| repl_input::game_keys::read_ui_scaler().ok().flatten())
            .unwrap_or(repl_core::viewport::DEFAULT_UI_SCALER);
        let viewport = Viewport::new(geometry.width, geometry.height, ui_scaler);

        // 明日方舟 PC 端以更高完整性级别运行，UIPI 会把低权限进程的输入静默丢弃
        // （SendInput 照常返回成功，游戏却收不到）。这是最容易踩、又最难自己看出来的坑，
        // 所以直接拦在会话建立阶段，而不是等到复刻跑了一半才发现动作全部无效。
        if !repl_input::is_elevated() {
            return Err(anyhow!(
                "本程序没有以管理员身份运行。明日方舟 PC 端以更高权限运行，\
                 Windows 会静默丢弃我们发出的所有按键和触控 —— 复刻会全程无效且没有报错。\
                 请以管理员身份重新启动本程序。"
            ));
        }

        TouchInjector::initialize().context("初始化触控注入失败")?;
        let game_keys = GameKeys::load();
        if !game_keys.from_registry() {
            log::warn!("未能读取游戏内按键设置，正在使用默认键位");
        }

        let capturer = WindowCapturer::new(window).context("建立截图会话失败")?;

        log::info!(
            "session ready: {}×{} client, level {} ({}×{}), ui_scaler {ui_scaler}",
            geometry.width,
            geometry.height,
            level.key.code,
            level.width(),
            level.height()
        );

        Ok(Self {
            window,
            capturer,
            geometry,
            viewport,
            level,
            projection,
            templates,
            touch: TouchInjector::new(),
            pause: PauseController::new(&game_keys),
            game_keys,
            avatars: HashMap::new(),
            battlefield: HashMap::new(),
        })
    }

    /// 抓一张客户区截图（原始分辨率）。
    pub fn capture(&mut self) -> Result<Frame> {
        self.capturer.grab().context("截图失败")
    }

    /// 抓一张已经重采样到 1280×720 的**参考帧**，供识别使用。
    ///
    /// 所有模板和 ROI 常量都是在 1280×720 下标定的，而模板匹配没有尺度不变性，
    /// 所以识别必须在参考坐标系里做。
    pub fn capture_reference(&mut self) -> Result<Frame> {
        let frame = self.capture()?;
        Ok(frame.resample(
            self.viewport.reference_source(),
            repl_core::REF_WIDTH as u32,
            repl_core::REF_HEIGHT as u32,
        ))
    }

    /// 识别当前部署栏。返回的矩形在**参考坐标系**里。
    pub fn scan_deployment(&mut self) -> Result<Vec<Card>> {
        let frame = self.capture_reference()?;
        Ok(repl_vision::analyze(
            &frame,
            &self.viewport,
            &self.templates,
        ))
    }

    /// 记录一次编队绑定。
    pub fn bind(&mut self, name: &str, card: &Card) -> Result<()> {
        let template = card
            .avatar_template(name)
            .ok_or_else(|| anyhow!("干员「{name}」的头像裁图无法作为模板（可能全是纯色）"))?;
        self.avatars.insert(name.to_owned(), template);
        Ok(())
    }

    /// 从保存的档案恢复绑定。
    pub fn restore_binding(&mut self, binding: &Binding, avatar_bgra: &[u8]) -> Result<()> {
        let template = Template::from_bgra(
            binding.name.clone(),
            binding.width,
            binding.height,
            avatar_bgra,
            None,
        )
        .map_err(|e| anyhow!("恢复干员「{}」的绑定失败：{e}", binding.name))?;
        self.avatars.insert(binding.name.clone(), template);
        Ok(())
    }

    pub fn bound_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.avatars.keys().cloned().collect();
        names.sort();
        names
    }

    /// 执行一个动作。
    pub fn execute(&mut self, action: &Action) -> Result<()> {
        // 每个动作前都重新确认窗口几何：尺寸变了所有坐标都作废，
        // 硬撑着继续跑只会点错地方。
        let geometry = self.window.geometry().context("读取窗口几何失败")?;
        if geometry.width != self.geometry.width || geometry.height != self.geometry.height {
            return Err(anyhow!(
                "游戏窗口尺寸从 {}×{} 变成了 {}×{}，坐标映射已失效，请重新开始",
                self.geometry.width,
                self.geometry.height,
                geometry.width,
                geometry.height
            ));
        }
        self.geometry = geometry;
        if !self.window.is_foreground() {
            return Err(anyhow!("游戏窗口不在前台，按键注入会打到别的程序上"));
        }

        match action.kind {
            ActionType::Deploy => self.do_deploy(action),
            ActionType::UseSkill => self.do_skill(action),
            ActionType::Retreat => self.do_retreat(action),
            ActionType::Output => Ok(()),
            other => Err(anyhow!("帧复刻模式不支持动作 {other}")),
        }
    }

    /// 部署：从卡片拖到格子，再拖一下设定朝向。
    ///
    /// 游戏此刻已经被停在目标帧上，所以**不做** MAA 的 swipe-with-pause ——
    /// 那会在拖拽中途再发一次 ESC，把游戏从暂停切回运行。
    fn do_deploy(&mut self, action: &Action) -> Result<()> {
        let loc = action
            .location
            .ok_or_else(|| anyhow!("部署动作缺少格子坐标"))?;

        // 卡片位置每次都重新识别：干员上场、进入冷却都会让部署栏重排。
        let cards = self.scan_deployment()?;
        if cards.is_empty() {
            return Err(anyhow!("识别不到部署栏，确认游戏在战斗界面且未被遮挡"));
        }
        let avatar = self
            .avatars
            .get(&action.name)
            .ok_or_else(|| anyhow!("干员「{}」还没有完成编队绑定", action.name))?;
        let threshold = cards
            .iter()
            .map(Card::tracking_threshold)
            .fold(f64::MAX, f64::min);
        let card = repl_vision::track(&cards, avatar, threshold).ok_or_else(|| {
            anyhow!(
                "在部署栏的 {} 张卡里找不到干员「{}」，可能已经上场或正在冷却",
                cards.len(),
                action.name
            )
        })?;
        if !card.available {
            log::warn!(
                "card for {} reads as unavailable; deploying anyway (frame timing wins)",
                action.name
            );
        }

        let tile_ref = self
            .projection
            .side_pos(loc)
            .ok_or_else(|| anyhow!("关卡里没有格子 {loc}"))?;
        let tile_client = self.viewport.field_to_client(tile_ref);
        // 卡片矩形来自参考帧，要映回真实客户区像素。
        let origin = self.viewport.field_to_client(card.drag_origin());

        let gesture = gesture::deploy(
            origin,
            tile_client,
            action.direction,
            (self.geometry.width as i32, self.geometry.height as i32),
            self.viewport.scale(),
        );

        log::info!(
            "deploy {} -> tile {loc} (client {tile_client}), drag {}ms{}",
            action.name,
            gesture.drag.duration().as_millis(),
            if action.direction == Direction::None {
                String::new()
            } else {
                format!(", facing {:?}", action.direction)
            }
        );

        self.swipe(&gesture.drag.points)?;
        precise_sleep(Duration::from_millis(gesture::AFTER_DROP_MS));
        if let Some(direction) = &gesture.direction {
            self.swipe(&direction.points)?;
            precise_sleep(Duration::from_millis(gesture::AFTER_DIRECTION_MS));
        }

        self.battlefield.insert(action.name.clone(), loc);
        Ok(())
    }

    /// 开技能：先"暂停时选中"，再在暂停+选中状态下按技能键。
    ///
    /// AFA `ActionPauseSkill` 的完整顺序（逐行对照 hotkey_actions.ahk 162–186 行）：
    ///
    /// 1. `Tap(左半)` — 触控触发解暂停
    /// 2. `Tap(干员)` — 触控点击干员（选中）
    /// 3. `Tap(右半)` — 触控触发重暂停
    ///    ↑ 三次 Tap 共约 3ms，远小于一帧（16.7ms），游戏来不及进入子弹时间
    /// 4. `sleep(CLICK_DELAY)` — 等选中稳定（游戏此时已重暂停）
    /// 5. `key_tap(技能键)` — 在暂停且已选中状态下按键，有效
    ///
    /// 错误的顺序（之前的实现）是把技能键夹在 Tap(干员) 和 Tap(右半) 之间：
    /// `key_tap` 有 50ms hold，游戏在重暂停之前运行了 50ms，足以进入子弹时间；
    /// 状态机检测到 0.2x 速度后中止。
    fn do_skill(&mut self, action: &Action) -> Result<()> {
        let target = self.resolve_target(action)?;
        let key = self
            .game_keys
            .get(func::RELEASE_SKILL)
            .ok_or_else(|| anyhow!("没有可用的技能键绑定"))?;
        self.select_while_paused(target)?;
        precise_sleep(POST_SELECT_PAUSE);
        key_tap(key, DEFAULT_TAP_HOLD)?;
        log::info!("skill on {} at {target}", action.name);
        Ok(())
    }

    /// 撤退：先"暂停时选中"，再在暂停+选中状态下按撤退键。
    ///
    /// 同 `do_skill`，参见其文档注释。
    fn do_retreat(&mut self, action: &Action) -> Result<()> {
        let target = self.resolve_target(action)?;
        let key = self
            .game_keys
            .get(func::RETREAT_CHAR)
            .ok_or_else(|| anyhow!("没有可用的撤退键绑定"))?;
        self.select_while_paused(target)?;
        precise_sleep(POST_SELECT_PAUSE);
        key_tap(key, DEFAULT_TAP_HOLD)?;

        if let Some(loc) = self.battlefield.remove(&action.name) {
            log::info!("retreat {} from {loc}", action.name);
        }
        self.cancel_selection()?;
        Ok(())
    }

    /// 找出动作目标在客户区里的位置。
    ///
    /// 优先用作业里写的 `location`；没写就查我们自己记的"谁在哪个格子"。
    fn resolve_target(&self, action: &Action) -> Result<Point> {
        let loc = action
            .location
            .or_else(|| self.battlefield.get(&action.name).copied())
            .ok_or_else(|| {
                anyhow!(
                    "不知道干员「{}」在场上哪个格子。请在作业里给这个动作补上 location",
                    action.name
                )
            })?;
        let reference = self
            .projection
            .normal_pos(loc)
            .ok_or_else(|| anyhow!("关卡里没有格子 {loc}"))?;
        if !repl_core::tile::is_on_screen(reference) {
            return Err(anyhow!("格子 {loc} 投影到了屏幕外 {reference}，点不到"));
        }
        Ok(self.viewport.field_to_client(reference))
    }

    /// 暂停时选中场上干员：Tap(左半) → Tap(目标) → Tap(右半)。
    ///
    /// AFA `ActionPauseSelect` 的完整实现（hotkey_actions.ahk 88–110 行）。
    /// 三次 Tap 共约 3ms，远小于一帧（16.7ms），游戏来不及进入子弹时间就已重暂停。
    /// 完成后用 `precise_sleep(CLICK_DELAY)` 等游戏确认选中状态，再发功能键。
    fn select_while_paused(&mut self, target: Point) -> Result<()> {
        let left = self.client_ratio(PAUSE_BTN_LEFT);
        let right = self.client_ratio(PAUSE_BTN_RIGHT);
        self.tap(left)?;
        self.tap(target)?;
        self.tap(right)?;
        Ok(())
    }

    fn cancel_selection(&mut self) -> Result<()> {
        let rect = self.viewport.hud_rect(
            CANCEL_SELECTION_REF,
            repl_core::HAnchor::Left,
            repl_core::VAnchor::Top,
        );
        self.tap(rect.center())
    }

    /// 参考坐标 → 客户区像素。识别结果都要过这一步才能拿去点。
    pub fn reference_to_client(&self, reference: Point) -> Point {
        self.viewport.field_to_client(reference)
    }

    /// 把鼠标指针停到安全区（客户区左侧 2% 宽、25% 高处）。
    ///
    /// 触控注入不移动鼠标，指针会一直停在用户最后放的位置；而 Arknights PC
    /// 自绘光标一旦停在费用条附近，尺子就会把每一帧标记 `cursorBlocked`、
    /// 冻结分析 —— 帧数就断供了。所以在战斗开始后和编队绑定后各停靠一次。
    ///
    /// 安全点的选取依据（都是要躲开的区域）：
    /// - 费用条和费用数字：右侧 ~74–78% 高
    /// - 倍速/暂停按钮（我们的像素触发器也在看）：右上角
    /// - 部署栏识别带：底部
    /// - 尺子 battle_begin 检测的左侧采样带：左侧 3–18% 宽、40–80% 高
    ///   （左侧 25% 高在这条带子上方，安全）
    pub fn park_cursor(&self) {
        let client = Point::new(
            (f64::from(self.geometry.width) * 0.02) as i32,
            (f64::from(self.geometry.height) * 0.25) as i32,
        );
        let screen = self.geometry.client_to_screen(client);
        match repl_input::mouse::set_cursor_pos(screen) {
            Ok(()) => log::info!("cursor parked at client {client} (screen {screen})"),
            Err(e) => log::warn!("failed to park cursor: {e}"),
        }
    }

    fn client_ratio(&self, (rx, ry): (f64, f64)) -> Point {
        Point::new(
            (f64::from(self.geometry.width) * rx).round() as i32,
            (f64::from(self.geometry.height) * ry).round() as i32,
        )
    }

    /// 点一下（客户区坐标）。
    pub fn tap(&mut self, client: Point) -> Result<()> {
        let screen = self.geometry.client_to_screen(client);
        self.touch.tap(screen).context("触控点击失败")
    }

    /// 按路径滑动（客户区坐标）。
    fn swipe(&mut self, path: &[Point]) -> Result<()> {
        let screen: Vec<Point> = path
            .iter()
            .map(|p| self.geometry.client_to_screen(*p))
            .collect();
        let result = self
            .touch
            .swipe_path(&screen, gesture::SWIPE_STEP)
            .context("触控滑动失败");
        if result.is_err() {
            // 出错时务必把触点抬起来，否则游戏会一直以为手指还按着，
            // 后面所有输入都会失灵。
            self.touch.release_if_down();
        }
        result
    }

    /// 紧急收尾：抬起可能卡住的触点。
    pub fn release_inputs(&mut self) {
        self.touch.release_if_down();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_button_halves_are_distinct() {
        // 两次点击必须落在不同像素，否则会被当成双击吞掉
        assert_ne!(PAUSE_BTN_LEFT, PAUSE_BTN_RIGHT);
        // 都应当在右上角的暂停按钮区域内
        for (x, y) in [PAUSE_BTN_LEFT, PAUSE_BTN_RIGHT] {
            assert!((0.9..1.0).contains(&x), "x 比例 {x} 不在右侧");
            assert!((0.0..0.15).contains(&y), "y 比例 {y} 不在顶部");
        }
        // 在 1920 宽的窗口上两点相距应当有十几像素，足以避开双击判定
        let gap = ((PAUSE_BTN_RIGHT.0 - PAUSE_BTN_LEFT.0) * 1920.0).round() as i32;
        assert!(gap >= 10, "两点间距只有 {gap}px，可能仍被当成双击");
    }

    #[test]
    fn cancel_selection_rect_matches_maa() {
        assert_eq!(CANCEL_SELECTION_REF, Rect::new(630, 0, 20, 20));
    }
}
