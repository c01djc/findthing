#![windows_subsystem = "windows"]

mod apps;
mod commands;
mod elevate;
mod hit;
mod index;
mod locate;
mod ntfs;
mod platform;
mod search;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use slint::{ComponentHandle, Model, VecModel};

use hit::{Hit, Kind};
use index::Catalog;

slint::include_modules!();

struct UiState {
    model: Rc<VecModel<Row>>,
    hits: Rc<RefCell<Vec<Hit>>>,
}

thread_local! {
    static UI_STATE: RefCell<Option<UiState>> = const { RefCell::new(None) };
    static MENU_HIT: RefCell<Option<Hit>> = const { RefCell::new(None) };
    static FOLDER_PICKS: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
}

static FOLDER: Mutex<String> = Mutex::new(String::new());

struct SearchJob {
    generation: u64,
    query: String,
    folder: String,
    recent: Vec<String>,
}

fn main() {
    if std::env::args().any(|arg| arg == "--startup") {
        platform::mark_quiet_launch();
    }
    let from_task = std::env::args().any(|arg| arg == "--elevated-task");
    if std::env::args().any(|arg| arg == "--app") {
        app_main();
        return;
    }
    if from_task && !elevate::is_elevated() {
        elevate::remove_task();
        watchdog();
        return;
    }
    if elevate::hand_off_for_fast_index() {
        return;
    }
    watchdog();
}

fn watchdog() {
    let Ok(exe) = std::env::current_exe() else {
        app_main();
        return;
    };
    let mut burst = 0u32;
    let mut last = std::time::Instant::now();
    loop {
        let mut child = match std::process::Command::new(&exe).arg("--app").spawn() {
            Ok(child) => child,
            Err(_) => {
                app_main();
                return;
            }
        };
        let code = child.wait().ok().and_then(|status| status.code());
        if code == Some(0) {
            break;
        }
        if last.elapsed() > std::time::Duration::from_secs(30) {
            burst = 0;
        }
        last = std::time::Instant::now();
        burst += 1;
        if burst >= 6 {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn app_main() {
    platform::refresh_startup_path();
    platform::allow_summon_from_lower_integrity();
    platform::enable_dpi();
    platform::load_theme();
    locate::ensure_com();
    apps::warm();
    if platform::notify_running_instance() {
        return;
    }

    let catalog = Arc::new(Catalog::new());
    let indexing = Arc::clone(&catalog);
    let recents = Arc::new(search::Recents::load());
    thread::Builder::new()
        .name("findthing-index".into())
        .spawn(move || {
            index::relax_thread();
            if let Some(saved) = index::Index::load(&index::store_path()) {
                indexing.restore(saved);
            }
            let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
            let mut drives = index::fixed_drives();
            if let Some(profile) = &profile {
                drives.sort_by_key(|drive| if profile.starts_with(drive) { 0 } else { 1 });
            }
            indexing.walk(drives, profile);
        })
        .ok();

    let ui = Launcher::new().expect("创建窗口失败");
    let model: Rc<VecModel<Row>> = Rc::new(VecModel::default());
    ui.set_rows(model.clone().into());
    let hits: Rc<RefCell<Vec<Hit>>> = Rc::new(RefCell::new(Vec::new()));

    bind(&ui, &catalog, &recents);
    UI_STATE.with(|state| {
        *state.borrow_mut() = Some(UiState {
            model: model.clone(),
            hits: hits.clone(),
        });
    });
    platform::spawn_shell({
        let weak = ui.as_weak();
        let catalog = Arc::clone(&catalog);
        move |event| {
            let weak = weak.clone();
            let catalog = Arc::clone(&catalog);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                UI_STATE.with(|state| {
                    let state = state.borrow();
                    let Some(state) = state.as_ref() else {
                        return;
                    };
                    match event {
                        platform::ShellEvent::Show => {
                            show_launcher(&ui, &catalog, &state.model, &state.hits)
                        }
                        platform::ShellEvent::Hide => {
                            if platform::save_dialog_in_use() {
                                return;
                            }
                            hide_launcher(&ui);
                        }
                        platform::ShellEvent::Quit => {
                            hide_launcher(&ui);
                            std::process::exit(0);
                        }
                        platform::ShellEvent::Capture => {
                            show_launcher(&ui, &catalog, &state.model, &state.hits);
                            ui.set_capturing(true);
                            ui.set_status("按下组合键，或再双击 Ctrl".into());
                        }
                        platform::ShellEvent::CaptureDone => {
                            ui.set_capturing(false);
                            paint_summon(&ui);
                            ui.set_status("".into());
                        }
                        platform::ShellEvent::Theme => present_theme(&ui),
                        platform::ShellEvent::Save => {
                            if ui.window().is_visible() && ui.get_docked() && ui.get_save_dialog() {
                                place_on_explorer(&ui);
                                return;
                            }
                            show_folder_launcher(&ui, &catalog, &state.model, &state.hits);
                            let weak = ui.as_weak();
                            slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
                                let Some(ui) = weak.upgrade() else {
                                    return;
                                };
                                platform::focus_launcher(ui.window());
                                ui.invoke_arm_input();
                                platform::prefer_chinese_input(ui.window());
                            });
                        }
                        platform::ShellEvent::Folder(generation) => {
                            if generation != platform::folder_generation() {
                                return;
                            }
                            let opened = !ui.window().is_visible() || !ui.get_docked();
                            if opened {
                                show_folder_launcher(&ui, &catalog, &state.model, &state.hits);
                            }
                            let backs = platform::take_folder_backs();
                            let keys = platform::take_folder_keys();
                            if backs == 0 && keys.is_empty() {
                                return;
                            }
                            let arm_ime = opened || !ui.get_input_focused();
                            let weak = ui.as_weak();
                            slint::Timer::single_shot(std::time::Duration::from_millis(30), move || {
                                let Some(ui) = weak.upgrade() else {
                                    return;
                                };
                                platform::focus_launcher(ui.window());
                                ui.invoke_arm_input();
                                if arm_ime {
                                    platform::prefer_chinese_input(ui.window());
                                }
                                platform::replay_folder_input(backs, &keys);
                            });
                        }
                    }
                });
            });
        }
    });

    let quiet = platform::take_quiet_launch();
    if quiet {
        ui.hide().ok();
    } else {
        show_launcher(&ui, &catalog, &model, &hits);
    }
    // Hiding the search window must leave the tray process running.
    slint::run_event_loop_until_quit().ok();
}

