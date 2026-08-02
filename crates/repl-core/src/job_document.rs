// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 可编辑的帧复刻作业文档。
//!
//! [`crate::Copilot`] 是严格、可运行的领域模型；本模块则保留原始 JSON，允许字段暂时缺失，
//! 供编辑器保存草稿。编辑完成后必须调用 [`JobDocument::compile`] 回到严格模型。

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::{ActionType, Copilot, Direction, Point};

/// 编辑会话内稳定的动作标识，不写入作业 JSON。
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActionId(u64);

impl ActionId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 属性面板使用的动作视图。未知字段仍保存在 [`JobDocument`] 的原始 JSON 中。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionDraft {
    pub id: ActionId,
    pub kind: String,
    pub frame: Option<i64>,
    pub name: String,
    pub location: Option<Point>,
    pub direction: Direction,
    pub doc: String,
}

impl ActionDraft {
    pub fn new(id: ActionId, kind: impl Into<String>, frame: Option<i64>) -> Self {
        Self {
            id,
            kind: kind.into(),
            frame,
            name: String::new(),
            location: None,
            direction: Direction::Right,
            doc: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub action_id: Option<ActionId>,
    pub field: String,
    pub message: String,
}

impl Diagnostic {
    fn error(action_id: Option<ActionId>, field: &str, message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            action_id,
            field: field.to_owned(),
            message: message.into(),
        }
    }

    fn warning(action_id: Option<ActionId>, field: &str, message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            action_id,
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JobDocumentError {
    #[error("作业 JSON 解析失败：{0}")]
    Json(#[from] serde_json::Error),
    #[error("作业 JSON 顶层必须是对象")]
    RootNotObject,
    #[error("找不到动作 {0}")]
    ActionNotFound(u64),
}

/// 保留原始 JSON 的编辑文档。
#[derive(Clone, Debug)]
pub struct JobDocument {
    root: Map<String, Value>,
    action_ids: Vec<ActionId>,
    next_action_id: u64,
}

impl Default for JobDocument {
    fn default() -> Self {
        let mut root = Map::new();
        root.insert("stage_name".into(), Value::String(String::new()));
        root.insert(
            "frame_replicator".into(),
            serde_json::json!({"version": 1, "speed": "1x", "after_last_action": "pause"}),
        );
        root.insert(
            "doc".into(),
            serde_json::json!({"title": "未命名作业", "details": ""}),
        );
        root.insert("opers".into(), Value::Array(Vec::new()));
        root.insert("actions".into(), Value::Array(Vec::new()));
        Self {
            root,
            action_ids: Vec::new(),
            next_action_id: 1,
        }
    }
}

impl JobDocument {
    pub fn parse(json: &str) -> Result<Self, JobDocumentError> {
        let value: Value = serde_json::from_str(json)?;
        let Value::Object(root) = value else {
            return Err(JobDocumentError::RootNotObject);
        };
        let action_count = root
            .get("actions")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let action_ids = (1..=action_count as u64).map(ActionId).collect();
        Ok(Self {
            root,
            action_ids,
            next_action_id: action_count as u64 + 1,
        })
    }

    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&Value::Object(self.root.clone()))
    }

    pub fn compile(&self) -> Result<Copilot, crate::CopilotError> {
        let json = serde_json::to_string(&self.root)?;
        Copilot::parse(&json)
    }

    pub fn root(&self) -> &Map<String, Value> {
        &self.root
    }

    pub fn stage_name(&self) -> String {
        string_field(&self.root, "stage_name")
    }

    pub fn set_stage_name(&mut self, stage_name: impl Into<String>) {
        self.root
            .insert("stage_name".into(), Value::String(stage_name.into()));
    }

    pub fn title(&self) -> String {
        self.doc_string("title")
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.set_doc_string("title", title.into());
    }

    pub fn details(&self) -> String {
        self.doc_string("details")
    }

    pub fn set_details(&mut self, details: impl Into<String>) {
        self.set_doc_string("details", details.into());
    }

    pub fn operator_names(&self) -> Vec<String> {
        self.root
            .get("opers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter_map(|oper| oper.get("name").and_then(Value::as_str))
            .map(str::to_owned)
            .collect()
    }

    pub fn add_operator(&mut self, name: impl Into<String>, skill: u8) {
        let opers = ensure_array(&mut self.root, "opers");
        opers.push(serde_json::json!({"name": name.into(), "skill": skill}));
    }

    pub fn rename_operator(&mut self, old_name: &str, new_name: &str) -> bool {
        let mut renamed = false;
        if let Some(opers) = self.root.get_mut("opers").and_then(Value::as_array_mut) {
            for oper in opers.iter_mut().filter_map(Value::as_object_mut) {
                if oper.get("name").and_then(Value::as_str) == Some(old_name) {
                    oper.insert("name".into(), Value::String(new_name.to_owned()));
                    renamed = true;
                }
            }
        }
        if renamed {
            if let Some(actions) = self.root.get_mut("actions").and_then(Value::as_array_mut) {
                for action in actions.iter_mut().filter_map(Value::as_object_mut) {
                    if action.get("name").and_then(Value::as_str) == Some(old_name) {
                        action.insert("name".into(), Value::String(new_name.to_owned()));
                    }
                }
            }
        }
        renamed
    }

    pub fn operator_reference_count(&self, name: &str) -> usize {
        self.root
            .get("actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .filter(|action| action.get("name").and_then(Value::as_str) == Some(name))
            .count()
    }

    pub fn remove_operator(&mut self, name: &str) -> bool {
        if self.operator_reference_count(name) > 0 {
            return false;
        }
        let Some(opers) = self.root.get_mut("opers").and_then(Value::as_array_mut) else {
            return false;
        };
        let old_len = opers.len();
        opers.retain(|oper| {
            oper.as_object()
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                != Some(name)
        });
        opers.len() != old_len
    }

    /// 召唤物和地图装置只供动作选择，不属于开局 `opers`。
    pub fn deferred_target_names(&self) -> Vec<String> {
        self.root
            .get("frame_replicator")
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("deferred_targets"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    pub fn target_names(&self) -> Vec<String> {
        let mut names = self.operator_names();
        for name in self.deferred_target_names() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    pub fn add_deferred_target(&mut self, name: impl Into<String>) {
        let targets = ensure_meta_array(&mut self.root, "deferred_targets");
        targets.push(Value::String(name.into()));
    }

    pub fn rename_deferred_target(&mut self, old_name: &str, new_name: &str) -> bool {
        let mut renamed = false;
        if let Some(targets) = self
            .root
            .get_mut("frame_replicator")
            .and_then(Value::as_object_mut)
            .and_then(|meta| meta.get_mut("deferred_targets"))
            .and_then(Value::as_array_mut)
        {
            for target in targets {
                if target.as_str() == Some(old_name) {
                    *target = Value::String(new_name.to_owned());
                    renamed = true;
                }
            }
        }
        if renamed {
            if let Some(actions) = self.root.get_mut("actions").and_then(Value::as_array_mut) {
                for action in actions.iter_mut().filter_map(Value::as_object_mut) {
                    if action.get("name").and_then(Value::as_str) == Some(old_name) {
                        action.insert("name".into(), Value::String(new_name.to_owned()));
                    }
                }
            }
        }
        renamed
    }

    pub fn remove_deferred_target(&mut self, name: &str) -> bool {
        if self.operator_reference_count(name) > 0 {
            return false;
        }
        let Some(targets) = self
            .root
            .get_mut("frame_replicator")
            .and_then(Value::as_object_mut)
            .and_then(|meta| meta.get_mut("deferred_targets"))
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let old_len = targets.len();
        targets.retain(|target| target.as_str() != Some(name));
        targets.len() != old_len
    }

    pub fn actions(&self) -> Vec<ActionDraft> {
        self.root
            .get("actions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, value)| self.action_from_value(index, value))
            .collect()
    }

    pub fn action(&self, id: ActionId) -> Option<ActionDraft> {
        let index = self
            .action_ids
            .iter()
            .position(|candidate| *candidate == id)?;
        let value = self
            .root
            .get("actions")
            .and_then(Value::as_array)?
            .get(index)?;
        Some(self.action_from_value(index, value))
    }

    pub fn new_action(&mut self, kind: impl Into<String>, frame: Option<i64>) -> ActionDraft {
        let id = ActionId(self.next_action_id);
        self.next_action_id += 1;
        ActionDraft::new(id, kind, frame)
    }

    pub fn insert_action(&mut self, draft: ActionDraft) {
        let value = action_to_value(&draft, None);
        let actions = ensure_array(&mut self.root, "actions");
        actions.push(value);
        self.action_ids.push(draft.id);
        self.sort_actions_stably();
    }

    pub fn update_action(&mut self, draft: &ActionDraft) -> Result<(), JobDocumentError> {
        let index = self
            .action_ids
            .iter()
            .position(|candidate| *candidate == draft.id)
            .ok_or(JobDocumentError::ActionNotFound(draft.id.get()))?;
        let actions = ensure_array(&mut self.root, "actions");
        let existing = actions
            .get(index)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        actions[index] = action_to_value(draft, Some(existing));
        self.sort_actions_stably();
        Ok(())
    }

    pub fn remove_action(&mut self, id: ActionId) -> Option<ActionDraft> {
        let index = self
            .action_ids
            .iter()
            .position(|candidate| *candidate == id)?;
        let draft = self.action(id)?;
        let actions = self.root.get_mut("actions")?.as_array_mut()?;
        actions.remove(index);
        self.action_ids.remove(index);
        Some(draft)
    }

    pub fn duplicate_action(&mut self, id: ActionId) -> Option<ActionId> {
        let index = self
            .action_ids
            .iter()
            .position(|candidate| *candidate == id)?;
        let value = self.root.get("actions")?.as_array()?.get(index)?.clone();
        let new_id = ActionId(self.next_action_id);
        self.next_action_id += 1;
        let actions = ensure_array(&mut self.root, "actions");
        actions.insert(index + 1, value);
        self.action_ids.insert(index + 1, new_id);
        self.sort_actions_stably();
        Some(new_id)
    }

    pub fn move_within_same_frame(&mut self, id: ActionId, delta: isize) -> bool {
        let Some(index) = self
            .action_ids
            .iter()
            .position(|candidate| *candidate == id)
        else {
            return false;
        };
        let actions = self
            .root
            .get_mut("actions")
            .and_then(Value::as_array_mut)
            .expect("action ids only exist for an actions array");
        let frame = action_frame(&actions[index]);
        let target = index as isize + delta;
        if target < 0 || target >= actions.len() as isize {
            return false;
        }
        let target = target as usize;
        if action_frame(&actions[target]) != frame {
            return false;
        }
        actions.swap(index, target);
        self.action_ids.swap(index, target);
        true
    }

    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        if self.stage_name().trim().is_empty() {
            diagnostics.push(Diagnostic::error(None, "stage_name", "作业缺少关卡标识"));
        }
        if !self.root.contains_key("frame_replicator") {
            diagnostics.push(Diagnostic::error(
                None,
                "frame_replicator",
                "作业缺少 frame_replicator 标记",
            ));
        } else if self
            .root
            .get("frame_replicator")
            .and_then(Value::as_object)
            .and_then(|meta| meta.get("speed"))
            .and_then(Value::as_str)
            .is_some_and(|speed| speed != "1x")
        {
            diagnostics.push(Diagnostic::error(
                None,
                "frame_replicator.speed",
                "帧复刻当前只支持 1x",
            ));
        }

        let mut declared: HashSet<String> = self.target_names().into_iter().collect();
        let actions = self.actions();
        if actions.is_empty() {
            diagnostics.push(Diagnostic::error(None, "actions", "作业里一个动作都没有"));
        }
        let mut previous = -1;
        let mut deployed = HashSet::new();
        for action in &actions {
            let id = Some(action.id);
            let kind = ActionType::parse(&action.kind);
            match kind {
                None => diagnostics.push(Diagnostic::error(
                    id,
                    "type",
                    format!("无法识别动作类型 `{}`", action.kind),
                )),
                Some(kind) if !kind.supported_in_frame_mode() => diagnostics.push(
                    Diagnostic::error(id, "type", format!("帧复刻不支持动作 `{kind}`")),
                ),
                _ => {}
            }
            match action.frame {
                None => diagnostics.push(Diagnostic::error(id, "frame", "动作缺少绝对帧")),
                Some(frame) if frame < 0 => {
                    diagnostics.push(Diagnostic::error(id, "frame", "帧号不能为负数"));
                }
                Some(frame) => {
                    if frame < previous {
                        diagnostics.push(Diagnostic::error(
                            id,
                            "frame",
                            "动作帧号必须按非递减顺序排列",
                        ));
                    }
                    previous = frame;
                }
            }
            if action.name.trim().is_empty()
                && matches!(
                    kind,
                    Some(ActionType::Deploy | ActionType::UseSkill | ActionType::Retreat)
                )
            {
                diagnostics.push(Diagnostic::error(id, "name", "动作缺少目标干员"));
            }
            if kind == Some(ActionType::Deploy) && !action.name.is_empty() {
                declared.insert(action.name.clone());
            }
            if !action.name.is_empty() && !declared.is_empty() && !declared.contains(&action.name) {
                diagnostics.push(Diagnostic::error(
                    id,
                    "name",
                    format!("目标 `{}` 未在编队或延迟目标中声明", action.name),
                ));
            }
            match kind {
                Some(ActionType::Deploy) => {
                    if action.location.is_none() {
                        diagnostics.push(Diagnostic::error(id, "location", "部署动作缺少格子"));
                    }
                    if !action.name.is_empty() && !deployed.insert(action.name.clone()) {
                        diagnostics.push(Diagnostic::warning(
                            id,
                            "name",
                            format!("干员 `{}` 在未记录撤退前再次部署", action.name),
                        ));
                    }
                }
                Some(ActionType::UseSkill) => {
                    if !action.name.is_empty() && !deployed.contains(&action.name) {
                        diagnostics.push(Diagnostic::warning(
                            id,
                            "name",
                            format!("干员 `{}` 在部署记录之前使用技能", action.name),
                        ));
                    }
                }
                Some(ActionType::Retreat) => {
                    if !action.name.is_empty() && !deployed.remove(&action.name) {
                        diagnostics.push(Diagnostic::warning(
                            id,
                            "name",
                            format!("干员 `{}` 在部署记录之前撤退", action.name),
                        ));
                    }
                }
                _ => {}
            }
        }
        if !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
        {
            if let Err(error) = self.compile() {
                diagnostics.push(Diagnostic::error(
                    None,
                    "document",
                    format!("作业无法编译：{error}"),
                ));
            }
        }
        diagnostics
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
            || self.compile().is_err()
    }

    fn action_from_value(&self, index: usize, value: &Value) -> ActionDraft {
        let id = self.action_ids.get(index).copied().unwrap_or(ActionId(0));
        let object = value.as_object();
        ActionDraft {
            id,
            kind: object
                .and_then(|map| map.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            frame: object
                .and_then(|map| map.get("frame"))
                .and_then(Value::as_i64),
            name: object
                .and_then(|map| map.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            location: object
                .and_then(|map| map.get("location"))
                .and_then(parse_location),
            direction: object
                .and_then(|map| map.get("direction"))
                .and_then(Value::as_str)
                .map(parse_direction)
                .unwrap_or_default(),
            doc: object
                .and_then(|map| map.get("doc"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        }
    }

    fn sort_actions_stably(&mut self) {
        let actions = ensure_array(&mut self.root, "actions");
        let mut paired: Vec<_> = actions
            .drain(..)
            .zip(self.action_ids.drain(..))
            .enumerate()
            .collect();
        paired.sort_by_key(|(original, (value, _))| {
            (action_frame(value).unwrap_or(i64::MAX), *original)
        });
        for (_, (value, id)) in paired {
            actions.push(value);
            self.action_ids.push(id);
        }
    }

    fn doc_string(&self, field: &str) -> String {
        self.root
            .get("doc")
            .and_then(Value::as_object)
            .and_then(|doc| doc.get(field))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn set_doc_string(&mut self, field: &str, value: String) {
        let doc = self
            .root
            .entry("doc")
            .or_insert_with(|| Value::Object(Map::new()));
        if !doc.is_object() {
            *doc = Value::Object(Map::new());
        }
        doc.as_object_mut()
            .expect("doc was normalized to an object")
            .insert(field.into(), Value::String(value));
    }
}

fn ensure_array<'a>(root: &'a mut Map<String, Value>, field: &str) -> &'a mut Vec<Value> {
    let value = root
        .entry(field.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value
        .as_array_mut()
        .expect("value was normalized to an array")
}

fn ensure_meta_array<'a>(root: &'a mut Map<String, Value>, field: &str) -> &'a mut Vec<Value> {
    let meta = root
        .entry("frame_replicator".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !meta.is_object() {
        *meta = Value::Object(Map::new());
    }
    let object = meta
        .as_object_mut()
        .expect("frame_replicator was normalized to an object");
    let value = object
        .entry(field.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value
        .as_array_mut()
        .expect("metadata field was normalized to an array")
}

fn string_field(root: &Map<String, Value>, field: &str) -> String {
    root.get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn action_frame(value: &Value) -> Option<i64> {
    value
        .as_object()
        .and_then(|object| object.get("frame"))
        .and_then(Value::as_i64)
}

fn parse_location(value: &Value) -> Option<Point> {
    let values = value.as_array()?;
    let x = i32::try_from(values.first()?.as_i64()?).ok()?;
    let y = i32::try_from(values.get(1)?.as_i64()?).ok()?;
    Some(Point::new(x, y))
}

fn parse_direction(direction: &str) -> Direction {
    match direction.to_ascii_lowercase().as_str() {
        "left" | "左" => Direction::Left,
        "up" | "上" => Direction::Up,
        "down" | "下" => Direction::Down,
        "none" | "无" => Direction::None,
        _ => Direction::Right,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::Right => "Right",
        Direction::Down => "Down",
        Direction::Left => "Left",
        Direction::Up => "Up",
        Direction::None => "None",
    }
}

fn action_to_value(draft: &ActionDraft, existing: Option<Map<String, Value>>) -> Value {
    let mut object = existing.unwrap_or_default();
    object.insert("type".into(), Value::String(draft.kind.clone()));
    set_optional(
        &mut object,
        "frame",
        draft.frame.map(|frame| Value::Number(frame.into())),
    );
    set_optional(
        &mut object,
        "name",
        (!draft.name.is_empty()).then(|| Value::String(draft.name.clone())),
    );
    set_optional(
        &mut object,
        "location",
        draft
            .location
            .map(|point| serde_json::json!([point.x, point.y])),
    );
    if ActionType::parse(&draft.kind) == Some(ActionType::Deploy) {
        object.insert(
            "direction".into(),
            Value::String(direction_name(draft.direction).to_owned()),
        );
    } else {
        object.remove("direction");
    }
    set_optional(
        &mut object,
        "doc",
        (!draft.doc.is_empty()).then(|| Value::String(draft.doc.clone())),
    );
    Value::Object(object)
}

fn set_optional(object: &mut Map<String, Value>, field: &str, value: Option<Value>) {
    if let Some(value) = value {
        object.insert(field.into(), value);
    } else {
        object.remove(field);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: &str = r#"{
        "_comment": ["keep me"],
        "stage_name": "main_01-07",
        "frame_replicator": {"version": 1, "speed": "1x", "after_last_action": "pause", "future": 9},
        "doc": {"title": "测试", "details": "说明", "author": "Doctor"},
        "opers": [{"name": "桃金娘", "skill": 1, "skill_usage": 0}],
        "actions": [
            {"type": "Deploy", "frame": 60, "name": "桃金娘", "location": [3, 2], "direction": "Right", "kills": 0},
            {"type": "Skill", "frame": 90, "name": "桃金娘", "costs": 12}
        ]
    }"#;

    #[test]
    fn unknown_fields_survive_round_trip_and_edit() {
        let mut document = JobDocument::parse(JOB).unwrap();
        document.set_title("改名");
        let mut action = document.actions()[0].clone();
        action.frame = Some(61);
        document.update_action(&action).unwrap();

        let value: Value = serde_json::from_str(&document.to_pretty_json().unwrap()).unwrap();
        assert_eq!(value["_comment"][0], "keep me");
        assert_eq!(value["frame_replicator"]["future"], 9);
        assert_eq!(value["doc"]["author"], "Doctor");
        assert_eq!(value["opers"][0]["skill_usage"], 0);
        assert_eq!(value["actions"][0]["kills"], 0);
        assert_eq!(value["actions"][1]["costs"], 12);
        assert_eq!(value["actions"][0]["frame"], 61);
    }

    #[test]
    fn invalid_draft_can_save_but_not_compile() {
        let mut document = JobDocument::default();
        let action = document.new_action("Deploy", None);
        document.insert_action(action);
        let saved = document.to_pretty_json().unwrap();
        assert!(saved.contains("Deploy"));
        assert!(document.has_errors());
        assert!(document.compile().is_err());
    }

    #[test]
    fn valid_document_compiles_through_existing_model() {
        let document = JobDocument::parse(JOB).unwrap();
        let compiled = document.compile().unwrap();
        assert_eq!(compiled.stage_name, "main_01-07");
        assert_eq!(compiled.actions.len(), 2);
        assert_eq!(compiled.actions[0].frame, 60);
    }

    #[test]
    fn same_frame_order_is_explicit_and_stable() {
        let mut document = JobDocument::parse(JOB).unwrap();
        let mut action = document.new_action("Output", Some(90));
        action.doc = "同帧注释".into();
        let new_id = action.id;
        document.insert_action(action);
        let actions = document.actions();
        assert_eq!(actions[1].kind, "Skill");
        assert_eq!(actions[2].id, new_id);
        assert!(document.move_within_same_frame(new_id, -1));
        let actions = document.actions();
        assert_eq!(actions[1].id, new_id);
        assert_eq!(actions[2].kind, "Skill");
    }

    #[test]
    fn renaming_operator_updates_action_references() {
        let mut document = JobDocument::parse(JOB).unwrap();
        assert!(document.rename_operator("桃金娘", "风笛"));
        assert_eq!(document.operator_names(), vec!["风笛"]);
        assert!(document
            .actions()
            .iter()
            .all(|action| action.name == "风笛"));
        assert!(!document.remove_operator("风笛"));
        for id in document
            .actions()
            .iter()
            .map(|action| action.id)
            .collect::<Vec<_>>()
        {
            document.remove_action(id);
        }
        assert!(document.remove_operator("风笛"));
    }

    #[test]
    fn deferred_targets_are_saved_outside_the_startup_formation() {
        let mut document = JobDocument::parse(JOB).unwrap();
        document.add_deferred_target("Mon3tr");
        let mut action = document.new_action("Deploy", Some(120));
        action.name = "Mon3tr".into();
        action.location = Some(Point::new(4, 2));
        document.insert_action(action);

        let compiled = document.compile().unwrap();
        assert_eq!(compiled.oper_names(), vec!["桃金娘"]);
        assert_eq!(compiled.deferred_target_names(), vec!["Mon3tr"]);
        assert_eq!(document.target_names(), vec!["桃金娘", "Mon3tr"]);
        let saved: Value = serde_json::from_str(&document.to_pretty_json().unwrap()).unwrap();
        assert_eq!(saved["frame_replicator"]["deferred_targets"][0], "Mon3tr");
        assert_eq!(saved["opers"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn diagnostics_allow_early_frame_without_obsolete_blind_spot_warning() {
        let early = JOB.replace("\"frame\": 60", "\"frame\": 10");
        let document = JobDocument::parse(&early).unwrap();
        assert!(!document
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("第 60 帧")));
        assert!(document.compile().is_ok());
    }

    #[test]
    fn malformed_known_action_fields_remain_openable() {
        let document = JobDocument::parse(
            r#"{"stage_name":"x","frame_replicator":true,"actions":[{"type":3,"frame":"later","extra":7}]}"#,
        )
        .unwrap();
        assert_eq!(document.actions().len(), 1);
        let saved: Value = serde_json::from_str(&document.to_pretty_json().unwrap()).unwrap();
        assert_eq!(saved["actions"][0]["extra"], 7);
        assert!(document.has_errors());
    }
}
