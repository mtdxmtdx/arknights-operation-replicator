// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 本文件移植自 MaaAssistantArknights (AGPL-3.0-only):
//   src/MaaCore/Vision/Battle/BattleFormationAnalyzer.cpp
//   src/MaaCore/Vision/TemplDetOCRer.cpp
//   src/MaaCore/Vision/RegionOCRer.cpp
//   src/MaaCore/Task/Experiment/CombatRecordRecognitionTask.cpp
//   src/MaaCore/Config/Miscellaneous/OcrPackNcnn.cpp
// 详见仓库根目录 THIRD-PARTY-NOTICES.md。

//! 战前编队识别。
//!
//! 这一模块只依赖截图、MAA 资源和 CPU ONNX 推理，不能访问 AFA、鼠标、键盘或触控接口。

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use ort::{session::Session, value::Tensor};
use repl_capture::Frame;
use repl_core::{Rect, Role};
use serde::Deserialize;

use crate::{
    best_match,
    deployment::Card,
    find_all,
    templates::{load_named_template, TemplateLoadError},
    Match, Template,
};

const BRIDGE_MARGIN: f64 = 0.05;
const BRIDGE_TIE_EPSILON: f64 = 1e-6;

const FLAG_TEMPLATE: &str = "BattleFormationOCRNameFlag.png";
const TASKS_JSON: &str = "tasks/tasks.json";
const BATTLE_DATA_JSON: &str = "battle_data.json";
const OCR_MODEL: &str = "PaddleOCR/rec/inference.onnx";
const OCR_KEYS: &str = "PaddleOCR/rec/keys.txt";
const MAX_FORMATION_SLOTS: usize = 16;
const REC_HEIGHT: u32 = 48;
const REC_WIDTH: u32 = 320;
const BIN_LOWER: u8 = 140;
const BIN_UPPER: u8 = 255;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorKey {
    pub id: String,
    pub display_name: String,
    pub role: Role,
    pub rarity: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuggestionSource {
    JobExact,
    JobFuzzy,
    CatalogExact,
    CatalogFuzzy,
}

impl SuggestionSource {
    pub fn zh(self) -> &'static str {
        match self {
            Self::JobExact => "作业精确",
            Self::JobFuzzy => "作业近似",
            Self::CatalogExact => "MAA 全库精确",
            Self::CatalogFuzzy => "MAA 全库近似",
        }
    }

    pub fn belongs_to_job(self) -> bool {
        matches!(self, Self::JobExact | Self::JobFuzzy)
    }
}

