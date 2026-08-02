// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! 作业编辑器的纯状态层。
//!
//! 这里不依赖窗口、尺子或输入注入。GUI 只能通过这些编辑命令改变文档，因此编辑模式
//! 可以在没有游戏、AFA 和尺子的情况下独立工作，也不会意外创建自动化会话。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use repl_core::{ActionDraft, ActionId, JobDocument};

use crate::config::atomic_write;

const HISTORY_LIMIT: usize = 200;

#[derive(Clone)]
struct Snapshot {
    document: JobDocument,
    selected: Option<ActionId>,
    state_id: u64,
}

/// 一个打开的作业编辑会话。
#[derive(Clone)]
pub struct EditorState {
    document: JobDocument,
    path: Option<PathBuf>,
    selected: Option<ActionId>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    state_id: u64,
    saved_state_id: u64,
    next_state_id: u64,
    cursor_frame: i64,
    follow_ruler: bool,
    pixels_per_frame: f32,
}

impl Default for EditorState {
    fn default() -> Self {
        Self {
            document: JobDocument::default(),
            path: None,
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            state_id: 0,
            saved_state_id: 0,
            next_state_id: 1,
            cursor_frame: 0,
            follow_ruler: false,
            pixels_per_frame: 4.0,
        }
    }
}

impl EditorState {
    pub fn open(json: &str, path: Option<PathBuf>) -> Result<Self> {
        let document = JobDocument::parse(json).context("无法打开作业文档")?;
        Ok(Self {
            document,
            path,
            ..Self::default()
        })
    }

