// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   3rdparty/include/Arknights-Tile-Pos/TileDef.hpp  —— Level / LevelKey / Tile
//   src/MaaCore/Config/Miscellaneous/TilePack.cpp    —— overview 索引与 tileKey 映射
// 关卡数据本身来自 https://github.com/yuanyan3060/Arknights-Tile-Pos，
// 由 MAA 二次分发；本仓库不包含这些数据文件，运行时从用户配置的 MAA 资源目录读取。
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 关卡地图数据（`Arknights-Tile-Pos`）。
//!
//! `overview.json` 是索引，每个关卡一个单独的 JSON 文件。索引很小（几 MB），
//! 启动时全量读入；关卡文件按需懒加载 —— 全部 4000 多个文件共 82MB，没必要全读。

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::geom::Point;

/// 地块类型。数值语义见 MAA `TilePack::TileKey`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TileKey {
    #[default]
    Invalid,
    /// 不能放干员，敌人也不能走
    Forbidden,
    /// 高台
    Wall,
    /// 地面，敌人可走
    Road,
    /// 蓝门
    Home,
    /// 红门
    EnemyHome,
    /// 绿门
    Green,
    /// 无人机始发站
    Airport,
    /// 不能放干员但敌人可走
    Floor,
    /// 空降兵掉落点
    Hole,
    Telin,
    Telout,
    Grass,
    DeepSea,
    Volcano,
    Healing,
    Fence,
    Infection,
}

impl TileKey {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "tile_forbidden" => Self::Forbidden,
            "tile_wall" => Self::Wall,
            "tile_road" => Self::Road,
            "tile_end" => Self::Home,
            "tile_start" => Self::EnemyHome,
            "tile_green" => Self::Green,
            "tile_flystart" => Self::Airport,
            "tile_floor" => Self::Floor,
            "tile_hole" => Self::Hole,
            "tile_telin" => Self::Telin,
            "tile_telout" => Self::Telout,
            "tile_grass" => Self::Grass,
            "tile_deepsea" | "tile_deepwater" => Self::DeepSea,
            "tile_volcano" => Self::Volcano,
            "tile_healing" => Self::Healing,
            "tile_fence" | "tile_fence_bound" => Self::Fence,
            "tile_infection" => Self::Infection,
            _ => Self::Invalid,
        }
    }

    /// 蓝门 / 红门。用于判断地图是否有多阶段（见 `TilePack::calc_`）。
    pub fn is_gate(self) -> bool {
        matches!(self, Self::Home | Self::EnemyHome)
    }
}

/// 可部署类型。数值与 MAA `battle::LocationType` 一致。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Buildable {
    Invalid = -1,
    #[default]
    None = 0,
    /// 只能放近战（地面）
    Melee = 1,
    /// 只能放远程（高台）
    Ranged = 2,
    All = 3,
}

