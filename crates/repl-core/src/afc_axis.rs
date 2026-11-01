// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors
//
// 轴文件的宽松语法（行首帧号 / `## 帧号` 小标题 + 多行说明 / `m:ss` 时间 / Markdown 装饰
// 剥离 / `<!-- -->` 与 `//` 注释）按 AFC（Arknights Frame Counter，MIT）的
// `src/akframe/timeline.py` 公开格式实现，未复制其代码。

//! AFC 打轴器 Markdown 轴文件与帧复刻作业之间的转换。
//!
//! AFC 的轴只有「帧号 + 说明文字」，没有结构化动作，所以两个方向不对称：
//!
//! - **导出**无损：每个动作写成一行固定语法 `- 帧号 动作 目标 (x,y) 朝向 · 说明`，
//!   AFC 可直接当提词器使用；关卡、标题、编队等元数据写进 AFC 会跳过的 HTML 注释。
//! - **导入**分三级：先按上述固定语法精确解析（自己导出的轴可以无损导回）；
//!   再按关键词 + 干员名尽力识别；仍认不出的整行转成 `Output` 注释，原文保留。
//!   导入结果只是草稿，缺失的格子、目标由编辑器诊断标红，补全前不能运行。
//!
//! 本模块不做 IO，文件读写与干员目录加载由调用方负责。

use serde::{Deserialize, Serialize};

use crate::{ActionDraft, ActionType, Direction, JobDocument, Point};

/// 轴里的帧号按 1 倍速逻辑帧计，`m:ss` 换算用。
const LOGICAL_FPS: f64 = 30.0;

/// 元数据注释的前缀。AFC 把 `<!--` 开头的行当注释跳过，所以放在这里不会被当成轴。
const META_PREFIX: &str = "<!-- replicator-meta ";

/// 说明与结构化字段之间的分隔符（两侧有空格，避免和「维娜·维多利亚」这类名字冲突）。
const DOC_SEPARATOR: &str = " · ";

/// 固定语法里出现的动作词。导出与精确导入共用，保证往返一致。
const KIND_WORDS: [(&str, &str); 4] = [
    ("Deploy", "部署"),
    ("Skill", "技能"),
    ("Retreat", "撤退"),
    ("Output", "注释"),
];

const DIRECTION_WORDS: [(Direction, &str); 5] = [
    (Direction::Right, "朝右"),
    (Direction::Left, "朝左"),
    (Direction::Up, "朝上"),
    (Direction::Down, "朝下"),
    (Direction::None, "无朝向"),
];

/// 写进轴文件注释里的作业元数据，导入时用来无损还原。
#[derive(Debug, Default, Deserialize, Serialize)]
struct AxisMeta {
    #[serde(default)]
    stage_name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    opers: Vec<MetaOperator>,
    #[serde(default)]
    deferred_targets: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct MetaOperator {
    name: String,
    #[serde(default = "default_skill")]
    skill: u8,
}

fn default_skill() -> u8 {
    1
}

// ———————————————————————— 导出 ————————————————————————

/// 把作业导出成 AFC 可读的 Markdown 轴文件。
///
/// 没有帧号的动作无法放进轴，会被跳过；返回值第二项是跳过的数量。
pub fn export_axis(document: &JobDocument) -> (String, usize) {
    let mut out = String::new();
    let title = document.title();
    // 标题加「作业：」前缀：AFC 会把数字开头的标题（如「# 1-7」）当成第 1 帧的一条轴。
    out.push_str(&format!(
        "# 作业：{}\n\n",
        if title.trim().is_empty() {
            "未命名作业"
        } else {
            title.trim()
        }
    ));
    out.push_str("<!-- 由明日方舟帧级操作复刻器导出：每行「帧号 动作 目标 (x,y) 朝向 · 说明」，帧号为 1 倍速绝对逻辑帧。 -->\n");

    let meta = AxisMeta {
        stage_name: document.stage_name(),
        title,
        opers: document
            .root()
            .get("opers")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|oper| {
                let name = oper.get("name")?.as_str()?.to_owned();
                let skill = oper
                    .get("skill")
                    .and_then(serde_json::Value::as_u64)
                    .map_or(1, |skill| skill.clamp(1, 3) as u8);
                Some(MetaOperator { name, skill })
            })
            .collect(),
        deferred_targets: document.deferred_target_names(),
    };
    // 注释里不能出现 `-->`，否则会提前结束注释；JSON 字符串里转义成 > 仍然合法。
    let meta_json = serde_json::to_string(&meta)
        .unwrap_or_else(|_| "{}".into())
        .replace("-->", "--\\u003e");
    out.push_str(&format!("{META_PREFIX}{meta_json} -->\n\n"));

