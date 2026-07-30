// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 配置与编队绑定的持久化。
//!
//! 两个文件都放在可执行文件所在目录（便携式，跟尺子的做法一致）：
//!
//! - `config.json` —— MAA 资源目录、旧脉冲兼容字段、免责声明是否已确认
//! - `bindings.json` —— 编队绑定档案，按干员头像的感知哈希索引

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = "config.json";
pub const BINDINGS_FILE: &str = "bindings.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// MAA 的 `resource` 目录。为空时启动向导会尝试自动定位。
    pub maa_resource_dir: String,
    /// 旧版 Rust 直接脉冲的初始间隔；保留用于兼容现有 config.json，主复刻不再使用。
    pub initial_gap_ms: u32,
    /// 用户是否已确认免责声明。
    pub disclaimer_accepted: bool,
    /// 尺子的 WebSocket 地址。
    pub ruler_ws_url: String,
    /// 游戏内 UI 缩放。为 `None` 时从注册表读取。
    pub ui_scaler_override: Option<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            maa_resource_dir: String::new(),
            initial_gap_ms: repl_core::stepping::DEFAULT_GAP_1X_MS,
            disclaimer_accepted: false,
            ruler_ws_url: repl_frames::DEFAULT_WS_URL.to_owned(),
            ui_scaler_override: None,
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let path = config_path(CONFIG_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(cfg) => {
                    log::info!("loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    // 配置坏了不该让程序起不来；用默认值继续，但要说清楚。
                    log::warn!(
                        "config at {} is malformed ({e}); using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path(CONFIG_FILE);
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)
    }

    /// 解析出 MAA 资源目录：优先用配置里的，否则自动定位。
    pub fn resolve_maa_resource_dir(&self) -> Option<PathBuf> {
        if !self.maa_resource_dir.is_empty() {
            let dir = PathBuf::from(&self.maa_resource_dir);
            if dir
                .join("Arknights-Tile-Pos")
                .join("overview.json")
                .is_file()
            {
                return Some(dir);
            }
            log::warn!(
                "configured MAA resource dir {} is not usable; falling back to auto-detect",
                dir.display()
            );
        }
        repl_vision::templates::locate_maa_resource_dir()
    }
}

/// 一个干员在部署栏上的绑定档案。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Binding {
    /// 干员名（作业里写的那个）。
    pub name: String,
    /// 绑定时卡片的左右序号，仅作提示。
    pub index: usize,
    /// 职业，用于绑定界面预匹配。
    pub role: String,
    /// 头像裁图的宽高。
    pub width: u32,
    pub height: u32,
    /// 头像裁图（BGRA），base64 存储。跨帧跟踪就靠它。
    pub avatar_base64: String,
}

/// 一套编队的绑定结果。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BindingProfile {
    /// 关卡标识，仅作展示。
    pub stage_name: String,
    pub bindings: Vec<Binding>,
}

/// 跨编队保存的最近一次干员头像；用于“继续”时恢复暂时不在部署栏的干员。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredAvatar {
    pub width: u32,
    pub height: u32,
    pub bgra_base64: String,
}

impl StoredAvatar {
    pub fn from_bgra(width: u32, height: u32, bgra: &[u8]) -> Self {
        Self {
            width,
            height,
            bgra_base64: STANDARD.encode(bgra),
        }
    }

    pub fn decode(&self) -> Result<Vec<u8>, base64::DecodeError> {
        STANDARD.decode(&self.bgra_base64)
    }
}

/// 所有已保存的绑定档案，按"编队指纹"索引。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct BindingStore {
    #[serde(default)]
    pub profiles: BTreeMap<String, BindingProfile>,
    #[serde(default)]
    pub avatars: BTreeMap<String, StoredAvatar>,
}

impl BindingStore {
    pub fn load() -> Self {
        let path = config_path(BINDINGS_FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("bindings file is malformed ({e}); starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path(BINDINGS_FILE);
        std::fs::write(&path, serde_json::to_string_pretty(self)?)
    }

    pub fn get(&self, fingerprint: &str) -> Option<&BindingProfile> {
        self.profiles.get(fingerprint)
    }

    pub fn put(&mut self, fingerprint: String, profile: BindingProfile) {
        self.profiles.insert(fingerprint, profile);
    }

    pub fn remember_avatar(
        &mut self,
        name: impl Into<String>,
        width: u32,
        height: u32,
        bgra: &[u8],
    ) {
        self.avatars
            .insert(name.into(), StoredAvatar::from_bgra(width, height, bgra));
    }

    pub fn avatar(&self, name: &str) -> Option<&StoredAvatar> {
        self.avatars.get(name)
    }
}

/// 一套部署栏的"指纹"。
///
/// 由每张卡片头像的感知哈希拼成。同一套编队再次进关时指纹一致，就能自动恢复绑定，
/// 不用让用户重新点一遍。
pub fn fingerprint(avatar_hashes: &[u64]) -> String {
    let mut sorted = avatar_hashes.to_vec();
    // 排序：卡片顺序会因冷却重排，但集合不变。
    sorted.sort_unstable();
    sorted
        .iter()
        .map(|h| format!("{h:016x}"))
        .collect::<Vec<_>>()
        .join("-")
}

/// 头像的感知哈希（8×8 均值哈希）。
///
/// 对轻微的亮度/压缩差异不敏感，足以认出"还是那个干员"。
/// 这里不用它做**判定**（判定走模板匹配），只用来查绑定档案。
pub fn perceptual_hash(bgra: &[u8], width: u32, height: u32) -> u64 {
    if width == 0 || height == 0 || bgra.len() < (width * height * 4) as usize {
        return 0;
    }
    // 缩到 8×8 灰度
    let mut cells = [0.0_f64; 64];
    for (cy, row) in cells.chunks_mut(8).enumerate() {
        for (cx, cell) in row.iter_mut().enumerate() {
            let x0 = cx as u32 * width / 8;
            let x1 = ((cx as u32 + 1) * width / 8).max(x0 + 1).min(width);
            let y0 = cy as u32 * height / 8;
            let y1 = ((cy as u32 + 1) * height / 8).max(y0 + 1).min(height);
            let mut sum = 0.0;
            let mut count = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = ((y * width + x) * 4) as usize;
                    let (b, g, r) = (
                        f64::from(bgra[i]),
                        f64::from(bgra[i + 1]),
                        f64::from(bgra[i + 2]),
                    );
                    sum += 0.114 * b + 0.587 * g + 0.299 * r;
                    count += 1.0;
                }
            }
            *cell = if count > 0.0 { sum / count } else { 0.0 };
        }
    }
    let mean = cells.iter().sum::<f64>() / 64.0;
    cells.iter().enumerate().fold(
        0_u64,
        |acc, (i, v)| if *v > mean { acc | (1 << i) } else { acc },
    )
}

