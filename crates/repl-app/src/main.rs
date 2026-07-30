// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! `replicator` —— 明日方舟帧级操作复刻器。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::Duration,
};

use repl_app::{
    config::{fingerprint, perceptual_hash, Binding, BindingProfile, BindingStore, Config},
    editor::EditorState,
    runner::{
        continuation_sample_ready, continuation_snapshot_plan, start_snapshot_ready, BindingMode,
        Progress, RunMode, Runner,
    },
    session::Session,
};
use repl_core::{
    continuation::ContinuationPlan, copilot::ActionType, level::Buildable, Copilot,
    DiagnosticSeverity, Direction, LevelPack, Point,
};
use repl_frames::{FrameSource, RulerClient};
use slint::{Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

/// 用户在绑定面板里填的"卡片序号 → 干员名"。
type BindingChoices = Vec<(usize, String)>;

/// 绑定面板与工作线程之间的握手：UI 线程在用户点"确认"时填入选择，
/// 工作线程轮询取走。卡片数据走 `Progress::NeedsBinding` 事件，不经过这里。
struct BindingBridge {
    choices: Mutex<Option<BindingChoices>>,
}

/// The UI window handle is captured on the Slint thread and used only to make the
/// binding panel visible once. The game window is never activated by this helper.
#[derive(Clone, Copy, Debug)]
struct UiWindowControl {
    hwnd: isize,
}

#[cfg(windows)]
impl UiWindowControl {
    fn from_ui(ui: &MainWindow) -> Option<Self> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let handle = ui.window().window_handle();
        let raw = handle.window_handle().ok()?.as_raw();
        match raw {
            RawWindowHandle::Win32(win32) => Some(Self {
                hwnd: win32.hwnd.get(),
            }),
            _ => None,
        }
    }

    fn activate_once(self) -> bool {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

        // SAFETY: the handle comes from Slint's live Win32 window.
        unsafe { SetForegroundWindow(HWND(self.hwnd as *mut core::ffi::c_void)).as_bool() }
    }

    fn is_foreground(self) -> bool {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

        // SAFETY: comparing HWND values does not dereference the handle.
        unsafe { GetForegroundWindow() == HWND(self.hwnd as *mut core::ffi::c_void) }
    }
}

#[cfg(not(windows))]
impl UiWindowControl {
    fn from_ui(_ui: &MainWindow) -> Option<Self> {
        None
    }

    fn activate_once(self) -> bool {
        let _ = self;
        false
    }