    let mut skipped = 0;
    for action in document.actions() {
        let Some(frame) = action.frame.filter(|frame| *frame >= 0) else {
            skipped += 1;
            continue;
        };
        out.push_str(&format!("- {frame} {}\n", describe_action(&action)));
    }
    (out, skipped)
}

fn describe_action(action: &ActionDraft) -> String {
    let kind = ActionType::parse(&action.kind);
    let word = match kind {
        Some(ActionType::Deploy) => "部署",
        Some(ActionType::UseSkill) => "技能",
        Some(ActionType::Retreat) => "撤退",
        Some(ActionType::Output) => "注释",
        // 帧模式不支持的类型原样写出，导入时会落到注释，不会被误执行。
        _ => action.kind.as_str(),
    };
    let mut parts = vec![word.to_owned()];
    let name = action.name.trim();
    if !name.is_empty() {
        parts.push(name.to_owned());
    }
    if let Some(point) = action.location {
        parts.push(format!("({},{})", point.x, point.y));
    }
    if kind == Some(ActionType::Deploy) {
        parts.push(direction_word(action.direction).to_owned());
    }
    let mut line = parts.join(" ");
    let doc = single_line(&action.doc);
    if !doc.is_empty() {
        line.push_str(DOC_SEPARATOR);
        line.push_str(&doc);
    }
    line
}

fn direction_word(direction: Direction) -> &'static str {
    DIRECTION_WORDS
        .iter()
        .find(|(candidate, _)| *candidate == direction)
        .map_or("朝右", |(_, word)| word)
}

/// AFC 用 `|` 当表格分隔、剥掉 `**` 和反引号；说明里出现这些字符会被改写，
/// 这里提前换掉，换行也压成一行。
fn single_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
        .replace('|', "/")
        .replace("**", "")
        .replace("__", "")
        .replace('`', "'")
}

// ———————————————————————— 导入 ————————————————————————

/// 导入结果。`document` 是未保存的草稿，可能带诊断错误。
#[derive(Debug)]
pub struct AxisImport {
    pub document: JobDocument,
    /// 按固定语法精确解析的动作数。
    pub exact: usize,
    /// 按关键词识别出的动作数（需要人工核对）。
    pub guessed: usize,
    /// 认不出动作、转成注释的条数。
    pub notes: usize,
    pub warnings: Vec<String>,
}

