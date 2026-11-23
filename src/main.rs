// Hide the console window: this is a GUI application.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod engine;
mod ui;
mod ui_strings;

use eframe::egui;
use engine::{CrashEntry, EntryStatus, ProcessConfig, RunningProcess, StatusSink, Watcher};
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use ui_strings::{Lang, Settings, Theme, strings};

/// A close request minimizes to tray only for the window's own X button.
/// Explicit Quit sets `quitting`, so the request is left uncancelled and eframe exits.
fn should_minimize_to_tray(close_requested: bool, quitting: bool) -> bool {
    close_requested && !quitting
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View { Overview, Processes, History, Settings }

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
    quitting: bool,
    view: View,
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
            about_open: false, search: String::new(), quitting: false, view: View::Overview,
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

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
        // Open with the system's default text viewer via the shell, no console window.
        let _ = std::process::Command::new("rundll32.exe")
            .arg("shell32.dll,ShellExec_RunDLL")
            .arg(log_path())
            .spawn();
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
        ui::apply_visuals(ctx, theme);
        self.persist_settings();
    }

    /// Processes view: the full guarded-process table, no charts.
    fn draw_processes(&mut self, ui: &mut egui::Ui) {
        let s = strings(self.lang);
        let p = ui::palette(self.theme);
        ui.heading(egui::RichText::new(s.nav_processes).size(24.0).color(p.ink));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(s.add_button).clicked() { self.add.open = true; self.add.processes_loaded = false; }
            if ui.button(s.remove_button).clicked() { self.remove_selected(); }
            let label = if self.paused { s.resume_button } else { s.pause_button };
            if ui.button(label).clicked() { self.toggle_pause(); }
            if ui.button(s.menu_restart_all).clicked() { self.restart_all(); }
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(s.search_hint).desired_width(160.0));
        });
        if let Some(toast) = &self.toast {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(toast).color(p.amber));
        }
        ui.add_space(8.0);
        let filter = self.search.to_lowercase();
        let visible: Vec<usize> = (0..self.configs.len())
            .filter(|&i| filter.is_empty() || self.configs[i].name.to_lowercase().contains(&filter))
            .collect();
        egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
            .corner_radius(6.0).inner_margin(egui::vec2(12.0, 8.0)).show(ui, |ui| {
            egui::Grid::new("processes").num_columns(5).striped(false).spacing([16.0, 8.0]).show(ui, |ui| {
                for h in [s.col_name, s.col_status, s.col_pid, s.col_restarts, s.col_last_exit] {
                    ui.label(egui::RichText::new(h).size(11.0).color(p.muted));
                }
                ui.end_row();
                for idx in visible {
                    let cfg = &self.configs[idx];
                    let status = self.statuses.get(idx);
                    if ui.selectable_label(self.selected == Some(idx), &cfg.name).clicked() { self.selected = Some(idx); }
                    let is_running = status.map(|x| x.running).unwrap_or(false);
                    let (label, color) = if self.paused { (s.paused, p.amber) }
                        else if is_running { (s.running, p.green) }
                        else { (s.stopped, p.red) };
                    ui.horizontal(|ui| {
                        ui.painter().circle_filled(ui.cursor().min + egui::vec2(3.0, 8.0), 3.0, color);
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(label).size(12.0).color(color));
                    });
                    ui.label(status.and_then(|x| x.pid).map(|v| v.to_string()).unwrap_or_else(|| "-".into()));
                    ui.label(status.map(|x| x.restarts.to_string()).unwrap_or_else(|| "0".into()));
                    ui.label(status.and_then(|x| x.last_exit.clone()).unwrap_or_else(|| "-".into()));
                    ui.end_row();
                }
            });
        });
    }

    /// History view: the full crash log, newest first.
    fn draw_history(&mut self, ui: &mut egui::Ui) {
        let s = strings(self.lang);
        let p = ui::palette(self.theme);
        ui.heading(egui::RichText::new(s.nav_history).size(24.0).color(p.ink));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} {}", self.crashes.len(), s.crash_count)).size(12.0).color(p.muted));
            if ui.button(s.menu_clear_history).clicked() { self.clear_history(); }
        });
        ui.add_space(8.0);
        egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
            .corner_radius(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
            if self.crashes.is_empty() {
                ui.label(egui::RichText::new(s.history_empty).size(12.0).color(p.muted));
            } else {
                for c in self.crashes.iter().rev() {
                    ui.label(egui::RichText::new(s.crash_line
                        .replace("{t}", &c.time).replace("{p}", &c.process)
                        .replace("{pid}", &c.pid.to_string()).replace("{code}", &c.exit_code)
                        .replace("{up}", &c.uptime_secs.to_string()).replace("{n}", &c.restart_number.to_string())
                    ).size(12.0).color(p.ink));
                }
            }
        });
    }

    /// Settings view: language, theme and autostart, each self-saving.
    fn draw_settings(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let s = strings(self.lang);
        let p = ui::palette(self.theme);
        ui.heading(egui::RichText::new(s.settings_title).size(24.0).color(p.ink));
        ui.label(egui::RichText::new(s.settings_hint).size(11.0).color(p.muted));
        ui.add_space(12.0);
        egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
            .corner_radius(6.0).inner_margin(egui::vec2(16.0, 14.0)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s.settings_language).size(13.0).strong().color(p.ink));
                ui.add_space(12.0);
                if ui.selectable_label(self.lang == Lang::Zh, s.menu_lang_zh).clicked() { self.set_lang(Lang::Zh, ctx); }
                if ui.selectable_label(self.lang == Lang::En, s.menu_lang_en).clicked() { self.set_lang(Lang::En, ctx); }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s.settings_theme).size(13.0).strong().color(p.ink));
                ui.add_space(12.0);
                if ui.selectable_label(self.theme == Theme::Light, s.menu_theme_light).clicked() { self.set_theme(Theme::Light, ctx); }
                if ui.selectable_label(self.theme == Theme::Dark, s.menu_theme_dark).clicked() { self.set_theme(Theme::Dark, ctx); }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(s.settings_autostart).size(13.0).strong().color(p.ink));
                ui.add_space(12.0);
                let label = if self.autostart { s.autostart_on } else { s.autostart_off };
                if ui.button(label).clicked() { self.toggle_autostart(); }
            });
        });
    }

    fn update_tray_labels(&mut self) {
        let s = strings(self.lang);
        if let Some(tray) = &self.tray {
            tray.show.set_text(s.tray_show);
            tray.autostart.set_text(if self.autostart { s.tray_autostart_on } else { s.tray_autostart_off });
            tray.quit.set_text(s.tray_quit);
        }
    }

    /// Restart counts per hour bucket over the last 12 hours, from real crash history.
    /// Crash times are "YYYY-MM-DD HH:MM:SS" in local time.
    fn trend_data(&self) -> (Vec<f32>, Vec<String>) {
        use chrono::{Local, NaiveDateTime};
        let now = Local::now().naive_local();
        let mut counts = vec![0.0f32; 12];
        for c in &self.crashes {
            if let Ok(t) = NaiveDateTime::parse_from_str(&c.time, "%Y-%m-%d %H:%M:%S") {
                let hours_ago = (now - t).num_hours();
                if hours_ago >= 0 && hours_ago < 12 {
                    counts[11 - hours_ago as usize] += 1.0;
                }
            }
        }
        let labels: Vec<String> = (0..12)
            .map(|i| (now - chrono::Duration::hours((11 - i) as i64)).format("%H:%M").to_string())
            .collect();
        (counts, labels)
    }

    /// Restart events in the last hour, grouped by process name (minutes ago).
    fn pulse_data(&self) -> Vec<(String, Vec<f32>)> {
        use chrono::{Local, NaiveDateTime};
        let now = Local::now().naive_local();
        let mut lanes: Vec<(String, Vec<f32>)> = self.configs.iter()
            .map(|c| (c.name.clone(), Vec::new())).collect();
        for crash in &self.crashes {
            let Ok(t) = NaiveDateTime::parse_from_str(&crash.time, "%Y-%m-%d %H:%M:%S") else { continue };
            let mins = (now - t).num_seconds() as f32 / 60.0;
            if mins < 0.0 || mins > 60.0 { continue; }
            if let Some(lane) = lanes.iter_mut().find(|(name, _)| *name == crash.process) {
                lane.1.push(mins);
            }
        }
        lanes
    }

    /// Fraction of the last 24h without a crash: online / total. No crashes => 100%.
    fn health_ratio(&self) -> f32 {
        use chrono::{Local, NaiveDateTime};
        let now = Local::now().naive_local();
        let in_window = self.crashes.iter().filter_map(|c| {
            NaiveDateTime::parse_from_str(&c.time, "%Y-%m-%d %H:%M:%S").ok()
        }).filter(|t| (now - *t).num_hours() < 24).count();
        // ponytail: one crash ≈ 2 minutes of downtime; no uptime log exists to be more precise.
        let downtime_secs = in_window as f32 * 120.0;
        (86400.0 - downtime_secs.min(86400.0)) / 86400.0
    }
}