#[derive(Clone, Debug)]
pub struct FormationAvatar {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct FormationScanRow {
    pub slot: usize,
    pub flag_rect: Rect,
    pub name_rect: Option<Rect>,
    pub avatar_rect: Option<Rect>,
    pub raw_text: String,
    pub confidence: f32,
    pub suggestion: Option<OperatorKey>,
    pub source: Option<SuggestionSource>,
    pub default_selected: bool,
    pub avatar: Option<FormationAvatar>,
}

#[derive(Clone, Debug)]
pub struct FormationScanReport {
    pub rows: Vec<FormationScanRow>,
    pub missing_job_opers: Vec<String>,
    pub foreign_opers: Vec<String>,
    pub used_old_layout: bool,
}

/// 战前编队头像与 F0 部署栏卡片之间的一次安全唯一匹配。
#[derive(Clone, Debug, PartialEq)]
pub struct FormationBridgeMatch {
    pub operator: OperatorKey,
    pub card_index: usize,
    pub score: f64,
}

/// 把用户确认过的 150×165 编队头像桥接为实际部署栏里的 60×60 头像。
///
/// 与 MAA 的 `BattleAvatarDataForFormation` 一样对部署栏中心裁图做多尺度 NCC；
/// 只有行列双方互为唯一最佳且都留有足够 margin 时才返回，绝不猜测歧义项。
pub fn bridge_formation_avatars(
    confirmed: &[(OperatorKey, FormationAvatar)],
    cards: &[Card],
    config: &FormationTaskConfig,
) -> Vec<FormationBridgeMatch> {
    if confirmed.is_empty() || cards.is_empty() {
        return Vec::new();
    }
    let scores = confirmed
        .iter()
        .map(|(operator, avatar)| {
            cards
                .iter()
                .map(|card| {
                    if !roles_compatible(operator, card.role) {
                        return f64::NEG_INFINITY;
                    }
                    bridge_score(operator, avatar, card, config.bridge_crop)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let mut matches = Vec::new();
    for (oper_index, (operator, _)) in confirmed.iter().enumerate() {
        let Some((card_index, score, row_margin)) = unique_best(&scores[oper_index]) else {
            continue;
        };
        let column = scores.iter().map(|row| row[card_index]).collect::<Vec<_>>();
        let Some((best_oper, _, column_margin)) = unique_best(&column) else {
            continue;
        };
        if best_oper == oper_index
            && score >= config.bridge_threshold
            && row_margin >= BRIDGE_MARGIN
            && column_margin >= BRIDGE_MARGIN
        {
            matches.push(FormationBridgeMatch {
                operator: operator.clone(),
                card_index: cards[card_index].index,
                score,
            });
        }
    }
    matches
}

fn bridge_score(
    operator: &OperatorKey,
    formation: &FormationAvatar,
    card: &Card,
    crop: Rect,
) -> f64 {
    let card_frame = Frame::new(card.avatar_width, card.avatar_height, card.avatar.clone());
    let crop = crop.clamped(card.avatar_width as i32, card.avatar_height as i32);
    if crop.is_empty() || formation.width == 0 || formation.height == 0 {
        return f64::NEG_INFINITY;
    }
    let formation_frame = Frame::new(formation.width, formation.height, formation.bgra.clone());
    let max_scale = if operator.rarity <= 1 { 199 } else { 124 };
    (100..=max_scale)
        .filter_map(|percent| {
            let width = ((crop.width * percent + 50) / 100).max(1) as u32;
            let height = ((crop.height * percent + 50) / 100).max(1) as u32;
            if width > formation.width || height > formation.height {
                return None;
            }
            let scaled = card_frame.resample(crop, width, height);
            let template =
                Template::from_bgra("formation-bridge", width, height, &scaled.pixels, None)
                    .ok()?;
            best_match(&formation_frame, formation_frame.bounds(), &template).map(|hit| hit.score)
        })
        .max_by(f64::total_cmp)
        .unwrap_or(f64::NEG_INFINITY)
}

fn roles_compatible(operator: &OperatorKey, card_role: Role) -> bool {
    operator.role == Role::Unknown
        || card_role == Role::Unknown
        || operator.role == card_role
        || (operator.id == "char_002_amiya" && matches!(card_role, Role::Caster | Role::Warrior))
}

fn unique_best(values: &[f64]) -> Option<(usize, f64, f64)> {
    let mut ranked = values
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, value)| value.is_finite())
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1));
    let (index, best) = *ranked.first()?;
    let second = ranked.get(1).map_or(f64::NEG_INFINITY, |(_, value)| *value);
    if (best - second).abs() <= BRIDGE_TIE_EPSILON {
        return None;
    }
    Some((index, best, best - second))
}

#[derive(Clone, Debug)]
pub struct FormationTaskConfig {
    pub flag_roi: Rect,
    pub flag_threshold: f64,
    pub name_move: Rect,
    pub old_name_move: Rect,
    pub avatar_move: Rect,
    pub bridge_crop: Rect,
    pub bridge_threshold: f64,
}

#[derive(Clone, Debug)]
pub struct OperatorCatalog {
    by_normalized_name: HashMap<String, Vec<OperatorKey>>,
    operators: Vec<OperatorKey>,
}

impl OperatorCatalog {
    pub fn load(path: &Path) -> Result<Self, FormationError> {
        let text = fs::read_to_string(path)
            .map_err(|error| FormationError::Resource(path.to_path_buf(), error.to_string()))?;
        let data: BattleData = serde_json::from_str(&text)
            .map_err(|error| FormationError::Resource(path.to_path_buf(), error.to_string()))?;
        let mut operators = Vec::with_capacity(data.chars.len());
        let mut by_normalized_name: HashMap<String, Vec<OperatorKey>> = HashMap::new();
        for (id, value) in data.chars {
            if value.name.trim().is_empty() {
                continue;
            }
            let operator = OperatorKey {
                id,
                display_name: value.name,
                role: parse_role(&value.profession),
                rarity: value.rarity,
            };
            by_normalized_name
                .entry(normalize_name(&operator.display_name))
                .or_default()
                .push(operator.clone());
            operators.push(operator);
        }
        if operators.is_empty() {
            return Err(FormationError::InvalidResource(
                "battle_data.json 没有可用干员".into(),
            ));
        }
        Ok(Self {
            by_normalized_name,
            operators,
        })
    }

    pub fn operator_for_job_name(&self, name: &str) -> OperatorKey {
        let normalized = normalize_name(name);
        self.by_normalized_name
            .get(&normalized)
            .and_then(|operators| {
                operators
                    .iter()
                    .find(|operator| operator.display_name == name)
                    .or_else(|| (operators.len() == 1).then(|| &operators[0]))
            })
            .cloned()
            .unwrap_or_else(|| OperatorKey {
                id: format!("job:{normalized}"),
                display_name: name.to_owned(),
                role: Role::Unknown,
                rarity: 0,
            })
    }

    pub fn find_by_id(&self, id: &str) -> Option<&OperatorKey> {
        self.operators.iter().find(|operator| operator.id == id)
    }

    /// 按显示名称搜索 MAA 干员目录，供作业编辑器的编队添加框使用。
    ///
    /// 精确匹配和前缀匹配排在包含匹配之前；名称去重后按短名称、字典序稳定排序。
    /// 这是纯本地资源查询，不触发 OCR、截图或任何游戏输入。
    pub fn search_names(&self, query: &str, limit: usize) -> Vec<String> {
        let needle = normalize_name(query);
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }

        let mut names = self
            .operators
            .iter()
            .filter_map(|operator| {
                let name = normalize_name(&operator.display_name);
                name.contains(&needle)
                    .then(|| operator.display_name.clone())
            })
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names.sort_by(|left, right| {
            let rank = |name: &str| {
                let normalized = normalize_name(name);
                if normalized == needle {
                    0
                } else if normalized.starts_with(&needle) {
                    1
                } else {
                    2
                }
            };
            rank(left)
                .cmp(&rank(right))
                .then_with(|| left.chars().count().cmp(&right.chars().count()))
                .then_with(|| left.cmp(right))
        });
        names.truncate(limit);
        names
    }

