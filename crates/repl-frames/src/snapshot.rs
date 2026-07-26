// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 尺子快照的数据模型。
//!
//! 字段名与语义来自 ArknightsCostBarRuler 的 `docs/API.md`（`apiVersion = 2`）。
//! 这里只描述协议，不包含上游代码。

use serde::Deserialize;

/// 本客户端针对的尺子 API 协议版本。收到更高版本会告警但继续工作
/// （上游承诺只新增字段、保留旧顶层字段）。
pub const EXPECTED_API_VERSION: u32 = 2;

/// 战斗状态。字符串取值见尺子 `analysis/scanner/mod.rs` 的 `BattleState::as_str`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BattleState {
    /// 0.2 倍速运行 —— 选中干员 / 拖拽待部署卡片时的慢放。
    PointTwoXRunning,
    OneXRunning,
    TwoXRunning,
    PointTwoXPaused,
    OneXPaused,
    TwoXPaused,
    /// 正在拖拽部署干员。
    DeployingOperator,
    /// 正在调整干员朝向。
    AdjustingOperatorFacing,
    /// 开局标题画面。尺子在此状态会把计时器归零。
    BattleBegin,
    /// 战斗前后（加载 / 结算）。
    BeforeOrAfterBattle,
    NotInBattle,
    /// 上游新增了我们不认识的状态，原样保留以便日志排查。
    Unknown(String),
}

impl BattleState {
    pub fn parse(s: &str) -> Self {
        match s {
            "0.2x_running" => Self::PointTwoXRunning,
            "1x_running" => Self::OneXRunning,
            "2x_running" => Self::TwoXRunning,
            "0.2x_paused" => Self::PointTwoXPaused,
            "1x_paused" => Self::OneXPaused,
            "2x_paused" => Self::TwoXPaused,
            "deploying_operator" => Self::DeployingOperator,
            "adjusting_operator_facing" => Self::AdjustingOperatorFacing,
            "battle_begin" => Self::BattleBegin,
            "before_or_after_battle" => Self::BeforeOrAfterBattle,
            "not_in_battle" => Self::NotInBattle,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// 是否处于"战斗进行中"（对应尺子的 `is_in_battle`）。
    /// 注意：暂停、部署慢放、调整朝向都算在战斗中。
    pub fn is_in_battle(&self) -> bool {
        matches!(
            self,
            Self::PointTwoXRunning
                | Self::OneXRunning
                | Self::TwoXRunning
                | Self::PointTwoXPaused
                | Self::OneXPaused
                | Self::TwoXPaused
                | Self::DeployingOperator
                | Self::AdjustingOperatorFacing
        )
    }

    /// 暂停按钮显示为"播放三角"，即时间被冻结。
    ///
    /// `DeployingOperator` / `AdjustingOperatorFacing` 时暂停按钮被遮挡，尺子无法
    /// 判断暂停与否，这里返回 `None` 表示"未知"。
    pub fn is_paused(&self) -> Option<bool> {
        match self {
            Self::PointTwoXPaused | Self::OneXPaused | Self::TwoXPaused => Some(true),
            Self::PointTwoXRunning | Self::OneXRunning | Self::TwoXRunning => Some(false),
            _ => None,
        }
    }

    /// 当前倍速档位；无法判断时返回 `None`。
    pub fn speed(&self) -> Option<repl_core::Speed> {
        use repl_core::Speed;
        match self {
            Self::OneXRunning | Self::OneXPaused => Some(Speed::One),
            Self::TwoXRunning | Self::TwoXPaused => Some(Speed::Two),
            Self::PointTwoXRunning | Self::PointTwoXPaused => Some(Speed::PointTwo),
            _ => None,
        }
    }

    /// 首版硬性要求 1 倍速。
    pub fn is_one_x(&self) -> bool {
        matches!(self, Self::OneXRunning | Self::OneXPaused)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::PointTwoXRunning => "0.2x_running",
            Self::OneXRunning => "1x_running",
            Self::TwoXRunning => "2x_running",
            Self::PointTwoXPaused => "0.2x_paused",
            Self::OneXPaused => "1x_paused",
            Self::TwoXPaused => "2x_paused",
            Self::DeployingOperator => "deploying_operator",
            Self::AdjustingOperatorFacing => "adjusting_operator_facing",
            Self::BattleBegin => "battle_begin",
            Self::BeforeOrAfterBattle => "before_or_after_battle",
            Self::NotInBattle => "not_in_battle",
            Self::Unknown(s) => s,
        }
    }
}

impl std::fmt::Display for BattleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 校准配置条目。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub filename: String,
    pub basename: String,
    #[serde(default)]
    pub total_frames: String,
    #[serde(default)]
    pub resolution: String,
    #[serde(default)]
    pub is_active: bool,
}