    fn is_foreground(self) -> bool {
        let _ = self;
        false
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // GUI 没有控制台，日志写到 exe 旁边的 replicator.log（每次启动覆盖）。
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("replicator.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("replicator.log"));
    // repl_app 开 debug：被拒样本的逐条记录只在 debug 级，出问题时全靠它定位。
    repl_app::init_to_file("info,repl_app=debug,repl_core::machine=debug", &log_path);

    let config = Rc::new(RefCell::new(Config::load()));
    let ui = MainWindow::new()?;

    ui.set_disclaimer_visible(!config.borrow().disclaimer_accepted);
    ui.set_actions(ModelRc::new(VecModel::<ActionRow>::default()));
    ui.set_cards(ModelRc::new(VecModel::<CardRow>::default()));
    ui.set_unbound_opers(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_afa_ready(false);
    ui.set_afa_detail("未检测".into());
    ui.set_can_continue(false);
    ui.set_continue_visible(false);
    ui.set_continue_confirm_enabled(false);
    ui.set_editor_actions(ModelRc::new(VecModel::<EditorActionRow>::default()));
    ui.set_editor_diagnostics(ModelRc::new(VecModel::<DiagnosticRow>::default()));
    ui.set_editor_operators(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_editor_map_cells(ModelRc::new(VecModel::<MapCell>::default()));

    let ruler = Arc::new(RulerClient::connect(config.borrow().ruler_ws_url.clone()));
    let copilot: Rc<RefCell<Option<Copilot>>> = Rc::new(RefCell::new(None));
    let pending_continuation: Rc<RefCell<Option<ContinuationPlan>>> = Rc::new(RefCell::new(None));
    let editor = Rc::new(RefCell::new(EditorState::default()));
    let level_pack = Rc::new(
        config
            .borrow()
            .resolve_maa_resource_dir()
            .and_then(|directory| LevelPack::load(directory.join("Arknights-Tile-Pos")).ok()),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let log = Rc::new(RefCell::new(String::new()));
    refresh_editor(&ui, &editor.borrow(), &copilot);
    refresh_editor_map(&ui, level_pack.as_ref().as_ref(), &editor.borrow());

    // ——— 免责声明 ———
    {
        let ui_weak = ui.as_weak();
        let config = Rc::clone(&config);
        ui.on_accept_disclaimer(move || {
            config.borrow_mut().disclaimer_accepted = true;
            if let Err(e) = config.borrow().save() {
                log::warn!("could not save config: {e}");
            }
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_disclaimer_visible(false);
            }
        });
    }

    // ——— 作业编辑器（纯文档操作，不创建 Session/AFA/输入注入）———
    {
        let ui_weak = ui.as_weak();
        ui.on_switch_mode(move |editor_mode| {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_running() {
                return;
            }
            ui.set_editor_mode(editor_mode);
            if editor_mode {
                ui.set_afa_ready(false);
                ui.set_afa_detail("编辑模式不探测".into());
                ui.set_game_found(false);
                ui.set_battle_state("—".into());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        let level_pack = Rc::clone(&level_pack);
        ui.on_editor_new(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            *editor.borrow_mut() = EditorState::default();
            refresh_editor(&ui, &editor.borrow(), &copilot);
            refresh_editor_map(&ui, level_pack.as_ref().as_ref(), &editor.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        let level_pack = Rc::clone(&level_pack);
        ui.on_editor_open(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = pick_job_file().map(std::path::PathBuf::from) else {
                return;
            };
            let opened = std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|json| EditorState::open(&json, Some(path.clone())));
            match opened {
                Ok(state) => {
                    *editor.borrow_mut() = state;
                    refresh_editor(&ui, &editor.borrow(), &copilot);
                    refresh_editor_map(&ui, level_pack.as_ref().as_ref(), &editor.borrow());
                }
                Err(error) => {
                    ui.set_editor_path(format!("打开失败：{error}").into());
                }
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_save(move |save_as| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let chosen = if save_as || editor.borrow().path().is_none() {
                pick_save_file().map(std::path::PathBuf::from)
            } else {
                None
            };
            if (save_as || editor.borrow().path().is_none()) && chosen.is_none() {
                return;
            }
            match save_editor_then(&editor, chosen.as_deref(), |state| {
                refresh_editor(&ui, state, &copilot);
            }) {
                Ok(()) => {}
                Err(error) => ui.set_editor_path(format!("保存失败：{error}").into()),
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_undo(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().undo();
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_redo(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().redo();
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_add_action(move |kind| {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().add_action(kind.as_str());
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_select(move |id| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let selected = editor
                .borrow()
                .document()
                .actions()
                .into_iter()
                .find(|action| action.id.get() == id as u64)
                .map(|action| action.id);
            editor.borrow_mut().select(selected);
            refresh_editor(&ui, &editor.borrow(), &copilot);
            set_editor_map_selection(
                &ui,
                editor.borrow().selected_action().and_then(|a| a.location),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_delete(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().remove_selected();
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_duplicate(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().duplicate_selected();
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_move_order(move |delta| {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().move_selected_same_frame(delta as isize);
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        let level_pack = Rc::clone(&level_pack);
        ui.on_editor_set_metadata(move |field, value| {
            let Some(ui) = ui_weak.upgrade() else { return };
            match field.as_str() {
                "title" => editor.borrow_mut().set_title(value.as_str()),
                "stage" => editor.borrow_mut().set_stage_name(value.as_str()),
                "details" => editor.borrow_mut().set_details(value.as_str()),
                _ => return,
            }
            refresh_editor(&ui, &editor.borrow(), &copilot);
            if field.as_str() == "stage" {
                refresh_editor_map(&ui, level_pack.as_ref().as_ref(), &editor.borrow());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_set_action(move |field, value| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(mut action) = editor.borrow().selected_action() else {
                return;
            };
            match field.as_str() {
                "frame" => action.frame = value.parse::<i64>().ok(),
                "name" => action.name = value.to_string(),
                "doc" => action.doc = value.to_string(),
                "direction" => action.direction = parse_editor_direction(value.as_str()),
                _ => return,
            }
            if let Err(error) = editor.borrow_mut().update_action(action) {
                log::warn!("editor update failed: {error}");
            }
            refresh_editor(&ui, &editor.borrow(), &copilot);
            set_editor_map_selection(
                &ui,
                editor.borrow().selected_action().and_then(|a| a.location),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_select_map_cell(move |x, y| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(mut action) = editor.borrow().selected_action() else {
                return;
            };
            action.location = Some(Point::new(x, y));
            if let Err(error) = editor.borrow_mut().update_action(action) {
                log::warn!("editor map selection failed: {error}");
            }
            refresh_editor(&ui, &editor.borrow(), &copilot);
            set_editor_map_selection(&ui, Some(Point::new(x, y)));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_add_operator(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let name = name.trim();
            if name.is_empty()
                || editor
                    .borrow()
                    .document()
                    .operator_names()
                    .iter()
                    .any(|n| n == name)
            {
                return;
            }
            editor.borrow_mut().add_operator(name, 1);
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_rename_operator(move |old_name, new_name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return;
            }
            editor
                .borrow_mut()
                .rename_operator(old_name.as_str(), new_name);
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_remove_operator(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().remove_operator(name.as_str());
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_set_location(move |x, y| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(mut action) = editor.borrow().selected_action() else {
                return;
            };
            let Some(location) = x
                .parse::<i32>()
                .ok()
                .zip(y.parse::<i32>().ok())
                .map(|(x, y)| Point::new(x, y))
            else {
                // 允许用户先填一个坐标框；两个框都成为有效整数后再提交一个编辑命令。
                return;
            };
            action.location = Some(location);
            if let Err(error) = editor.borrow_mut().update_action(action) {
                log::warn!("editor location update failed: {error}");
            }
            refresh_editor(&ui, &editor.borrow(), &copilot);
            set_editor_map_selection(
                &ui,
                editor.borrow().selected_action().and_then(|a| a.location),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        ui.on_editor_set_follow(move |follow| {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().set_follow_ruler(follow);
            ui.set_editor_follow_ruler(follow);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        ui.on_editor_set_cursor(move |value| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Ok(frame) = value.parse::<i64>() else {
                return;
            };
            editor.borrow_mut().set_cursor_frame(frame);
            ui.set_editor_cursor_frame(frame.min(i32::MAX as i64) as i32);
            ui.set_editor_view_start(frame.saturating_sub(30).min(i32::MAX as i64) as i32);
        });
    }

    // ——— 装载作业 ———
    {
        let ui_weak = ui.as_weak();
        let copilot = Rc::clone(&copilot);
        let log = Rc::clone(&log);
        ui.on_load_job(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = pick_job_file() else { return };
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| Copilot::parse(&text).map_err(|e| e.to_string()))
            {
                Ok(job) => {
                    ui.set_job_name(
                        format!(
                            "{} · {} 个动作 · 关卡 {}",
                            if job.title.is_empty() {
                                "未命名作业"
                            } else {
                                &job.title
                            },
                            job.actions.len(),
                            job.stage_name
                        )
                        .into(),
                    );
                    ui.set_actions(build_timeline(&job));
                    // The live timer enables Start only after AFA and the ruler prove
                    // that the user is already in a paused battle.
                    ui.set_can_start(false);
                    ui.set_status_line(
                        "作业已装载：进入关卡并等待 AFA 暂停后再点「开始复刻」".into(),
                    );
                    append_log(&ui, &log, &format!("已装载作业：{}", path));
                    *copilot.borrow_mut() = Some(job);
                }
                Err(e) => {
                    ui.set_can_start(false);
                    ui.set_status_line(format!("作业装载失败：{e}").into());
                    append_log(&ui, &log, &format!("装载失败：{e}"));
                }
            }
        });
    }

    // ——— 绑定桥 ———
    let bridge = Arc::new(BindingBridge {
        choices: Mutex::new(None),
    });
    let pending_choices: Rc<RefCell<BindingChoices>> = Rc::new(RefCell::new(Vec::new()));
    // 本次绑定要求的干员名单（来自 NeedsBinding 事件），用来实时计算"还没绑定"。
    let binding_opers: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let ui_weak = ui.as_weak();
        let pending = Rc::clone(&pending_choices);
        let opers = Rc::clone(&binding_opers);
        ui.on_bind_card(move |index, name| {
            let mut pending = pending.borrow_mut();
            let index = index as usize;
            pending.retain(|(i, _)| *i != index);
            if !name.trim().is_empty() {
                pending.push((index, name.trim().to_owned()));
            }
            // 实时刷新"还没绑定"列表 —— 确认按钮的可用性就靠它。
            // 只认作业里声明过的名字：填错名字不会让按钮亮起来，
            // 用户能立刻从"还没绑定：N 名干员"看出有问题。
            if let Some(ui) = ui_weak.upgrade() {
                let assigned: Vec<&str> = pending.iter().map(|(_, n)| n.as_str()).collect();
                let unbound: Vec<SharedString> = opers
                    .borrow()
                    .iter()
                    .filter(|o| !assigned.contains(&o.as_str()))
                    .map(|o| SharedString::from(o.as_str()))
                    .collect();
                ui.set_unbound_opers(ModelRc::new(VecModel::from(unbound)));
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let bridge = Arc::clone(&bridge);
        let pending = Rc::clone(&pending_choices);
        ui.on_binding_confirm(move || {
            *bridge.choices.lock().unwrap() = Some(pending.borrow().clone());
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_binding_visible(false);
            }
        });
    }

    // ——— 开始 / 中止 ———
    {
        let ui_weak = ui.as_weak();
        let copilot = Rc::clone(&copilot);
        let config = Rc::clone(&config);
        let ruler = Arc::clone(&ruler);
        let cancel = Arc::clone(&cancel);
        let bridge = Arc::clone(&bridge);
        let log = Rc::clone(&log);
        let pending_choices = Rc::clone(&pending_choices);
        let binding_opers = Rc::clone(&binding_opers);

        ui.on_start_run(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_editor_mode() {
                ui.set_status_line("编辑模式严格禁止输入；请先切到「复刻执行」".into());
                return;
            }
            let Some(job) = copilot.borrow().clone() else {
                ui.set_status_line("请先装载作业".into());
                return;
            };
            let afa = repl_input::AfaController::probe();
            if !afa.ready {
                ui.set_status_line(format!("AFA 未就绪：{}", afa.detail).into());
                append_log(
                    &ui,
                    &log,
                    &format!("开始被拒绝：AFA 未就绪：{}", afa.detail),
                );
                return;
            }
            let Some(snapshot) = ruler.latest() else {
                ui.set_status_line("开始被拒绝：尚未收到尺子样本".into());
                append_log(&ui, &log, "开始被拒绝：尚未收到尺子样本");
                return;
            };
            let Some(first_action) = job.actions.first().map(|action| action.frame) else {
                ui.set_status_line("开始被拒绝：作业没有动作".into());
                return;
            };
            let view = repl_app::runner::to_view(&snapshot);
            if !start_snapshot_ready(&snapshot, first_action) {
                let message = format!(
                    "开始被拒绝：请先进入关卡并等待 AFA 停在 1x_paused（当前 state={} elapsed={}）",
                    view.battle_state, view.elapsed
                );
                ui.set_status_line(message.clone().into());
                append_log(&ui, &log, &message);
                return;
            }
            launch_run(
                &ui,
                ui_weak.clone(),
                job,
                config.borrow().clone(),
                RunMode::FromZero,
                Arc::clone(&ruler),
                Arc::clone(&cancel),
                Arc::clone(&bridge),
                Rc::clone(&log),
                Rc::clone(&pending_choices),
                Rc::clone(&binding_opers),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let copilot = Rc::clone(&copilot);
        let ruler = Arc::clone(&ruler);
        let pending = Rc::clone(&pending_continuation);
        let log = Rc::clone(&log);
        ui.on_request_continue(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_editor_mode() || ui.get_running() {
                return;
            }
            let Some(job) = copilot.borrow().clone() else {
                ui.set_status_line("继续被拒绝：请先装载作业".into());
                return;
            };
            let afa = repl_input::AfaController::probe();
            if !afa.ready {
                ui.set_status_line(format!("继续被拒绝：AFA 未就绪：{}", afa.detail).into());
                return;
            }
            let Some(snapshot) = ruler.latest() else {
                ui.set_status_line("继续被拒绝：尚未收到尺子样本".into());
                return;
            };
            let view = repl_app::runner::to_view(&snapshot);
            if snapshot.frame_id.is_none() || !continuation_sample_ready(&view) {
                let message = format!(
                    "继续被拒绝：需要带 frame_id 的可信战斗内 1x_paused 非零帧；当前 state={} elapsed={}",
                    view.battle_state, view.elapsed
                );
                ui.set_status_line(message.clone().into());
                append_log(&ui, &log, &message);
                return;
            }
            let plan = match ContinuationPlan::build(&job, view.elapsed) {
                Ok(plan) => plan,
                Err(error) => {
                    let message = format!("继续被拒绝：{error}");
                    ui.set_status_line(message.clone().into());
                    append_log(&ui, &log, &message);
                    return;
                }
            };

            let skipped = job.actions[..plan.next_action_index]
                .iter()
                .enumerate()
                .map(|(index, action)| {
                    let already_confirmed = ui
                        .get_actions()
                        .row_data(index)
                        .is_some_and(|row| row.status == 2);
                    format!(
                        "#{index}  F{}  {}  {}{}",
                        action.frame,
                        action_type_label(action.kind),
                        action.name,
                        if already_confirmed { "（尺子已确认）" } else { "" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let skipped = if skipped.is_empty() {
                "（无；下一动作仍按原作业执行）".to_owned()
            } else {
                skipped
            };
            let battlefield = if plan.battlefield.is_empty() {
                "（按作业推导为空）".to_owned()
            } else {
                plan.battlefield
                    .iter()
                    .map(|(name, point)| format!("{name} @ ({}, {})", point.x, point.y))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            let next = plan.next_action(&job);
            let issues = continuation_archive_issues(&plan, &BindingStore::load());
            ui.set_continue_cutoff_text(
                format!(
                    "冻结截止帧：F{}（该帧及以前共 {} 个动作）",
                    plan.cutoff_frame,
                    plan.assumed_action_count()
                )
                .into(),
            );
            ui.set_continue_skipped_text(skipped.into());
            ui.set_continue_battlefield_text(battlefield.into());
            ui.set_continue_next_text(
                format!(
                    "#{}  F{}  {}  {}",
                    plan.next_action_index,
                    next.frame,
                    action_type_label(next.kind),
                    next.name
                )
                .into(),
            );
            ui.set_continue_archive_text(if issues.is_empty() {
                "头像档案检查通过。".into()
            } else {
                format!("无法继续：{}", issues.join("；")).into()
            });
            ui.set_continue_confirm_enabled(issues.is_empty());
            *pending.borrow_mut() = Some(plan);
            ui.set_continue_visible(true);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let copilot = Rc::clone(&copilot);
        let config = Rc::clone(&config);
        let ruler = Arc::clone(&ruler);
        let cancel = Arc::clone(&cancel);
        let bridge = Arc::clone(&bridge);
        let log = Rc::clone(&log);
        let pending_choices = Rc::clone(&pending_choices);
        let binding_opers = Rc::clone(&binding_opers);
        let pending = Rc::clone(&pending_continuation);
        ui.on_confirm_continue(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(plan) = pending.borrow().clone() else {
                ui.set_continue_visible(false);
                return;
            };
            let Some(job) = copilot.borrow().clone() else {
                ui.set_status_line("继续被拒绝：作业已卸载".into());
                return;
            };
            let afa = repl_input::AfaController::probe();
            let snapshot = ruler.latest();
            let next_frame = plan.next_action(&job).frame;
            let valid_snapshot = snapshot.as_ref().is_some_and(|snapshot| {
                let view = repl_app::runner::to_view(snapshot);
                snapshot.frame_id.is_some()
                    && continuation_sample_ready(&view)
                    && view.elapsed >= plan.cutoff_frame
                    && view.elapsed <= next_frame
            });
            let issues = continuation_archive_issues(&plan, &BindingStore::load());
            if !afa.ready || !valid_snapshot || !issues.is_empty() {
                let current = snapshot.as_ref().map_or_else(
                    || "无快照".to_owned(),
                    |snapshot| {
                        format!(
                            "state={} elapsed={}",
                            snapshot.battle_state_str(),
                            snapshot.total_elapsed_frames
                        )
                    },
                );
                let message = format!(
                    "继续确认已失效：AFA ready={}；{}；允许区间 F{}..=F{}{}",
                    afa.ready,
                    current,
                    plan.cutoff_frame,
                    next_frame,
                    if issues.is_empty() {
                        String::new()
                    } else {
                        format!("；{}", issues.join("；"))
                    }
                );
                ui.set_status_line(message.clone().into());
                append_log(&ui, &log, &message);
                ui.set_continue_confirm_enabled(false);
                return;
            }

            for index in 0..plan.next_action_index {
                if ui
                    .get_actions()
                    .row_data(index)
                    .is_none_or(|row| row.status != 2)
                {
                    set_row_status(&ui, index, 4);
                }
            }
            *pending.borrow_mut() = None;
            launch_run(
                &ui,
                ui_weak.clone(),
                job,
                config.borrow().clone(),
                RunMode::Continue(plan),
                Arc::clone(&ruler),
                Arc::clone(&cancel),
                Arc::clone(&bridge),
                Rc::clone(&log),
                Rc::clone(&pending_choices),
                Rc::clone(&binding_opers),
            );
        });
    }
    {
        let ui_weak = ui.as_weak();
        let pending = Rc::clone(&pending_continuation);
        ui.on_cancel_continue(move || {
            *pending.borrow_mut() = None;
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_continue_visible(false);
            }
        });
    }
    {
        let cancel = Arc::clone(&cancel);
        ui.on_stop_run(move || cancel.store(true, Ordering::Relaxed));
    }
    {
        let ui_weak = ui.as_weak();
        let config = Rc::clone(&config);
        let log = Rc::clone(&log);
        ui.on_open_settings(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let dir = config
                .borrow()
                .resolve_maa_resource_dir()
                .map_or_else(|| "未找到".to_owned(), |p| p.display().to_string());
            append_log(
                &ui,
                &log,
                &format!(
                    "MAA 资源目录：{dir}\n脉冲初值：{}ms\n配置文件在可执行文件旁边的 config.json",
                    config.borrow().initial_gap_ms
                ),
            );
        });
    }

    // ——— 尺子状态轮询 ———
    {
        let ui_weak = ui.as_weak();
        let ruler = Arc::clone(&ruler);
        let copilot = Rc::clone(&copilot);
        let editor = Rc::clone(&editor);
        let timer = slint::Timer::default();
        // 管线停滞检测：frameId 长时间不涨 = 尺子没在分析新画面。
        // 注意静止画面（菜单挂机）下 WGC 不产帧、frameId 停住是正常的，
        // 所以这里只提示"可能停滞"，硬判定在 Runner 里结合"战斗画面已出现"做。
        let stall = std::cell::RefCell::new((0_u64, std::time::Instant::now()));
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(400),
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                ui.set_ruler_connected(ruler.status().is_connected());
                let snapshot = ruler.latest();
                if ui.get_editor_mode() {
                    ui.set_afa_ready(false);
                    ui.set_afa_detail("编辑模式不探测".into());
                    ui.set_can_continue(false);
                    ui.set_game_found(false);
                    if let Some(snapshot) = snapshot.as_ref() {
                        ui.set_ruler_profile(
                            snapshot
                                .active_profile
                                .clone()
                                .unwrap_or_else(|| "未校准".into())
                                .into(),
                        );
                        if editor.borrow().follow_ruler() {
                            let frame =
                                snapshot.total_elapsed_frames.clamp(0, i64::from(i32::MAX)) as i32;
                            editor.borrow_mut().set_cursor_frame(i64::from(frame));
                            ui.set_editor_cursor_frame(frame);
                            ui.set_editor_view_start(frame.saturating_sub(30));
                        }
                    }
                    return;
                }
                let afa = repl_input::AfaController::probe();
                ui.set_afa_ready(afa.ready);
                ui.set_afa_detail(afa.detail.into());
                let start_ready = copilot
                    .borrow()
                    .as_ref()
                    .and_then(|job| job.actions.first())
                    .is_some_and(|action| {
                        snapshot
                            .as_ref()
                            .is_some_and(|snapshot| start_snapshot_ready(snapshot, action.frame))
                    });
                ui.set_can_start(afa.ready && !ui.get_running() && start_ready);
                let continue_ready = copilot.borrow().as_ref().is_some_and(|job| {
                    snapshot
                        .as_ref()
                        .is_some_and(|snapshot| continuation_snapshot_plan(snapshot, job).is_some())
                });
                ui.set_can_continue(afa.ready && !ui.get_running() && continue_ready);
                if let Some(snapshot) = snapshot.as_ref() {
                    ui.set_ruler_profile(
                        snapshot
                            .active_profile
                            .clone()
                            .unwrap_or_else(|| "未校准".into())
                            .into(),
                    );
                    let frame_id = snapshot.frame_id.unwrap_or(0);
                    let stalled_secs = {
                        let mut guard = stall.borrow_mut();
                        if guard.0 != frame_id {
                            *guard = (frame_id, std::time::Instant::now());
                            0
                        } else {
                            guard.1.elapsed().as_secs()
                        }
                    };
                    if stalled_secs >= 10 {
                        ui.set_battle_state(
                            format!(
                                "{}（分析已停滞 {stalled_secs}s，若游戏画面在动请重启尺子）",
                                snapshot.battle_state_str()
                            )
                            .into(),
                        );
                    } else {
                        ui.set_battle_state(snapshot.battle_state_str().into());
                    }
                    // 绝对帧始终以尺子为准。运行期间 Machine 的游标可能在动作注入或
                    // 确认屏障中暂时落后，不能用它覆盖尺子的最新读数。
                    ui.set_cursor_frame(snapshot.total_elapsed_frames as i32);
                }
                ui.set_game_found(repl_capture::GameWindow::find().is_ok());
            },
        );
        // Timer 必须活到窗口关闭为止，泄漏掉最省事也最安全。
        std::mem::forget(timer);
    }

    ui.run()
}

#[allow(clippy::too_many_arguments)]
fn launch_run(
    ui: &MainWindow,
    ui_weak: slint::Weak<MainWindow>,
    job: Copilot,
    config: Config,
    run_mode: RunMode,
    ruler: Arc<RulerClient>,
    cancel: Arc<AtomicBool>,
    bridge: Arc<BindingBridge>,
    log: Rc<RefCell<String>>,
    pending_choices: Rc<RefCell<BindingChoices>>,
    binding_opers: Rc<RefCell<Vec<String>>>,
) {
    let ui_window = UiWindowControl::from_ui(ui);
    if ui_window.is_none() {
        append_log(
            ui,
            &log,
            "无法取得复刻器窗口句柄；人工绑定时请手动切回复刻器",
        );
    }
    let label = match &run_mode {
        RunMode::FromZero => "开始复刻".to_owned(),
        RunMode::Continue(plan) => format!(
            "承担风险继续：冻结截止帧 {}，用户确认完成 {} 个动作",
            plan.cutoff_frame,
            plan.assumed_action_count()
        ),
    };
    cancel.store(false, Ordering::Relaxed);
    ui.set_running(true);
    ui.set_continue_visible(false);
    ui.set_status_line("正在准备…".into());
    append_log(ui, &log, &label);

    let (tx, rx) = mpsc::channel::<Progress>();
    let worker_cancel = Arc::clone(&cancel);
    let worker_bridge = Arc::clone(&bridge);
    std::thread::Builder::new()
        .name("replicator-run".into())
        .spawn(move || {
            if let Err(e) = run_once(
                job,
                config,
                run_mode,
                &*ruler,
                tx.clone(),
                worker_cancel,
                worker_bridge,
                ui_window,
            ) {
                let _ = tx.send(Progress::Failed(e.to_string()));
            }
        })
        .expect("failed to spawn run thread");

    pump_progress(ui_weak, rx, log, pending_choices, binding_opers);
}

/// 工作线程：跑完一整场复刻。
#[allow(clippy::too_many_arguments)]
fn run_once(
    job: Copilot,
    config: Config,
    run_mode: RunMode,
    frames: &dyn FrameSource,
    events: mpsc::Sender<Progress>,
    cancel: Arc<AtomicBool>,
    bridge: Arc<BindingBridge>,
    ui_window: Option<UiWindowControl>,
) -> anyhow::Result<()> {
    let mut session = Session::open(&config, &job.stage_name)?;
    let mut store = BindingStore::load();
    let wanted = match &run_mode {
        RunMode::FromZero => job.oper_names(),
        RunMode::Continue(plan) => {
            session.seed_battlefield(plan.battlefield.clone());
            for name in &plan.archived_binding_names {
                let avatar = store.avatar(name).ok_or_else(|| {
                    anyhow::anyhow!("继续需要干员「{name}」的已保存头像档案，但当前档案不存在")
                })?;
                let bgra = avatar
                    .decode()
                    .map_err(|e| anyhow::anyhow!("干员「{name}」的头像档案损坏：{e}"))?;
                session.restore_avatar(name, avatar.width, avatar.height, &bgra)?;
            }
            plan.visible_binding_names.clone()
        }
    };
    let stage = job.stage_name.clone();
    // binder 在工作线程里自己给 UI 发事件（卡片数据只有它知道）。
    let events_for_binder = events.clone();

    let binder = Box::new(
        move |session: &mut Session,
              mode: BindingMode,
              cancel_flag: &AtomicBool|
              -> anyhow::Result<bool> {
            if wanted.is_empty() {
                log::info!("no visible deployment bindings are required for this run");
                return Ok(true);
            }
            let cards = session.scan_deployment()?;
            if cards.is_empty() {
                anyhow::bail!("识别不到部署栏，确认游戏停在战斗界面");
            }
            let hashes: Vec<u64> = cards
                .iter()
                .map(|c| perceptual_hash(&c.avatar, c.avatar_width, c.avatar_height))
                .collect();
            let key = fingerprint(&hashes);

            // 先试着从档案恢复
            if let Some(profile) = store.get(&key).cloned() {
                let mut restored = 0;
                for binding in &profile.bindings {
                    if !wanted.contains(&binding.name) {
                        continue;
                    }
                    if let Some(card) = cards.get(binding.index) {
                        if session.bind(&binding.name, card).is_ok() {
                            store.remember_avatar(
                                &binding.name,
                                card.avatar_width,
                                card.avatar_height,
                                &card.avatar,
                            );
                            restored += 1;
                        }
                    }
                }
                if restored == wanted.len() {
                    log::info!("restored {restored} bindings from saved profile");
                    if let Err(e) = store.save() {
                        log::warn!("could not update avatar archive: {e}");
                    }
                    return Ok(true);
                }
                log::info!(
                    "saved profile only covered {restored}/{} opers; asking user",
                    wanted.len()
                );
            }

            // 交给 UI 让用户填。卡片数据随 NeedsBinding 事件送过去 ——
            // UI 面板展示的一切都来自这条事件。
            if mode == BindingMode::AutomaticOnly {
                anyhow::bail!(
                "焦点后的第一条样本为 running，但没有完整的编队绑定档案；为避免在运行中弹出人工绑定面板，本轮已中止"
            );
            }

            let card_infos: Vec<repl_app::runner::BindingCardInfo> = cards
                .iter()
                .map(|c| repl_app::runner::BindingCardInfo {
                    index: c.index,
                    role: c.role.zh().to_owned(),
                    available: c.available,
                    cooling: c.cooling,
                })
                .collect();
            *bridge.choices.lock().unwrap() = None;
            let _ = events_for_binder.send(Progress::NeedsBinding {
                cards: card_infos.clone(),
                opers: wanted.clone(),
            });

            // 等用户点"确认"。填错 / 填漏不判死刑：报告缺谁、重新弹面板再来，
            // 只有超时才放弃。
            if let Some(window) = ui_window {
                let activated = window.activate_once();
                if !activated {
                    let _ = events_for_binder.send(Progress::Log(
                        "复刻器窗口未能自动激活，请手动切回复刻器；等待上限 15 秒".into(),
                    ));
                }
                let focus_deadline = std::time::Instant::now() + Duration::from_secs(15);
                while !window.is_foreground() && std::time::Instant::now() < focus_deadline {
                    if cancel_flag.load(Ordering::Relaxed) {
                        anyhow::bail!("用户中止编队绑定");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if !window.is_foreground() {
                    anyhow::bail!("等待复刻器窗口回到前台超时，编队绑定中止");
                }
            }

            let deadline = std::time::Instant::now() + Duration::from_secs(600);
            loop {
                let choices = loop {
                    if cancel_flag.load(Ordering::Relaxed) {
                        anyhow::bail!("用户中止编队绑定");
                    }
                    if let Some(c) = bridge.choices.lock().unwrap().take() {
                        break c;
                    }
                    if std::time::Instant::now() > deadline {
                        anyhow::bail!("等待编队绑定超时（10 分钟）");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                };

                let mut profile = store.get(&key).cloned().unwrap_or(BindingProfile {
                    stage_name: stage.clone(),
                    bindings: Vec::new(),
                });
                profile.stage_name = stage.clone();
                for (index, name) in &choices {
                    let card = cards
                        .get(*index)
                        .ok_or_else(|| anyhow::anyhow!("卡片序号 {index} 不存在"))?;
                    session.bind(name, card)?;
                    store.remember_avatar(
                        name,
                        card.avatar_width,
                        card.avatar_height,
                        &card.avatar,
                    );
                    profile
                        .bindings
                        .retain(|binding| binding.name != *name && binding.index != *index);
                    profile.bindings.push(Binding {
                        name: name.clone(),
                        index: *index,
                        role: card.role.zh().to_owned(),
                        width: card.avatar_width,
                        height: card.avatar_height,
                        avatar_base64: store
                            .avatar(name)
                            .map_or_else(String::new, |avatar| avatar.bgra_base64.clone()),
                    });
                }
                let bound = session.bound_names();
                let missing: Vec<_> = wanted.iter().filter(|n| !bound.contains(n)).collect();
                if missing.is_empty() {
                    store.put(key, profile);
                    if let Err(e) = store.save() {
                        log::warn!("could not save bindings: {e}");
                    }
                    return Ok(true);
                }
                let _ = events_for_binder.send(Progress::Log(format!(
                    "这些干员还没绑定：{missing:?}。名字要和作业里写的完全一致，请补全后再点确认"
                )));
                let _ = events_for_binder.send(Progress::NeedsBinding {
                    cards: card_infos.clone(),
                    opers: wanted.clone(),
                });
            }
        },
    );

    let mut runner = Runner::new(
        job,
        &config,
        run_mode,
        &mut session,
        frames,
        events,
        cancel,
        binder,
    );
    runner.run()
}

/// 把工作线程的进度事件搬到 UI 上。
fn pump_progress(
    ui_weak: slint::Weak<MainWindow>,
    rx: mpsc::Receiver<Progress>,
    log: Rc<RefCell<String>>,
    pending_choices: Rc<RefCell<BindingChoices>>,
    binding_opers: Rc<RefCell<Vec<String>>>,
) {
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(60),
        move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            while let Ok(event) = rx.try_recv() {
                match event {
                    Progress::Startup { phase } => {
                        ui.set_phase(phase.zh().into());
                        ui.set_status_line(format!("启动准备：{}", phase.zh()).into());
                    }
                    Progress::Phase {
                        phase,
                        cursor: _,
                        target,
                    } => {
                        ui.set_phase(phase.zh().into());
                        ui.set_target_frame(target.map_or(-1, |t| t as i32));
                    }
                    Progress::NeedsBinding { cards, opers } => {
                        // 面板的全部内容都从这条事件来：卡片列表、待绑定名单。
                        let rows: Vec<CardRow> = cards
                            .iter()
                            .map(|c| CardRow {
                                index: c.index as i32,
                                role: c.role.as_str().into(),
                                available: c.available,
                                cooling: c.cooling,
                                bound_to: SharedString::new(),
                            })
                            .collect();
                        ui.set_cards(ModelRc::new(VecModel::from(rows)));
                        let unbound: Vec<SharedString> =
                            opers.iter().map(|o| SharedString::from(o.as_str())).collect();
                        ui.set_unbound_opers(ModelRc::new(VecModel::from(unbound)));
                        *binding_opers.borrow_mut() = opers;
                        pending_choices.borrow_mut().clear();
                        ui.set_binding_visible(true);
                        ui.set_status_line("请完成编队匹配".into());
                    }
                    Progress::ActionStarted { index, frame } => {
                        set_row_status(&ui, index, 1);
                        ui.set_status_line(
                            format!("第 {frame} 帧：正在派发并等待动作 #{index} 确认").into(),
                        );
                    }
                    Progress::ActionDone { index } => set_row_status(&ui, index, 2),
                    Progress::Log(line) => append_log(&ui, &log, &line),
                    Progress::Finished {
                        pulses,
                        zero_pulses,
                        overshoots,
                        assumed_actions,
                        continued,
                    } => {
                        ui.set_running(false);
                        let summary = if !continued {
                            format!(
                                "复刻完成。脉冲 {pulses} 次（空脉冲 {zero_pulses}，跨帧 {overshoots}）"
                            )
                        } else {
                            format!(
                                "带风险完成，{assumed_actions} 个动作为用户确认完成。后续脉冲 {pulses} 次（空脉冲 {zero_pulses}，跨帧 {overshoots}）；本次不能计入精确复刻验收"
                            )
                        };
                        ui.set_status_line(summary.clone().into());
                        append_log(&ui, &log, &summary);
                        if overshoots > 0 {
                            append_log(
                                &ui,
                                &log,
                                "警告：出现过跨帧，本次复刻的帧精度不可信。可以把 config.json 里的 initial_gap_ms 调小 2–3ms 再试。",
                            );
                        }
                    }
                    Progress::Failed(why) => {
                        ui.set_running(false);
                        ui.set_binding_visible(false);
                        ui.set_status_line(format!("中止：{why}").into());
                        append_log(&ui, &log, &format!("中止：{why}"));
                    }
                }
            }
        },
    );
    std::mem::forget(timer);
}

fn set_row_status(ui: &MainWindow, index: usize, status: i32) {
    let model = ui.get_actions();
    if let Some(mut row) = model.row_data(index) {
        row.status = status;
        model.set_row_data(index, row);
    }
}

fn append_log(ui: &MainWindow, log: &Rc<RefCell<String>>, line: &str) {
    let mut buffer = log.borrow_mut();
    buffer.push_str(line);
    buffer.push('\n');
    // 只留最后 8000 字符，避免长时间运行把内存吃掉
    if buffer.len() > 8000 {
        let cut = buffer.len() - 8000;
        let boundary = buffer
            .char_indices()
            .find(|(i, _)| *i >= cut)
            .map_or(buffer.len(), |(i, _)| i);
        *buffer = buffer[boundary..].to_owned();
    }
    ui.set_log_text(buffer.as_str().into());
}

/// 执行保存后，在同一个 UI 回调中安全读取最新编辑状态。
fn save_editor_then<T>(
    editor: &Rc<RefCell<EditorState>>,
    path: Option<&std::path::Path>,
    after_save: impl FnOnce(&EditorState) -> T,
) -> anyhow::Result<T> {
    // 不要把 `borrow_mut()` 放在 match 条件里：条件临时值会活到整个 match 结束，
    // 成功分支刷新 Slint 属性时再次借用就会让 GUI 线程 panic。
    let save_result = editor.borrow_mut().save(path);
    match save_result {
        Ok(_) => Ok(after_save(&editor.borrow())),
        Err(error) => Err(error),
    }
}

fn refresh_editor(ui: &MainWindow, editor: &EditorState, runnable: &Rc<RefCell<Option<Copilot>>>) {
    let document = editor.document();
    let diagnostics = document.diagnostics();
    let mut action_severity = std::collections::HashMap::new();
    for diagnostic in &diagnostics {
        if let Some(id) = diagnostic.action_id {
            let severity = match diagnostic.severity {
                DiagnosticSeverity::Error => 2,
                DiagnosticSeverity::Warning => 1,
            };
            action_severity
                .entry(id)
                .and_modify(|current: &mut i32| *current = (*current).max(severity))
                .or_insert(severity);
        }
    }
    let rows: Vec<EditorActionRow> = document
        .actions()
        .into_iter()
        .map(|action| {
            let kind = editor_kind_label(&action.kind);
            let location = action
                .location
                .map_or_else(String::new, |point| format!("{},{}", point.x, point.y));
            EditorActionRow {
                id: action.id.get().min(i32::MAX as u64) as i32,
                frame: action
                    .frame
                    .unwrap_or_default()
                    .clamp(i32::MIN as i64, i32::MAX as i64) as i32,
                kind: kind.into(),
                lane: if action.kind.eq_ignore_ascii_case("Output") {
                    "输出 / 注释".into()
                } else if action.name.is_empty() {
                    "未分配".into()
                } else {
                    action.name.as_str().into()
                },
                target: action.name.as_str().into(),
                location: location.into(),
                direction: direction_name(action.direction).into(),
                doc: action.doc.as_str().into(),
                selected: editor.selected() == Some(action.id),
                severity: action_severity.get(&action.id).copied().unwrap_or(0),
            }
        })
        .collect();
    let diagnostic_rows: Vec<DiagnosticRow> = diagnostics
        .iter()
        .map(|diagnostic| DiagnosticRow {
            severity: match diagnostic.severity {
                DiagnosticSeverity::Error => 2,
                DiagnosticSeverity::Warning => 1,
            },
            message: diagnostic.message.as_str().into(),
        })
        .collect();
    let operators: Vec<SharedString> = document
        .operator_names()
        .iter()
        .map(|name| name.as_str().into())
        .collect();

    ui.set_editor_title(document.title().into());
    ui.set_editor_stage(document.stage_name().into());
    ui.set_editor_path(
        editor
            .path()
            .map_or_else(|| "尚未保存".to_owned(), |path| path.display().to_string())
            .into(),
    );
    ui.set_editor_dirty(editor.is_dirty());
    ui.set_editor_can_undo(editor.can_undo());
    ui.set_editor_can_redo(editor.can_redo());
    ui.set_editor_follow_ruler(editor.follow_ruler());
    let cursor = editor
        .cursor_frame()
        .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    ui.set_editor_cursor_frame(cursor);
    ui.set_editor_view_start(cursor.saturating_sub(30));
    ui.set_editor_actions(ModelRc::new(VecModel::from(rows)));
    ui.set_editor_diagnostics(ModelRc::new(VecModel::from(diagnostic_rows)));
    ui.set_editor_operators(ModelRc::new(VecModel::from(operators)));

    if let Some(action) = editor.selected_action() {
        ui.set_editor_selected_id(action.id.get().min(i32::MAX as u64) as i32);
        ui.set_editor_selected_kind(editor_kind_label(&action.kind).into());
        ui.set_editor_selected_frame(
            action
                .frame
                .map_or_else(String::new, |frame| frame.to_string())
                .into(),
        );
        ui.set_editor_selected_name(action.name.into());
        ui.set_editor_selected_x(
            action
                .location
                .map_or_else(String::new, |point| point.x.to_string())
                .into(),
        );
        ui.set_editor_selected_y(
            action
                .location
                .map_or_else(String::new, |point| point.y.to_string())
                .into(),
        );
        ui.set_editor_selected_direction(direction_name(action.direction).into());
        ui.set_editor_selected_doc(action.doc.into());
    } else {
        ui.set_editor_selected_id(-1);
        ui.set_editor_selected_kind("".into());
        ui.set_editor_selected_frame("".into());
        ui.set_editor_selected_name("".into());
        ui.set_editor_selected_x("".into());
        ui.set_editor_selected_y("".into());
        ui.set_editor_selected_direction("Right".into());
        ui.set_editor_selected_doc("".into());
    }

    match document.compile() {
        Ok(job)
            if !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error) =>
        {
            ui.set_editor_can_run(true);
            ui.set_job_name(
                format!(
                    "{} · {} 个动作 · 关卡 {}",
                    if job.title.is_empty() {
                        "未命名作业"
                    } else {
                        &job.title
                    },
                    job.actions.len(),
                    job.stage_name
                )
                .into(),
            );
            ui.set_actions(build_timeline(&job));
            *runnable.borrow_mut() = Some(job);
        }
        _ => {
            ui.set_editor_can_run(false);
            ui.set_can_start(false);
            ui.set_actions(ModelRc::new(VecModel::<ActionRow>::default()));
            *runnable.borrow_mut() = None;
        }
    }
}

fn refresh_editor_map(ui: &MainWindow, level_pack: Option<&LevelPack>, editor: &EditorState) {
    let stage = editor.document().stage_name();
    let selected = editor.selected_action().and_then(|action| action.location);
    let Some(level_pack) = level_pack else {
        ui.set_editor_map_cells(ModelRc::new(VecModel::<MapCell>::default()));
        ui.set_editor_map_width(0);
        ui.set_editor_map_height(0);
        ui.set_editor_map_status("未找到 MAA 的 Arknights-Tile-Pos，仍可手填 X/Y".into());
        return;
    };
    if stage.trim().is_empty() {
        ui.set_editor_map_cells(ModelRc::new(VecModel::<MapCell>::default()));
        ui.set_editor_map_width(0);
        ui.set_editor_map_height(0);
        ui.set_editor_map_status("输入关卡标识以载入逻辑地图".into());
        return;
    }
    match level_pack.load_level(&stage) {
        Ok(level) => {
            let cells: Vec<MapCell> = level
                .locations()
                .filter_map(|location| {
                    let tile = level.tile(location)?;
                    Some(MapCell {
                        x: location.x,
                        y: location.y,
                        buildable: match tile.buildable {
                            Buildable::Melee => 1,
                            Buildable::Ranged => 2,
                            Buildable::All => 3,
                            Buildable::None | Buildable::Invalid => 0,
                        },
                        selected: selected == Some(location),
                    })
                })
                .collect();
            ui.set_editor_map_width(level.width());
            ui.set_editor_map_height(level.height());
            ui.set_editor_map_cells(ModelRc::new(VecModel::from(cells)));
            ui.set_editor_map_status(
                format!("{} × {} 逻辑地图", level.width(), level.height()).into(),
            );
        }
        Err(error) => {
            ui.set_editor_map_cells(ModelRc::new(VecModel::<MapCell>::default()));
            ui.set_editor_map_width(0);
            ui.set_editor_map_height(0);
            ui.set_editor_map_status(format!("地图未匹配：{error}；仍可手填 X/Y").into());
        }
    }
}

fn set_editor_map_selection(ui: &MainWindow, selected: Option<Point>) {
    let model = ui.get_editor_map_cells();
    for index in 0..model.row_count() {
        if let Some(mut cell) = model.row_data(index) {
            let should_select = selected == Some(Point::new(cell.x, cell.y));
            if cell.selected != should_select {
                cell.selected = should_select;
                model.set_row_data(index, cell);
            }
        }
    }
}

fn editor_kind_label(kind: &str) -> &'static str {
    match ActionType::parse(kind) {
        Some(ActionType::Deploy) => "部署",
        Some(ActionType::UseSkill) => "技能",
        Some(ActionType::Retreat) => "撤退",
        Some(ActionType::Output) => "注释",
        Some(_) => "不支持",
        None => "未知",
    }
}

fn parse_editor_direction(value: &str) -> Direction {
    match value.trim().to_ascii_lowercase().as_str() {
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

fn action_type_label(kind: ActionType) -> &'static str {
    match kind {
        ActionType::Deploy => "部署",
        ActionType::UseSkill => "技能",
        ActionType::Retreat => "撤退",
        ActionType::Output => "注释",
        _ => "不支持",
    }
}

fn continuation_archive_issues(plan: &ContinuationPlan, store: &BindingStore) -> Vec<String> {
    plan.archived_binding_names
        .iter()
        .filter_map(|name| match store.avatar(name) {
            None => Some(format!("缺少干员「{name}」的头像档案")),
            Some(avatar) => match avatar.decode() {
                Ok(bytes)
                    if avatar.width > 0
                        && avatar.height > 0
                        && bytes.len()
                            == (u64::from(avatar.width) * u64::from(avatar.height) * 4)
                                as usize =>
                {
                    None
                }
                _ => Some(format!("干员「{name}」的头像档案损坏")),
            },
        })
        .collect()
}

fn build_timeline(job: &Copilot) -> ModelRc<ActionRow> {
    let rows: Vec<ActionRow> = job
        .actions
        .iter()
        .map(|a| ActionRow {
            frame: a.frame as i32,
            kind: action_type_label(a.kind).into(),
            target: a.name.clone().into(),
            location: a
                .location
                .map_or_else(String::new, |p| format!("({}, {})", p.x, p.y))
                .into(),
            doc: a.doc.clone().into(),
            status: 0,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// 打开系统的"打开文件"对话框。
///
/// 直接用 Win32 的 `GetOpenFileNameW`，省掉一个 GUI 依赖。
#[cfg(windows)]
fn pick_job_file() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut buffer = [0u16; 1024];
    let filter: Vec<u16> = "作业文件 (*.json)\0*.json\0所有文件\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "选择帧复刻作业\0".encode_utf16().collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    // SAFETY: 所有指针都指向本函数栈上、在调用期间保持有效的缓冲区。
    let ok = unsafe { GetOpenFileNameW(&mut ofn) }.as_bool();
    if !ok {
        return None;
    }
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

/// 打开系统的“另存为”对话框。编辑器允许保存尚未通过校验的草稿。
#[cfg(windows)]
fn pick_save_file() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut buffer = [0u16; 1024];
    let filter: Vec<u16> = "作业文件 (*.json)\0*.json\0所有文件\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "保存帧复刻作业\0".encode_utf16().collect();
    let extension: Vec<u16> = "json\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        lpstrDefExt: PCWSTR(extension.as_ptr()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST,
        ..Default::default()
    };
    // SAFETY: 所有指针都指向本函数栈上、在调用期间保持有效的缓冲区。
    let ok = unsafe { GetSaveFileNameW(&mut ofn) }.as_bool();
    if !ok {
        return None;
    }
    let end = buffer.iter().position(|character| *character == 0)?;
    Some(String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(not(windows))]
fn pick_job_file() -> Option<String> {
    std::env::args().nth(1)
}

#[cfg(not(windows))]
fn pick_save_file() -> Option<String> {
    std::env::args().nth(2)
}

#[cfg(test)]
mod editor_callback_tests {
    use super::*;

    #[test]
    fn save_callback_releases_mutable_borrow_before_refreshing() {
        let editor = Rc::new(RefCell::new(EditorState::default()));
        let path =
            std::env::temp_dir().join(format!("repl-save-callback-{}.json", std::process::id()));

        save_editor_then(&editor, Some(&path), |state| {
            assert!(!state.is_dirty());
        })
        .unwrap();

        std::fs::remove_file(path).unwrap();
    }
}