impl Buildable {
    pub fn from_i64(v: i64) -> Self {
        match v {
            0 => Self::None,
            1 => Self::Melee,
            2 => Self::Ranged,
            3 => Self::All,
            _ => Self::Invalid,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct RawTile {
    #[serde(default)]
    #[serde(rename = "heightType")]
    height_type: i64,
    #[serde(default)]
    #[serde(rename = "buildableType")]
    buildable_type: i64,
    #[serde(default)]
    #[serde(rename = "tileKey")]
    tile_key: String,
}

#[derive(Clone, Copy, Debug)]
pub struct Tile {
    /// 高度类型，直接参与世界坐标的 z 分量：`z = height_type * -0.4`。
    ///
    /// **实测语义：`0` = 地面（`tile_road`），`1` = 高台（`tile_wall`）。**
    ///
    /// 注意 MAA 的 `TilePack::HeightType` 枚举把它标成了
    /// `Highland = 0, Floor = 1` —— 与真实数据相反。跨 40 张主线地图统计：
    /// `tile_wall` 的 heightType 全是 1（529/529），`tile_road` 全是 0（906/906）。
    ///
    /// 所幸投影只用原始整数、从不看枚举名，所以 MAA 和本项目算出来的坐标都是对的。
    /// 这里保留原始 `i64` 而不引入枚举，就是为了避免被那个错误命名带偏。
    pub height_type: i64,
    pub buildable: Buildable,
    pub key: TileKey,
}

/// 关卡标识。四个字段任一匹配即可（与 MAA `LevelKey::match` 一致）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct LevelKey {
    #[serde(rename = "stageId", default)]
    pub stage_id: String,
    #[serde(default)]
    pub code: String,
    #[serde(rename = "levelId", default)]
    pub level_id: String,
    #[serde(default)]
    pub name: String,
}

impl LevelKey {
    /// 任一字段与 `key` 完全相等即算命中。
    pub fn matches(&self, key: &str) -> bool {
        !key.is_empty()
            && (self.stage_id == key
                || self.code == key
                || self.level_id == key
                || self.name == key)
    }
}

/// 一张关卡地图。
#[derive(Clone, Debug)]
pub struct Level {
    pub key: LevelKey,
    width: i32,
    height: i32,
    /// 相机位置，`view[0]` 是正常视角、`view[1]` 是侧视角（部署态）。
    pub view: Vec<[f64; 3]>,
    tiles: Vec<Vec<Tile>>,
}

#[derive(Deserialize)]
struct RawLevel {
    #[serde(rename = "stageId", default)]
    stage_id: String,
    #[serde(default)]
    code: String,
    #[serde(rename = "levelId", default)]
    level_id: String,
    #[serde(default)]
    name: String,
    width: i32,
    height: i32,
    view: Vec<[f64; 3]>,
    tiles: Vec<Vec<RawTile>>,
}

impl Level {
    pub fn from_json(json: &str) -> Result<Self, LevelError> {
        let raw: RawLevel = serde_json::from_str(json).map_err(LevelError::Parse)?;
        if raw.view.len() < 2 {
            return Err(LevelError::Malformed(format!(
                "关卡 {} 的 view 只有 {} 项，至少需要 2 项（正常视角 + 侧视角）",
                raw.stage_id,
                raw.view.len()
            )));
        }
        if raw.tiles.len() != raw.height as usize
            || raw.tiles.iter().any(|row| row.len() != raw.width as usize)
        {
            return Err(LevelError::Malformed(format!(
                "关卡 {} 的 tiles 尺寸与 {}×{} 不符",
                raw.stage_id, raw.width, raw.height
            )));
        }
        let tiles = raw
            .tiles
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|t| Tile {
                        height_type: t.height_type,
                        buildable: Buildable::from_i64(t.buildable_type),
                        key: TileKey::parse(&t.tile_key),
                    })
                    .collect()
            })
            .collect();
        Ok(Self {
            key: LevelKey {
                stage_id: raw.stage_id,
                code: raw.code,
                level_id: raw.level_id,
                name: raw.name,
            },
            width: raw.width,
            height: raw.height,
            view: raw.view,
            tiles,
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// 取 (x, y) 处的地块。越界返回 `None`。
    pub fn tile(&self, loc: Point) -> Option<Tile> {
        if loc.x < 0 || loc.y < 0 || loc.x >= self.width || loc.y >= self.height {
            return None;
        }
        Some(self.tiles[loc.y as usize][loc.x as usize])
    }

    /// 遍历所有格子坐标。
    pub fn locations(&self) -> impl Iterator<Item = Point> + '_ {
        (0..self.height).flat_map(move |y| (0..self.width).map(move |x| Point::new(x, y)))
    }
}

/// `overview.json` 里的一条索引。
#[derive(Clone, Debug, Deserialize)]
pub struct LevelSummary {
    #[serde(flatten)]
    pub key: LevelKey,
    pub filename: String,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub height: i32,
}

/// 关卡索引 + 懒加载。
#[derive(Debug)]
pub struct LevelPack {
    dir: PathBuf,
    summaries: Vec<LevelSummary>,
}

impl LevelPack {
    /// 从 MAA 资源目录下的 `Arknights-Tile-Pos/` 加载索引。
    pub fn load(tile_pos_dir: impl AsRef<Path>) -> Result<Self, LevelError> {
        let dir = tile_pos_dir.as_ref().to_path_buf();
        let overview = dir.join("overview.json");
        let text = std::fs::read_to_string(&overview)
            .map_err(|e| LevelError::Io(overview.display().to_string(), e.to_string()))?;
        Self::from_overview_json(dir, &text)
    }