fn bind(ui: &Launcher, catalog: &Arc<Catalog>, recents: &Arc<search::Recents>) {
    let weak = ui.as_weak();
    let catalog_query = Arc::clone(catalog);
    let recents_query = Arc::clone(recents);
    let (tx, rx) = mpsc::channel::<SearchJob>();
    let latest = Arc::new(AtomicU64::new(0));
    let latest_worker = Arc::clone(&latest);
    let weak_worker = weak.clone();
    thread::Builder::new()
        .name("findthing-search".into())
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    job = newer;
                }
                let SearchJob {
                    generation,
                    query,
                    folder,
                    recent,
                } = job;
                if latest_worker.load(Ordering::Relaxed) != generation {
                    continue;
                }
                let found = if query.trim().is_empty() {
                    Vec::new()
                } else {
                    catalog_query.read(|index| {
                        search::query(index, query.as_str(), 30, folder.as_str(), &recent)
                    })
                };
                let latest_apply = Arc::clone(&latest_worker);
                let weak_apply = weak_worker.clone();
                let catalog_apply = Arc::clone(&catalog_query);
                let query_apply = query.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if latest_apply.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    let Some(ui) = weak_apply.upgrade() else {
                        return;
                    };
                    if ui.get_docked() {
                        return;
                    }
                    UI_STATE.with(|state| {
                        let state = state.borrow();
                        let Some(state) = state.as_ref() else {
                            return;
                        };
                        apply_hits(
                            &ui,
                            &state.model,
                            &state.hits,
                            found,
                            &catalog_apply,
                            &query_apply,
                        );
                    });
                });
            }
        })
        .ok();
    let weak_query = weak.clone();
    let catalog_folder = Arc::clone(catalog);
    ui.on_query_changed(move |query| {
        let text = query.to_string();
        if platform::folder_synced() {
            let Some(ui) = weak_query.upgrade() else {
                return;
            };
            if text.trim().is_empty() {
                if ui.get_save_dialog() && platform::save_dialog_in_use() {
                    return;
                }
                if ui.get_query().trim().is_empty()
                    && platform::folder_saw_text()
                    && !platform::folder_arming()
                {
                    hide_launcher(&ui);
                }
                return;
            }
            platform::mark_folder_text();
            UI_STATE.with(|state| {
                let state = state.borrow();
                let Some(state) = state.as_ref() else {
                    return;
                };
                show_folder_matches(&ui, &state.model, &state.hits, &catalog_folder, &text);
            });
            return;
        }
        let generation = latest.fetch_add(1, Ordering::Relaxed) + 1;
        let folder = FOLDER.lock().unwrap_or_else(|err| err.into_inner()).clone();
        let recent = recents_query.snapshot();
        platform::follow_folder(&text);
        let _ = tx.send(SearchJob {
            generation,
            query: text,
            folder,
            recent,
        });
    });

    let weak = ui.as_weak();
    ui.on_nudge(move |delta| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let len = ui.get_rows().row_count();
        if len == 0 {
            return;
        }
        let last = len as i32 - 1;
        let next = (ui.get_selected() + delta).clamp(0, last);
        ui.set_selected(next);
        if ui.get_docked() && !ui.get_save_dialog() {
            sync_pick(next);
        }
    });

    let weak = ui.as_weak();
    let recents_open = Arc::clone(recents);
    ui.on_submitted(move |index, reveal| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if ui.get_docked() && ui.get_save_dialog() {
            let path = UI_STATE.with(|state| {
                let state = state.borrow();
                state.as_ref().and_then(|state| {
                    state.hits.borrow().get(index as usize).map(|hit| hit.target.clone())
                })
            });
            if let Some(path) = path {
                if locate::browse_pinned(&path) {
                    let wanted = path.clone();
                    let weak = ui.as_weak();
                    slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
                        let Some(ui) = weak.upgrade() else {
                            return;
                        };
                        if locate::filename_holds(&wanted) {
                            ui.set_status("没有转到这个目录".into());
                            return;
                        }
                        ui.set_query("".into());
                        ui.set_status("".into());
                    });
                } else {
                    ui.set_status("没有转到这个目录".into());
                }
            }
            return;
        }
        activate(&ui, index, reveal, &recents_open);
    });

    let weak = ui.as_weak();
    ui.on_jump(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.get_docked() || ui.get_save_dialog() {
            return;
        }
        let len = ui.get_rows().row_count() as i32;
        if index < 0 || index >= len {
            return;
        }
        ui.set_selected(index);
        sync_pick(index);
    });

    let weak = ui.as_weak();
    ui.on_context_menu(move |index| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ui.set_selected(index);
        let chosen = UI_STATE.with(|state| {
            let state = state.borrow();
            let Some(state) = state.as_ref() else {
                return None;
            };
            let chosen = state.hits.borrow().get(index as usize).cloned();
            chosen
        });
        let Some(hit) = chosen else {
            return;
        };
        let Some((x, y)) = platform::cursor_in_window(ui.window()) else {
            return;
        };
        ui.set_menu_command(hit.kind == Kind::Command);
        ui.set_menu_x(x);
        ui.set_menu_y(y);
        MENU_HIT.with(|slot| *slot.borrow_mut() = Some(hit));
        ui.invoke_open_menu();
    });

    let weak = ui.as_weak();
    let recents_menu = Arc::clone(recents);
    ui.on_menu_pick(move |choice| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let hit = MENU_HIT.with(|slot| slot.borrow_mut().take());
        let Some(hit) = hit else {
            return;
        };
        let outcome = platform::perform_item(ui.window(), choice as u32, &hit.title, &hit.target);
        let Some(outcome) = outcome else {
            ui.invoke_wake_input();
            return;
        };
        match outcome {
            platform::ItemMenu::Open => {
                let opened = match hit.kind {
                    Kind::Command => {
                        let folder = locate::foreground_folder();
                        commands::run(&hit.target, folder.as_deref())
                    }
                    Kind::File | Kind::Dir => platform::open_path(&hit.target),
                };
                if opened {
                    if matches!(hit.kind, Kind::File | Kind::Dir) {
                        recents_menu.remember(&hit.target);
                    }
                    hide_launcher(&ui);
                } else {
                    ui.set_status("没有打开".into());
                    ui.invoke_wake_input();
                }
            }
            platform::ItemMenu::Reveal => {
                if platform::reveal(&hit.target) {
                    hide_launcher(&ui);
                } else {
                    ui.set_status("没有定位到文件夹".into());
                    ui.invoke_wake_input();
                }
            }
            platform::ItemMenu::Status(text) => {
                ui.set_status(text.into());
                ui.invoke_wake_input();
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_begin_drag(move || {
        platform::release_anchor();
        if let Some(ui) = weak.upgrade() {
            platform::begin_drag(ui.window());
        }
    });
    let weak = ui.as_weak();
    ui.on_drag_move(move || {
        if let Some(ui) = weak.upgrade() {
            platform::drag_move(ui.window());
        }
    });
    ui.on_end_drag(move || {
        platform::end_drag();
    });

    let weak = ui.as_weak();
    ui.on_vanished(move || {
        if let Some(ui) = weak.upgrade() {
            finish_hide(&ui);
        }
    });

    let weak = ui.as_weak();
    ui.on_sized(move || {
        if let Some(ui) = weak.upgrade() {
            fit(&ui);
        }
    });

    let weak = ui.as_weak();
    ui.on_dismissed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if platform::save_dialog_in_use() {
            return;
        }
        hide_launcher(&ui);
    });

    let weak = ui.as_weak();
    let catalog_tick = Arc::clone(catalog);
    ui.on_tick(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        ui.set_index_done(catalog_tick.is_done());
    });

    let weak = ui.as_weak();
    ui.on_focus_check(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        platform::keep_save_bar();
        if !ui.window().is_visible() {
            return;
        }
        if ui.get_docked() {
            if ui.get_save_dialog() {
                if platform::save_dialog_in_use() {
                    follow_save_dialog(&ui);
                } else {
                    hide_launcher(&ui);
                }
                return;
            }
            let started = FOLDER.lock().unwrap_or_else(|err| err.into_inner()).clone();
            // Folder path lookup goes through the shell. Doing it while a save
            // dialog is being dragged stalls the bar behind the pointer.
            let left_folder = platform::pinned_folder_left(&started);
            if platform::pinned_window_gone()
                || left_folder
                || platform::clicked_outside(ui.window())
            {
                hide_launcher(&ui);
            }
            return;
        }
        if ui.get_capturing() {
            if platform::clicked_outside(ui.window()) {
                hide_launcher(&ui);
            }
            return;
        }
        if platform::explicit_search() && ui.get_composing().is_empty() && platform::clicked_outside(ui.window()) {
            hide_launcher(&ui);
            return;
        }
        if !ui.get_composing().is_empty() {
            return;
        }
        if platform::clicked_outside(ui.window()) {
            hide_launcher(&ui);
        }
    });

    let weak = ui.as_weak();
    ui.on_nudge_save(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        if !ui.window().is_visible() || !ui.get_save_dialog() || !platform::folder_synced() {
            return;
        }
        let (width, height) = window_size(&ui);
        let scale = ui.window().scale_factor();
        platform::nudge_save_bar(
            ui.window(),
            (width * scale) as i32,
            (height * scale) as i32,
        );
    });

    let weak = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak.upgrade() {
            hide_launcher(&ui);
        }
        slint::CloseRequestResponse::HideWindow
    });
}