/// 一条尺子快照。
///
/// 只反序列化本项目实际用得到的字段；上游新增字段被忽略（`serde` 默认行为），
/// 缺失字段走 `Default`，这样尺子小版本升级不会把客户端打挂。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(default)]
    pub api_version: u32,
    #[serde(default)]
    pub app_version: String,

    /// 当前是否识别到有效费用条。
    #[serde(default)]
    pub is_running: bool,

    /// 当前费用循环内的原始逻辑帧。`None` 表示本帧未被采信（冻结）。
    #[serde(default)]
    pub current_frame: Option<i32>,

    /// **本场战斗累计逻辑帧 —— 这就是"绝对帧数"。** `battle_begin` 时归零。
    #[serde(default)]
    pub total_elapsed_frames: i64,

    #[serde(default)]
    pub total_frames_in_cycle: i32,

    /// 当前加载的校准配置基础名。为 `None` 时尺子无法计帧。
    #[serde(default)]
    pub active_profile: Option<String>,

    /// 截图管线的单调递增帧号。**用来判断"这是不是一条新样本"** ——
    /// 尺子只在状态变化时推送，暂停时同一份快照可能被反复读到。
    #[serde(default)]
    pub frame_id: Option<u64>,

    #[serde(default)]
    pub sample_index: u64,
    #[serde(default)]
    pub dropped_since_previous: u64,
    #[serde(default)]
    pub raw_pixel_width: Option<i32>,
    #[serde(default)]
    pub cost_is_negative: bool,

    #[serde(default, deserialize_with = "de_battle_state")]
    pub battle_state: Option<BattleState>,

    #[serde(default)]
    pub capture_width: Option<u32>,
    #[serde(default)]
    pub capture_height: Option<u32>,
    #[serde(default)]
    pub capture_timestamp_ns: Option<u64>,
    #[serde(default)]
    pub capture_duration_us: Option<u64>,

    /// PC 自绘光标是否遮挡费用条。为 `true` 的帧不会写入分析历史，
    /// **读数无效，不能当作"帧数没涨"**。
    #[serde(default)]
    pub cursor_blocked: bool,

    /// 已格式化计时器，`MM:SS:FF`。
    #[serde(default)]
    pub time: String,

    #[serde(default)]
    pub profiles: Vec<Profile>,
}

fn de_battle_state<'de, D>(de: D) -> Result<Option<BattleState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(de)?;
    Ok(raw.as_deref().map(BattleState::parse))
}

impl Snapshot {
    /// 这条快照的帧数读数是否可信。
    ///
    /// 三个条件缺一不可：识别到费用条、本帧被采信、光标没遮挡。三者任一不满足时
    /// `total_elapsed_frames` 保持的是**上一次**的值，把它当作"帧数没变"会误判。
    pub fn frame_reading_is_trustworthy(&self) -> bool {
        self.is_running && self.current_frame.is_some() && !self.cursor_blocked
    }

    /// 尺子是否已经加载了校准配置。没有配置就没有帧数。
    pub fn has_calibration(&self) -> bool {
        self.active_profile.is_some()
    }

