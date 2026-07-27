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
    runner::{Progress, Runner},
    session::Session,
};
use repl_core::{copilot::ActionType, Copilot};
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

fn main() -> Result<(), slint::PlatformError> {
    // GUI 没有控制台，日志写到 exe 旁边的 replicator.log（每次启动覆盖）。
    let log_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("replicator.log")))
        .unwrap_or_else(|| std::path::PathBuf::from("replicator.log"));
    // repl_app 开 debug：被拒样本的逐条记录只在 debug 级，出问题时全靠它定位。
    repl_app::init_to_file("info,repl_app=debug", &log_path);

    let config = Rc::new(RefCell::new(Config::load()));
    let ui = MainWindow::new()?;

    ui.set_disclaimer_visible(!config.borrow().disclaimer_accepted);
    ui.set_actions(ModelRc::new(VecModel::<ActionRow>::default()));
    ui.set_cards(ModelRc::new(VecModel::<CardRow>::default()));
    ui.set_unbound_opers(ModelRc::new(VecModel::<SharedString>::default()));
    ui.set_afa_ready(false);
    ui.set_afa_detail("未检测".into());

    let ruler = Arc::new(RulerClient::connect(config.borrow().ruler_ws_url.clone()));
    let copilot: Rc<RefCell<Option<Copilot>>> = Rc::new(RefCell::new(None));
    let cancel = Arc::new(AtomicBool::new(false));
    let log = Rc::new(RefCell::new(String::new()));

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
                    ui.set_can_start(true);
                    ui.set_status_line("作业已装载，进入关卡前点「开始复刻」".into());
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
            cancel.store(false, Ordering::Relaxed);
            ui.set_running(true);
            ui.set_status_line("正在准备…".into());
            append_log(&ui, &log, "开始复刻");

            let (tx, rx) = mpsc::channel::<Progress>();
            let cfg = config.borrow().clone();
            let ruler = Arc::clone(&ruler);
            let cancel = Arc::clone(&cancel);
            let bridge = Arc::clone(&bridge);

            std::thread::Builder::new()
                .name("replicator-run".into())
                .spawn(move || {
                    if let Err(e) = run_once(job, cfg, &*ruler, tx.clone(), cancel, bridge) {
                        let _ = tx.send(Progress::Failed(e.to_string()));
                    }
                })
                .expect("failed to spawn run thread");

            pump_progress(
                ui_weak.clone(),
                rx,
                Rc::clone(&log),
                Rc::clone(&pending_choices),
                Rc::clone(&binding_opers),
            );
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
                let afa = repl_input::AfaController::probe();
                ui.set_afa_ready(afa.ready);
                ui.set_afa_detail(afa.detail.into());
                if !afa.ready {
                    ui.set_can_start(false);
                } else if !ui.get_running() && copilot.borrow().is_some() {
                    ui.set_can_start(true);
                }
                if let Some(snapshot) = ruler.latest() {
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
                    if !ui.get_running() {
                        ui.set_cursor_frame(snapshot.total_elapsed_frames as i32);
                    }
                }
                ui.set_game_found(repl_capture::GameWindow::find().is_ok());
            },
        );
        // Timer 必须活到窗口关闭为止，泄漏掉最省事也最安全。
        std::mem::forget(timer);
    }

    ui.run()
}

/// 工作线程：跑完一整场复刻。
fn run_once(
    job: Copilot,
    config: Config,
    frames: &dyn FrameSource,
    events: mpsc::Sender<Progress>,
    cancel: Arc<AtomicBool>,
    bridge: Arc<BindingBridge>,
) -> anyhow::Result<()> {
    let mut session = Session::open(&config, &job.stage_name)?;
    let mut store = BindingStore::load();
    let wanted = job.oper_names();
    let stage = job.stage_name.clone();
    // binder 在工作线程里自己给 UI 发事件（卡片数据只有它知道）。
    let events_for_binder = events.clone();

    let binder = Box::new(move |session: &mut Session| -> anyhow::Result<bool> {
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
        if let Some(profile) = store.get(&key) {
            let mut restored = 0;
            for binding in &profile.bindings {
                if let Some(card) = cards.get(binding.index) {
                    if session.bind(&binding.name, card).is_ok() {
                        restored += 1;
                    }
                }
            }
            if restored == wanted.len() {
                log::info!("restored {restored} bindings from saved profile");
                return Ok(true);
            }
            log::info!(
                "saved profile only covered {restored}/{} opers; asking user",
                wanted.len()
            );
        }

        // 交给 UI 让用户填。卡片数据随 NeedsBinding 事件送过去 ——
        // UI 面板展示的一切都来自这条事件。
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
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        loop {
            let choices = loop {
                if let Some(c) = bridge.choices.lock().unwrap().take() {
                    break c;
                }
                if std::time::Instant::now() > deadline {
                    anyhow::bail!("等待编队绑定超时（10 分钟）");
                }
                std::thread::sleep(Duration::from_millis(50));
            };

            let mut profile = BindingProfile {
                stage_name: stage.clone(),
                bindings: Vec::new(),
            };
            for (index, name) in &choices {
                let card = cards
                    .get(*index)
                    .ok_or_else(|| anyhow::anyhow!("卡片序号 {index} 不存在"))?;
                session.bind(name, card)?;
                profile.bindings.push(Binding {
                    name: name.clone(),
                    index: *index,
                    role: card.role.zh().to_owned(),
                    width: card.avatar_width,
                    height: card.avatar_height,
                    avatar_base64: String::new(),
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
    });

    let mut runner = Runner::new(job, &config, &mut session, frames, events, cancel, binder);
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
                    Progress::Phase {
                        phase,
                        cursor,
                        target,
                    } => {
                        ui.set_phase(phase.zh().into());
                        ui.set_cursor_frame(cursor as i32);
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
                    } => {
                        ui.set_running(false);
                        let summary = format!(
                            "复刻完成。脉冲 {pulses} 次（空脉冲 {zero_pulses}，跨帧 {overshoots}）"
                        );
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

fn build_timeline(job: &Copilot) -> ModelRc<ActionRow> {
    let rows: Vec<ActionRow> = job
        .actions
        .iter()
        .map(|a| ActionRow {
            frame: a.frame as i32,
            kind: match a.kind {
                ActionType::Deploy => "部署",
                ActionType::UseSkill => "技能",
                ActionType::Retreat => "撤退",
                ActionType::Output => "注释",
                _ => "不支持",
            }
            .into(),
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

#[cfg(not(windows))]
fn pick_job_file() -> Option<String> {
    std::env::args().nth(1)
}