fn present_theme(ui: &Launcher) {
    platform::set_launcher_visible(true);
    platform::extend_hold(1200);
    ui.set_theme(platform::theme_name().into());
    paint_summon(ui);
    if !ui.window().is_visible() {
        ui.show().ok();
    }
    ui.invoke_play_in();
    fit(ui);
    platform::focus_launcher(ui.window());
    ui.invoke_take_focus();
    keep_front(ui);
}

fn keep_front(ui: &Launcher) {
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(80), move || {
        if let Some(ui) = weak.upgrade() {
            platform::raise_launcher(ui.window());
            platform::prefer_chinese_input(ui.window());
        }
    });
}

fn paint_summon(ui: &Launcher) {
    let (doubled, keys) = platform::summon_parts();
    ui.set_summon_double(doubled);
    let keys = keys
        .into_iter()
        .map(slint::SharedString::from)
        .collect::<Vec<_>>();
    ui.set_summon_keys(Rc::new(VecModel::from(keys)).into());
}

fn activate(ui: &Launcher, index: i32, reveal: bool, recents: &search::Recents) {
    let chosen = UI_STATE.with(|state| {
        let state = state.borrow();
        let Some(state) = state.as_ref() else {
            return None;
        };
        let chosen = state.hits.borrow().get(index as usize).cloned();
        chosen
    });
    let Some(hit) = chosen else {
        return;
    };
    let opened = match hit.kind {
        Kind::Command => {
            let folder = locate::foreground_folder();
            commands::run(&hit.target, folder.as_deref())
        }
        Kind::File | Kind::Dir if reveal => platform::reveal(&hit.target),
        Kind::File | Kind::Dir => platform::open_path(&hit.target),
    };
    if opened {
        if matches!(hit.kind, Kind::File | Kind::Dir) && !reveal {
            recents.remember(&hit.target);
        }
        hide_launcher(ui);
    } else {
        ui.set_status("没有打开".into());
    }
}

