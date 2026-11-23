// Hide the console window: this is a GUI application.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod engine;
mod ui_strings;

use eframe::egui::{self, Color32};
use engine::{CrashEntry, EntryStatus, ProcessConfig, RunningProcess, StatusSink, Watcher};
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use ui_strings::{Lang, Settings, Theme, strings};

// ---- Design tokens (Minimalism & Swiss Style) ----
const GREEN: Color32 = Color32::from_rgb(38, 166, 91);
const RED: Color32 = Color32::from_rgb(220, 70, 70);
const AMBER: Color32 = Color32::from_rgb(190, 130, 20);
const BLUE: Color32 = Color32::from_rgb(54, 132, 232);
const MUTED: Color32 = Color32::from_rgb(120, 126, 136);
const CARD_LIGHT: Color32 = Color32::from_rgb(246, 248, 250);
const CARD_DARK: Color32 = Color32::from_rgb(38, 42, 48);

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}
fn config_path() -> PathBuf { exe_dir().join("guardian.toml") }
fn crash_path() -> PathBuf { exe_dir().join("crash-history.jsonl") }
fn log_path() -> PathBuf { exe_dir().join("guardian.log") }
fn settings_path() -> PathBuf { exe_dir().join("settings.json") }

fn load_settings() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
        .unwrap_or_default()
}

fn save_settings(s: &Settings) {
    let _ = std::fs::write(settings_path(), serde_json::to_string_pretty(s).unwrap_or_default());
}

/// Install a Chinese-capable system font into egui so CJK is not rendered as tofu.
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
        r"C:\Windows\Fonts\Deng.ttf",
    ];
    let found = candidates.iter().find_map(|p| std::fs::read(p).ok().map(|b| (p, b)));
    let Some((path, data)) = found else { return };
    let mut fonts = egui::FontDefinitions::default();
    let name = format!("CJK-{}", PathBuf::from(path).file_name().unwrap_or_default().to_string_lossy());
    fonts.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(data)));
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) { list.insert(0, name.clone()); }
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) { list.insert(0, name); }
    ctx.set_fonts(fonts);
}

fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let mut visuals = match theme {
        Theme::Light => egui::Visuals::light(),
        Theme::Dark => egui::Visuals::dark(),
    };
    let accent = match theme {
        Theme::Light => Color32::from_rgb(30, 100, 200),
        Theme::Dark => Color32::from_rgb(90, 160, 250),
    };
    visuals.selection.bg_fill = accent;
    visuals.hyperlink_color = accent;
    ctx.set_visuals(visuals);
}

#[derive(Default)]
struct AddDialog {
    open: bool,
    tab: usize,
    name: String,
    command: String,
    args: String,
    processes: Vec<RunningProcess>,
    processes_loaded: bool,
    search: String,
    selected: Option<usize>,
}

struct TrayState {
    _icon: TrayIcon,
    show: MenuItem,
    autostart: MenuItem,
    quit: MenuItem,
}

struct GuardianApp {
    configs: Vec<ProcessConfig>,
    statuses: Vec<EntryStatus>,
    crashes: Vec<CrashEntry>,
    sink: StatusSink,
    selected: Option<usize>,
    add: AddDialog,
    watcher_stop: Option<Arc<AtomicBool>>,
    pause_handle: Option<Arc<AtomicBool>>,
    paused: bool,
    autostart: bool,
    toast: Option<String>,
    last_crash_refresh: std::time::Instant,
    tray: Option<TrayState>,
    lang: Lang,
    theme: Theme,
    font_installed: bool,
    about_open: bool,
    search: String,
}

impl GuardianApp {
    fn new(tray: Option<TrayState>) -> Self {
        let st = load_settings();
        let mut app = Self {
            configs: Vec::new(), statuses: Vec::new(), crashes: Vec::new(),
            sink: Arc::new(Mutex::new(Vec::new())), selected: None, add: AddDialog::default(),
            watcher_stop: None, pause_handle: None, paused: false,
            autostart: engine::task_installed(), toast: None,
            last_crash_refresh: std::time::Instant::now(), tray,
            lang: st.lang, theme: st.theme, font_installed: false,
            about_open: false, search: String::new(),
        };
        app.load();
        app.spawn_watcher();
        app
    }

    fn load(&mut self) {
        let path = config_path();
        if path.exists() {
            match engine::load_config(&path) {
                Ok(config) => self.configs = config.process,
                Err(e) => self.toast = Some(strings(self.lang).toast_load_fail.replace("{}", &e.to_string())),
            }
        }
        self.crashes = engine::read_crash_history(&crash_path());
    }