/// 解析 AFC 轴文件为作业草稿。
///
/// `known_names` 是可用于识别干员名的候选（通常是 MAA 干员目录），只影响关键词识别；
/// 固定语法里的名字总是原样采用。
pub fn import_axis(text: &str, known_names: &[String]) -> AxisImport {
    let parsed = parse_cues(text);
    let mut warnings = parsed.warnings;
    let meta =
        parsed
            .meta
            .as_deref()
            .and_then(|json| match serde_json::from_str::<AxisMeta>(json) {
                Ok(meta) => Some(meta),
                Err(error) => {
                    warnings.push(format!("元数据注释无法解析，已忽略：{error}"));
                    None
                }
            });

    let mut document = JobDocument::default();
    let title = meta
        .as_ref()
        .map(|meta| meta.title.clone())
        .filter(|title| !title.trim().is_empty())
        .or(parsed.title)
        .unwrap_or_else(|| "AFC 轴导入".into());
    document.set_title(title);
    if let Some(meta) = &meta {
        document.set_stage_name(meta.stage_name.clone());
        for oper in &meta.opers {
            document.add_operator(oper.name.clone(), oper.skill.clamp(1, 3));
        }
        for target in &meta.deferred_targets {
            document.add_deferred_target(target.clone());
        }
    } else {
        warnings.push("轴里没有关卡信息，请在编辑器里填写关卡标识".into());
    }

    // 关键词识别时优先匹配作业里已声明的名字，再查干员目录。
    let mut candidates: Vec<String> = document.target_names();
    for name in known_names {
        if !candidates.contains(name) {
            candidates.push(name.clone());
        }
    }
    // 长名字优先，避免「能天使」被「天使」之类的短名截胡。
    candidates.sort_by_key(|name| std::cmp::Reverse(name.chars().count()));

    let mut exact = 0;
    let mut guessed = 0;
    let mut notes = 0;
    for cue in parsed.cues {
        for line in cue
            .text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let (draft, source) = match parse_exact(line) {
                Some(parsed) => (parsed, Source::Exact),
                None => match guess_action(line, &candidates) {
                    Some(parsed) => (parsed, Source::Guessed),
                    None => (
                        ParsedAction {
                            kind: "Output",
                            doc: line.to_owned(),
                            ..ParsedAction::default()
                        },
                        Source::Note,
                    ),
                },
            };
            match source {
                Source::Exact => exact += 1,
                Source::Guessed => guessed += 1,
                Source::Note => notes += 1,
            }
            let mut action = document.new_action(draft.kind, Some(cue.frame));
            action.name = draft.name;
            action.location = draft.location;
            action.direction = draft.direction;
            action.doc = draft.doc;
            document.insert_action(action);
        }
    }

    // 没有元数据时按动作补全编队：部署过的进开局编队，只被技能/撤退引用的作为延迟目标。
    if meta.is_none() {
        declare_targets(&mut document);
    }
    if exact + guessed + notes == 0 {
        warnings.push("没有识别到任何带帧号的轴".into());
    }
    AxisImport {
        document,
        exact,
        guessed,
        notes,
        warnings,
    }
}

enum Source {
    Exact,
    Guessed,
    Note,
}

#[derive(Debug)]
struct ParsedAction {
    kind: &'static str,
    name: String,
    location: Option<Point>,
    direction: Direction,
    doc: String,
}

impl Default for ParsedAction {
    fn default() -> Self {
        Self {
            kind: "Output",
            name: String::new(),
            location: None,
            direction: Direction::Right,
            doc: String::new(),
        }
    }
}

fn declare_targets(document: &mut JobDocument) {
    let actions = document.actions();
    let mut deployed = Vec::new();
    let mut referenced = Vec::new();
    for action in &actions {
        let name = action.name.trim();
        if name.is_empty() {
            continue;
        }
        let list = if ActionType::parse(&action.kind) == Some(ActionType::Deploy) {
            &mut deployed
        } else {
            &mut referenced
        };
        if !list.iter().any(|existing: &String| existing == name) {
            list.push(name.to_owned());
        }
    }
    for name in &deployed {
        document.add_operator(name.clone(), 1);
    }
    for name in referenced {
        if !deployed.contains(&name) {
            document.add_deferred_target(name);
        }
    }
}