fn show_folder_matches(
    ui: &Launcher,
    model: &VecModel<Row>,
    hits: &RefCell<Vec<Hit>>,
    catalog: &Catalog,
    query: &str,
) {
    if platform::in_save_dialog() {
        ui.set_save_dialog(true);
        let found = catalog.read(|index| search::folders(index, query, 80));
        FOLDER_PICKS.with(|slot| slot.borrow_mut().clear());
        apply_hits(ui, model, hits, found, catalog, query);
        return;
    }
    ui.set_save_dialog(false);
    let folder = FOLDER.lock().unwrap_or_else(|err| err.into_inner()).clone();
    let matches = locate::folder_matches(query);
    let mut picks = Vec::with_capacity(matches.len());
    let mut found = Vec::with_capacity(matches.len());
    for (index, name) in matches {
        let path = PathBuf::from(&folder).join(&name);
        let dir = path.is_dir();
        picks.push(index);
        found.push(Hit {
            kind: if dir { Kind::Dir } else { Kind::File },
            title: name,
            detail: if dir { String::new() } else { file_size_label(&path) },
            target: path.to_string_lossy().into_owned(),
        });
    }
    FOLDER_PICKS.with(|slot| *slot.borrow_mut() = picks);
    apply_hits(ui, model, hits, found, catalog, query);
    sync_pick(ui.get_selected());
}