    fn persist(&mut self) {
        let config = engine::Config { process: self.configs.clone() };
        if let Err(e) = engine::save_config(&config_path(), &config) {
            self.toast = Some(strings(self.lang).toast_save_fail.replace("{}", &e.to_string()));
        }
    }

    fn persist_settings(&mut self) {
        save_settings(&Settings { lang: self.lang, theme: self.theme });
    }

    fn spawn_watcher(&mut self) {
        if let Some(stop) = &self.watcher_stop { stop.store(true, Ordering::SeqCst); }
        let _ = engine::init_logs(&log_path(), &crash_path());
        let config = engine::Config { process: self.configs.clone() };
        let mut watcher = Watcher::new(&config, self.sink.clone());
        self.watcher_stop = Some(watcher.stop_handle());
        self.pause_handle = Some(watcher.pause_handle());
        if self.paused {
            if let Some(h) = &self.pause_handle { h.store(true, Ordering::SeqCst); }
        }
        std::thread::spawn(move || watcher.run());
    }

    fn add_from_file(&mut self) {
        let name = if self.add.name.trim().is_empty() {
            PathBuf::from(&self.add.command).file_stem()
                .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "app".into())
        } else { self.add.name.trim().to_string() };
        let mut cfg = ProcessConfig::new(name, self.add.command.trim().to_string());
        if !self.add.args.trim().is_empty() {
            cfg.args = self.add.args.split_whitespace().map(|s| s.to_string()).collect();
        }
        let st = strings(self.lang);
        if let Err(e) = cfg.validate() {
            self.toast = Some(st.toast_add_fail.replace("{}", &e.to_string())); return;
        }
        self.configs.push(cfg);
        self.add.open = false;
        self.add.name.clear(); self.add.command.clear(); self.add.args.clear();
        self.persist(); self.spawn_watcher();
        self.toast = Some(st.toast_added.to_string());
    }

    fn add_from_process(&mut self) {
        let Some(idx) = self.add.selected else {
            self.toast = Some(strings(self.lang).toast_select_first.to_string()); return;
        };
        let Some(proc) = self.add.processes.get(idx).cloned() else { return };
        let name = PathBuf::from(&proc.name).file_stem()
            .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| proc.name.clone());
        let command = proc.path.clone().unwrap_or_else(|| proc.name.clone());
        let mut cfg = ProcessConfig::new(name, command);
        cfg.adopt = Some(proc.name);
        let st = strings(self.lang);
        if let Err(e) = cfg.validate() {
            self.toast = Some(st.toast_add_fail.replace("{}", &e.to_string())); return;
        }
        self.configs.push(cfg);
        self.add.open = false; self.add.selected = None;
        self.persist(); self.spawn_watcher();
        self.toast = Some(st.toast_adopted.to_string());
    }

    fn remove_selected(&mut self) {
        if let Some(idx) = self.selected {
            if idx < self.configs.len() { self.configs.remove(idx); }
            self.selected = None;
            self.persist(); self.spawn_watcher();
            self.toast = Some(strings(self.lang).toast_removed.to_string());
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if let Some(h) = &self.pause_handle { h.store(self.paused, Ordering::SeqCst); }
        self.toast = Some(if self.paused {
            strings(self.lang).toast_paused.to_string()
        } else {
            strings(self.lang).toast_resumed.to_string()
        });
    }

    fn restart_all(&mut self) {
        self.spawn_watcher();
        self.toast = Some(strings(self.lang).toast_restarted.to_string());
    }

    fn toggle_autostart(&mut self) {
        let result = if self.autostart { engine::uninstall_task() } else {
            engine::install_task(&std::env::current_exe().unwrap_or_default(), &config_path())
        };
        let st = strings(self.lang);
        match result {
            Ok(()) => {
                self.autostart = engine::task_installed();
                self.toast = Some(if self.autostart { st.toast_auto_on.to_string() } else { st.toast_auto_off.to_string() });
            }
            Err(e) => self.toast = Some(st.toast_auto_fail.replace("{}", &e.to_string())),
        }
    }

    fn clear_history(&mut self) {
        let _ = engine::clear_crash_history(&crash_path());
        self.crashes.clear();
        self.toast = Some(strings(self.lang).toast_history_cleared.to_string());
    }

    fn open_folder(&mut self) {
        let _ = std::process::Command::new("explorer.exe").arg(exe_dir()).spawn();
    }

    fn open_log(&mut self) {
        let _ = std::process::Command::new("notepad.exe").arg(log_path()).spawn();
    }

    fn set_lang(&mut self, lang: Lang, ctx: &egui::Context) {
        self.lang = lang;
        self.font_installed = false;
        self.persist_settings();
        ctx.request_repaint();
        self.update_tray_labels();
    }

    fn set_theme(&mut self, theme: Theme, ctx: &egui::Context) {
        self.theme = theme;
        apply_theme(ctx, theme);
        self.persist_settings();
    }

    fn update_tray_labels(&mut self) {
        let s = strings(self.lang);
        if let Some(tray) = &self.tray {
            tray.show.set_text(s.tray_show);
            tray.autostart.set_text(if self.autostart { s.tray_autostart_on } else { s.tray_autostart_off });
            tray.quit.set_text(s.tray_quit);
        }
    }
}