    fn resolve(&self, raw: &str, job_names: &[String]) -> NameResolution {
        let normalized = normalize_name(raw);
        if normalized.is_empty() {
            return NameResolution::default();
        }

        let job = job_names
            .iter()
            .map(|name| self.operator_for_job_name(name))
            .collect::<Vec<_>>();
        let exact_job = job
            .iter()
            .filter(|operator| normalize_name(&operator.display_name) == normalized)
            .collect::<Vec<_>>();
        if exact_job.len() == 1 {
            return NameResolution::new(exact_job[0].clone(), SuggestionSource::JobExact);
        }

        if normalized.chars().count() >= 2 {
            if let Some(operator) = unique_distance_one(&normalized, &job) {
                return NameResolution::new(operator, SuggestionSource::JobFuzzy);
            }
        }

        if let Some(operators) = self.by_normalized_name.get(&normalized) {
            if operators.len() == 1 {
                return NameResolution::new(operators[0].clone(), SuggestionSource::CatalogExact);
            }
        }

        if normalized.chars().count() >= 3 {
            if let Some(operator) = unique_distance_one(&normalized, &self.operators) {
                return NameResolution::new(operator, SuggestionSource::CatalogFuzzy);
            }
        }

        NameResolution::default()
    }
}

#[derive(Default)]
struct NameResolution {
    operator: Option<OperatorKey>,
    source: Option<SuggestionSource>,
}

impl NameResolution {
    fn new(operator: OperatorKey, source: SuggestionSource) -> Self {
        Self {
            operator: Some(operator),
            source: Some(source),
        }
    }
}

pub struct FormationResources {
    pub config: FormationTaskConfig,
    pub catalog: OperatorCatalog,
    flag: Template,
    model_path: PathBuf,
    keys: Vec<String>,
}

impl std::fmt::Debug for FormationResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormationResources")
            .field("config", &self.config)
            .field("catalog_size", &self.catalog.operators.len())
            .field("model_path", &self.model_path)
            .field("keys", &self.keys.len())
            .finish()
    }
}

impl FormationResources {
    pub fn load(resource_dir: impl AsRef<Path>) -> Result<Self, FormationError> {
        let root = resource_dir.as_ref();
        let tasks_path = root.join(TASKS_JSON);
        let task_text = fs::read_to_string(&tasks_path)
            .map_err(|error| FormationError::Resource(tasks_path.clone(), error.to_string()))?;
        let tasks: serde_json::Value = serde_json::from_str(&task_text)
            .map_err(|error| FormationError::Resource(tasks_path.clone(), error.to_string()))?;
        let task = |name: &str| {
            tasks
                .get(name)
                .ok_or_else(|| FormationError::MissingTask(name.to_owned()))
        };

        let flag_task = task("BattleFormationOCRNameFlag")?;
        let names_task = task("BattleFormationOperNames")?;
        let old_names_task = task("BattleFormationOperNamesOldVersion")?;
        let avatar_task = task("BattleFormationOperAvatarMove")?;
        let bridge_task = task("BattleAvatarDataForFormation")?;
        let config = FormationTaskConfig {
            flag_roi: parse_rect_field(flag_task, "roi", "BattleFormationOCRNameFlag")?,
            flag_threshold: parse_number_field(
                flag_task,
                "templThreshold",
                "BattleFormationOCRNameFlag",
            )?,
            name_move: parse_rect_field(names_task, "roi", "BattleFormationOperNames")?,
            old_name_move: parse_rect_field(
                old_names_task,
                "roi",
                "BattleFormationOperNamesOldVersion",
            )?,
            avatar_move: parse_rect_field(
                avatar_task,
                "rectMove",
                "BattleFormationOperAvatarMove",
            )?,
            bridge_crop: parse_rect_field(bridge_task, "rectMove", "BattleAvatarDataForFormation")?,
            bridge_threshold: parse_number_field(
                bridge_task,
                "templThreshold",
                "BattleAvatarDataForFormation",
            )?,
        };
        validate_config(&config)?;

        let flag = load_named_template(root, FLAG_TEMPLATE, None)?;
        let model_path = root.join(OCR_MODEL);
        if !model_path.is_file() {
            return Err(FormationError::Resource(model_path, "文件不存在".into()));
        }
        let keys_path = root.join(OCR_KEYS);
        let keys = load_keys(&keys_path)?;
        let catalog = OperatorCatalog::load(&root.join(BATTLE_DATA_JSON))?;

        Ok(Self {
            config,
            catalog,
            flag,
            model_path,
            keys,
        })
    }
}

pub struct FormationScanner {
    resources: FormationResources,
    session: Session,
}