fn file_size_label(path: &std::path::Path) -> String {
    let Ok(meta) = std::fs::metadata(path) else {
        return String::new();
    };
    format_bytes(meta.len())
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{} KB", (bytes + KB / 2) / KB)
    } else if bytes < GB {
        let tenths = (bytes * 10 + MB / 2) / MB;
        if tenths >= 100 {
            format!("{} MB", tenths / 10)
        } else {
            format!("{}.{} MB", tenths / 10, tenths % 10)
        }
    } else {
        let tenths = (bytes * 10 + GB / 2) / GB;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

fn sync_pick(index: i32) {
    let pick = FOLDER_PICKS.with(|picks| picks.borrow().get(index as usize).copied());
    if let Some(pick) = pick {
        locate::select_item(pick);
    }
}

fn show_folder_launcher(ui: &Launcher, catalog: &Catalog, model: &VecModel<Row>, hits: &RefCell<Vec<Hit>>) {
    platform::set_folder_synced(true);
    platform::extend_hold(900);
    ui.set_motion(false);
    ui.set_docked(true);
    ui.set_save_dialog(platform::in_save_dialog());
    let scale = ui.window().scale_factor();
    if ui.get_save_dialog() {
        if let Some(width) = platform::save_bar_width(scale) {
            ui.set_dock_w(width);
        }
    } else {
        ui.set_dock_w(572.0);
    }
    let folder = locate::foreground_folder()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    *FOLDER.lock().unwrap_or_else(|err| err.into_inner()) = folder;
    platform::set_launcher_visible(true);
    ui.set_theme(platform::theme_name().into());
    paint_summon(ui);
    apply_hits(ui, model, hits, Vec::new(), catalog, "");
    ui.set_motion(true);
    ui.invoke_reset();
    ui.show().ok();
    place_on_explorer(ui);
    let (width, height) = window_size(ui);
    if platform::explorer_slot((width * scale) as i32, (height * scale) as i32).is_none() {
        place(ui);
    }
    platform::apply_window_icon(ui.window());
    platform::prepare_launcher(ui.window());
    ui.invoke_arm_input();
    platform::prefer_chinese_input(ui.window());
    ui.invoke_play_in();
    keep_front(ui);
}

fn show_launcher(ui: &Launcher, catalog: &Catalog, model: &VecModel<Row>, hits: &RefCell<Vec<Hit>>) {
    platform::set_explicit_search(true);
    platform::extend_hold(900);
    platform::end_folder_session();
    ui.set_motion(false);
    ui.set_docked(false);
    ui.set_save_dialog(false);
    let folder = locate::foreground_folder()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    *FOLDER.lock().unwrap_or_else(|err| err.into_inner()) = folder;
    platform::set_launcher_visible(true);
    ui.set_theme(platform::theme_name().into());
    paint_summon(ui);
    ui.set_motion(false);
    apply_hits(ui, model, hits, Vec::new(), catalog, "");
    ui.set_motion(true);
    ui.invoke_reset();
    ui.show().ok();
    place(ui);
    platform::apply_window_icon(ui.window());
    platform::prepare_launcher(ui.window());
    ui.invoke_arm_input();
    platform::prefer_chinese_input(ui.window());
    ui.invoke_play_in();
    keep_front(ui);
}

fn hide_launcher(ui: &Launcher) {
    if ui.get_closing() {
        return;
    }
    platform::set_explicit_search(false);
    platform::end_drag();
    platform::end_folder_session();
    platform::set_launcher_visible(false);
    platform::cancel_capture();
    ui.set_capturing(false);
    ui.invoke_play_out();
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(170), move || {
        if let Some(ui) = weak.upgrade() {
            finish_hide(&ui);
        }
    });
}