impl eframe::App for GuardianApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.font_installed { install_cjk_font(ctx); self.font_installed = true; }

        // Tray events
        let tray_ids = self.tray.as_ref().map(|t| (t.show.id().clone(), t.autostart.id().clone(), t.quit.id().clone()));
        if let (Some((show_id, auto_id, quit_id)), Ok(event)) = (tray_ids, MenuEvent::receiver().try_recv()) {
            if event.id == show_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if event.id == auto_id { self.toggle_autostart(); self.update_tray_labels(); }
            else if event.id == quit_id { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
        }

        if let Ok(s) = self.sink.lock() { self.statuses = s.clone(); }
        if self.last_crash_refresh.elapsed() >= std::time::Duration::from_millis(500) {
            self.crashes = engine::read_crash_history(&crash_path());
            self.last_crash_refresh = std::time::Instant::now();
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        let s = strings(self.lang);

        // ---- Menu bar ----
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button(s.menu_file, |ui| {
                    if ui.button(s.menu_add).clicked() { self.add.open = true; self.add.processes_loaded = false; ui.close_menu(); }
                    if ui.button(s.menu_remove).clicked() { self.remove_selected(); ui.close_menu(); }
                    ui.separator();
                    if ui.button(s.menu_open_folder).clicked() { self.open_folder(); ui.close_menu(); }
                    if ui.button(s.menu_open_log).clicked() { self.open_log(); ui.close_menu(); }
                    ui.separator();
                    if ui.button(s.menu_minimize_tray).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false)); ui.close_menu();
                    }
                    if ui.button(s.menu_quit).clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                });
                ui.menu_button(s.menu_guard, |ui| {
                    let label = if self.paused { s.menu_resume } else { s.menu_pause };
                    if ui.button(label).clicked() { self.toggle_pause(); ui.close_menu(); }
                    if ui.button(s.menu_restart_all).clicked() { self.restart_all(); ui.close_menu(); }
                    ui.separator();
                    let mark = if self.autostart { "✓ " } else { "   " };
                    if ui.button(format!("{}{}", mark, s.menu_autostart)).clicked() {
                        self.toggle_autostart();
                        ui.close_menu();
                    }
                    if ui.button(s.menu_clear_history).clicked() { self.clear_history(); ui.close_menu(); }
                });
                ui.menu_button(s.menu_view, |ui| {
                    ui.menu_button(s.language_label, |ui| {
                        if ui.button(s.menu_lang_zh).clicked() { self.set_lang(Lang::Zh, ctx); ui.close_menu(); }
                        if ui.button(s.menu_lang_en).clicked() { self.set_lang(Lang::En, ctx); ui.close_menu(); }
                    });
                    ui.menu_button("主题 / Theme", |ui| {
                        if ui.button(s.menu_theme_light).clicked() { self.set_theme(Theme::Light, ctx); ui.close_menu(); }
                        if ui.button(s.menu_theme_dark).clicked() { self.set_theme(Theme::Dark, ctx); ui.close_menu(); }
                    });
                    ui.separator();
                    if ui.button(s.menu_minimize_tray).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false)); ui.close_menu();
                    }
                });
                ui.menu_button(s.menu_help, |ui| {
                    if ui.button(s.menu_about).clicked() { self.about_open = true; ui.close_menu(); }
                });
            });
        });

        // ---- Status bar ----
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s.status_bar).size(11.0).color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.paused {
                        ui.label(egui::RichText::new(s.paused).color(AMBER).strong());
                    } else {
                        ui.label(egui::RichText::new(s.running).color(GREEN).strong());
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // ---- Header + stat cards ----
            ui.horizontal(|ui| {
                ui.heading(s.title);
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(s.language_label);
                    if ui.selectable_label(self.lang == Lang::Zh, s.menu_lang_zh).clicked() { self.set_lang(Lang::Zh, ctx); }
                    if ui.selectable_label(self.lang == Lang::En, s.menu_lang_en).clicked() { self.set_lang(Lang::En, ctx); }
                });
            });
            ui.add_space(8.0);

            let running = self.statuses.iter().filter(|x| x.running).count();
            let total_restarts: i64 = self.statuses.iter().map(|x| x.restarts).sum();
            ui.horizontal(|ui| {
                stat_card(ui, s.guarding, &running.to_string(), GREEN, self.theme);
                stat_card(ui, s.total, &self.configs.len().to_string(), BLUE, self.theme);
                stat_card(ui, s.restarts_total, &total_restarts.to_string(), AMBER, self.theme);
                stat_card(ui, s.crash_records, &self.crashes.len().to_string(), RED, self.theme);
            });
            ui.add_space(10.0);

            // ---- Toolbar ----
            ui.horizontal(|ui| {
                if ui.button(s.add_button).clicked() { self.add.open = true; self.add.processes_loaded = false; }
                if ui.button(s.remove_button).clicked() { self.remove_selected(); }
                let pause_label = if self.paused { s.resume_button } else { s.pause_button };
                if ui.button(pause_label).clicked() { self.toggle_pause(); }
                if ui.button(s.menu_restart_all).clicked() { self.restart_all(); }
                let auto_label = if self.autostart { s.autostart_on } else { s.autostart_off };
                if ui.button(auto_label).clicked() { self.toggle_autostart(); }
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(s.search_hint).desired_width(150.0));
            });

            if let Some(toast) = &self.toast {
                ui.add_space(4.0);
                ui.colored_label(AMBER, toast);
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            let filter = self.search.to_lowercase();
            let visible: Vec<usize> = (0..self.configs.len()).filter(|&i| {
                filter.is_empty() || self.configs[i].name.to_lowercase().contains(&filter)
            }).collect();

            if self.configs.is_empty() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(s.empty_title).size(18.0).color(MUTED));
                    ui.add_space(8.0);
                    ui.label(s.empty_line1);
                    ui.label(s.empty_line2);
                });
            } else if visible.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| { ui.label(egui::RichText::new(s.no_crashes).color(MUTED)); });
            } else {
                egui::Grid::new("entries").num_columns(5).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.strong(s.col_name); ui.strong(s.col_status); ui.strong(s.col_pid);
                    ui.strong(s.col_restarts); ui.strong(s.col_last_exit); ui.end_row();
                    for idx in visible {
                        let cfg = &self.configs[idx];
                        let status = self.statuses.get(idx);
                        if ui.selectable_label(self.selected == Some(idx), &cfg.name).clicked() { self.selected = Some(idx); }
                        let running = status.map(|x| x.running).unwrap_or(false);
                        ui.colored_label(if running { GREEN } else { RED },
                            if running { s.running } else { s.stopped });
                        ui.label(status.and_then(|x| x.pid).map(|p| p.to_string()).unwrap_or_else(|| "-".into()));
                        ui.label(status.map(|x| x.restarts.to_string()).unwrap_or_else(|| "0".into()));
                        ui.label(status.and_then(|x| x.last_exit.clone()).unwrap_or_else(|| "-".into()));
                        ui.end_row();
                    }
                });

                ui.add_space(8.0);
                ui.collapsing(s.crash_history, |ui| {
                    ui.label(format!("{} {} {}", self.crashes.len(), s.total, s.crash_count));
                    for c in self.crashes.iter().rev().take(20) {
                        ui.label(s.crash_line
                            .replace("{t}", &c.time).replace("{p}", &c.process)
                            .replace("{pid}", &c.pid.to_string()).replace("{code}", &c.exit_code)
                            .replace("{up}", &c.uptime_secs.to_string()).replace("{n}", &c.restart_number.to_string()));
                    }
                });
            }
        });

        // ---- Add dialog ----
        if self.add.open {
            let mut open = self.add.open;
            egui::Window::new(s.add_window_title).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.add.tab, 0, s.tab_file);
                    ui.selectable_value(&mut self.add.tab, 1, s.tab_process);
                });
                ui.separator();
                if self.add.tab == 0 {
                    ui.horizontal(|ui| {
                        ui.label(s.label_program);
                        ui.text_edit_singleline(&mut self.add.command);
                        if ui.button(s.browse).clicked() {
                            if let Some(path) = FileDialog::new().pick_file() {
                                self.add.command = path.to_string_lossy().into_owned();
                                if self.add.name.is_empty() {
                                    self.add.name = path.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                                }
                            }
                        }
                    });
                    ui.horizontal(|ui| { ui.label(s.label_name); ui.text_edit_singleline(&mut self.add.name); });
                    ui.horizontal(|ui| { ui.label(s.label_args); ui.text_edit_singleline(&mut self.add.args); });
                    ui.add_space(6.0);
                    if ui.button(s.add_and_guard).clicked() { self.add_from_file(); }
                } else {
                    if !self.add.processes_loaded { self.add.processes = engine::list_running_processes(); self.add.processes_loaded = true; }
                    ui.horizontal(|ui| {
                        ui.label(s.label_filter); ui.text_edit_singleline(&mut self.add.search);
                        if ui.button(s.refresh).clicked() { self.add.processes = engine::list_running_processes(); }
                    });
                    let filtered: Vec<(usize, &RunningProcess)> = self.add.processes.iter().enumerate()
                        .filter(|(_, p)| self.add.search.is_empty() || p.name.to_lowercase().contains(&self.add.search.to_lowercase()))
                        .collect();
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for (idx, proc) in filtered {
                            if ui.selectable_label(self.add.selected == Some(idx), format!("{}  (PID {})", proc.name, proc.pid)).clicked() {
                                self.add.selected = Some(idx);
                            }
                        }
                    });
                    if ui.button(s.add_adopt).clicked() { self.add_from_process(); }
                }
            });
            self.add.open = open;
        }

        // ---- About dialog ----
        if self.about_open {
            let mut open = self.about_open;
            egui::Window::new(s.about_title).collapsible(false).resizable(false).open(&mut open).show(ctx, |ui| {
                ui.label(s.about_body);
                ui.add_space(8.0);
                if ui.button(s.close).clicked() { self.about_open = false; }
            });
            self.about_open = open;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        engine::SHUTDOWN.store(true, Ordering::SeqCst);
    }
}