impl FormationScanner {
    pub fn new(resources: FormationResources) -> Result<Self, FormationError> {
        let threads = std::thread::available_parallelism()
            .map_or(2, usize::from)
            .clamp(1, 4);
        let session = Session::builder()
            .map_err(FormationError::Ort)?
            .with_intra_threads(threads)
            .map_err(FormationError::Ort)?
            .commit_from_file(&resources.model_path)
            .map_err(FormationError::Ort)?;
        if session.inputs.len() != 1 || session.outputs.len() != 1 {
            return Err(FormationError::InvalidResource(format!(
                "OCR 模型应有 1 个输入和 1 个输出，实际为 {}/{}",
                session.inputs.len(),
                session.outputs.len()
            )));
        }
        Ok(Self { resources, session })
    }

    pub fn resources(&self) -> &FormationResources {
        &self.resources
    }

    pub fn scan(
        &mut self,
        frame: &Frame,
        job_names: &[String],
    ) -> Result<FormationScanReport, FormationError> {
        if frame.width != repl_core::REF_WIDTH as u32
            || frame.height != repl_core::REF_HEIGHT as u32
        {
            return Err(FormationError::InvalidFrame(format!(
                "编队识别要求 1280×720 参考帧，实际为 {}×{}",
                frame.width, frame.height
            )));
        }
        let mut flags = find_all(
            frame,
            self.resources.config.flag_roi,
            &self.resources.flag,
            self.resources.config.flag_threshold,
        );
        if flags.is_empty() {
            return Err(FormationError::NoFlags);
        }
        if flags.len() > MAX_FORMATION_SLOTS {
            return Err(FormationError::TooManyFlags(flags.len()));
        }
        sort_formation_flags(&mut flags);

        let current = self.scan_layout(frame, &flags, job_names, false)?;
        let current_has_name = current
            .iter()
            .any(|row| row.suggestion.is_some() && !row.raw_text.is_empty());
        let (rows, used_old_layout) = if current_has_name {
            (current, false)
        } else {
            (self.scan_layout(frame, &flags, job_names, true)?, true)
        };

        let selected_job = rows
            .iter()
            .filter(|row| row.source.is_some_and(SuggestionSource::belongs_to_job))
            .filter_map(|row| {
                row.suggestion
                    .as_ref()
                    .map(|operator| operator.display_name.clone())
            })
            .collect::<HashSet<_>>();
        let missing_job_opers = job_names
            .iter()
            .filter(|name| !selected_job.contains(*name))
            .cloned()
            .collect();
        let foreign_opers = rows
            .iter()
            .filter(|row| row.source.is_some_and(|source| !source.belongs_to_job()))
            .filter_map(|row| {
                row.suggestion
                    .as_ref()
                    .map(|operator| operator.display_name.clone())
            })
            .collect();

        Ok(FormationScanReport {
            rows,
            missing_job_opers,
            foreign_opers,
            used_old_layout,
        })
    }

    fn scan_layout(
        &mut self,
        frame: &Frame,
        flags: &[Match],
        job_names: &[String],
        old_layout: bool,
    ) -> Result<Vec<FormationScanRow>, FormationError> {
        let name_move = if old_layout {
            self.resources.config.old_name_move
        } else {
            self.resources.config.name_move
        };
        let expansion = if old_layout { 2 } else { 3 };
        let mut rows = Vec::with_capacity(flags.len());
        for (slot, flag) in flags.iter().enumerate() {
            let fixed_name_roi = flag
                .rect
                .moved(name_move)
                .clamped(frame.width as i32, frame.height as i32);
            let Some(text_rect) = bright_text_bounds(frame, fixed_name_roi, expansion) else {
                rows.push(FormationScanRow {
                    slot,
                    flag_rect: flag.rect,
                    name_rect: None,
                    avatar_rect: None,
                    raw_text: String::new(),
                    confidence: 0.0,
                    suggestion: None,
                    source: None,
                    default_selected: false,
                    avatar: None,
                });
                continue;
            };
            let (raw_text, confidence) = self.recognize_line(frame, text_rect)?;
            let resolution = self.resources.catalog.resolve(&raw_text, job_names);
            let avatar_base = Rect::new(text_rect.right(), text_rect.y, 0, 0);
            let avatar_rect = avatar_base.moved(self.resources.config.avatar_move);
            let avatar = exact_crop(frame, avatar_rect).map(|bgra| FormationAvatar {
                width: avatar_rect.width as u32,
                height: avatar_rect.height as u32,
                bgra,
            });
            let default_selected = confidence >= 0.70
                && resolution.source == Some(SuggestionSource::JobExact)
                && avatar.is_some();
            rows.push(FormationScanRow {
                slot,
                flag_rect: flag.rect,
                name_rect: Some(text_rect),
                avatar_rect: avatar.as_ref().map(|_| avatar_rect),
                raw_text,
                confidence,
                suggestion: resolution.operator,
                source: resolution.source,
                default_selected,
                avatar,
            });
        }
        Ok(rows)
    }

