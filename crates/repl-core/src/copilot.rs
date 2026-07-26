// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Config/Miscellaneous/CopilotConfig.cpp —— 字段名与别名表
//   src/MaaCore/Common/AsstBattleDef.h —— battle::copilot::Action / CombatData
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 作业文件。
//!
//! 格式是 **MAA copilot schema 的超集** —— 现成的 MAA 作业可以直接导入，
//! 补上帧号即可。只新增两处：
//!
//! - 顶层 `frame_replicator` 块（标记这是帧复刻作业）
//! - 每个动作的 `frame` 字段（绝对逻辑帧）
//!
//! MAA 原有的 `kills` / `costs` / `cost_changes` / `cooling` / `pre_delay` /
//! `post_delay` 会被解析但**忽略** —— 帧模式下等待条件只看帧号。保留解析是为了
//! 让同一个文件在 MAA 里也能用。

use serde::{Deserialize, Serialize};

use crate::geom::{Direction, Point};

/// 最后一个动作执行完之后怎么办。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterLastAction {
    /// 恢复运行，让战斗自己打完。
    #[default]
    Resume,
    /// 保持暂停，交回给用户。
    Pause,
}

/// 帧复刻元数据。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FrameReplicatorMeta {
    #[serde(default = "default_version")]
    pub version: u32,
    /// 复刻期间要求的倍速。首版只支持 `"1x"`。
    #[serde(default = "default_speed")]
    pub speed: String,
    #[serde(default)]
    pub after_last_action: AfterLastAction,
}

fn default_version() -> u32 {
    1
}

fn default_speed() -> String {
    "1x".to_owned()
}

impl Default for FrameReplicatorMeta {
    fn default() -> Self {
        Self {
            version: default_version(),
            speed: default_speed(),
            after_last_action: AfterLastAction::default(),
        }
    }
}

/// 动作类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionType {
    /// 部署干员
    Deploy,
    /// 开技能
    UseSkill,
    /// 撤退干员
    Retreat,
    /// 仅注释，不产生任何输入
    Output,
    // ——— 以下是 MAA 有、但帧复刻模式不支持的类型 ———
    /// 切换二倍速
    SwitchSpeed,
    /// 子弹时间（0.2 倍速）
    BulletTime,
    /// 技能用法设置
    SkillUsage,
    /// 摆完挂机
    SkillDaemon,
    /// 移动镜头
    MoveCamera,
    /// 保全派驻的调配干员
    DrawCard,
    /// 保全派驻的检查重开
    CheckIfStartOver,
}

impl ActionType {
    /// 帧复刻模式是否支持这个动作。
    ///
    /// 变速类动作被明确拒绝：脉冲时长、尺子的帧率假设、乃至"一帧多久"这件事
    /// 本身都会跟着变，首版不冒这个险。
    pub fn supported_in_frame_mode(self) -> bool {
        matches!(
            self,
            Self::Deploy | Self::UseSkill | Self::Retreat | Self::Output
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "Deploy",
            Self::UseSkill => "Skill",
            Self::Retreat => "Retreat",
            Self::Output => "Output",
            Self::SwitchSpeed => "SpeedUp",
            Self::BulletTime => "BulletTime",
            Self::SkillUsage => "SkillUsage",
            Self::SkillDaemon => "SkillDaemon",
            Self::MoveCamera => "MoveCamera",
            Self::DrawCard => "DrawCard",
            Self::CheckIfStartOver => "CheckIfStartOver",
        }
    }