    pub fn from_overview_json(dir: PathBuf, json: &str) -> Result<Self, LevelError> {
        let raw: BTreeMap<String, LevelSummary> =
            serde_json::from_str(json).map_err(LevelError::Parse)?;
        let summaries: Vec<_> = raw.into_values().collect();
        if summaries.is_empty() {
            return Err(LevelError::Malformed("overview.json 里没有任何关卡".into()));
        }
        log::info!(
            "loaded {} stage summaries from tile-pos overview",
            summaries.len()
        );
        Ok(Self { dir, summaries })
    }

    pub fn len(&self) -> usize {
        self.summaries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.summaries.is_empty()
    }

    /// 按 stageId / code / levelId / name 任一匹配查找关卡索引。
    ///
    /// 有多个条目命中时取第一个（见 [`Self::find_all`] 的说明）。
    pub fn find(&self, key: &str) -> Option<&LevelSummary> {
        self.summaries.iter().find(|s| s.key.matches(key))
    }

    /// 列出所有命中的条目。
    ///
    /// 同一个关卡号常常对应多个 `stageId`：主线关几乎都有一个 `#f#` 后缀的变体
    /// （例如 `main_01-07` 与 `main_01-07#f#` 的 `code` 都是 `1-7`）。
    /// 已核对过全部 686 组这样的配对，**地图数据完全一致**，所以取哪个都行；
    /// 但保留这个方法是为了：万一将来上游数据变了，`load_level` 的告警能第一时间暴露。
    pub fn find_all(&self, key: &str) -> Vec<&LevelSummary> {
        self.summaries
            .iter()
            .filter(|s| s.key.matches(key))
            .collect()
    }

    /// 查找并加载关卡地图。
    pub fn load_level(&self, key: &str) -> Result<Level, LevelError> {
        let matches = self.find_all(key);
        let summary = *matches
            .first()
            .ok_or_else(|| LevelError::UnknownStage(key.to_owned()))?;
        if matches.len() > 1 {
            let ids: Vec<&str> = matches.iter().map(|s| s.key.stage_id.as_str()).collect();
            log::warn!(
                "stage key '{key}' matched {} entries {ids:?}; using '{}'. \
                 Known-benign for #f# variants (identical maps); if the projection looks \
                 wrong, pin the exact stageId in the job file",
                matches.len(),
                summary.key.stage_id
            );
        } else {
            log::info!("stage '{key}' resolved to '{}'", summary.key.stage_id);
        }
        let path = self.dir.join(&summary.filename);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| LevelError::Io(path.display().to_string(), e.to_string()))?;
        Level::from_json(&text)
    }

    /// 模糊搜索，供 UI 里的关卡选择器用。
    pub fn search(&self, needle: &str, limit: usize) -> Vec<&LevelSummary> {
        let needle = needle.to_lowercase();
        self.summaries
            .iter()
            .filter(|s| {
                s.key.stage_id.to_lowercase().contains(&needle)
                    || s.key.code.to_lowercase().contains(&needle)
                    || s.key.name.to_lowercase().contains(&needle)
            })
            .take(limit)
            .collect()
    }
}

/// 环境变量：直接指定 `Arknights-Tile-Pos` 目录。
pub const TILE_POS_DIR_ENV: &str = "REPLICATOR_TILE_POS_DIR";
/// 环境变量：指定 MAA 的 `resource` 目录（会自动往下找 `Arknights-Tile-Pos`）。
pub const MAA_RESOURCE_ENV: &str = "REPLICATOR_MAA_RESOURCE";