/// 精确解析固定语法：`动作 [目标] [(x,y)] [朝向] [ · 说明]`。
fn parse_exact(line: &str) -> Option<ParsedAction> {
    // 只认两侧带空格的分隔符，名字里的 `·`（如「维娜·维多利亚」）不受影响。
    let (body, doc) = match line.split_once(DOC_SEPARATOR) {
        Some((body, doc)) => (body.trim(), doc.trim().to_owned()),
        None => (line.trim(), String::new()),
    };
    let (kind, rest) = KIND_WORDS.iter().find_map(|(kind, word)| {
        let rest = body.strip_prefix(word)?;
        (rest.is_empty() || rest.starts_with(' ')).then_some((*kind, rest.trim()))
    })?;

    let mut rest = rest.to_owned();
    let mut direction = Direction::Right;
    if let Some((candidate, word)) = DIRECTION_WORDS
        .iter()
        .find(|(_, word)| rest == *word || rest.ends_with(&format!(" {word}")))
    {
        direction = *candidate;
        rest.truncate(rest.len() - word.len());
        rest = rest.trim().to_owned();
    }
    let mut location = None;
    if rest.ends_with(')') {
        if let Some(start) = rest.rfind('(') {
            if let Some(point) = parse_point(&rest[start..]) {
                location = Some(point);
                rest.truncate(start);
                rest = rest.trim().to_owned();
            }
        }
    }
    // 固定语法里目标名不含空白：「部署 银灰 在 A2」这种手写句子交给关键词识别，
    // 不能把「银灰 在 A2」整段当成名字。注释行则不该有目标。
    if rest.contains(char::is_whitespace) || (kind == "Output" && !rest.is_empty()) {
        return None;
    }
    Some(ParsedAction {
        kind,
        name: rest,
        location,
        direction,
        doc,
    })
}

/// 关键词识别：动作词 + 干员名 + 可选格子 / 朝向。认不出返回 `None`。
fn guess_action(line: &str, candidates: &[String]) -> Option<ParsedAction> {
    let name = find_name(line, candidates);
    let location = find_point(line);
    // 既没有干员名也没有格子时不生成动作：「开局确认：技能等级」只是说明，
    // 当成无目标的技能反而会误导。地图装置技能可以只有格子。
    if name.is_none() && location.is_none() {
        return None;
    }
    // 弱动作词（「下」「撤」）必须找到干员名，避免「先下奶」被当成部署。
    let kind = if line.contains("撤退") {
        "Retreat"
    } else if line.contains("技能") {
        "Skill"
    } else if line.contains("部署") {
        "Deploy"
    } else if name.is_some() && line.contains('撤') {
        "Retreat"
    } else if name.is_some() && line.contains('下') {
        "Deploy"
    } else {
        return None;
    };
    Some(ParsedAction {
        kind,
        name: name.unwrap_or_default(),
        location,
        direction: find_direction(line).unwrap_or(Direction::Right),
        doc: line.to_owned(),
    })
}

fn find_name(line: &str, candidates: &[String]) -> Option<String> {
    candidates
        .iter()
        .filter(|name| !name.trim().is_empty())
        .find(|name| {
            if name.chars().count() >= 2 {
                line.contains(name.as_str())
            } else {
                // 单字名（如 W）只在作为独立词出现时才算，避免误命中普通文字。
                line.split(|ch: char| ch.is_whitespace() || ch.is_ascii_punctuation())
                    .any(|token| token == name.as_str())
            }
        })
        .cloned()
}

fn find_direction(line: &str) -> Option<Direction> {
    const WORDS: [(&str, Direction); 13] = [
        ("无朝向", Direction::None),
        ("无方向", Direction::None),
        ("朝右", Direction::Right),
        ("向右", Direction::Right),
        ("面右", Direction::Right),
        ("朝左", Direction::Left),
        ("向左", Direction::Left),
        ("面左", Direction::Left),
        ("朝上", Direction::Up),
        ("向上", Direction::Up),
        ("朝下", Direction::Down),
        ("向下", Direction::Down),
        ("面下", Direction::Down),
    ];
    WORDS
        .iter()
        .find(|(word, _)| line.contains(word))
        .map(|(_, direction)| *direction)
}

/// 找 `(x,y)` / `（x，y）` / `[x,y]` 形式的逻辑格坐标。`A2` 这类写法没有统一标准，不猜。
fn find_point(line: &str) -> Option<Point> {
    let chars: Vec<char> = line.chars().collect();
    for (start, ch) in chars.iter().enumerate() {
        let close = match ch {
            '(' => ')',
            '（' => '）',
            '[' => ']',
            _ => continue,
        };
        if let Some(len) = chars[start..]
            .iter()
            .position(|candidate| *candidate == close)
        {
            let inner: String = chars[start..=start + len].iter().collect();
            if let Some(point) = parse_point(&inner) {
                return Some(point);
            }
        }
    }
    None
}