fn finish_hide(ui: &Launcher) {
    if ui.get_closing() {
        ui.hide().ok();
    }
}

fn publish_rows(model: &VecModel<Row>, rows: &[Row]) {
    let have = model.row_count();
    let need = rows.len();
    let shared = have.min(need);
    for index in 0..shared {
        model.set_row_data(index, rows[index].clone());
    }
    if need > have {
        for row in rows.iter().skip(shared) {
            model.push(row.clone());
        }
    } else {
        for index in (need..have).rev() {
            model.remove(index);
        }
    }
}

fn apply_hits(
    ui: &Launcher,
    model: &VecModel<Row>,
    hits: &RefCell<Vec<Hit>>,
    found: Vec<Hit>,
    catalog: &Catalog,
    query: &str,
) {
    let rows = found
        .iter()
        .map(|hit| {
            let shown = search::shown_parts(&hit.title, &search::highlight_of(query));
            Row {
                kind: kind_name(hit).into(),
                lead: shown.lead.into(),
                hit: shown.hit.into(),
                tail: shown.tail.into(),
                detail: hit.detail.as_str().into(),
            }
        })
        .collect::<Vec<_>>();
    let unchanged = {
        let current = hits.borrow();
        current.len() == found.len()
            && current.iter().zip(found.iter()).all(|(old, new)| {
                old.kind == new.kind && old.title == new.title && old.detail == new.detail
            })
            && (0..rows.len()).all(|index| {
                model
                    .row_data(index)
                    .is_some_and(|row| {
                        row.lead == rows[index].lead
                            && row.hit == rows[index].hit
                            && row.tail == rows[index].tail
                    })
            })
    };
    if !unchanged {
        publish_rows(model, &rows);
        ui.set_selected(0);
        *hits.borrow_mut() = found;
    }
    let count = hits.borrow().len();
    let slots = if ui.get_docked() {
        let cap = if ui.get_save_dialog() {
            platform::save_slot_limit(ui.window().scale_factor())
        } else {
            9
        };
        count.min(cap as usize) as i32
    } else if count == 0 {
        0
    } else {
        count.min(8) as i32
    };
    if ui.get_slots() != slots {
        ui.set_slots(slots);
    }
    ui.set_index_done(catalog.is_done());
    fit(ui);
}

fn window_size(ui: &Launcher) -> (f32, f32) {
    (ui.get_win_w(), ui.get_win_h())
}