/// 自动定位 `Arknights-Tile-Pos` 目录。
///
/// 本仓库不分发关卡数据，所以必须找到用户自己的 MAA 资源目录。按以下顺序尝试：
///
/// 1. `REPLICATOR_TILE_POS_DIR` 环境变量
/// 2. `REPLICATOR_MAA_RESOURCE` 环境变量下的 `Arknights-Tile-Pos`
/// 3. 当前目录及其上溯各级目录里的 `MaaAssistantArknights/resource/Arknights-Tile-Pos`
///    （开发时三个参考项目并排放着，这条就能命中）
///
/// 找不到返回 `None`，由调用方提示用户手动指定。
pub fn locate_tile_pos_dir() -> Option<PathBuf> {
    fn valid(dir: PathBuf) -> Option<PathBuf> {
        dir.join("overview.json").is_file().then_some(dir)
    }

    if let Ok(dir) = std::env::var(TILE_POS_DIR_ENV) {
        if let Some(found) = valid(PathBuf::from(dir)) {
            return Some(found);
        }
    }
    if let Ok(dir) = std::env::var(MAA_RESOURCE_ENV) {
        if let Some(found) = valid(PathBuf::from(dir).join("Arknights-Tile-Pos")) {
            return Some(found);
        }
    }

    let mut cursor = std::env::current_dir().ok();
    while let Some(dir) = cursor {
        let candidate = dir
            .join("MaaAssistantArknights")
            .join("resource")
            .join("Arknights-Tile-Pos");
        if let Some(found) = valid(candidate) {
            return Some(found);
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    None
}

#[derive(Debug, thiserror::Error)]
pub enum LevelError {
    #[error("读取 {0} 失败：{1}")]
    Io(String, String),
    #[error("解析关卡 JSON 失败：{0}")]
    Parse(#[from] serde_json::Error),
    #[error("关卡数据格式不对：{0}")]
    Malformed(String),
    #[error("在 Arknights-Tile-Pos 索引里找不到关卡「{0}」")]
    UnknownStage(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一张 3×2 的小地图，字段与真实关卡 JSON 同构。
    const TINY: &str = r#"{
        "stageId": "test_01",
        "code": "TT-1",
        "levelId": "activities/test/level_test_01",
        "name": "测试关",
        "width": 3,
        "height": 2,
        "view": [[0.0, -4.2, 5.8], [-1.3, -3.0, 5.0]],
        "tiles": [
            [{"heightType":1,"buildableType":2,"tileKey":"tile_wall"},
             {"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":0,"tileKey":"tile_start"}],
            [{"heightType":0,"buildableType":1,"tileKey":"tile_road"},
             {"heightType":0,"buildableType":0,"tileKey":"tile_end"},
             {"heightType":1,"buildableType":0,"tileKey":"tile_forbidden"}]
        ]
    }"#;

    #[test]
    fn parses_a_level() {
        let level = Level::from_json(TINY).unwrap();
        assert_eq!(level.width(), 3);
        assert_eq!(level.height(), 2);
        assert_eq!(level.key.code, "TT-1");
        assert_eq!(level.view.len(), 2);

        let t = level.tile(Point::new(0, 0)).unwrap();
        // tile_wall（高台）的 heightType 是 1 —— 与 MAA 枚举名相反，见字段文档
        assert_eq!(t.height_type, 1);
        assert_eq!(t.buildable, Buildable::Ranged);
        assert_eq!(t.key, TileKey::Wall);

        let t = level.tile(Point::new(1, 1)).unwrap();
        assert_eq!(t.key, TileKey::Home);
        assert!(t.key.is_gate());

        assert!(level.tile(Point::new(3, 0)).is_none());
        assert!(level.tile(Point::new(-1, 0)).is_none());
    }

    #[test]
    fn locations_covers_every_cell() {
        let level = Level::from_json(TINY).unwrap();
        let all: Vec<_> = level.locations().collect();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0], Point::new(0, 0));
        assert_eq!(all[5], Point::new(2, 1));
        assert!(all.iter().all(|p| level.tile(*p).is_some()));
    }

    #[test]
    fn level_key_matches_any_field() {
        let level = Level::from_json(TINY).unwrap();
        assert!(level.key.matches("test_01"));
        assert!(level.key.matches("TT-1"));
        assert!(level.key.matches("activities/test/level_test_01"));
        assert!(level.key.matches("测试关"));
        assert!(!level.key.matches("TT-2"));
        // 空串不能匹配任何东西，否则会把带空字段的关卡全命中
        assert!(!level.key.matches(""));
    }

    #[test]
    fn tile_keys_cover_maa_mapping() {
        assert_eq!(TileKey::parse("tile_road"), TileKey::Road);
        assert_eq!(TileKey::parse("tile_wall"), TileKey::Wall);
        assert_eq!(TileKey::parse("tile_end"), TileKey::Home);
        assert_eq!(TileKey::parse("tile_start"), TileKey::EnemyHome);
        assert_eq!(TileKey::parse("tile_deepwater"), TileKey::DeepSea);
        assert_eq!(TileKey::parse("tile_deepsea"), TileKey::DeepSea);
        assert_eq!(TileKey::parse("tile_fence_bound"), TileKey::Fence);
        assert_eq!(TileKey::parse("tile_who_knows"), TileKey::Invalid);
    }

    #[test]
    fn malformed_levels_are_rejected() {
        // tiles 行数和 height 对不上
        let bad = TINY.replace("\"height\": 2", "\"height\": 3");
        assert!(matches!(
            Level::from_json(&bad),
            Err(LevelError::Malformed(_))
        ));
        // view 只有一项（缺侧视角）
        let bad = TINY.replace(
            "[[0.0, -4.2, 5.8], [-1.3, -3.0, 5.0]]",
            "[[0.0, -4.2, 5.8]]",
        );
        assert!(matches!(
            Level::from_json(&bad),
            Err(LevelError::Malformed(_))
        ));
    }

    #[test]
    fn overview_index_lookup() {
        let overview = r#"{
            "a001_01-activities/a001/level_a001_01": {
                "code": "GT-1",
                "filename": "a001_01-activities-a001-level_a001_01.json",
                "height": 7,
                "levelId": "activities/a001/level_a001_01",
                "name": "日正当中",
                "stageId": "a001_01",
                "width": 10
            },
            "main_10-07": {
                "code": "10-7",
                "filename": "main_10-07.json",
                "height": 9,
                "levelId": "obt/main/level_main_10-07",
                "name": "夜风",
                "stageId": "main_10-07",
                "width": 13
            }
        }"#;
        let pack = LevelPack::from_overview_json(PathBuf::from("."), overview).unwrap();
        assert_eq!(pack.len(), 2);
        assert_eq!(pack.find("GT-1").unwrap().key.stage_id, "a001_01");
        assert_eq!(pack.find("10-7").unwrap().key.stage_id, "main_10-07");
        assert_eq!(pack.find("夜风").unwrap().key.code, "10-7");
        assert_eq!(
            pack.find("obt/main/level_main_10-07").unwrap().key.code,
            "10-7"
        );
        assert!(pack.find("nope").is_none());

        let hits = pack.search("gt", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key.code, "GT-1");
    }

    #[test]
    fn ambiguous_stage_keys_are_all_reported() {
        // 主线关几乎都有一个 #f# 变体，两条索引的 code 相同。
        // find 取第一个，但 find_all 必须能列出全部，供 load_level 发告警。
        let overview = r#"{
            "main_01-07#f#-obt/main/level_main_01-07": {
                "code": "1-7", "filename": "a.json", "height": 7, "width": 11,
                "levelId": "obt/main/level_main_01-07", "name": "暴君",
                "stageId": "main_01-07#f#"
            },
            "main_01-07-obt/main/level_main_01-07": {
                "code": "1-7", "filename": "b.json", "height": 7, "width": 11,
                "levelId": "obt/main/level_main_01-07", "name": "暴君",
                "stageId": "main_01-07"
            }
        }"#;
        let pack = LevelPack::from_overview_json(PathBuf::from("."), overview).unwrap();

        // 按关卡号查 => 两条都命中
        assert_eq!(pack.find_all("1-7").len(), 2);
        assert!(pack.find("1-7").is_some());

        // 按精确 stageId 查 => 只命中一条，可以用来消除歧义
        let exact = pack.find_all("main_01-07");
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].filename, "b.json");
        let variant = pack.find_all("main_01-07#f#");
        assert_eq!(variant.len(), 1);
        assert_eq!(variant[0].filename, "a.json");
    }

    #[test]
    fn empty_overview_is_rejected() {
        assert!(matches!(
            LevelPack::from_overview_json(PathBuf::from("."), "{}"),
            Err(LevelError::Malformed(_))
        ));
    }
}