/// 解析带括号的 `x,y`，括号可以是半角/全角圆括号或方括号。
fn parse_point(text: &str) -> Option<Point> {
    let inner = text
        .trim()
        .trim_start_matches(['(', '（', '['])
        .trim_end_matches([')', '）', ']']);
    let (x, y) = inner.split_once([',', '，'])?;
    Some(Point {
        x: x.trim().parse().ok()?,
        y: y.trim().parse().ok()?,
    })
}

// ———————————————————————— AFC 轴语法 ————————————————————————

#[derive(Debug)]
struct Cue {
    frame: i64,
    text: String,
}

#[derive(Debug, Default)]
struct ParsedAxis {
    cues: Vec<Cue>,
    title: Option<String>,
    meta: Option<String>,
    warnings: Vec<String>,
}

/// 按 AFC 打轴器的规则把轴文本切成 `(帧号, 说明)`：
///
/// - 行首帧号：`120 说明`、`120帧 说明`、`120f 说明`，或 `1:23(.5) 说明`（按 30 帧/秒）；
/// - 帧号后没有文字时，说明取后续行，直到下一条轴或下一个 `#` 标题，空行不打断；
/// - 说明块里只有写法明确的帧号（带 帧/f 后缀、`m:ss`、标题或列表行）才开启新轴；
/// - `<!--`、`-->`、`//` 开头的行是注释；列表符号、`#`、`**`、反引号、`|` 等装饰会被剥掉。
fn parse_cues(text: &str) -> ParsedAxis {
    let mut parsed = ParsedAxis::default();
    // pending：帧号后没写文字、正在收集后续说明行的那条轴的下标。
    let mut pending: Option<usize> = None;
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim_start().trim_start_matches('\u{feff}');
        if let Some(meta) = trimmed.strip_prefix(META_PREFIX) {
            parsed.meta = Some(meta.trim_end().trim_end_matches("-->").trim().to_owned());
            continue;
        }
        if trimmed.starts_with("<!--") || trimmed.starts_with("-->") || trimmed.starts_with("//") {
            continue;
        }
        let body = strip_markup(trimmed);
        if body.is_empty() {
            continue;
        }
        let heading = trimmed.starts_with('#');
        let listed = is_list_line(trimmed);
        let hit = match_frame(&body);
        let is_cue = match &hit {
            Some((_, _, explicit)) => pending.is_none() || *explicit || heading || listed,
            None => false,
        };
        if is_cue {
            let (frame, rest, _) = hit.expect("is_cue implies a frame match");
            parsed.cues.push(Cue {
                frame,
                text: rest.clone(),
            });
            pending = rest.is_empty().then_some(parsed.cues.len() - 1);
        } else if heading {
            // 第一个不是帧号的标题当作作业标题。
            if parsed.title.is_none() && parsed.cues.is_empty() {
                parsed.title = Some(body);
            }
            pending = None;
        } else if let Some(cue) = pending.and_then(|index| parsed.cues.get_mut(index)) {
            if !cue.text.is_empty() {
                cue.text.push('\n');
            }
            cue.text.push_str(&body);
        } else {
            parsed.warnings.push(format!(
                "第 {line_no} 行认不出帧号，已忽略：{}",
                line.trim()
            ));
        }
    }
    let empty = parsed
        .cues
        .iter()
        .filter(|cue| cue.text.trim().is_empty())
        .count();
    if empty > 0 {
        parsed
            .warnings
            .push(format!("{empty} 条轴没有说明，已跳过"));
    }
    // 和 AFC 一致：按帧号稳定排序，同帧保持原顺序。
    parsed.cues.sort_by_key(|cue| cue.frame);
    parsed
}

