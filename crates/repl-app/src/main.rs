// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Arknights Operation Replicator contributors

//! `replicator` —— 明日方舟帧级操作复刻器。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use repl_app::{
    config::{
        fingerprint, normalize_maa_resource_dir, perceptual_hash, Binding, BindingProfile,
        BindingStore, Config,
    },
    editor::EditorState,
    runner::{
        continuation_sample_ready, continuation_snapshot_plan, start_snapshot_ready, BindingMode,
        BindingRequest, Progress, RunMode, Runner,
    },
    session::Session,
};
use repl_core::{
    continuation::ContinuationPlan, copilot::ActionType, level::Buildable, Copilot,
    DiagnosticSeverity, Direction, LevelPack, Point,
};
use repl_frames::{FrameSource, RulerClient};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

slint::include_modules!();

/// 用户在绑定面板里填的"卡片序号 → 干员名"。
type BindingChoices = Vec<(usize, String)>;

const FORMATION_PENDING_TTL: Duration = Duration::from_secs(30 * 60);
/// 编辑页的尺子帧只做共享快照读取和一个属性更新，可按约 30 FPS 刷新；
/// AFA 探测、窗口扫描等较重状态仍保留在 400ms 定时器中。
const EDITOR_RULER_REFRESH: Duration = Duration::from_millis(33);

struct FormationDraft {
    report: repl_vision::FormationScanReport,
    catalog: repl_vision::OperatorCatalog,
    config: repl_vision::FormationTaskConfig,
    selections: HashMap<usize, String>,
    roster_fingerprint: u64,
    game_hwnd: isize,
    width: u32,
    height: u32,
    created_at: Instant,
}

#[derive(Clone)]
struct PendingFormationScan {
    confirmed: Vec<(repl_vision::OperatorKey, repl_vision::FormationAvatar)>,
    config: repl_vision::FormationTaskConfig,
    roster_fingerprint: u64,
    game_hwnd: isize,
    width: u32,
    height: u32,
    created_at: Instant,
}

type FormationScanMessage = (u64, Vec<String>, Result<FormationDraft, String>);

/// 绑定面板与工作线程之间的握手：UI 线程在用户点"确认"时填入选择，
/// 工作线程轮询取走。卡片数据走 `Progress::NeedsBinding` 事件，不经过这里。
struct BindingBridge {
    response: Mutex<Option<BindingResponse>>,
}

enum BindingResponse {
    Confirm(BindingChoices),
    Cancel,
}

impl BindingBridge {
    fn reset(&self) {
        *self.response.lock().unwrap() = None;
    }

    fn submit(&self, response: BindingResponse) {
        *self.response.lock().unwrap() = Some(response);
    }