    pub fn document(&self) -> &JobDocument {
        &self.document
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn selected(&self) -> Option<ActionId> {
        self.selected
    }

    pub fn select(&mut self, id: Option<ActionId>) {
        self.selected = id.filter(|id| self.document.action(*id).is_some());
    }

    pub fn selected_action(&self) -> Option<ActionDraft> {
        self.selected.and_then(|id| self.document.action(id))
    }

    pub fn cursor_frame(&self) -> i64 {
        self.cursor_frame
    }

    pub fn set_cursor_frame(&mut self, frame: i64) {
        self.cursor_frame = frame.max(0);
    }

    pub fn follow_ruler(&self) -> bool {
        self.follow_ruler
    }

    pub fn set_follow_ruler(&mut self, follow: bool) {
        self.follow_ruler = follow;
    }

    pub fn pixels_per_frame(&self) -> f32 {
        self.pixels_per_frame
    }

    pub fn set_pixels_per_frame(&mut self, value: f32) {
        self.pixels_per_frame = value.clamp(0.5, 24.0);
    }

    pub fn is_dirty(&self) -> bool {
        self.path.is_none() || self.state_id != self.saved_state_id
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn set_title(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.document.title() != value {
            self.edit(|document| document.set_title(value));
        }
    }

    pub fn set_stage_name(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.document.stage_name() != value {
            self.edit(|document| document.set_stage_name(value));
        }
    }

    pub fn set_details(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.document.details() != value {
            self.edit(|document| document.set_details(value));
        }
    }

    pub fn add_action(&mut self, kind: impl Into<String>) -> ActionId {
        let kind = kind.into();
        let before = self.snapshot();
        let action = self.document.new_action(kind, Some(self.cursor_frame));
        let id = action.id;
        self.document.insert_action(action);
        self.selected = Some(id);
        self.commit(before);
        id
    }

    pub fn update_action(&mut self, action: ActionDraft) -> Result<()> {
        if self.document.action(action.id).as_ref() == Some(&action) {
            return Ok(());
        }
        let before = self.snapshot();
        self.document
            .update_action(&action)
            .context("无法更新动作")?;
        self.selected = Some(action.id);
        self.commit(before);
        Ok(())
    }

    pub fn remove_selected(&mut self) -> bool {
        let Some(id) = self.selected else {
            return false;
        };
        let before = self.snapshot();
        if self.document.remove_action(id).is_none() {
            return false;
        }
        self.selected = None;
        self.commit(before);
        true
    }

    pub fn duplicate_selected(&mut self) -> Option<ActionId> {
        let id = self.selected?;
        let before = self.snapshot();
        let duplicated = self.document.duplicate_action(id)?;
        self.selected = Some(duplicated);
        self.commit(before);
        Some(duplicated)
    }

    pub fn move_selected_same_frame(&mut self, delta: isize) -> bool {
        let Some(id) = self.selected else {
            return false;
        };
        let before = self.snapshot();
        if !self.document.move_within_same_frame(id, delta) {
            return false;
        }
        self.commit(before);
        true
    }

    pub fn add_operator(&mut self, name: impl Into<String>, skill: u8) {
        let name = name.into();
        self.edit(|document| document.add_operator(name, skill));
    }

    pub fn rename_operator(&mut self, old_name: &str, new_name: &str) -> bool {
        let before = self.snapshot();
        if !self.document.rename_operator(old_name, new_name) {
            return false;
        }
        self.commit(before);
        true
    }

    pub fn remove_operator(&mut self, name: &str) -> bool {
        let before = self.snapshot();
        if !self.document.remove_operator(name) {
            return false;
        }
        self.commit(before);
        true
    }

    pub fn add_deferred_target(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.edit(|document| document.add_deferred_target(name));
    }

    pub fn rename_deferred_target(&mut self, old_name: &str, new_name: &str) -> bool {
        let before = self.snapshot();
        if !self.document.rename_deferred_target(old_name, new_name) {
            return false;
        }
        self.commit(before);
        true
    }

    pub fn remove_deferred_target(&mut self, name: &str) -> bool {
        let before = self.snapshot();
        if !self.document.remove_deferred_target(name) {
            return false;
        }
        self.commit(before);
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    /// 把当前草稿安全地写到同目录临时文件，再原子替换目标文件。
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf> {
        let target = path
            .map(Path::to_path_buf)
            .or_else(|| self.path.clone())
            .context("尚未选择作业保存位置")?;
        let json = self.document.to_pretty_json().context("无法序列化作业")?;
        atomic_write(&target, json.as_bytes())
            .with_context(|| format!("无法保存作业到 {}", target.display()))?;
        self.path = Some(target.clone());
        self.saved_state_id = self.state_id;
        Ok(target)
    }

    fn edit(&mut self, operation: impl FnOnce(&mut JobDocument)) {
        let before = self.snapshot();
        operation(&mut self.document);
        self.commit(before);
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            document: self.document.clone(),
            selected: self.selected,
            state_id: self.state_id,
        }
    }

    fn commit(&mut self, before: Snapshot) {
        self.undo.push(before);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.state_id = self.next_state_id;
        self.next_state_id += 1;
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.document = snapshot.document;
        self.selected = snapshot.selected;
        self.state_id = snapshot.state_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn edits_undo_redo_and_dirty_state_follow_document_revisions() {
        let json = JobDocument::default().to_pretty_json().unwrap();
        let mut editor = EditorState::open(&json, Some(PathBuf::from("job.json"))).unwrap();
        assert!(!editor.is_dirty());
        editor.set_title("测试作业");
        let action_id = editor.add_action("Output");
        assert_eq!(editor.selected(), Some(action_id));
        assert!(editor.is_dirty());
        assert!(editor.undo());
        assert!(editor.document().actions().is_empty());
        assert!(editor.undo());
        assert_eq!(editor.document().title(), "未命名作业");
        assert!(!editor.is_dirty());
        assert!(editor.redo());
        assert!(editor.is_dirty());
    }

    #[test]
    fn new_action_uses_the_timeline_cursor_frame() {
        let mut editor = EditorState::default();
        editor.set_cursor_frame(321);
        editor.add_action("Deploy");
        assert_eq!(editor.selected_action().unwrap().frame, Some(321));
    }

    #[test]
    fn invalid_draft_saves_and_reopens_without_becoming_runnable() {
        let unique = format!("repl-editor-{}.json", std::process::id());
        let path = std::env::temp_dir().join(unique);
        let mut editor = EditorState::default();
        editor.add_action("Deploy");
        editor.save(Some(&path)).unwrap();
        assert!(!editor.is_dirty());

        let json = fs::read_to_string(&path).unwrap();
        let reopened = EditorState::open(&json, Some(path.clone())).unwrap();
        assert!(reopened.document().has_errors());
        assert_eq!(reopened.document().actions().len(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_new_edit_after_undo_clears_redo_history() {
        let mut editor = EditorState::default();
        editor.set_title("一");
        editor.set_title("二");
        assert!(editor.undo());
        assert!(editor.can_redo());
        editor.set_title("三");
        assert!(!editor.can_redo());
    }
}