    fn recognize_line(
        &mut self,
        frame: &Frame,
        text_rect: Rect,
    ) -> Result<(String, f32), FormationError> {
        let input = prepare_recognizer_input(frame, text_rect)?;
        let tensor = Tensor::from_array((
            [1usize, 3, REC_HEIGHT as usize, REC_WIDTH as usize],
            input.into_boxed_slice(),
        ))
        .map_err(FormationError::Ort)?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .map_err(FormationError::Ort)?;
        let output = outputs[0]
            .try_extract_array::<f32>()
            .map_err(FormationError::Ort)?;
        let shape = output.shape();
        if shape.len() != 3 || shape[0] != 1 {
            return Err(FormationError::InvalidResource(format!(
                "OCR 输出 shape 应为 [1,T,C]，实际为 {shape:?}"
            )));
        }
        let time_steps = shape[1];
        let classes = shape[2];
        let data = output
            .as_slice()
            .ok_or_else(|| FormationError::InvalidResource("OCR 输出不是连续内存".into()))?;
        decode_ctc(data, time_steps, classes, &self.resources.keys)
    }
}

fn parse_rect_field(
    task: &serde_json::Value,
    field: &str,
    task_name: &str,
) -> Result<Rect, FormationError> {
    let values = task
        .get(field)
        .and_then(serde_json::Value::as_array)
        .filter(|values| values.len() == 4)
        .ok_or_else(|| FormationError::MissingTaskField(task_name.into(), field.into()))?;
    let mut numbers = [0_i32; 4];
    for (index, value) in values.iter().enumerate() {
        numbers[index] = value
            .as_i64()
            .and_then(|number| i32::try_from(number).ok())
            .ok_or_else(|| FormationError::MissingTaskField(task_name.into(), field.into()))?;
    }
    Ok(Rect::new(numbers[0], numbers[1], numbers[2], numbers[3]))
}

fn parse_number_field(
    task: &serde_json::Value,
    field: &str,
    task_name: &str,
) -> Result<f64, FormationError> {
    task.get(field)
        .and_then(serde_json::Value::as_f64)
        .ok_or_else(|| FormationError::MissingTaskField(task_name.into(), field.into()))
}

fn validate_config(config: &FormationTaskConfig) -> Result<(), FormationError> {
    if config.flag_roi.is_empty()
        || config.name_move.is_empty()
        || config.old_name_move.is_empty()
        || config.avatar_move.is_empty()
        || config.bridge_crop.is_empty()
        || !(0.0..=1.0).contains(&config.flag_threshold)
        || !(0.0..=1.0).contains(&config.bridge_threshold)
    {
        return Err(FormationError::InvalidResource(
            "编队识别 task ROI 或阈值不合法".into(),
        ));
    }
    Ok(())
}

fn load_keys(path: &Path) -> Result<Vec<String>, FormationError> {
    let bytes = fs::read(path)
        .map_err(|error| FormationError::Resource(path.to_path_buf(), error.to_string()))?;
    let text = String::from_utf8(bytes)
        .map_err(|error| FormationError::Resource(path.to_path_buf(), error.to_string()))?;
    let mut keys = text
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect::<Vec<_>>();
    if let Some(first) = keys.first_mut() {
        *first = first.trim_start_matches('\u{feff}').to_owned();
    }
    if keys.is_empty() {
        return Err(FormationError::InvalidResource(
            "PaddleOCR keys.txt 为空".into(),
        ));
    }
    Ok(keys)
}

fn parse_role(raw: &str) -> Role {
    match raw.to_ascii_uppercase().as_str() {
        "CASTER" => Role::Caster,
        "MEDIC" => Role::Medic,
        "PIONEER" => Role::Pioneer,
        "SNIPER" => Role::Sniper,
        "SPECIAL" => Role::Special,
        "SUPPORT" => Role::Support,
        "TANK" => Role::Tank,
        "WARRIOR" => Role::Warrior,
        "DRONE" => Role::Drone,
        _ => Role::Unknown,
    }
}