    fn take(&self) -> Option<BindingResponse> {
        self.response.lock().unwrap().take()
    }
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
    ui.set_binding_options(ModelRc::new(VecModel::from(vec![SharedString::from(
        "— 当前卡片不绑定 —",
    )])));
    ui.set_afa_ready(false);
    ui.set_afa_detail("未检测".into());
    ui.set_can_continue(false);
    ui.set_continue_visible(false);
    ui.set_continue_confirm_enabled(false);
    ui.set_editor_actions(ModelRc::new(VecModel::<EditorActionRow>::default()));
    ui.set_editor_diagnostics(ModelRc::new(VecModel::<DiagnosticRow>::default()));
    ui.set_editor_operators(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_editor_deferred_targets(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_editor_operator_options(ModelRc::new(VecModel::from(vec![SharedString::from(
        "— 选择目标 —",
    )])));
    ui.set_editor_operator_search_results(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_editor_map_cells(ModelRc::new(VecModel::<MapCell>::default()));
    ui.set_formation_rows(ModelRc::new(VecModel::<FormationRow>::default()));
    ui.set_formation_options(ModelRc::new(VecModel::from(vec![SharedString::from(
        "— 忽略此槽位 —",
    )])));

    let ruler = Arc::new(RulerClient::connect(config.borrow().ruler_ws_url.clone()));
    let copilot: Rc<RefCell<Option<Copilot>>> = Rc::new(RefCell::new(None));
    let pending_continuation: Rc<RefCell<Option<ContinuationPlan>>> = Rc::new(RefCell::new(None));
    let formation_draft: Rc<RefCell<Option<FormationDraft>>> = Rc::new(RefCell::new(None));
    let pending_formation: Rc<RefCell<Option<PendingFormationScan>>> = Rc::new(RefCell::new(None));
    let formation_generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (formation_tx, formation_rx) = mpsc::channel::<FormationScanMessage>();
    let editor = Rc::new(RefCell::new(EditorState::default()));
    let initial_resource_dir = config.borrow().resolve_maa_resource_dir();
    let (initial_level_pack, initial_operator_catalog, initial_resource_status) =
        initial_resource_dir.as_ref().map_or_else(
            || (None, None, "请选择资源目录".to_owned()),
            |directory| match load_editor_resources(directory) {
                Ok((level_pack, operator_catalog)) => (
                    Some(level_pack),
                    Some(operator_catalog),
                    "已加载".to_owned(),
                ),
                Err(error) => {
                    log::warn!(
                        "editor resources unavailable from {}: {error}",
                        directory.display()
                    );
                    (None, None, format!("加载失败：{error}"))
                }
            },
        );
    let operator_catalog = Rc::new(RefCell::new(initial_operator_catalog));
    let level_pack = Rc::new(RefCell::new(initial_level_pack));
    ui.set_editor_resource_path(
        initial_resource_dir
            .as_ref()
            .map_or_else(
                || "未找到 MAA resource".to_owned(),
                |path| path.display().to_string(),
            )
            .into(),
    );
    ui.set_editor_resource_status(initial_resource_status.into());
    let cancel = Arc::new(AtomicBool::new(false));
    let log = Rc::new(RefCell::new(String::new()));
    refresh_editor(&ui, &editor.borrow(), &copilot);
    refresh_editor_map(&ui, level_pack.borrow().as_ref(), &editor.borrow());

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
        let formation_generation = Arc::clone(&formation_generation);
        let formation_draft = Rc::clone(&formation_draft);
        let pending_formation = Rc::clone(&pending_formation);
        ui.on_switch_mode(move |editor_mode| {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_running() {
                return;
            }
            ui.set_editor_mode(editor_mode);
            if editor_mode {
                formation_generation.fetch_add(1, Ordering::Relaxed);
                *formation_draft.borrow_mut() = None;
                *pending_formation.borrow_mut() = None;
                ui.set_formation_scanning(false);
                ui.set_formation_visible(false);
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
        let operator_catalog = Rc::clone(&operator_catalog);
        ui.on_editor_search_operators(move |query| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let existing = editor.borrow().document().operator_names();
            let results = operator_catalog
                .borrow()
                .as_ref()
                .map_or_else(Vec::new, |catalog| catalog.search_names(query.as_str(), 12))
                .into_iter()
                .filter(|name| !existing.iter().any(|operator| operator == name))
                .map(SharedString::from)
                .collect::<Vec<_>>();
            ui.set_editor_operator_search_results(ModelRc::new(VecModel::from(results)));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let config = Rc::clone(&config);
        let editor = Rc::clone(&editor);
        let operator_catalog = Rc::clone(&operator_catalog);
        let level_pack = Rc::clone(&level_pack);
        ui.on_editor_select_resource(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let owner = UiWindowControl::from_ui(&ui).map(|window| window.hwnd);
            let Some(selection) = pick_maa_resource_dir(owner).map(std::path::PathBuf::from) else {
                return;
            };
            let directory = match normalize_maa_resource_dir(&selection) {
                Ok(directory) => directory,
                Err(error) => {
                    ui.set_editor_resource_status(format!("选择失败：{error}").into());
                    return;
                }
            };
            let (new_level_pack, new_operator_catalog) = match load_editor_resources(&directory) {
                Ok(resources) => resources,
                Err(error) => {
                    ui.set_editor_resource_status(format!("加载失败：{error}").into());
                    return;
                }
            };

            let previous = config.borrow().maa_resource_dir.clone();
            config.borrow_mut().maa_resource_dir = directory.display().to_string();
            if let Err(error) = config.borrow().save() {
                config.borrow_mut().maa_resource_dir = previous;
                ui.set_editor_resource_status(format!("保存失败：{error}").into());
                return;
            }

            *level_pack.borrow_mut() = Some(new_level_pack);
            *operator_catalog.borrow_mut() = Some(new_operator_catalog);
            ui.set_editor_resource_path(directory.display().to_string().into());
            ui.set_editor_resource_status("已加载".into());
            refresh_editor_map(&ui, level_pack.borrow().as_ref(), &editor.borrow());

            let existing = editor.borrow().document().operator_names();
            let query = ui.get_editor_operator_query();
            let results = operator_catalog
                .borrow()
                .as_ref()
                .map_or_else(Vec::new, |catalog| catalog.search_names(query.as_str(), 12))
                .into_iter()
                .filter(|name| !existing.iter().any(|operator| operator == name))
                .map(SharedString::from)
                .collect::<Vec<_>>();
            ui.set_editor_operator_search_results(ModelRc::new(VecModel::from(results)));
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_select_operator_search_result(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let name = name.trim();
            if name.is_empty()
                || editor
                    .borrow()
                    .document()
                    .operator_names()
                    .iter()
                    .any(|operator| operator == name)
            {
                return;
            }
            editor.borrow_mut().add_operator(name, 1);
            ui.set_editor_operator_query("".into());
            ui.set_editor_operator_search_results(
                ModelRc::new(VecModel::<SharedString>::default()),
            );
            refresh_editor(&ui, &editor.borrow(), &copilot);
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
            refresh_editor_map(&ui, level_pack.borrow().as_ref(), &editor.borrow());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_add_deferred_target(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let name = name.trim();
            if name.is_empty()
                || editor
                    .borrow()
                    .document()
                    .target_names()
                    .iter()
                    .any(|n| n == name)
            {
                return;
            }
            editor.borrow_mut().add_deferred_target(name);
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_rename_deferred_target(move |old_name, new_name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let new_name = new_name.trim();
            if new_name.is_empty() {
                return;
            }
            editor
                .borrow_mut()
                .rename_deferred_target(old_name.as_str(), new_name);
            refresh_editor(&ui, &editor.borrow(), &copilot);
        });
    }
    {
        let ui_weak = ui.as_weak();
        let editor = Rc::clone(&editor);
        let copilot = Rc::clone(&copilot);
        ui.on_editor_remove_deferred_target(move |name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            editor.borrow_mut().remove_deferred_target(name.as_str());
            refresh_editor(&ui, &editor.borrow(), &copilot);
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
                    refresh_editor_map(&ui, level_pack.borrow().as_ref(), &editor.borrow());
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
        ui.on_editor_clear_location(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(mut action) = editor.borrow().selected_action() else {
                return;
            };
            action.location = None;
            if let Err(error) = editor.borrow_mut().update_action(action) {
                log::warn!("editor location clear failed: {error}");
            }
            refresh_editor(&ui, &editor.borrow(), &copilot);
            set_editor_map_selection(&ui, None);
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
                refresh_editor_map(&ui, level_pack.borrow().as_ref(), &editor.borrow());
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
        let formation_draft = Rc::clone(&formation_draft);
        let pending_formation = Rc::clone(&pending_formation);
        let formation_generation = Arc::clone(&formation_generation);
        ui.on_load_job(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let Some(path) = pick_job_file() else { return };
            match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| Copilot::parse(&text).map_err(|e| e.to_string()))
            {
                Ok(job) => {
                    formation_generation.fetch_add(1, Ordering::Relaxed);
                    *formation_draft.borrow_mut() = None;
                    *pending_formation.borrow_mut() = None;
                    ui.set_formation_visible(false);
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

    // ——— 战前编队扫描（只读截图 + 本地 OCR，不创建 Session/AFA/输入）———
    {
        let ui_weak = ui.as_weak();
        let copilot = Rc::clone(&copilot);
        let config = Rc::clone(&config);
        let pending = Rc::clone(&pending_formation);
        let generation = Arc::clone(&formation_generation);
        let tx = formation_tx.clone();
        ui.on_scan_formation(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_running() || ui.get_editor_mode() {
                ui.set_status_line("请在复刻执行模式、未运行时扫描编队".into());
                return;
            }
            let Some(job) = copilot.borrow().clone() else {
                ui.set_status_line("请先装载作业".into());
                return;
            };
            let job_names = job.oper_names();
            let config = config.borrow().clone();
            let current_generation = generation.fetch_add(1, Ordering::Relaxed) + 1;
            *pending.borrow_mut() = None;
            ui.set_formation_scanning(true);
            ui.set_formation_visible(false);
            ui.set_status_line("正在只读扫描战前编队…".into());
            let result_tx = tx.clone();
            std::thread::Builder::new()
                .name("formation-scan".into())
                .spawn(move || {
                    let result =
                        scan_formation(&job, &config).map_err(|error| format!("{error:#}"));
                    let _ = result_tx.send((current_generation, job_names, result));
                })
                .expect("failed to spawn formation scan thread");
        });
    }
    {
        let ui_weak = ui.as_weak();
        let draft = Rc::clone(&formation_draft);
        let pending = Rc::clone(&pending_formation);
        let generation = Arc::clone(&formation_generation);
        let log = Rc::clone(&log);
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                while let Ok((result_generation, job_names, result)) = formation_rx.try_recv() {
                    if result_generation != generation.load(Ordering::Relaxed) {
                        continue;
                    }
                    ui.set_formation_scanning(false);
                    match result {
                        Ok(mut value) => {
                            let detail = present_formation_scan(&ui, &mut value, &job_names);
                            append_log(&ui, &log, &format!("编队扫描完成：{detail}"));
                            *pending.borrow_mut() = None;
                            *draft.borrow_mut() = Some(value);
                        }
                        Err(error) => {
                            let message = format!("编队扫描失败：{error}");
                            ui.set_status_line(message.clone().into());
                            append_log(&ui, &log, &message);
                        }
                    }
                }
            },
        );
        std::mem::forget(timer);
    }
    {
        let ui_weak = ui.as_weak();
        let draft = Rc::clone(&formation_draft);
        ui.on_formation_select(move |slot, name| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let mut draft_guard = draft.borrow_mut();
            let Some(draft) = draft_guard.as_mut() else {
                return;
            };
            let name = name.trim();
            draft.selections.retain(|existing_slot, existing_name| {
                *existing_slot != slot as usize && (name.is_empty() || existing_name != name)
            });
            if !name.is_empty() {
                draft.selections.insert(slot as usize, name.to_owned());
            }
            let options = ui.get_formation_options();
            let selected_index = (0..options.row_count())
                .find(|index| {
                    options
                        .row_data(*index)
                        .is_some_and(|value| value.as_str() == name)
                })
                .unwrap_or(0) as i32;
            let rows = ui.get_formation_rows();
            for index in 0..rows.row_count() {
                let Some(mut row) = rows.row_data(index) else {
                    continue;
                };
                if row.slot == slot {
                    row.selected_index = selected_index;
                } else if !name.is_empty() && row.selected_index == selected_index {
                    row.selected_index = 0;
                }
                rows.set_row_data(index, row);
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let draft = Rc::clone(&formation_draft);
        let pending = Rc::clone(&pending_formation);
        ui.on_formation_confirm(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            let draft_guard = draft.borrow();
            let Some(draft) = draft_guard.as_ref() else {
                return;
            };
            let mut confirmed = Vec::new();
            for row in &draft.report.rows {
                let Some(name) = draft.selections.get(&row.slot) else {
                    continue;
                };
                let Some(avatar) = row.avatar.clone() else {
                    continue;
                };
                confirmed.push((draft.catalog.operator_for_job_name(name), avatar));
            }
            *pending.borrow_mut() = Some(PendingFormationScan {
                confirmed,
                config: draft.config.clone(),
                roster_fingerprint: draft.roster_fingerprint,
                game_hwnd: draft.game_hwnd,
                width: draft.width,
                height: draft.height,
                created_at: draft.created_at,
            });
            ui.set_formation_visible(false);
            ui.set_status_line("编队识别已确认；进入关卡后点击开始复刻".into());
        });
    }
    {
        let ui_weak = ui.as_weak();
        let draft = Rc::clone(&formation_draft);
        let pending = Rc::clone(&pending_formation);
        let generation = Arc::clone(&formation_generation);
        ui.on_formation_cancel(move || {
            generation.fetch_add(1, Ordering::Relaxed);
            *draft.borrow_mut() = None;
            *pending.borrow_mut() = None;
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_formation_visible(false);
            }
        });
    }

    // ——— 绑定桥 ———
    let bridge = Arc::new(BindingBridge {
        response: Mutex::new(None),
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
            let name = name.trim();
            pending.retain(|(i, existing)| *i != index && (name.is_empty() || existing != name));
            if !name.is_empty() {
                pending.push((index, name.to_owned()));
            }
            // 实时刷新"还没绑定"列表 —— 确认按钮的可用性就靠它。
            // 只认作业里声明过的名字：填错名字不会让按钮亮起来，
            // 用户能立刻从"还没绑定：N 名干员"看出有问题。
            if let Some(ui) = ui_weak.upgrade() {
                let cards = ui.get_cards();
                let options = ui.get_binding_options();
                let selected_index = (0..options.row_count())
                    .find(|row| {
                        options
                            .row_data(*row)
                            .is_some_and(|value| value.as_str() == name)
                    })
                    .unwrap_or(0) as i32;
                for row_index in 0..cards.row_count() {
                    let Some(mut row) = cards.row_data(row_index) else {
                        continue;
                    };
                    if row.index == index as i32 {
                        row.bound_to = name.into();
                        row.selected_index = selected_index;
                        cards.set_row_data(row_index, row);
                    } else if !name.is_empty() && row.bound_to.as_str() == name {
                        row.bound_to = SharedString::new();
                        row.selected_index = 0;
                        cards.set_row_data(row_index, row);
                    }
                }
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
            bridge.submit(BindingResponse::Confirm(pending.borrow().clone()));
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_binding_submitting(true);
                ui.set_status_line("正在校验并保存绑定，请勿关闭程序…".into());
            }
        });
    }
    {
        let ui_weak = ui.as_weak();
        let bridge = Arc::clone(&bridge);
        ui.on_binding_cancel(move || {
            bridge.submit(BindingResponse::Cancel);
            if let Some(ui) = ui_weak.upgrade() {
                ui.set_binding_visible(false);
                ui.set_binding_submitting(false);
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
        let pending_formation = Rc::clone(&pending_formation);

        ui.on_start_run(move || {
            let Some(ui) = ui_weak.upgrade() else { return };
            if ui.get_editor_mode() {
                ui.set_status_line("编辑模式严格禁止输入；请先切到「复刻执行」".into());
                return;
            }
            if ui.get_formation_scanning() {
                ui.set_status_line("编队扫描尚未完成，请等待或取消".into());
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
            let formation = match pending_formation.borrow_mut().take() {
                Some(pending) => match validate_pending_formation(&pending, &job) {
                    Ok(()) => Some(pending),
                    Err(error) => {
                        append_log(
                            &ui,
                            &log,
                            &format!("战前编队识别已失效，将回退到既有/人工绑定：{error}"),
                        );
                        None
                    }
                },
                None => None,
            };
            launch_run(
                &ui,
                ui_weak.clone(),
                job,
                config.borrow().clone(),
                RunMode::FromZero,
                formation,
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
                None,
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
                    "MAA 资源目录：{dir}\n逐帧推进：AFA [Hotkeys]/33ms\n配置文件在可执行文件旁边的 config.json"
                ),
            );
        });
    }

    // ——— 编辑页实时尺子帧 ———
    {
        let ui_weak = ui.as_weak();
        let ruler = Arc::clone(&ruler);
        let timer = slint::Timer::default();
        timer.start(
            slint::TimerMode::Repeated,
            EDITOR_RULER_REFRESH,
            move || {
                let Some(ui) = ui_weak.upgrade() else { return };
                if !ui.get_editor_mode() {
                    return;
                }
                let connected = ruler.status().is_connected();
                ui.set_ruler_connected(connected);
                let frame = if connected {
                    ruler
                        .latest()
                        .map(|snapshot| {
                            snapshot.total_elapsed_frames.clamp(0, i64::from(i32::MAX)) as i32
                        })
                        .unwrap_or(-1)
                } else {
                    -1
                };
                ui.set_editor_ruler_frame(frame);
            },
        );
        std::mem::forget(timer);
    }

    // ——— 尺子与外部程序状态轮询 ———
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

fn scan_formation(job: &Copilot, config: &Config) -> anyhow::Result<FormationDraft> {
    let job_names = job.oper_names();
    if job_names.is_empty() {
        anyhow::bail!("作业 opers 为空，没有可确认的开局编队");
    }
    let resource_dir = config
        .resolve_maa_resource_dir()
        .ok_or_else(|| anyhow::anyhow!("找不到可用的 MAA resource 目录"))?;
    let window = repl_capture::GameWindow::find()?;
    let geometry = window.geometry()?;
    let ui_scaler = config
        .ui_scaler_override
        .or_else(|| repl_input::game_keys::read_ui_scaler().ok().flatten())
        .unwrap_or(repl_core::viewport::DEFAULT_UI_SCALER);
    let viewport = repl_core::Viewport::new(geometry.width, geometry.height, ui_scaler);
    let mut capturer = repl_capture::WindowCapturer::new(window)?;
    let raw = capturer.grab()?;
    let reference = raw.resample(
        viewport.reference_source(),
        repl_core::REF_WIDTH as u32,
        repl_core::REF_HEIGHT as u32,
    );
    let resources = repl_vision::FormationResources::load(&resource_dir)?;
    let catalog = resources.catalog.clone();
    let task_config = resources.config.clone();
    let mut scanner = repl_vision::FormationScanner::new(resources)?;
    let report = scanner.scan(&reference, &job_names)?;
    Ok(FormationDraft {
        report,
        catalog,
        config: task_config,
        selections: HashMap::new(),
        roster_fingerprint: roster_fingerprint(&job_names),
        game_hwnd: window.raw_handle(),
        width: geometry.width,
        height: geometry.height,
        created_at: Instant::now(),
    })
}

fn formation_image(avatar: &repl_vision::FormationAvatar) -> Image {
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(avatar.width, avatar.height);
    for (target, source) in buffer
        .make_mut_bytes()
        .chunks_exact_mut(4)
        .zip(avatar.bgra.chunks_exact(4))
    {
        target.copy_from_slice(&[source[2], source[1], source[0], source[3]]);
    }
    Image::from_rgba8(buffer)
}

fn present_formation_scan(
    ui: &MainWindow,
    value: &mut FormationDraft,
    job_names: &[String],
) -> String {
    value.roster_fingerprint = roster_fingerprint(job_names);
    let mut options = vec![SharedString::from("— 忽略此槽位 —")];
    options.extend(job_names.iter().map(SharedString::from));
    let mut rows = Vec::with_capacity(value.report.rows.len());
    for row in &value.report.rows {
        let selected_name = row
            .default_selected
            .then_some(row.suggestion.as_ref())
            .flatten()
            .map(|operator| operator.display_name.as_str())
            .unwrap_or("");
        if !selected_name.is_empty() {
            value.selections.insert(row.slot, selected_name.to_owned());
        }
        let selected_index = job_names
            .iter()
            .position(|name| name == selected_name)
            .map_or(0, |index| index + 1) as i32;
        rows.push(FormationRow {
            slot: row.slot as i32,
            avatar: row
                .avatar
                .as_ref()
                .map_or_else(Image::default, formation_image),
            raw_text: row.raw_text.as_str().into(),
            confidence: format!("{:.1}%", row.confidence * 100.0).into(),
            suggestion: row
                .suggestion
                .as_ref()
                .map_or("", |operator| operator.display_name.as_str())
                .into(),
            source: row
                .source
                .map_or("无建议", repl_vision::SuggestionSource::zh)
                .into(),
            selected_index,
        });
    }
    let detail = formation_report_detail(&value.report);
    ui.set_formation_options(ModelRc::new(VecModel::from(options)));
    ui.set_formation_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_formation_detail(detail.clone().into());
    ui.set_formation_visible(true);
    ui.set_status_line("编队扫描完成，请核对后确认".into());
    detail
}

fn formation_report_detail(report: &repl_vision::FormationScanReport) -> String {
    let recognized = report
        .rows
        .iter()
        .filter(|row| row.suggestion.is_some())
        .count();
    let mut parts = vec![format!(
        "识别到 {} 个槽位，其中 {} 个有姓名建议{}",
        report.rows.len(),
        recognized,
        if report.used_old_layout {
            "（旧版布局）"
        } else {
            ""
        }
    )];
    if !report.missing_job_opers.is_empty() {
        parts.push(format!(
            "作业未命中：{}",
            report.missing_job_opers.join("、")
        ));
    }
    if !report.foreign_opers.is_empty() {
        parts.push(format!(
            "疑似非作业干员：{}",
            report.foreign_opers.join("、")
        ));
    }
    parts.join("；")
}

fn roster_fingerprint(names: &[String]) -> u64 {
    let mut names = names.to_vec();
    names.sort();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

fn validate_pending_formation(pending: &PendingFormationScan, job: &Copilot) -> anyhow::Result<()> {
    if pending.created_at.elapsed() > FORMATION_PENDING_TTL {
        anyhow::bail!("已超过 30 分钟有效期");
    }
    if pending.roster_fingerprint != roster_fingerprint(&job.oper_names()) {
        anyhow::bail!("当前作业编队已变化");
    }
    let window = repl_capture::GameWindow::find()?;
    let geometry = window.geometry()?;
    if window.raw_handle() != pending.game_hwnd {
        anyhow::bail!("游戏窗口已更换");
    }
    if geometry.width != pending.width || geometry.height != pending.height {
        anyhow::bail!(
            "游戏窗口尺寸从 {}×{} 变为 {}×{}",
            pending.width,
            pending.height,
            geometry.width,
            geometry.height
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn launch_run(
    ui: &MainWindow,
    ui_weak: slint::Weak<MainWindow>,
    job: Copilot,
    config: Config,
    run_mode: RunMode,
    pending_formation: Option<PendingFormationScan>,
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
                pending_formation,
                &*ruler,
                tx.clone(),
                worker_cancel,
                worker_bridge,
                ui_window,
            ) {
                log::error!("replication run failed: {e:#}");
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
    pending_formation: Option<PendingFormationScan>,
    frames: &dyn FrameSource,
    events: mpsc::Sender<Progress>,
    cancel: Arc<AtomicBool>,
    bridge: Arc<BindingBridge>,
    ui_window: Option<UiWindowControl>,
) -> anyhow::Result<()> {
    let mut session = Session::open(&config, &job.stage_name)?;
    let mut store = BindingStore::load();
    let startup_wanted = match &run_mode {
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
    // 全局头像档案不参与编队指纹；任何作业中出现过的部署目标都可以跨编队复用。
    for name in job.deploy_target_names() {
        if session.bound_names().contains(&name) {
            continue;
        }
        let Some(avatar) = store.avatar(&name).cloned() else {
            continue;
        };
        let restored = avatar
            .decode()
            .map_err(anyhow::Error::from)
            .and_then(|bgra| session.restore_avatar(&name, avatar.width, avatar.height, &bgra));
        match restored {
            Ok(()) => log::info!("restored global avatar for deploy target {name}"),
            Err(error) => log::warn!(
                "ignored damaged global avatar for deploy target {name}; it will require deferred binding: {error}"
            ),
        }
    }
    let stage = job.stage_name.clone();
    let mut pending_formation = pending_formation;
    // binder 在工作线程里自己给 UI 发事件（卡片数据只有它知道）。
    let events_for_binder = events.clone();

    let binder = Box::new(
        move |session: &mut Session,
              request: BindingRequest,
              cancel_flag: &AtomicBool|
              -> anyhow::Result<()> {
            match request {
                BindingRequest::Startup { mode } => {
                    if startup_wanted.is_empty() {
                        log::info!("no startup deployment bindings are required for this run");
                        return Ok(());
                    }
                    let cards = session.scan_deployment()?;
                    if cards.is_empty() {
                        anyhow::bail!("识别不到部署栏，确认游戏停在战斗界面");
                    }
                    let hashes = cards
                        .iter()
                        .map(|card| {
                            perceptual_hash(&card.avatar, card.avatar_width, card.avatar_height)
                        })
                        .collect::<Vec<_>>();
                    let key = fingerprint(&hashes);
                    let mut profile = store.get(&key).cloned().unwrap_or(BindingProfile {
                        stage_name: stage.clone(),
                        bindings: Vec::new(),
                    });
                    profile.stage_name = stage.clone();
                    let mut assignments = Vec::new();
                    let mut already_bound = session.bound_names();
                    for binding in profile.bindings.clone() {
                        if !startup_wanted.contains(&binding.name)
                            || already_bound.contains(&binding.name)
                        {
                            continue;
                        }
                        if let Some(card) = cards.iter().find(|card| card.index == binding.index) {
                            if session.bind(&binding.name, card).is_ok() {
                                store.remember_avatar(
                                    &binding.name,
                                    card.avatar_width,
                                    card.avatar_height,
                                    &card.avatar,
                                );
                                assignments.push((card.index, binding.name.clone()));
                                already_bound.push(binding.name);
                            }
                        }
                    }
                    // 既有 Session/global/profile 绑定优先；战前扫描只解释剩余卡片。
                    if let Some(pending) = pending_formation.take() {
                        let still_wanted = pending
                            .confirmed
                            .into_iter()
                            .filter(|(operator, _)| {
                                startup_wanted.contains(&operator.display_name)
                                    && !already_bound.contains(&operator.display_name)
                            })
                            .collect::<Vec<_>>();
                        let candidates = session.unrecognized_cards(&cards);
                        let bridged = repl_vision::bridge_formation_avatars(
                            &still_wanted,
                            &candidates,
                            &pending.config,
                        );
                        for matched in bridged {
                            let Some(card) = candidates
                                .iter()
                                .find(|card| card.index == matched.card_index)
                            else {
                                continue;
                            };
                            let name = matched.operator.display_name;
                            session.bind(&name, card)?;
                            store.remember_avatar(
                                &name,
                                card.avatar_width,
                                card.avatar_height,
                                &card.avatar,
                            );
                            profile.bindings.retain(|binding| {
                                binding.name != name && binding.index != card.index
                            });
                            profile.bindings.push(binding_record(&store, &name, card));
                            assignments.push((card.index, name.clone()));
                            already_bound.push(name.clone());
                            log::info!(
                                "formation bridge bound {name} to card #{} with NCC {:.3}",
                                card.index,
                                matched.score
                            );
                        }
                    }
                    let bound = session.bound_names();
                    let missing = startup_wanted
                        .iter()
                        .filter(|name| !bound.contains(name))
                        .cloned()
                        .collect::<Vec<_>>();
                    let visible_unrecognized = session.unrecognized_cards(&cards);
                    if !startup_binding_prompt_needed(
                        mode,
                        missing.len(),
                        visible_unrecognized.len(),
                    ) {
                        store.put(key, profile);
                        store.save()?;
                        if mode == BindingMode::AutomaticOnly && !visible_unrecognized.is_empty() {
                            log::info!(
                                "startup is running; deferred {} visible unrecognized cards and {} job names",
                                visible_unrecognized.len(),
                                missing.len(),
                            );
                        } else if !missing.is_empty() {
                            log::info!(
                                "startup has no unrecognized visible cards; deferred {} currently invisible job names",
                                missing.len()
                            );
                        }
                        return Ok(());
                    }

                    bridge.reset();
                    let _ = events_for_binder.send(Progress::NeedsBinding {
                        cards: binding_card_infos(&visible_unrecognized),
                        options: missing.clone(),
                        opers: missing,
                        assignments,
                        allow_partial: true,
                        title: "开局可见卡片绑定".into(),
                        detail: "只绑定部署栏里当前实际可见的干员；尚未出现的召唤物或装置可直接留空，首次部署时会再次暂停并要求绑定。绑定期间不会发送任何游戏输入。".into(),
                    });
                    let choices = wait_for_binding_response(
                        &bridge,
                        cancel_flag,
                        ui_window,
                        &events_for_binder,
                        "开局绑定",
                        None,
                    )?;
                    log::info!(
                        "startup binding confirmation received with {} visible assignments",
                        choices.len()
                    );
                    let mut chosen_names = std::collections::HashSet::new();
                    for (index, name) in choices {
                        if !startup_wanted.contains(&name) {
                            anyhow::bail!("绑定名称「{name}」不在当前开局编队中");
                        }
                        if !chosen_names.insert(name.clone()) {
                            anyhow::bail!("开局绑定中目标「{name}」被分配给多张卡片");
                        }
                        let card = cards
                            .iter()
                            .find(|card| card.index == index)
                            .ok_or_else(|| anyhow::anyhow!("卡片序号 {index} 不存在"))?;
                        session.bind(&name, card)?;
                        store.remember_avatar(
                            &name,
                            card.avatar_width,
                            card.avatar_height,
                            &card.avatar,
                        );
                        profile
                            .bindings
                            .retain(|binding| binding.name != name && binding.index != index);
                        profile.bindings.push(binding_record(&store, &name, card));
                    }
                    store.put(key, profile);
                    store.save()?;
                    log::info!("startup binding profile and global avatars saved");
                    let _ = events_for_binder.send(Progress::BindingCommitted {
                        message: "可见绑定已保存；请手动点击游戏窗口继续复刻".into(),
                    });
                    Ok(())
                }
                BindingRequest::Deferred { target, frame } => {
                    if session.deploy_target_visible(&target)? {
                        return Ok(());
                    }
                    let cards = session.deferred_binding_candidates(&target)?;
                    if cards.is_empty() {
                        anyhow::bail!(
                            "目标召唤物或装置「{target}」尚未出现在部署栏；本轮停止且未推进帧"
                        );
                    }
                    bridge.reset();
                    let _ = events_for_binder.send(Progress::NeedsBinding {
                        cards: binding_card_infos(&cards),
                        options: vec![target.clone()],
                        opers: vec![target.clone()],
                        assignments: Vec::new(),
                        allow_partial: false,
                        title: "新增召唤物绑定".into(),
                        detail: format!(
                            "目标「{target}」· F{frame}。游戏必须保持暂停；请从尚未被历史头像识别的卡片中选择目标。取消、游戏恢复或越过目标帧都会安全中止。"
                        ),
                    });
                    let choices = wait_for_binding_response(
                        &bridge,
                        cancel_flag,
                        ui_window,
                        &events_for_binder,
                        "延迟绑定",
                        Some((frames, frame)),
                    )?;
                    let matches = choices
                        .iter()
                        .filter(|(_, name)| name == &target)
                        .collect::<Vec<_>>();
                    if matches.len() != 1 {
                        anyhow::bail!("延迟绑定必须为「{target}」选择且只选择一张卡片");
                    }
                    let index = matches[0].0;
                    let card = cards
                        .iter()
                        .find(|card| card.index == index)
                        .ok_or_else(|| anyhow::anyhow!("卡片序号 {index} 不存在"))?;
                    session.bind(&target, card)?;
                    store.remember_avatar(
                        &target,
                        card.avatar_width,
                        card.avatar_height,
                        &card.avatar,
                    );
                    store.save()?;
                    log::info!("deferred binding avatar for {target} saved");
                    let _ = events_for_binder.send(Progress::BindingCommitted {
                        message: format!(
                            "「{target}」头像已保存；请手动点击游戏窗口继续 F{frame} 部署"
                        ),
                    });
                    Ok(())
                }
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

fn binding_card_infos(cards: &[repl_vision::Card]) -> Vec<repl_app::runner::BindingCardInfo> {
    cards
        .iter()
        .map(|card| repl_app::runner::BindingCardInfo {
            index: card.index,
            role: card.role.zh().to_owned(),
            available: card.available,
            cooling: card.cooling,
        })
        .collect()
}

fn startup_binding_prompt_needed(
    mode: BindingMode,
    missing_name_count: usize,
    visible_unrecognized_card_count: usize,
) -> bool {
    mode == BindingMode::InteractiveAllowed
        && missing_name_count > 0
        && visible_unrecognized_card_count > 0
}

fn binding_record(store: &BindingStore, name: &str, card: &repl_vision::Card) -> Binding {
    Binding {
        name: name.to_owned(),
        index: card.index,
        role: card.role.zh().to_owned(),
        width: card.avatar_width,
        height: card.avatar_height,
        avatar_base64: store
            .avatar(name)
            .map_or_else(String::new, |avatar| avatar.bgra_base64.clone()),
    }
}

fn wait_for_binding_response(
    bridge: &BindingBridge,
    cancel: &AtomicBool,
    ui_window: Option<UiWindowControl>,
    events: &mpsc::Sender<Progress>,
    context: &str,
    deferred_guard: Option<(&dyn FrameSource, i64)>,
) -> anyhow::Result<BindingChoices> {
    if let Some(window) = ui_window {
        if !window.activate_once() {
            let _ = events.send(Progress::Log(
                "复刻器窗口未能自动激活，请手动切回复刻器；等待上限 15 秒".into(),
            ));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !window.is_foreground() && std::time::Instant::now() < deadline {
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("用户中止{context}");
            }
            check_deferred_binding_guard(deferred_guard)?;
            std::thread::sleep(Duration::from_millis(50));
        }
        if !window.is_foreground() {
            anyhow::bail!("等待复刻器窗口回到前台超时，{context}中止");
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("用户中止{context}");
        }
        check_deferred_binding_guard(deferred_guard)?;
        if let Some(response) = bridge.take() {
            log::info!("{context} UI response received by worker");
            return match response {
                BindingResponse::Confirm(choices) => Ok(choices),
                BindingResponse::Cancel => anyhow::bail!("用户取消{context}，本轮安全中止"),
            };
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!("等待{context}超时（10 分钟）");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn check_deferred_binding_guard(guard: Option<(&dyn FrameSource, i64)>) -> anyhow::Result<()> {
    let Some((frames, target_frame)) = guard else {
        return Ok(());
    };
    let Some(snapshot) = frames.latest() else {
        return Ok(());
    };
    let view = repl_app::runner::to_view(&snapshot);
    if !view.trustworthy {
        return Ok(());
    }
    if !view.in_battle {
        anyhow::bail!("延迟绑定期间游戏离开战斗，本轮安全中止");
    }
    if !view.one_x {
        anyhow::bail!("延迟绑定期间游戏不再是 1x，本轮安全中止");
    }
    if view.paused == Some(false) {
        anyhow::bail!("延迟绑定期间检测到游戏恢复运行，本轮安全中止");
    }
    if view.elapsed != target_frame {
        anyhow::bail!(
            "延迟绑定期间尺子离开目标帧：目标 F{target_frame}，当前 F{}；未发送部署输入",
            view.elapsed
        );
    }
    Ok(())
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
                        ui.set_status_line(
                            match phase {
                                repl_app::runner::StartupPhase::WaitingFinalFocus => {
                                    "绑定已保存；请手动点击游戏窗口继续复刻".to_owned()
                                }
                                _ => format!("启动准备：{}", phase.zh()),
                            }
                            .into(),
                        );
                    }
                    Progress::Phase {
                        phase,
                        cursor: _,
                        target,
                    } => {
                        ui.set_phase(phase.zh().into());
                        ui.set_target_frame(target.map_or(-1, |t| t as i32));
                    }
                    Progress::NeedsBinding {
                        cards,
                        options,
                        opers,
                        assignments,
                        allow_partial,
                        title,
                        detail,
                    } => {
                        // 面板的全部内容都从这条事件来：卡片列表、待绑定名单。
                        let mut option_rows = vec![SharedString::from("— 当前卡片不绑定 —")];
                        option_rows.extend(
                            options
                                .iter()
                                .map(|name| SharedString::from(name.as_str())),
                        );
                        let rows: Vec<CardRow> = cards
                            .iter()
                            .map(|c| CardRow {
                                index: c.index as i32,
                                role: c.role.as_str().into(),
                                available: c.available,
                                cooling: c.cooling,
                                bound_to: assignments
                                    .iter()
                                    .find(|(index, _)| *index == c.index)
                                    .map_or_else(SharedString::new, |(_, name)| name.as_str().into()),
                                selected_index: assignments
                                    .iter()
                                    .find(|(index, _)| *index == c.index)
                                    .and_then(|(_, name)| {
                                        options.iter().position(|option| option == name)
                                    })
                                    .map_or(0, |index| index.saturating_add(1) as i32),
                            })
                            .collect();
                        ui.set_binding_options(ModelRc::new(VecModel::from(option_rows)));
                        ui.set_cards(ModelRc::new(VecModel::from(rows)));
                        let assigned_names = assignments
                            .iter()
                            .map(|(_, name)| name.as_str())
                            .collect::<Vec<_>>();
                        let unbound = opers
                            .iter()
                            .filter(|name| !assigned_names.contains(&name.as_str()))
                            .map(|name| SharedString::from(name.as_str()))
                            .collect::<Vec<_>>();
                        ui.set_unbound_opers(ModelRc::new(VecModel::from(unbound)));
                        *binding_opers.borrow_mut() = opers;
                        *pending_choices.borrow_mut() = assignments;
                        ui.set_binding_allow_partial(allow_partial);
                        ui.set_binding_title(title.into());
                        ui.set_binding_detail(detail.into());
                        ui.set_binding_submitting(false);
                        ui.set_binding_visible(true);
                        ui.set_status_line("游戏保持暂停：请在复刻器中完成绑定".into());
                    }
                    Progress::BindingCommitted { message } => {
                        ui.set_binding_submitting(false);
                        ui.set_binding_visible(false);
                        ui.set_status_line(message.clone().into());
                        append_log(&ui, &log, &message);
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
                                "警告：AFA 逐帧动作出现过跨帧，本次复刻的帧精度不可信。请保留录像、CSV 和日志后停止验收。",
                            );
                        }
                    }
                    Progress::Failed(why) => {
                        ui.set_running(false);
                        ui.set_binding_submitting(false);
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
                } else if action.kind.eq_ignore_ascii_case("Skill")
                    && action.name.is_empty()
                    && action.location.is_some()
                {
                    "地图装置".into()
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
    let deferred_targets: Vec<SharedString> = document
        .deferred_target_names()
        .iter()
        .map(|name| name.as_str().into())
        .collect();
    let mut operator_options = vec![SharedString::from("— 选择目标 —")];
    operator_options.extend(
        document
            .target_names()
            .iter()
            .map(|name| SharedString::from(name.as_str())),
    );

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
    ui.set_editor_deferred_targets(ModelRc::new(VecModel::from(deferred_targets)));
    ui.set_editor_operator_options(ModelRc::new(VecModel::from(operator_options)));

    if let Some(action) = editor.selected_action() {
        let operator_index = document
            .target_names()
            .iter()
            .position(|name| name == &action.name)
            .map_or(0, |index| {
                index.saturating_add(1).min(i32::MAX as usize) as i32
            });
        ui.set_editor_selected_id(action.id.get().min(i32::MAX as u64) as i32);
        ui.set_editor_selected_kind(editor_kind_label(&action.kind).into());
        ui.set_editor_selected_frame(
            action
                .frame
                .map_or_else(String::new, |frame| frame.to_string())
                .into(),
        );
        ui.set_editor_selected_operator_index(operator_index);
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
        ui.set_editor_selected_direction_index(direction_index(action.direction));
        ui.set_editor_selected_direction(direction_name(action.direction).into());
        ui.set_editor_selected_doc(action.doc.into());
    } else {
        ui.set_editor_selected_id(-1);
        ui.set_editor_selected_kind("".into());
        ui.set_editor_selected_frame("".into());
        ui.set_editor_selected_operator_index(0);
        ui.set_editor_selected_name("".into());
        ui.set_editor_selected_x("".into());
        ui.set_editor_selected_y("".into());
        ui.set_editor_selected_direction_index(3);
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
        "none" | "无" | "无方向" => Direction::None,
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

fn direction_index(direction: Direction) -> i32 {
    match direction {
        Direction::Up => 0,
        Direction::Down => 1,
        Direction::Left => 2,
        Direction::Right => 3,
        Direction::None => 4,
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

fn load_editor_resources(
    resource_dir: &std::path::Path,
) -> Result<(LevelPack, repl_vision::OperatorCatalog), String> {
    let level_pack = LevelPack::load(resource_dir.join("Arknights-Tile-Pos"))
        .map_err(|error| format!("加载关卡索引失败：{error}"))?;
    let catalog_path = resource_dir.join("battle_data.json");
    let operator_catalog = repl_vision::OperatorCatalog::load(&catalog_path)
        .map_err(|error| format!("加载干员目录失败：{error}"))?;
    Ok((level_pack, operator_catalog))
}

/// 选择 MAA 的 `resource` 目录；也允许用户选择其上一级 MAA 目录，后续统一归一化。
#[cfg(windows)]
fn pick_maa_resource_dir(owner: Option<isize>) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS,
        BROWSEINFOW,
    };

    let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let selected = (|| {
        let mut display_name = [0u16; 260];
        let title: Vec<u16> = "选择 MAA 的 resource 目录（也可选择 MAA 根目录）\0"
            .encode_utf16()
            .collect();
        let browse = BROWSEINFOW {
            hwndOwner: HWND(owner.unwrap_or_default() as *mut core::ffi::c_void),
            pszDisplayName: PWSTR(display_name.as_mut_ptr()),
            lpszTitle: PCWSTR(title.as_ptr()),
            ulFlags: BIF_RETURNONLYFSDIRS
                | if com_initialized {
                    BIF_NEWDIALOGSTYLE
                } else {
                    0
                },
            ..Default::default()
        };
        // SAFETY: BROWSEINFOW 在调用期间持有有效缓冲区；返回的 PIDL 由 shell 分配并在下方释放。
        let pidl = unsafe { SHBrowseForFolderW(&browse) };
        if pidl.is_null() {
            return None;
        }
        let mut path = [0u16; 260];
        // SAFETY: path 是固定长度 MAX_PATH 缓冲区，pidl 直到 CoTaskMemFree 前保持有效。
        let ok = unsafe { SHGetPathFromIDListW(pidl, &mut path) }.as_bool();
        // SAFETY: pidl 来自 SHBrowseForFolderW，并且只释放一次。
        unsafe { CoTaskMemFree(Some(pidl.cast())) };
        if !ok {
            return None;
        }
        let end = path.iter().position(|character| *character == 0)?;
        Some(String::from_utf16_lossy(&path[..end]))
    })();
    if com_initialized {
        // SAFETY: 与本函数内成功的 CoInitializeEx 成对。
        unsafe { CoUninitialize() };
    }
    selected
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
fn pick_maa_resource_dir(_owner: Option<isize>) -> Option<String> {
    std::env::var("REPLICATOR_MAA_RESOURCE").ok()
}

#[cfg(not(windows))]
fn pick_save_file() -> Option<String> {
    std::env::args().nth(2)
}

#[cfg(test)]
mod editor_callback_tests {
    use super::*;

    #[test]
    fn editor_exposes_a_persistent_maa_resource_picker() {
        let editor_ui = include_str!("../../../ui/main.slint");
        let main_source = include_str!("main.rs");

        assert!(editor_ui.contains("callback editor-select-resource()"));
        assert!(editor_ui.contains("root.editor-select-resource()"));
        assert!(editor_ui.contains("editor-resource-path"));
        assert!(editor_ui.contains("MAA 资源"));
        assert!(main_source.contains("on_editor_select_resource"));
        assert!(main_source.contains("normalize_maa_resource_dir"));
        assert!(main_source.contains("config.borrow().save()"));
    }

    #[test]
    fn editor_field_tracks_the_next_selected_action_after_user_input() {
        let editor_ui = include_str!("../../../ui/main.slint");
        for property in [
            "editor-selected-frame",
            "editor-selected-x",
            "editor-selected-y",
            "editor-selected-doc",
        ] {
            assert!(
                editor_ui.contains(&format!("value: root.{property}")),
                "{property} must remain bound to its committed editor value"
            );
        }
        assert!(editor_ui.contains("changed value =>"));
        assert!(editor_ui.contains("root.text = root.value;"));
    }

    #[test]
    fn editor_text_fields_commit_as_one_edit_instead_of_each_keystroke() {
        let editor_ui = include_str!("../../../ui/main.slint");

        assert!(
            editor_ui.contains("component CommitLineEdit"),
            "editor fields need a draft/commit boundary"
        );
        assert!(
            !editor_ui.contains("edited => { root.editor-set-action(\"frame\", self.text); }"),
            "frame edits must not enter undo history on every keystroke"
        );
        assert!(
            editor_ui.contains("commit(value) => { root.editor-set-action(\"frame\", value); }"),
            "the confirmed frame value must still reach the editor callback"
        );
    }

    #[test]
    fn confirmed_frame_edit_is_one_undo_step() {
        let mut editor = EditorState::default();
        editor.add_action("Deploy");
        let mut action = editor.selected_action().unwrap();
        assert_eq!(action.frame, Some(0));

        action.frame = Some(249);
        editor.update_action(action).unwrap();
        assert_eq!(editor.selected_action().unwrap().frame, Some(249));

        assert!(editor.undo());
        assert_eq!(editor.selected_action().unwrap().frame, Some(0));
        assert!(editor.undo());
        assert!(editor.document().actions().is_empty());
    }

    #[test]
    fn editor_dropdowns_use_roster_and_five_supported_directions() {
        let editor_ui = include_str!("../../../ui/main.slint");
        assert!(editor_ui.contains("model: root.editor-operator-options;"));
        assert!(editor_ui.contains("model: root.editor-direction-options;"));
        assert!(editor_ui.contains("[\"上\", \"下\", \"左\", \"右\", \"无方向\"]"));

        for (label, direction, index) in [
            ("上", Direction::Up, 0),
            ("下", Direction::Down, 1),
            ("左", Direction::Left, 2),
            ("右", Direction::Right, 3),
            ("无方向", Direction::None, 4),
        ] {
            assert_eq!(parse_editor_direction(label), direction);
            assert_eq!(direction_index(direction), index);
        }
    }

    #[test]
    fn editor_roster_addition_exposes_local_catalog_search() {
        let editor_ui = include_str!("../../../ui/main.slint");
        let main_source = include_str!("main.rs");

        assert!(editor_ui.contains("placeholder-text: \"搜索或手动输入干员\""));
        assert!(editor_ui.contains("editor-operator-search-results"));
        assert!(editor_ui.contains("callback editor-search-operators(string)"));
        assert!(editor_ui.contains("callback editor-select-operator-search-result(string)"));
        assert!(main_source.contains("OperatorCatalog::load(&catalog_path)"));
        assert!(main_source.contains("catalog.search_names(query.as_str(), 12)"));
        assert!(main_source.contains("ui.on_editor_select_operator_search_result"));
    }

    #[test]
    fn editor_shows_live_ruler_absolute_frame_without_enabling_follow_mode() {
        let editor_ui = include_str!("../../../ui/main.slint");
        let main_source = include_str!("main.rs");

        assert!(editor_ui.contains("in property <int> editor-ruler-frame: -1;"));
        assert!(editor_ui.contains("text: \"尺子绝对帧\""));
        assert!(editor_ui.contains("root.editor-ruler-frame >= 0"));
        assert!(main_source.contains("ui.set_editor_ruler_frame("));
        assert!(main_source.contains("snapshot.total_elapsed_frames.clamp"));
        assert!(EDITOR_RULER_REFRESH <= Duration::from_millis(34));
    }

    #[test]
    fn editor_exposes_map_device_skill_location_and_clear_mode() {
        let editor_ui = include_str!("../../../ui/main.slint");
        let main_source = include_str!("main.rs");

        assert!(editor_ui.contains("+ 技能 / 装置"));
        assert!(editor_ui.contains(
            "root.editor-selected-kind == \"部署\" || root.editor-selected-kind == \"技能\""
        ));
        assert!(editor_ui.contains("技能目标格（地图装置必填；普通干员可留空）"));
        assert!(editor_ui.contains("callback editor-clear-location()"));
        assert!(main_source.contains("ui.on_editor_clear_location"));
        assert!(main_source.contains("\"地图装置\".into()"));
    }

    #[test]
    fn binding_confirmation_waits_for_commit_and_uses_job_roster() {
        let main_source = include_str!("main.rs");
        let binding_ui = include_str!("../../../ui/main.slint");
        let callback_start = main_source.find("ui.on_binding_confirm").unwrap();
        let callback_end = main_source[callback_start..]
            .find("ui.on_binding_cancel")
            .map(|offset| callback_start + offset)
            .unwrap();
        let callback = &main_source[callback_start..callback_end];

        assert!(
            !callback.contains("set_binding_visible(false)"),
            "确认只能提交响应；必须等工作线程验证并保存后再关闭绑定面板"
        );
        assert!(
            binding_ui.contains("model: root.binding-options"),
            "可见绑定必须从作业编队下拉选择，不能继续使用自由文本"
        );
    }

    #[test]
    fn invisible_job_names_do_not_reopen_startup_binding() {
        assert!(!startup_binding_prompt_needed(
            BindingMode::InteractiveAllowed,
            8,
            0,
        ));
        assert!(startup_binding_prompt_needed(
            BindingMode::InteractiveAllowed,
            8,
            2,
        ));
        assert!(!startup_binding_prompt_needed(
            BindingMode::AutomaticOnly,
            8,
            2,
        ));
    }

    #[test]
    fn binding_bridge_delivers_confirmation_once() {
        let bridge = BindingBridge {
            response: Mutex::new(None),
        };
        bridge.submit(BindingResponse::Confirm(vec![(3, "极境".into())]));
        match bridge.take() {
            Some(BindingResponse::Confirm(choices)) => {
                assert_eq!(choices, vec![(3, "极境".into())]);
            }
            _ => panic!("confirmation was not delivered"),
        }
        assert!(bridge.take().is_none());
    }

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

    #[test]
    fn formation_roster_fingerprint_is_order_independent() {
        let left = vec!["桃金娘".to_owned(), "风笛".to_owned()];
        let right = vec!["风笛".to_owned(), "桃金娘".to_owned()];
        assert_eq!(roster_fingerprint(&left), roster_fingerprint(&right));
        assert_ne!(
            roster_fingerprint(&left),
            roster_fingerprint(&["桃金娘".to_owned()])
        );
    }

    #[test]
    fn formation_ui_is_explicit_confirmable_and_generation_guarded() {
        let main_source = include_str!("main.rs");
        let ui_source = include_str!("../../../ui/main.slint");
        assert!(ui_source.contains("text: formation-scanning ? \"扫描中…\" : \"扫描编队\""));
        assert!(ui_source.contains("callback formation-confirm()"));
        assert!(ui_source.contains("— 忽略此槽位 —"));
        assert!(main_source.contains("result_generation != generation.load(Ordering::Relaxed)"));
        assert!(main_source.contains("RunMode::Continue(plan),\n                None,"));
    }

    #[test]
    fn read_only_formation_scan_helper_has_no_input_dependency() {
        let source = include_str!("main.rs");
        let start = source.find("fn scan_formation(").unwrap();
        let end = source[start..]
            .find("fn formation_image(")
            .map(|offset| start + offset)
            .unwrap();
        let helper = &source[start..end];
        for forbidden in [
            "AfaController",
            "Session::",
            "dispatch(",
            "InjectTouch",
            "SendInput",
        ] {
            assert!(!helper.contains(forbidden), "scan helper used {forbidden}");
        }
    }
}