fn fit(ui: &Launcher) {
    let (width, height) = window_size(ui);
    let scale = ui.window().scale_factor();
    if platform::folder_synced() {
        let pw = (width * scale) as i32;
        let ph = (height * scale) as i32;
        if let Some((x, y)) = platform::explorer_slot(pw, ph) {
            // Size grows downward. The top edge stays on the dialog border.
            platform::move_window(ui.window(), x, y, pw, ph);
            let current = ui.window().size().to_logical(scale);
            if (current.width - width).abs() >= 0.5 || (current.height - height).abs() >= 0.5 {
                ui.window().set_size(slint::LogicalSize::new(width, height));
                platform::move_window(ui.window(), x, y, pw, ph);
            }
            return;
        }
    }
    let current = ui.window().size().to_logical(scale);
    if (current.width - width).abs() < 0.5 && (current.height - height).abs() < 0.5 {
        return;
    }
    ui.window().set_size(slint::LogicalSize::new(width, height));
}

fn follow_save_dialog(ui: &Launcher) {
    let scale = ui.window().scale_factor();
    if let Some(width) = platform::save_bar_width(scale) {
        if (ui.get_dock_w() - width).abs() >= 1.0 {
            ui.set_dock_w(width);
        }
    }
    let cap = platform::save_slot_limit(scale);
    let count = ui.get_rows().row_count() as i32;
    let slots = if count == 0 { 0 } else { count.min(cap) };
    if ui.get_slots() != slots {
        ui.set_motion(false);
        ui.set_slots(slots);
        ui.invoke_reveal_row();
        ui.set_motion(true);
    }
    fit(ui);
}

fn place_on_explorer(ui: &Launcher) {
    let (width, height) = window_size(ui);
    let scale = ui.window().scale_factor();
    let Some((x, y)) = platform::explorer_slot((width * scale) as i32, (height * scale) as i32) else {
        return;
    };
    let current = ui.window().position();
    if current.x != x || current.y != y {
        ui.window().set_position(slint::PhysicalPosition::new(x, y));
    }
}

fn place(ui: &Launcher) {
    let (width, height) = window_size(ui);
    let scale = ui.window().scale_factor();
    let (x, y) = platform::centered((width * scale) as i32, (height * scale) as i32);
    ui.window().set_position(slint::PhysicalPosition::new(x, y));
}

fn kind_name(hit: &Hit) -> &'static str {
    match hit.kind {
        Kind::Command => "command",
        Kind::Dir => "dir",
        Kind::File => file_type(&hit.title),
    }
}

fn file_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    if ext.eq_ignore_ascii_case("zip")
        || ext.eq_ignore_ascii_case("rar")
        || ext.eq_ignore_ascii_case("7z")
        || ext.eq_ignore_ascii_case("tar")
        || ext.eq_ignore_ascii_case("gz")
    {
        "zip"
    } else if ext.eq_ignore_ascii_case("png")
        || ext.eq_ignore_ascii_case("jpg")
        || ext.eq_ignore_ascii_case("jpeg")
        || ext.eq_ignore_ascii_case("gif")
        || ext.eq_ignore_ascii_case("webp")
        || ext.eq_ignore_ascii_case("svg")
        || ext.eq_ignore_ascii_case("bmp")
        || ext.eq_ignore_ascii_case("ico")
    {
        "image"
    } else if ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm") {
        "html"
    } else if ext.eq_ignore_ascii_case("sh")
        || ext.eq_ignore_ascii_case("bat")
        || ext.eq_ignore_ascii_case("cmd")
        || ext.eq_ignore_ascii_case("ps1")
        || ext.eq_ignore_ascii_case("js")
        || ext.eq_ignore_ascii_case("ts")
        || ext.eq_ignore_ascii_case("py")
        || ext.eq_ignore_ascii_case("rs")
        || ext.eq_ignore_ascii_case("css")
    {
        "code"
    } else if ext.eq_ignore_ascii_case("doc")
        || ext.eq_ignore_ascii_case("docx")
        || ext.eq_ignore_ascii_case("txt")
        || ext.eq_ignore_ascii_case("md")
        || ext.eq_ignore_ascii_case("rtf")
    {
        "doc"
    } else if ext.eq_ignore_ascii_case("xls")
        || ext.eq_ignore_ascii_case("xlsx")
        || ext.eq_ignore_ascii_case("csv")
    {
        "sheet"
    } else if ext.eq_ignore_ascii_case("pdf") {
        "pdf"
    } else {
        "file"
    }
}