    /// 解析 MAA 的动作类型字符串。别名表照抄上游，中英大小写各种写法都认。
    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.to_ascii_lowercase();
        let action = match normalized.as_str() {
            "deploy" | "部署" => Self::Deploy,
            "skill" | "技能" => Self::UseSkill,
            "retreat" | "撤退" => Self::Retreat,
            "output" | "输出" | "打印" => Self::Output,
            "speedup" | "二倍速" => Self::SwitchSpeed,
            "bullettime" | "子弹时间" => Self::BulletTime,
            "skillusage" | "技能用法" => Self::SkillUsage,
            "skilldaemon" | "donothing" | "摆完挂机" | "开摆" => Self::SkillDaemon,
            "movecamera" | "移动镜头" => Self::MoveCamera,
            "drawcard" | "抽卡" | "抽牌" | "调配" | "调配干员" => Self::DrawCard,
            "checkifstartover" | "检查重开" => Self::CheckIfStartOver,
            _ => return None,
        };
        Some(action)
    }
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条动作。
#[derive(Clone, Debug)]
pub struct Action {
    /// **绝对逻辑帧** —— 帧复刻模式下唯一的等待条件。
    pub frame: i64,
    pub kind: ActionType,
    /// 目标干员名（对应作业 `opers` 里的名字）。
    pub name: String,
    /// 格子坐标。`Deploy` 必填；`Skill`/`Retreat` 可以只给 name。
    pub location: Option<Point>,
    pub direction: Direction,
    /// 注释，显示在时间轴上。
    pub doc: String,
}

/// 作业里声明的一个干员。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Oper {
    pub name: String,
    /// 技能序号 0–3，0 表示用默认/上次的技能。帧复刻模式下只作展示，
    /// 因为技能是由用户在编队界面自己选的。
    #[serde(default)]
    pub skill: u8,
}

/// 一份解析并校验过的帧复刻作业。
#[derive(Clone, Debug)]
pub struct Copilot {
    /// 关卡标识（stageId / code / levelId / name 任一）。
    pub stage_name: String,
    pub meta: FrameReplicatorMeta,
    pub opers: Vec<Oper>,
    pub actions: Vec<Action>,
    pub title: String,
    pub details: String,
}