fn normalize_name(raw: &str) -> String {
    raw.chars()
        .filter(|ch| {
            !ch.is_whitespace()
                && !matches!(
                    ch,
                    '<' | '>' | '_' | '~' | '^' | '。' | '《' | '》' | '「' | '」'
                )
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn unique_distance_one(raw: &str, candidates: &[OperatorKey]) -> Option<OperatorKey> {
    let mut best_distance = usize::MAX;
    let mut best: Option<&OperatorKey> = None;
    let mut tied = false;
    for candidate in candidates {
        let distance = levenshtein(raw, &normalize_name(&candidate.display_name));
        if distance < best_distance {
            best_distance = distance;
            best = Some(candidate);
            tied = false;
        } else if distance == best_distance {
            tied = true;
        }
    }
    (best_distance <= 1 && !tied)
        .then(|| best.cloned())
        .flatten()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars = b.chars().collect::<Vec<_>>();
    let mut previous = (0..=b_chars.len()).collect::<Vec<_>>();
    for (row, a_char) in a.chars().enumerate() {
        let mut current = Vec::with_capacity(b_chars.len() + 1);
        current.push(row + 1);
        for (column, b_char) in b_chars.iter().enumerate() {
            let replace = previous[column] + usize::from(a_char != *b_char);
            let insert = current[column] + 1;
            let delete = previous[column + 1] + 1;
            current.push(replace.min(insert).min(delete));
        }
        previous = current;
    }
    previous[b_chars.len()]
}

fn sort_formation_flags(flags: &mut [Match]) {
    let row_tolerance = flags
        .first()
        .map_or(8, |flag| (flag.rect.height * 2).max(8));
    flags.sort_by_key(|flag| (flag.rect.center().y, flag.rect.x));
    let mut row_start = 0;
    while row_start < flags.len() {
        let anchor_y = flags[row_start].rect.center().y;
        let mut row_end = row_start + 1;
        while row_end < flags.len() && flags[row_end].rect.center().y - anchor_y <= row_tolerance {
            row_end += 1;
        }
        flags[row_start..row_end].sort_by_key(|flag| flag.rect.x);
        row_start = row_end;
    }
}

fn bright_text_bounds(frame: &Frame, roi: Rect, expansion: i32) -> Option<Rect> {
    if roi.is_empty() {
        return None;
    }
    let mut left = roi.right();
    let mut right = roi.x - 1;
    let mut top = roi.bottom();
    let mut bottom = roi.y - 1;
    for y in roi.y..roi.bottom() {
        for x in roi.x..roi.right() {
            let offset = ((y as u32 * frame.width + x as u32) * 4) as usize;
            let b = frame.pixels[offset];
            let g = frame.pixels[offset + 1];
            let r = frame.pixels[offset + 2];
            let gray =
                ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8;
            if (BIN_LOWER..=BIN_UPPER).contains(&gray) {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
        }
    }
    if right < left || bottom < top {
        return None;
    }
    let expanded = Rect::new(
        left - expansion,
        top - expansion,
        right - left + 1 + expansion * 2,
        bottom - top + 1 + expansion * 2,
    );
    let roi_right = expanded.right().min(roi.right());
    let roi_bottom = expanded.bottom().min(roi.bottom());
    let x = expanded.x.max(roi.x);
    let y = expanded.y.max(roi.y);
    Some(Rect::new(x, y, roi_right - x, roi_bottom - y)).filter(|rect| !rect.is_empty())
}

fn exact_crop(frame: &Frame, rect: Rect) -> Option<Vec<u8>> {
    if rect.is_empty()
        || rect.x < 0
        || rect.y < 0
        || rect.right() > frame.width as i32
        || rect.bottom() > frame.height as i32
    {
        return None;
    }
    let mut pixels = Vec::with_capacity((rect.width * rect.height * 4) as usize);
    let stride = frame.width as usize * 4;
    for y in rect.y as usize..rect.bottom() as usize {
        let start = y * stride + rect.x as usize * 4;
        let end = start + rect.width as usize * 4;
        pixels.extend_from_slice(&frame.pixels[start..end]);
    }
    Some(pixels)
}

fn crop_frame(frame: &Frame, rect: Rect) -> Result<Frame, FormationError> {
    let pixels = exact_crop(frame, rect)
        .ok_or_else(|| FormationError::InvalidFrame(format!("裁剪区域越界：{rect}")))?;
    Ok(Frame::new(rect.width as u32, rect.height as u32, pixels))
}

fn prepare_recognizer_input(frame: &Frame, rect: Rect) -> Result<Vec<f32>, FormationError> {
    let crop = crop_frame(frame, rect)?;
    let ratio = crop.width as f64 / f64::from(crop.height.max(1));
    let resized_width = ((f64::from(REC_HEIGHT) * ratio).ceil() as u32).clamp(1, REC_WIDTH);
    let resized = crop.resample(crop.bounds(), resized_width, REC_HEIGHT);
    let plane = (REC_WIDTH * REC_HEIGHT) as usize;
    let mut input = vec![(127.0_f32 - 127.5) / 127.5; plane * 3];
    for y in 0..REC_HEIGHT {
        for x in 0..resized_width {
            let source = ((y * resized.width + x) * 4) as usize;
            let target = (y * REC_WIDTH + x) as usize;
            for channel in 0..3 {
                input[channel * plane + target] =
                    (f32::from(resized.pixels[source + channel]) - 127.5) / 127.5;
            }
        }
    }
    Ok(input)
}

fn decode_ctc(
    data: &[f32],
    time_steps: usize,
    classes: usize,
    keys: &[String],
) -> Result<(String, f32), FormationError> {
    if time_steps == 0 || classes == 0 || data.len() != time_steps * classes {
        return Err(FormationError::InvalidResource(format!(
            "OCR 输出长度不合法：T={time_steps}, C={classes}, len={}",
            data.len()
        )));
    }
    let mut charset = Vec::with_capacity(keys.len() + 2);
    charset.push(String::new());
    charset.extend(keys.iter().cloned());
    if classes == charset.len() + 1 {
        charset.push(" ".into());
    }
    if classes != charset.len() {
        return Err(FormationError::InvalidResource(format!(
            "OCR 类别数 {classes} 与 keys.txt {} 项不兼容",
            keys.len()
        )));
    }

    let mut text = String::new();
    let mut confidence_sum = 0.0_f32;
    let mut confidence_count = 0_u32;
    let mut previous = usize::MAX;
    for timestep in 0..time_steps {
        let row = &data[timestep * classes..(timestep + 1) * classes];
        let (best, best_value) = row
            .iter()
            .copied()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap();
        let sum = row.iter().copied().sum::<f32>();
        let min = row.iter().copied().fold(f32::INFINITY, f32::min);
        let confidence = if min >= -1e-6 && (sum - 1.0).abs() < 1e-2 {
            best_value
        } else {
            let denominator = row
                .iter()
                .map(|value| (*value - best_value).exp())
                .sum::<f32>();
            if denominator > 0.0 {
                1.0 / denominator
            } else {
                0.0
            }
        };
        if best != 0 && best != previous {
            text.push_str(&charset[best]);
            confidence_sum += confidence;
            confidence_count += 1;
        }
        previous = best;
    }
    let confidence = if confidence_count == 0 {
        0.0
    } else {
        confidence_sum / confidence_count as f32
    };
    Ok((text.trim().to_owned(), confidence))
}

#[derive(Debug, thiserror::Error)]
pub enum FormationError {
    #[error("无法读取 MAA 资源 {0}: {1}")]
    Resource(PathBuf, String),
    #[error("MAA tasks.json 缺少任务 {0}")]
    MissingTask(String),
    #[error("MAA 任务 {0} 缺少或错误定义字段 {1}")]
    MissingTaskField(String, String),
    #[error("MAA 编队识别资源不兼容：{0}")]
    InvalidResource(String),
    #[error("编队截图不可用：{0}")]
    InvalidFrame(String),
    #[error("当前画面找不到编队姓名锚点，请停在编队确认界面后重试")]
    NoFlags,
    #[error("识别到 {0} 个编队槽位，超过安全上限 {MAX_FORMATION_SLOTS}")]
    TooManyFlags(usize),
    #[error("加载 MAA 模板失败：{0}")]
    Template(#[from] TemplateLoadError),
    #[error("ONNX OCR 推理失败：{0}")]
    Ort(#[source] ort::Error),
}

#[derive(Deserialize)]
struct BattleData {
    chars: HashMap<String, BattleOperator>,
}

#[derive(Deserialize)]
struct BattleOperator {
    name: String,
    #[serde(default)]
    profession: String,
    #[serde(default)]
    rarity: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn operator(id: &str, name: &str) -> OperatorKey {
        OperatorKey {
            id: id.into(),
            display_name: name.into(),
            role: Role::Unknown,
            rarity: 6,
        }
    }

    fn catalog(names: &[(&str, &str)]) -> OperatorCatalog {
        let operators = names
            .iter()
            .map(|(id, name)| operator(id, name))
            .collect::<Vec<_>>();
        let mut by_normalized_name: HashMap<String, Vec<OperatorKey>> = HashMap::new();
        for operator in &operators {
            by_normalized_name
                .entry(normalize_name(&operator.display_name))
                .or_default()
                .push(operator.clone());
        }
        OperatorCatalog {
            by_normalized_name,
            operators,
        }
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks() {
        let keys = vec!["甲".into(), "乙".into()];
        // classes: blank, 甲, 乙. Path: 甲 甲 blank 乙
        let data = [
            0.0, 1.0, 0.0, // 甲
            0.0, 1.0, 0.0, // repeat
            1.0, 0.0, 0.0, // blank
            0.0, 0.0, 1.0, // 乙
        ];
        let (text, confidence) = decode_ctc(&data, 4, 3, &keys).unwrap();
        assert_eq!(text, "甲乙");
        assert_eq!(confidence, 1.0);
    }

    #[test]
    fn ctc_accepts_optional_space_class() {
        let keys = vec!["甲".into()];
        let data = [0.0, 0.0, 1.0];
        let (text, _) = decode_ctc(&data, 1, 3, &keys).unwrap();
        assert_eq!(text, "");
    }

    #[test]
    fn job_names_win_before_catalog() {
        let catalog = catalog(&[("char_a", "桃金娘"), ("char_b", "桃金姬")]);
        let job = vec!["桃金娘".to_owned()];
        let exact = catalog.resolve(" 桃金娘 ", &job);
        assert_eq!(exact.source, Some(SuggestionSource::JobExact));
        assert_eq!(exact.operator.unwrap().id, "char_a");

        let fuzzy = catalog.resolve("桃金粮", &job);
        assert_eq!(fuzzy.source, Some(SuggestionSource::JobFuzzy));
        assert_eq!(fuzzy.operator.unwrap().display_name, "桃金娘");
    }

    #[test]
    fn catalog_name_search_prioritizes_prefixes_and_deduplicates() {
        let catalog = catalog(&[
            ("char_ansel", "安赛尔"),
            ("char_angelina", "安洁莉娜"),
            ("char_ambriel", "安比尔"),
            ("char_duplicate", "安赛尔"),
            ("char_silence", "赫默"),
        ]);

        assert_eq!(
            catalog.search_names("安", 10),
            vec!["安比尔", "安赛尔", "安洁莉娜"]
        );
        assert_eq!(catalog.search_names("赛", 10), vec!["安赛尔"]);
        assert!(catalog.search_names("", 10).is_empty());
        assert_eq!(catalog.search_names("安", 2).len(), 2);
    }

    #[test]
    fn catalog_fallback_reports_foreign_operator() {
        let catalog = catalog(&[("char_a", "桃金娘"), ("char_b", "风笛")]);
        let resolution = catalog.resolve("风笛", &["桃金娘".into()]);
        assert_eq!(resolution.source, Some(SuggestionSource::CatalogExact));
        assert_eq!(resolution.operator.unwrap().id, "char_b");
    }

    #[test]
    fn short_catalog_names_are_exact_only() {
        let catalog = catalog(&[("char_a", "风笛"), ("char_b", "芬")]);
        let resolution = catalog.resolve("风迪", &[]);
        assert!(resolution.operator.is_none());
    }

    #[test]
    fn ambiguous_fuzzy_match_is_rejected() {
        let candidates = vec![operator("a", "甲乙丙"), operator("b", "甲乙丁")];
        assert!(unique_distance_one("甲乙戊", &candidates).is_none());
    }

    #[test]
    fn bright_bounds_expand_but_stay_inside_roi() {
        let mut frame = Frame::new(20, 10, vec![0; 20 * 10 * 4]);
        for y in 3..5 {
            for x in 5..8 {
                let offset = ((y * 20 + x) * 4) as usize;
                frame.pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        assert_eq!(
            bright_text_bounds(&frame, Rect::new(4, 2, 8, 5), 3),
            Some(Rect::new(4, 2, 7, 5))
        );
    }

    #[test]
    fn recognizer_input_is_bgr_chw_and_padded() {
        let frame = Frame::new(1, 1, vec![0, 127, 255, 255]);
        let input = prepare_recognizer_input(&frame, frame.bounds()).unwrap();
        let plane = (REC_WIDTH * REC_HEIGHT) as usize;
        assert!((input[0] + 1.0).abs() < 1e-6); // B
        assert!((input[plane] - ((127.0 - 127.5) / 127.5)).abs() < 1e-6); // G
        assert!((input[plane * 2] - 1.0).abs() < 1e-6); // R
        assert!((input[REC_WIDTH as usize - 1] - ((127.0 - 127.5) / 127.5)).abs() < 1e-6);
    }

    fn patterned_avatar(seed: u8) -> Vec<u8> {
        let mut pixels = Vec::with_capacity(60 * 60 * 4);
        for y in 0..60_u32 {
            for x in 0..60_u32 {
                let value =
                    ((x * (3 + u32::from(seed)) + y * y + u32::from(seed) * 17) % 251) as u8;
                pixels.extend_from_slice(&[
                    value,
                    value.wrapping_mul(3).wrapping_add(seed),
                    value.wrapping_mul(7).wrapping_add(x as u8),
                    255,
                ]);
            }
        }
        pixels
    }

    fn card(index: usize, seed: u8, role: Role) -> Card {
        Card {
            index,
            role,
            available: true,
            cooling: false,
            click_rect: Rect::new(0, 0, 75, 120),
            avatar_rect: Rect::new(0, 0, 60, 60),
            avatar: patterned_avatar(seed),
            avatar_width: 60,
            avatar_height: 60,
        }
    }

    fn bridge_config() -> FormationTaskConfig {
        FormationTaskConfig {
            flag_roi: Rect::new(0, 0, 1, 1),
            flag_threshold: 0.8,
            name_move: Rect::new(0, 0, 1, 1),
            old_name_move: Rect::new(0, 0, 1, 1),
            avatar_move: Rect::new(0, 0, 1, 1),
            bridge_crop: Rect::new(15, 15, 30, 30),
            bridge_threshold: 0.8,
        }
    }

    #[test]
    fn formation_bridge_requires_mutual_unique_best() {
        let cards = vec![card(4, 1, Role::Caster), card(9, 5, Role::Sniper)];
        let mut caster = operator("caster", "术师甲");
        caster.role = Role::Caster;
        let mut sniper = operator("sniper", "狙击乙");
        sniper.role = Role::Sniper;
        let confirmed = vec![
            (
                caster,
                FormationAvatar {
                    width: 60,
                    height: 60,
                    bgra: cards[0].avatar.clone(),
                },
            ),
            (
                sniper,
                FormationAvatar {
                    width: 60,
                    height: 60,
                    bgra: cards[1].avatar.clone(),
                },
            ),
        ];
        let matches = bridge_formation_avatars(&confirmed, &cards, &bridge_config());
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].card_index, 4);
        assert_eq!(matches[1].card_index, 9);
        assert!(matches.iter().all(|matched| matched.score > 0.99));
    }

    #[test]
    fn formation_bridge_rejects_equal_card_ambiguity_and_role_mismatch() {
        let cards = vec![card(0, 2, Role::Medic), card(1, 2, Role::Medic)];
        let mut medic = operator("medic", "医疗甲");
        medic.role = Role::Medic;
        let confirmed = vec![(
            medic,
            FormationAvatar {
                width: 60,
                height: 60,
                bgra: cards[0].avatar.clone(),
            },
        )];
        assert!(bridge_formation_avatars(&confirmed, &cards, &bridge_config()).is_empty());

        let wrong_role = vec![card(0, 2, Role::Tank)];
        assert!(bridge_formation_avatars(&confirmed, &wrong_role, &bridge_config()).is_empty());
    }

    #[test]
    fn real_maa_ocr_resources_initialize_when_available() {
        let Some(resource_dir) = crate::templates::locate_maa_resource_dir() else {
            return;
        };
        let resources = FormationResources::load(resource_dir).unwrap();
        let scanner = FormationScanner::new(resources).unwrap();
        assert!(!scanner.resources().catalog.operators.is_empty());
    }
}