fn is_list_line(line: &str) -> bool {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix(['-', '*', '+']) {
        return rest.starts_with(char::is_whitespace);
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && line[digits..].starts_with(['.', ')'])
        && line[digits + 1..].starts_with(char::is_whitespace)
}

fn strip_markup(line: &str) -> String {
    let mut text = line.trim();
    text = text.trim_start_matches('>').trim_start();
    if is_list_line(text) {
        // 无序列表只去掉一个符号；有序列表去掉「数字 + . 或 )」。不能连带吃掉帧号。
        text = match text.strip_prefix(['-', '*', '+']) {
            Some(rest) => rest.trim_start(),
            None => {
                let digits = text.chars().take_while(char::is_ascii_digit).count();
                text[digits + 1..].trim_start()
            }
        };
    }
    text = text.trim_start_matches('#').trim_start();
    text.replace('|', " ")
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .trim()
        .to_owned()
}

/// 把一行开头解析成帧号。返回 (帧号, 剩余文字, 写法是否明确)。
fn match_frame(text: &str) -> Option<(i64, String, bool)> {
    // m:ss 或 m:ss.s
    let minutes_len = text.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&minutes_len) && text[minutes_len..].starts_with(':') {
        let after = &text[minutes_len + 1..];
        let seconds_len = after
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .map(|(index, ch)| index + ch.len_utf8())
            .last()
            .unwrap_or(0);
        let seconds_text = &after[..seconds_len];
        let whole_seconds = seconds_text.split('.').next().unwrap_or_default();
        if (1..=2).contains(&whole_seconds.len()) {
            if let (Ok(minutes), Ok(seconds)) = (
                text[..minutes_len].parse::<f64>(),
                seconds_text.parse::<f64>(),
            ) {
                let frame = ((minutes * 60.0 + seconds) * LOGICAL_FPS).round() as i64;
                return Some((frame, after[seconds_len..].trim().to_owned(), true));
            }
        }
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let frame = text[..digits].parse::<i64>().ok()?;
    let rest = text[digits..].trim_start();
    for suffix in ["帧", "f", "F"] {
        if let Some(after) = rest.strip_prefix(suffix) {
            return Some((frame, after.trim().to_owned(), true));
        }
    }
    Some((frame, rest.trim().to_owned(), false))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JOB: &str = r#"{
        "stage_name": "main_01-07",
        "frame_replicator": {"version": 1, "speed": "1x", "deferred_targets": ["哨戒炮台"]},
        "doc": {"title": "桃金娘单人", "details": ""},
        "opers": [{"name": "桃金娘", "skill": 2}, {"name": "维娜·维多利亚", "skill": 1}],
        "actions": [
            {"type": "Deploy", "frame": 10, "name": "桃金娘", "location": [3, 2], "direction": "Left", "doc": "第一行\n第二行 | 竖线"},
            {"type": "Deploy", "frame": 10, "name": "维娜·维多利亚", "location": [4, 2], "direction": "Down"},
            {"type": "Skill", "frame": 40, "name": "桃金娘"},
            {"type": "Skill", "frame": 45, "name": "", "location": [6, 1], "doc": "地图装置"},
            {"type": "Retreat", "frame": 60, "name": "桃金娘"},
            {"type": "Output", "frame": 60, "doc": "收尾 · 结束"}
        ]
    }"#;

    #[test]
    fn export_writes_one_afc_readable_line_per_action() {
        let document = JobDocument::parse(SAMPLE_JOB).unwrap();
        let (text, skipped) = export_axis(&document);
        assert_eq!(skipped, 0);
        assert!(text.starts_with("# 作业：桃金娘单人\n"));
        assert!(text.contains("- 10 部署 桃金娘 (3,2) 朝左 · 第一行 / 第二行 / 竖线\n"));
        assert!(text.contains("- 10 部署 维娜·维多利亚 (4,2) 朝下\n"));
        assert!(text.contains("- 40 技能 桃金娘\n"));
        assert!(text.contains("- 45 技能 (6,1) · 地图装置\n"));
        assert!(text.contains("- 60 撤退 桃金娘\n"));
        assert!(text.contains("- 60 注释 · 收尾 · 结束\n"));
        // AFC 会剥掉竖线、粗体和反引号，导出时不能留下它们。
        let cue_lines: Vec<_> = text.lines().filter(|line| line.starts_with("- ")).collect();
        assert!(cue_lines
            .iter()
            .all(|line| !line.contains('|') && !line.contains('`')));
    }

    #[test]
    fn exported_axis_round_trips_without_losing_structure() {
        let original = JobDocument::parse(SAMPLE_JOB).unwrap();
        let (text, _) = export_axis(&original);
        let imported = import_axis(&text, &[]);
        assert_eq!(imported.exact, 6);
        assert_eq!(imported.guessed, 0);
        assert_eq!(imported.notes, 0);

        let document = imported.document;
        assert_eq!(document.stage_name(), "main_01-07");
        assert_eq!(document.title(), "桃金娘单人");
        assert_eq!(document.operator_names(), vec!["桃金娘", "维娜·维多利亚"]);
        assert_eq!(document.deferred_target_names(), vec!["哨戒炮台"]);
        let skills: Vec<_> = document.root()["opers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|oper| oper["skill"].as_u64().unwrap())
            .collect();
        assert_eq!(skills, vec![2, 1]);

        let key = |action: &ActionDraft| {
            (
                ActionType::parse(&action.kind),
                action.frame,
                action.name.clone(),
                action.location,
                (ActionType::parse(&action.kind) == Some(ActionType::Deploy))
                    .then_some(action.direction),
            )
        };
        let before: Vec<_> = original.actions().iter().map(key).collect();
        let after: Vec<_> = document.actions().iter().map(key).collect();
        assert_eq!(before, after);
        // 说明里的换行在 AFC 中无法表达，往返后压成 " / "；其余文字保持不变。
        assert_eq!(document.actions()[0].doc, "第一行 / 第二行 / 竖线");
        assert_eq!(document.actions()[5].doc, "收尾 · 结束");
        assert!(!document.has_errors(), "{:?}", document.diagnostics());
    }

    #[test]
    fn afc_example_axis_imports_as_a_draft_with_guesses_and_notes() {
        // 节选自 AFC 仓库的 maps/example.md。
        let text = "# 示例轴（照着改）\n\
            <!-- 每行一条轴：帧号 + 空格 + 说明。 -->\n\
            \n\
            ## 0 帧\n\
            开局确认：编队 / 技能等级 / 装置位\n\
            \n\
            ## 120 帧\n\
            部署 银灰 在 A2\n\
            先下奶，再下输出\n\
            \n\
            - 350 开银灰技能\n\
            - 600 部署 能天使 朝右\n\
            - 900 撤退 银灰\n\
            - 1:05 开能天使技能\n";
        let names = vec!["银灰".to_owned(), "能天使".to_owned(), "天使".to_owned()];
        let imported = import_axis(text, &names);
        let actions = imported.document.actions();
        let summary: Vec<_> = actions
            .iter()
            .map(|action| {
                (
                    action.frame.unwrap(),
                    action.kind.as_str(),
                    action.name.as_str(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, "Output", ""),
                (120, "Deploy", "银灰"),
                (120, "Output", ""),
                (350, "Skill", "银灰"),
                (600, "Deploy", "能天使"),
                (900, "Retreat", "银灰"),
                (1950, "Skill", "能天使"),
            ]
        );
        assert_eq!(imported.document.title(), "示例轴（照着改）");
        // 「A2」不猜格子，部署缺格子会被诊断标红，草稿不能直接运行。
        assert_eq!(actions[1].location, None);
        assert!(imported.document.has_errors());
        // 认不出的行原文保留在注释里。
        assert_eq!(actions[0].doc, "开局确认：编队 / 技能等级 / 装置位");
        assert_eq!(actions[2].doc, "先下奶，再下输出");
        assert_eq!(
            (imported.exact, imported.guessed, imported.notes),
            (2, 3, 2)
        );
        assert_eq!(imported.document.operator_names(), vec!["银灰", "能天使"]);
        assert!(imported
            .warnings
            .iter()
            .any(|warning| warning.contains("关卡")));
    }

    #[test]
    fn afc_cue_syntax_variants_are_recognized() {
        let text = "120 部署 A\n120帧 部署 B\n120f 部署 C\n**130** 撤退 A\n| 140 | 技能 B |\n\
                    1:23.5 注释 · 时间写法\n// 注释行\n## 200\n多行说明第一行\n\n第二行\n# 新标题\n不属于任何轴\n";
        let parsed = parse_cues(text);
        let frames: Vec<_> = parsed.cues.iter().map(|cue| cue.frame).collect();
        assert_eq!(frames, vec![120, 120, 120, 130, 140, 200, 2505]);
        assert_eq!(parsed.cues[5].text, "多行说明第一行\n第二行");
        assert_eq!(parsed.cues[4].text, "技能 B");
        assert!(parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("第 13 行")));
    }

    #[test]
    fn plain_numbers_inside_a_description_block_do_not_start_new_cues() {
        let text = "## 120\n2 秒后开技能\n300帧 撤退\n";
        let parsed = parse_cues(text);
        let summary: Vec<_> = parsed
            .cues
            .iter()
            .map(|cue| (cue.frame, cue.text.as_str()))
            .collect();
        assert_eq!(summary, vec![(120, "2 秒后开技能"), (300, "撤退")]);
    }

    #[test]
    fn weak_verbs_need_an_operator_name_and_points_parse_in_several_forms() {
        let names = vec!["银灰".to_owned()];
        assert!(guess_action("先下奶，再下输出", &names).is_none());
        let deploy = guess_action("下银灰（3，4）向下", &names).unwrap();
        assert_eq!(deploy.kind, "Deploy");
        assert_eq!(deploy.location, Some(Point { x: 3, y: 4 }));
        assert_eq!(deploy.direction, Direction::Down);
        assert_eq!(find_point("部署 [5,6]"), Some(Point { x: 5, y: 6 }));
        assert_eq!(find_point("部署 在 A2"), None);
        // 单字名只在独立出现时命中。
        let short = vec!["W".to_owned()];
        assert!(find_name("WOW 部署", &short).is_none());
        assert_eq!(find_name("部署 W 朝左", &short).as_deref(), Some("W"));
    }

    #[test]
    fn every_repository_example_job_round_trips_through_an_axis() {
        let examples = [
            include_str!("../../../examples/sample-job.json"),
            include_str!("../../../examples/sample-job 2.json"),
            include_str!("../../../examples/sample-job 3.json"),
            include_str!("../../../examples/test1.json"),
            include_str!("../../../examples/test2.json"),
            include_str!("../../../examples/test3.json"),
        ];
        for (index, json) in examples.iter().enumerate() {
            let original = JobDocument::parse(json).unwrap();
            let (text, skipped) = export_axis(&original);
            assert_eq!(skipped, 0, "example #{index}");
            // 导出的轴里除了动作行不能有任何会被 AFC 当成轴的行（例如数字开头的标题）。
            assert_eq!(
                parse_cues(&text).cues.len(),
                original.actions().len(),
                "example #{index}"
            );
            let imported = import_axis(&text, &[]);
            assert_eq!(imported.guessed + imported.notes, 0, "example #{index}");

            let key = |action: &ActionDraft| {
                let kind = ActionType::parse(&action.kind);
                (
                    kind,
                    action.frame,
                    action.name.clone(),
                    action.location,
                    (kind == Some(ActionType::Deploy)).then_some(action.direction),
                    single_line(&action.doc),
                )
            };
            let before: Vec<_> = original.actions().iter().map(key).collect();
            let after: Vec<_> = imported.document.actions().iter().map(key).collect();
            assert_eq!(before, after, "example #{index}");
            assert_eq!(imported.document.stage_name(), original.stage_name());
            assert_eq!(
                imported.document.operator_names(),
                original.operator_names()
            );
            assert_eq!(
                original.has_errors(),
                imported.document.has_errors(),
                "example #{index}: {:?}",
                imported.document.diagnostics()
            );
        }
    }

    #[test]
    fn frames_without_a_value_are_skipped_on_export() {
        let mut document = JobDocument::default();
        let action = document.new_action("Deploy", None);
        document.insert_action(action);
        let (text, skipped) = export_axis(&document);
        assert_eq!(skipped, 1);
        assert!(!text.lines().any(|line| line.starts_with("- ")));
    }
}