impl Copilot {
    /// 解析并校验。
    pub fn parse(json: &str) -> Result<Self, CopilotError> {
        let raw: serde_json::Value = serde_json::from_str(json)?;

        let stage_name = raw
            .get("stage_name")
            .and_then(serde_json::Value::as_str)
            .ok_or(CopilotError::MissingField("stage_name"))?
            .to_owned();
        if stage_name.trim().is_empty() {
            return Err(CopilotError::MissingField("stage_name"));
        }

        let meta = match raw.get("frame_replicator") {
            None => return Err(CopilotError::NotAFrameReplicatorJob),
            Some(serde_json::Value::Object(_)) => {
                serde_json::from_value(raw["frame_replicator"].clone())?
            }
            // 允许写成 `"frame_replicator": true` 的简写
            Some(_) => FrameReplicatorMeta::default(),
        };
        if meta.speed != "1x" {
            return Err(CopilotError::UnsupportedSpeed(meta.speed));
        }

        let opers: Vec<Oper> = match raw.get("opers") {
            Some(v) => serde_json::from_value(v.clone())?,
            None => Vec::new(),
        };

        let actions_raw = raw
            .get("actions")
            .and_then(serde_json::Value::as_array)
            .ok_or(CopilotError::MissingField("actions"))?;

        let mut actions = Vec::with_capacity(actions_raw.len());
        for (index, item) in actions_raw.iter().enumerate() {
            actions.push(parse_action(item, index)?);
        }
        if actions.is_empty() {
            return Err(CopilotError::NoActions);
        }

        let copilot = Self {
            stage_name,
            meta,
            opers,
            actions,
            title: raw
                .pointer("/doc/title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            details: raw
                .pointer("/doc/details")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        };
        copilot.validate()?;
        Ok(copilot)
    }

    fn validate(&self) -> Result<(), CopilotError> {
        let mut previous = -1_i64;
        for (index, action) in self.actions.iter().enumerate() {
            if !action.kind.supported_in_frame_mode() {
                return Err(CopilotError::UnsupportedAction {
                    index,
                    kind: action.kind,
                });
            }
            if action.frame < 0 {
                return Err(CopilotError::MissingFrame { index });
            }
            // 允许相等 —— 同一帧可以有多个动作，按数组顺序依次注入。
            if action.frame < previous {
                return Err(CopilotError::FramesOutOfOrder {
                    index,
                    previous,
                    current: action.frame,
                });
            }
            previous = action.frame;

            if action.kind == ActionType::Deploy {
                if action.location.is_none() {
                    return Err(CopilotError::DeployWithoutLocation { index });
                }
                if action.name.trim().is_empty() {
                    return Err(CopilotError::DeployWithoutName { index });
                }
            }
            if matches!(action.kind, ActionType::UseSkill | ActionType::Retreat)
                && action.name.trim().is_empty()
                && action.location.is_none()
            {
                return Err(CopilotError::TargetlessAction {
                    index,
                    kind: action.kind,
                });
            }
        }

        // 动作里引用的干员必须在 opers 里声明过，否则编队绑定时无从下手。
        if !self.opers.is_empty() {
            for (index, action) in self.actions.iter().enumerate() {
                if action.name.is_empty() {
                    continue;
                }
                if !self.opers.iter().any(|o| o.name == action.name) {
                    return Err(CopilotError::UndeclaredOper {
                        index,
                        name: action.name.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    /// 作业中出现的全部干员名（按首次出现顺序）。
    pub fn oper_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.opers.iter().map(|o| o.name.clone()).collect();
        for action in &self.actions {
            if !action.name.is_empty() && !names.contains(&action.name) {
                names.push(action.name.clone());
            }
        }
        names
    }

    /// 最后一个动作的帧号。
    pub fn last_frame(&self) -> i64 {
        self.actions.last().map_or(0, |a| a.frame)
    }
}

fn parse_action(value: &serde_json::Value, index: usize) -> Result<Action, CopilotError> {
    let type_str = value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Deploy");
    let kind = ActionType::parse(type_str).ok_or_else(|| CopilotError::UnknownActionType {
        index,
        raw: type_str.to_owned(),
    })?;

    // 缺 frame 字段时给 -1，交由 validate 报一个更清楚的错。
    let frame = value
        .get("frame")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(-1);

    let location = value
        .get("location")
        .and_then(serde_json::Value::as_array)
        .and_then(|arr| {
            let x = arr.first()?.as_i64()?;
            let y = arr.get(1)?.as_i64()?;
            Some(Point::new(x as i32, y as i32))
        });

    let direction = value
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .map_or(Direction::Right, parse_direction);

    Ok(Action {
        frame,
        kind,
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        location,
        direction,
        doc: value
            .get("doc")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// 解析朝向。别名表照抄 MAA，认不出的一律当 `Right`（与上游一致）。
fn parse_direction(raw: &str) -> Direction {
    match raw.to_ascii_lowercase().as_str() {
        "left" | "左" => Direction::Left,
        "up" | "上" => Direction::Up,
        "down" | "下" => Direction::Down,
        "none" | "无" => Direction::None,
        _ => Direction::Right,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CopilotError {
    #[error("作业 JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("作业缺少必填字段 `{0}`")]
    MissingField(&'static str),
    #[error("这不是帧复刻作业：缺少顶层 `frame_replicator` 字段。普通 MAA 作业需要先给每个动作补上 `frame`")]
    NotAFrameReplicatorJob,
    #[error("暂不支持 `{0}` 倍速，首版只支持 1x")]
    UnsupportedSpeed(String),
    #[error("作业里一个动作都没有")]
    NoActions,
    #[error("第 {index} 个动作的类型 `{raw}` 无法识别")]
    UnknownActionType { index: usize, raw: String },
    #[error("第 {index} 个动作 `{kind}` 在帧复刻模式下不支持（变速类动作会让帧率假设失效）")]
    UnsupportedAction { index: usize, kind: ActionType },
    #[error("第 {index} 个动作缺少 `frame`（帧复刻模式下每个动作都必须指定绝对逻辑帧）")]
    MissingFrame { index: usize },
    #[error("第 {index} 个动作的帧号倒退了：上一个是 {previous}，这个是 {current}。动作必须按帧号非递减排列")]
    FramesOutOfOrder {
        index: usize,
        previous: i64,
        current: i64,
    },
    #[error("第 {index} 个动作是部署，但没有给 `location`")]
    DeployWithoutLocation { index: usize },
    #[error("第 {index} 个动作是部署，但没有给 `name`")]
    DeployWithoutName { index: usize },
    #[error("第 {index} 个动作 `{kind}` 既没有 `name` 也没有 `location`，不知道该对谁生效")]
    TargetlessAction { index: usize, kind: ActionType },
    #[error("第 {index} 个动作引用了未在 `opers` 里声明的干员 `{name}`")]
    UndeclaredOper { index: usize, name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "stage_name": "main_10-07",
        "frame_replicator": { "version": 1, "speed": "1x", "after_last_action": "resume" },
        "doc": { "title": "测试作业", "details": "一句说明" },
        "opers": [ { "name": "山", "skill": 2 }, { "name": "史尔特尔", "skill": 3 } ],
        "actions": [
            { "type": "Deploy",  "frame": 0,   "name": "山", "location": [4, 2], "direction": "Right", "doc": "开局挡路" },
            { "type": "Skill",   "frame": 37,  "name": "山" },
            { "type": "Deploy",  "frame": 90,  "name": "史尔特尔", "location": [6, 2], "direction": "左" },
            { "type": "Retreat", "frame": 240, "name": "山" }
        ]
    }"#;

    #[test]
    fn parses_a_valid_job() {
        let c = Copilot::parse(GOOD).unwrap();
        assert_eq!(c.stage_name, "main_10-07");
        assert_eq!(c.meta.version, 1);
        assert_eq!(c.meta.after_last_action, AfterLastAction::Resume);
        assert_eq!(c.title, "测试作业");
        assert_eq!(c.opers.len(), 2);
        assert_eq!(c.actions.len(), 4);
        assert_eq!(c.last_frame(), 240);

        assert_eq!(c.actions[0].kind, ActionType::Deploy);
        assert_eq!(c.actions[0].frame, 0);
        assert_eq!(c.actions[0].location, Some(Point::new(4, 2)));
        assert_eq!(c.actions[0].direction, Direction::Right);
        assert_eq!(c.actions[0].doc, "开局挡路");

        // 中文朝向别名
        assert_eq!(c.actions[2].direction, Direction::Left);
        // Skill 只给 name 也合法
        assert_eq!(c.actions[1].kind, ActionType::UseSkill);
        assert!(c.actions[1].location.is_none());
    }

    #[test]
    fn action_type_aliases_match_maa() {
        for (raw, expected) in [
            ("Deploy", ActionType::Deploy),
            ("DEPLOY", ActionType::Deploy),
            ("部署", ActionType::Deploy),
            ("Skill", ActionType::UseSkill),
            ("技能", ActionType::UseSkill),
            ("retreat", ActionType::Retreat),
            ("撤退", ActionType::Retreat),
            ("SpeedUp", ActionType::SwitchSpeed),
            ("二倍速", ActionType::SwitchSpeed),
            ("BulletTime", ActionType::BulletTime),
            ("SkillDaemon", ActionType::SkillDaemon),
            ("DoNothing", ActionType::SkillDaemon),
            ("开摆", ActionType::SkillDaemon),
        ] {
            assert_eq!(ActionType::parse(raw), Some(expected), "别名 {raw} 没对上");
        }
        assert_eq!(ActionType::parse("Teleport"), None);
    }

    #[test]
    fn only_frame_safe_actions_are_supported() {
        assert!(ActionType::Deploy.supported_in_frame_mode());
        assert!(ActionType::UseSkill.supported_in_frame_mode());
        assert!(ActionType::Retreat.supported_in_frame_mode());
        assert!(ActionType::Output.supported_in_frame_mode());
        // 变速类必须被拒
        assert!(!ActionType::SwitchSpeed.supported_in_frame_mode());
        assert!(!ActionType::BulletTime.supported_in_frame_mode());
        assert!(!ActionType::SkillDaemon.supported_in_frame_mode());
    }

    #[test]
    fn same_frame_actions_are_allowed() {
        let json = GOOD.replace(r#""frame": 37"#, r#""frame": 0"#);
        let c = Copilot::parse(&json).unwrap();
        assert_eq!(c.actions[0].frame, 0);
        assert_eq!(c.actions[1].frame, 0);
    }

    #[test]
    fn frames_must_not_go_backwards() {
        let json = GOOD.replace(r#""frame": 90"#, r#""frame": 10"#);
        let err = Copilot::parse(&json).unwrap_err();
        assert!(
            matches!(err, CopilotError::FramesOutOfOrder { index: 2, .. }),
            "实际 {err}"
        );
    }

    #[test]
    fn missing_frame_is_rejected_with_a_useful_message() {
        let json = GOOD.replace(r#""frame": 37, "#, "");
        let err = Copilot::parse(&json).unwrap_err();
        assert!(matches!(err, CopilotError::MissingFrame { index: 1 }));
        assert!(err.to_string().contains("绝对逻辑帧"));
    }

    #[test]
    fn plain_maa_job_is_rejected_clearly() {
        let maa = r#"{
            "stage_name": "1-7",
            "opers": [{"name": "山"}],
            "actions": [{"type":"Deploy","name":"山","location":[3,2],"kills":0}]
        }"#;
        let err = Copilot::parse(maa).unwrap_err();
        assert!(matches!(err, CopilotError::NotAFrameReplicatorJob));
        // 报错要告诉用户怎么改
        assert!(err.to_string().contains("frame"));
    }

    #[test]
    fn speed_change_actions_are_rejected() {
        let json = r#"{
            "stage_name": "1-7",
            "frame_replicator": {},
            "actions": [
                {"type":"Deploy","frame":0,"name":"山","location":[3,2]},
                {"type":"SpeedUp","frame":10}
            ]
        }"#;
        let err = Copilot::parse(json).unwrap_err();
        assert!(
            matches!(
                err,
                CopilotError::UnsupportedAction {
                    kind: ActionType::SwitchSpeed,
                    ..
                }
            ),
            "实际 {err}"
        );
    }

    #[test]
    fn non_1x_speed_is_rejected() {
        let json = GOOD.replace(r#""speed": "1x""#, r#""speed": "2x""#);
        let err = Copilot::parse(&json).unwrap_err();
        assert!(matches!(err, CopilotError::UnsupportedSpeed(_)));
    }

    #[test]
    fn deploy_needs_a_location_and_a_name() {
        let json = GOOD.replace(r#", "location": [4, 2]"#, "");
        assert!(matches!(
            Copilot::parse(&json).unwrap_err(),
            CopilotError::DeployWithoutLocation { index: 0 }
        ));

        let json = GOOD.replace(
            r#""name": "山", "location": [4, 2]"#,
            r#""location": [4, 2]"#,
        );
        assert!(matches!(
            Copilot::parse(&json).unwrap_err(),
            CopilotError::DeployWithoutName { index: 0 }
        ));
    }

    #[test]
    fn undeclared_opers_are_caught_before_the_run_starts() {
        let json = GOOD.replace(
            r#""name": "史尔特尔", "location": [6, 2]"#,
            r#""name": "陈", "location": [6, 2]"#,
        );
        let err = Copilot::parse(&json).unwrap_err();
        let rendered = err.to_string();
        assert!(
            matches!(&err, CopilotError::UndeclaredOper { name, .. } if name == "陈"),
            "实际 {rendered}"
        );
    }

    #[test]
    fn empty_action_list_is_rejected() {
        let json = r#"{"stage_name":"1-7","frame_replicator":{},"actions":[]}"#;
        assert!(matches!(
            Copilot::parse(json).unwrap_err(),
            CopilotError::NoActions
        ));
    }

    #[test]
    fn shorthand_frame_replicator_true_works() {
        let json = r#"{
            "stage_name": "1-7",
            "frame_replicator": true,
            "actions": [{"type":"Deploy","frame":0,"name":"山","location":[3,2]}]
        }"#;
        let c = Copilot::parse(json).unwrap();
        assert_eq!(c.meta.speed, "1x");
        assert_eq!(c.meta.after_last_action, AfterLastAction::Resume);
    }

    #[test]
    fn oper_names_includes_action_only_targets() {
        let json = r#"{
            "stage_name": "1-7",
            "frame_replicator": {},
            "actions": [
                {"type":"Deploy","frame":0,"name":"山","location":[3,2]},
                {"type":"Deploy","frame":5,"name":"能天使","location":[4,1]}
            ]
        }"#;
        let c = Copilot::parse(json).unwrap();
        assert_eq!(c.oper_names(), vec!["山".to_owned(), "能天使".to_owned()]);
    }
}