    pub fn battle_state_str(&self) -> &str {
        self.battle_state.as_ref().map_or("<none>", |s| s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// API.md 里给出的示例快照，必须能原样解析。
    const SAMPLE: &str = r#"{
      "apiVersion": 2, "appVersion": "v2.1.0", "isRunning": true,
      "currentFrame": 15, "totalFramesInCycle": 30, "totalElapsedFrames": 75,
      "activeProfile": "正常回费", "frameId": 4242, "sampleIndex": 311,
      "droppedSincePrevious": 2, "rawPixelWidth": 123, "costIsNegative": false,
      "battleState": "battle", "captureWidth": 1280, "captureHeight": 720,
      "captureFormat": "rgba", "captureTimestampNs": 1234567890,
      "captureDurationUs": 1500, "cursorBlocked": false,
      "displayMode": "0_to_n-1", "displayFrame": "15", "displayTotal": "/29",
      "time": "00:02:15", "lapFrames": null, "canUndoReset": true,
      "profiles": [], "historyOldestFrameId": 4200, "historyLatestFrameId": 4242,
      "timingDebug": { "requiredFp": 16777216, "speedFp": 559240,
        "accumulatorFp": 8388608, "advancedFrames": 1, "framesSinceCycleStart": 15,
        "framesUntilNextCost": 15, "matchErrorPx": 0, "boundaryCorrected": false }
    }"#;

    #[test]
    fn parses_api_md_sample() {
        let s: Snapshot = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(s.api_version, 2);
        assert_eq!(s.total_elapsed_frames, 75);
        assert_eq!(s.frame_id, Some(4242));
        assert_eq!(s.current_frame, Some(15));
        assert_eq!(s.active_profile.as_deref(), Some("正常回费"));
        assert!(s.frame_reading_is_trustworthy());
        // 文档示例里的 "battle" 不在实际枚举里，必须落到 Unknown 而不是解析失败。
        assert_eq!(
            s.battle_state,
            Some(BattleState::Unknown("battle".to_owned()))
        );
    }

    #[test]
    fn parses_minimal_object() {
        let s: Snapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(s.total_elapsed_frames, 0);
        assert_eq!(s.frame_id, None);
        assert!(!s.has_calibration());
        assert!(!s.frame_reading_is_trustworthy());
    }

    #[test]
    fn real_battle_states_round_trip() {
        for raw in [
            "0.2x_running",
            "1x_running",
            "2x_running",
            "0.2x_paused",
            "1x_paused",
            "2x_paused",
            "deploying_operator",
            "adjusting_operator_facing",
            "battle_begin",
            "before_or_after_battle",
            "not_in_battle",
        ] {
            let st = BattleState::parse(raw);
            assert!(!matches!(st, BattleState::Unknown(_)), "{raw} unmapped");
            assert_eq!(st.as_str(), raw);
        }
    }

    #[test]
    fn battle_state_predicates() {
        assert!(BattleState::OneXPaused.is_in_battle());
        assert!(BattleState::DeployingOperator.is_in_battle());
        assert!(!BattleState::BattleBegin.is_in_battle());
        assert_eq!(BattleState::OneXPaused.is_paused(), Some(true));
        assert_eq!(BattleState::OneXRunning.is_paused(), Some(false));
        // 部署慢放时暂停按钮被遮挡，尺子给不出暂停与否。
        assert_eq!(BattleState::DeployingOperator.is_paused(), None);
        assert!(BattleState::OneXRunning.is_one_x());
        assert!(!BattleState::TwoXRunning.is_one_x());
    }

    #[test]
    fn cursor_blocked_reading_is_untrusted() {
        let s: Snapshot =
            serde_json::from_str(r#"{"isRunning":true,"currentFrame":3,"cursorBlocked":true}"#)
                .unwrap();
        assert!(!s.frame_reading_is_trustworthy());
    }
}