/// 配置文件所在目录：可执行文件旁边；取不到就用当前工作目录。
fn config_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_path(file: &str) -> PathBuf {
    config_dir().join(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_avatar(seed: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16u32 {
            for x in 0..16u32 {
                let g = ((x * 8 + y * 4) as u8).wrapping_add(seed);
                v.extend_from_slice(&[g, g / 2, g / 3, 255]);
            }
        }
        v
    }

    #[test]
    fn perceptual_hash_is_stable_and_discriminating() {
        let a = gradient_avatar(0);
        let b = gradient_avatar(0);
        let c = gradient_avatar(200);
        let ha = perceptual_hash(&a, 16, 16);
        assert_eq!(ha, perceptual_hash(&b, 16, 16), "同一张图哈希必须一致");
        assert_ne!(ha, 0, "有结构的图不该哈希成 0");
        let hc = perceptual_hash(&c, 16, 16);
        // 不同的图应当有不同的哈希（这里只要求不完全相等）
        assert_ne!(ha, hc);
    }

    #[test]
    fn perceptual_hash_survives_uniform_brightness_change() {
        // 均值哈希比的是"高于/低于自身均值"，整体调亮不该改变结果
        let base = gradient_avatar(0);
        let brighter: Vec<u8> = base
            .chunks(4)
            .flat_map(|px| {
                [
                    px[0].saturating_add(30),
                    px[1].saturating_add(30),
                    px[2].saturating_add(30),
                    255,
                ]
            })
            .collect();
        assert_eq!(
            perceptual_hash(&base, 16, 16),
            perceptual_hash(&brighter, 16, 16)
        );
    }

    #[test]
    fn perceptual_hash_handles_degenerate_input() {
        assert_eq!(perceptual_hash(&[], 0, 0), 0);
        assert_eq!(
            perceptual_hash(&[1, 2, 3, 4], 16, 16),
            0,
            "数据不足应返回 0"
        );
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let a = fingerprint(&[3, 1, 2]);
        let b = fingerprint(&[1, 2, 3]);
        assert_eq!(a, b, "卡片顺序变了（冷却重排）不该换指纹");
        assert_ne!(a, fingerprint(&[1, 2, 4]));
    }

    #[test]
    fn default_config_is_usable() {
        let c = Config::default();
        assert_eq!(c.initial_gap_ms, repl_core::stepping::DEFAULT_GAP_1X_MS);
        assert!(!c.disclaimer_accepted, "免责声明默认必须是未确认");
        assert!(c.ruler_ws_url.contains("2606"));
    }

    #[test]
    fn config_round_trips_through_json() {
        let c = Config {
            maa_resource_dir: "D:/maa/resource".into(),
            disclaimer_accepted: true,
            ui_scaler_override: Some(0.75),
            ..Config::default()
        };
        let text = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.maa_resource_dir, c.maa_resource_dir);
        assert!(back.disclaimer_accepted);
        assert_eq!(back.ui_scaler_override, Some(0.75));
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        // 老版本写的配置文件缺字段时不该解析失败
        let back: Config = serde_json::from_str(r#"{"initial_gap_ms": 25}"#).unwrap();
        assert_eq!(back.initial_gap_ms, 25);
        assert!(!back.disclaimer_accepted);
        assert!(back.ruler_ws_url.contains("2606"));
    }

    #[test]
    fn binding_store_round_trips() {
        let mut store = BindingStore::default();
        store.put(
            "abc".into(),
            BindingProfile {
                stage_name: "1-7".into(),
                bindings: vec![Binding {
                    name: "山".into(),
                    index: 0,
                    role: "近卫".into(),
                    width: 60,
                    height: 60,
                    avatar_base64: "AAAA".into(),
                }],
            },
        );
        store.remember_avatar("山", 16, 16, &[1, 2, 3, 4]);
        let text = serde_json::to_string(&store).unwrap();
        let back: BindingStore = serde_json::from_str(&text).unwrap();
        assert_eq!(back.get("abc").unwrap().bindings[0].name, "山");
        assert!(back.get("nope").is_none());
        assert_eq!(back.avatar("山").unwrap().decode().unwrap(), [1, 2, 3, 4]);
    }

    #[test]
    fn legacy_binding_store_without_avatar_library_still_loads() {
        let store: BindingStore = serde_json::from_str(r#"{"profiles":{}}"#).unwrap();
        assert!(store.avatars.is_empty());
    }
}