impl eframe::App for GuardianApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.font_installed { install_cjk_font(ctx); self.font_installed = true; }
        // eframe may re-apply system visuals; keep chrome in sync with our palette every frame.
        ui::apply_visuals(ctx, self.theme);

        // Tray events
        let tray_ids = self.tray.as_ref().map(|t| (t.show.id().clone(), t.autostart.id().clone(), t.quit.id().clone()));
        if let (Some((show_id, auto_id, quit_id)), Ok(event)) = (tray_ids, MenuEvent::receiver().try_recv()) {
            if event.id == show_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if event.id == auto_id { self.toggle_autostart(); self.update_tray_labels(); }
            else if event.id == quit_id { self.quit(ctx); }
        }

        if let Ok(s) = self.sink.lock() { self.statuses = s.clone(); }
        if self.last_crash_refresh.elapsed() >= std::time::Duration::from_millis(500) {
            self.crashes = engine::read_crash_history(&crash_path());
            self.last_crash_refresh = std::time::Instant::now();
        }
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if should_minimize_to_tray(close_requested, self.quitting) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        let s = strings(self.lang);
        let p = ui::palette(self.theme);

        // ---- Icon rail: switch between the four views ----
        egui::SidePanel::left("rail").exact_width(48.0).resizable(false).show(ctx, |ui| {
            let mut rail_btn = |ui: &mut egui::Ui, glyph: &str, tip: &str, view: View| {
                let active = self.view == view;
                let btn = ui.add_sized([40.0, 38.0], egui::Button::new(
                    egui::RichText::new(glyph).size(17.0).color(if active { p.ink } else { p.muted }))
                    .frame(false));
                if btn.clicked() { self.view = view; }
                let r = btn.rect;
                btn.on_hover_text(tip);
                if active {
                    ui.painter().line_segment(
                        [egui::pos2(r.left(), r.top() + 8.0), egui::pos2(r.left(), r.bottom() - 8.0)],
                        egui::Stroke::new(2.0_f32, p.accent));
                }
            };
            ui.add_space(6.0);
            rail_btn(ui, "▦", s.nav_overview, View::Overview);
            rail_btn(ui, "≣", s.nav_processes, View::Processes);
            rail_btn(ui, "◷", s.nav_history, View::History);
            ui.add_space(6.0);
            ui.separator();
            rail_btn(ui, "⚙", s.nav_settings, View::Settings);
        });

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
                    if ui.button(s.menu_quit).clicked() { self.quit(ctx); }
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
                    ui.menu_button(s.menu_theme, |ui| {
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
                ui.label(egui::RichText::new(s.status_bar).size(11.0).color(p.muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.paused {
                        ui.label(egui::RichText::new(s.paused).color(p.amber).strong());
                    } else {
                        ui.label(egui::RichText::new(s.running).color(p.green).strong());
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // Views other than Overview get their own body.
            match self.view {
                View::Processes => { self.draw_processes(ui); return; }
                View::History => { self.draw_history(ui); return; }
                View::Settings => { self.draw_settings(ui, ctx); return; }
                View::Overview => {}
            }

            let running = self.statuses.iter().filter(|x| x.running).count();
            let total_restarts: i64 = self.statuses.iter().map(|x| x.restarts).sum();

            // Title row
            ui.heading(egui::RichText::new(s.title).size(24.0).color(p.ink));
            let running_fmt = s.subtitle_running.replace("{}", &running.to_string()).replacen("{}", &self.configs.len().to_string(), 1);
            ui.label(egui::RichText::new(running_fmt).size(12.0).color(p.muted));
            ui.add_space(12.0);

            // KPI strip: one container, hairline dividers
            ui::stat_strip(ui, self.theme, &[
                (s.guarding, &running.to_string(), p.green),
                (s.total, &self.configs.len().to_string(), p.accent),
                (s.restarts_total, &total_restarts.to_string(), p.amber),
                (s.crash_records, &self.crashes.len().to_string(), p.red),
            ]);
            ui.add_space(12.0);

            // Toolbar
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
                ui.label(egui::RichText::new(toast).color(p.amber));
            }
            ui.add_space(10.0);

            // Chart band: trend + health ring. Side by side when wide, stacked when narrow.
            let (trend_values, trend_labels) = self.trend_data();
            let health = self.health_ratio();
            let trend_card = |ui: &mut egui::Ui| {
                egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                        ui.label(egui::RichText::new(s.chart_restart_trend).size(14.0).strong().color(p.ink));
                        ui.label(egui::RichText::new(s.chart_trend_meta).size(11.0).color(p.muted));
                        ui.add_space(6.0);
                        ui::line_chart(ui, self.theme, &trend_values, &trend_labels);
                    });
            };
            let health_card = |ui: &mut egui::Ui| {
                egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                        ui.label(egui::RichText::new(s.chart_uptime).size(14.0).strong().color(p.ink));
                        ui.label(egui::RichText::new(s.chart_uptime_meta).size(11.0).color(p.muted));
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui::ring_chart(ui, self.theme, health);
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(s.label_online).size(12.0).color(p.muted));
                                ui.label(egui::RichText::new(s.label_restarting).size(12.0).color(p.muted));
                                ui.label(egui::RichText::new(s.label_offline).size(12.0).color(p.muted));
                            });
                        });
                    });
            };
            let wide = ui.available_width() >= 720.0;
            if wide {
                let half = (ui.available_width() - 10.0) / 2.0;
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(egui::vec2(half, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(half - 8.0);
                        trend_card(ui);
                    });
                    ui.add_space(10.0);
                    ui.allocate_ui_with_layout(egui::vec2(half, 0.0), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(half - 8.0);
                        health_card(ui);
                    });
                });
            } else {
                trend_card(ui);
                ui.add_space(10.0);
                health_card(ui);
            }
            ui.add_space(12.0);

            // Restart pulse: this product's signature view
            let pulse = self.pulse_data();
            egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
                .corner_radius(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                    ui.label(egui::RichText::new(s.chart_pulse).size(14.0).strong().color(p.ink));
                    ui.label(egui::RichText::new(s.chart_pulse_meta).size(11.0).color(p.muted));
                    ui.horizontal(|ui| {
                        let a = ui.cursor().min;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(a + egui::vec2(0.0, 5.0), egui::vec2(8.0, 8.0)), 2.0, p.muted);
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new(s.pulse_event).size(10.0).color(p.muted));
                        ui.add_space(8.0);
                        let b = ui.cursor().min;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(b + egui::vec2(0.0, 5.0), egui::vec2(8.0, 8.0)), 2.0, p.red);
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new(s.pulse_cluster).size(10.0).color(p.muted));
                    });
                    ui::pulse_chart(ui, self.theme, &pulse);
                });
            ui.add_space(12.0);

            // Guarded processes table
            let filter = self.search.to_lowercase();
            let visible: Vec<usize> = (0..self.configs.len()).filter(|&i| {
                filter.is_empty() || self.configs[i].name.to_lowercase().contains(&filter)
            }).collect();

            if self.configs.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(s.empty_title).size(18.0).color(p.muted));
                    ui.add_space(8.0);
                    ui.label(s.empty_line1);
                    ui.label(s.empty_line2);
                });
            } else if visible.is_empty() {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| { ui.label(egui::RichText::new(s.no_crashes).color(p.muted)); });
            } else {
                egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(6.0).inner_margin(egui::vec2(12.0, 8.0)).show(ui, |ui| {
                    egui::Grid::new("entries").num_columns(5).striped(false).spacing([16.0, 6.0]).show(ui, |ui| {
                        for h in [s.col_name, s.col_status, s.col_pid, s.col_restarts, s.col_last_exit] {
                            ui.label(egui::RichText::new(h).size(11.0).color(p.muted));
                        }
                        ui.end_row();
                        for idx in visible {
                            let cfg = &self.configs[idx];
                            let status = self.statuses.get(idx);
                            if ui.selectable_label(self.selected == Some(idx), &cfg.name).clicked() { self.selected = Some(idx); }
                            let is_running = status.map(|x| x.running).unwrap_or(false);
                            let (label, color) = if self.paused {
                                (s.paused, p.amber)
                            } else if is_running {
                                (s.running, p.green)
                            } else {
                                (s.stopped, p.red)
                            };
                            ui.horizontal(|ui| {
                                ui.painter().circle_filled(ui.cursor().min + egui::vec2(3.0, 8.0), 3.0, color);
                                ui.add_space(10.0);
                                ui.label(egui::RichText::new(label).size(12.0).color(color));
                            });
                            ui.label(status.and_then(|x| x.pid).map(|v| v.to_string()).unwrap_or_else(|| "-".into()));
                            ui.label(status.map(|x| x.restarts.to_string()).unwrap_or_else(|| "0".into()));
                            ui.label(status.and_then(|x| x.last_exit.clone()).unwrap_or_else(|| "-".into()));
                            ui.end_row();
                        }
                    });
                });

                ui.add_space(10.0);
                egui::Frame::NONE.fill(p.surface).stroke(egui::Stroke::new(1.0_f32, p.border))
                    .corner_radius(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.crash_history).size(14.0).strong().color(p.ink));
                        ui.label(egui::RichText::new(format!("{} {}", self.crashes.len(), s.crash_count)).size(11.0).color(p.muted));
                    });
                    if self.crashes.is_empty() {
                        ui.label(egui::RichText::new(s.no_crashes).size(12.0).color(p.muted));
                    } else {
                        for c in self.crashes.iter().rev().take(6) {
                            ui.label(egui::RichText::new(s.crash_line
                                .replace("{t}", &c.time).replace("{p}", &c.process)
                                .replace("{pid}", &c.pid.to_string()).replace("{code}", &c.exit_code)
                                .replace("{up}", &c.uptime_secs.to_string()).replace("{n}", &c.restart_number.to_string())
                            ).size(11.0).color(p.muted));
                        }
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
        // Logical points; keep it DPI-safe so all columns fit at 150% scaling.
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 620.0]).with_title(title),
        ..Default::default()
    };
    eframe::run_native("Guardian", options, Box::new(move |cc| {
        // Apply saved theme + CJK font before first frame.
        install_cjk_font(&cc.egui_ctx);
        ui::apply_visuals(&cc.egui_ctx, st.theme);
        let tray = build_tray(st.lang);
        let mut app = GuardianApp::new(tray);
        app.font_installed = true;
        Ok(Box::new(app))
    }))
}

#[cfg(test)]
mod close_tests {
    use super::should_minimize_to_tray;

    #[test]
    fn only_window_close_minimizes_to_tray() {
        assert!(should_minimize_to_tray(true, false));  // X button -> tray
        assert!(!should_minimize_to_tray(true, true));  // tray/menu Quit -> exit
        assert!(!should_minimize_to_tray(false, false)); // no request -> nothing
        assert!(!should_minimize_to_tray(false, true));  // quitting set, no request yet
    }
}