fn stat_card(ui: &mut egui::Ui, label: &str, value: &str, color: Color32, theme: Theme) {
    let bg = match theme { Theme::Light => CARD_LIGHT, Theme::Dark => CARD_DARK };
    egui::Frame::NONE.fill(bg).corner_radius(6.0).inner_margin(egui::vec2(10.0, 6.0)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(label).size(11.0).color(MUTED));
            ui.label(egui::RichText::new(value).size(20.0).strong().color(color));
        });
    });
}

fn make_tray_icon() -> tray_icon::Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size { for x in 0..size {
        let edge = x < 2 || y < 2 || x >= size - 2 || y >= size - 2;
        let (r, g, b) = if edge { (40, 44, 52) } else { (54, 132, 232) };
        rgba.extend_from_slice(&[r, g, b, 255]);
    }}
    tray_icon::Icon::from_rgba(rgba, size, size).expect("tray icon")
}

fn build_tray(lang: Lang) -> Option<TrayState> {
    let s = strings(lang);
    let menu = Menu::new();
    let show = MenuItem::new(s.tray_show, true, None);
    let auto_label = if engine::task_installed() { s.tray_autostart_on } else { s.tray_autostart_off };
    let autostart = MenuItem::new(auto_label, true, None);
    let quit = MenuItem::new(s.tray_quit, true, None);
    menu.append(&show).ok()?; menu.append(&autostart).ok()?; menu.append(&quit).ok()?;
    let icon = TrayIconBuilder::new().with_tooltip(s.app_title).with_icon(make_tray_icon())
        .with_menu(Box::new(menu)).build().ok()?;
    Some(TrayState { _icon: icon, show, autostart, quit })
}

fn main() -> eframe::Result<()> {
    let st = load_settings();
    let title = strings(st.lang).app_title;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([880.0, 600.0]).with_title(title),
        ..Default::default()
    };
    eframe::run_native("Guardian", options, Box::new(move |cc| {
        // Apply saved theme + CJK font before first frame.
        install_cjk_font(&cc.egui_ctx);
        apply_theme(&cc.egui_ctx, st.theme);
        let tray = build_tray(st.lang);
        let mut app = GuardianApp::new(tray);
        app.font_installed = true;
        Ok(Box::new(app))
    }))
}
