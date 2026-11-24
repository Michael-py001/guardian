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

/// Fill two `{}` placeholders in order. `str::replace` would fill both with the first value.
fn fill2(template: &str, first: &str, second: &str) -> String {
    template.replacen("{}", first, 1).replacen("{}", second, 1)
}

/// Seconds to the prototype's "HH:MM:SS" uptime label.
fn format_hms(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
}

/// Format one crash-history row with the app's bilingual template.
fn crash_line_text(template: &str, c: &CrashEntry) -> String {
    template
        .replace("{t}", &c.time)
        .replace("{p}", &c.process)
        .replace("{pid}", &c.pid.to_string())
        .replace("{code}", &c.exit_code)
        .replace("{up}", &format_hms(c.uptime_secs))
        .replace("{n}", &c.restart_number.to_string())
}

/// Seconds to a compact "14h 22m" / "41s" label, like the prototype's uptime figures.
fn format_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 { format!("{}h {:02}m", h, m) }
    else if m > 0 { format!("{}m {:02}s", m, secs % 60) }
    else { format!("{}s", secs) }
}

/// Four clock labels for the restart-pulse axis: 60, 40, 20 minutes ago, now.
fn pulse_axis_labels() -> [String; 4] {
    use chrono::{Local, Duration};
    let now = Local::now();
    [60, 40, 20, 0].map(|m| (now - Duration::minutes(m)).format("%H:%M").to_string())
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

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path()).ok().and_then(|m| m.modified().ok())
}

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
    config_tx: Option<engine::ConfigSender>,
    pause_handle: Option<Arc<AtomicBool>>,
    paused: bool,
    autostart: bool,
    toast: Option<String>,
    toast_at: std::time::Instant,
    last_crash_refresh: std::time::Instant,
    last_config_check: std::time::Instant,
    last_config_mtime: Option<std::time::SystemTime>,
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
            watcher_stop: None, config_tx: None, pause_handle: None, paused: false,
            autostart: engine::task_installed(), toast: None, toast_at: std::time::Instant::now(),
            last_crash_refresh: std::time::Instant::now(),
            last_config_check: std::time::Instant::now(),
            last_config_mtime: config_mtime(),
            tray,
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
                Err(e) => self.set_toast(strings(self.lang).toast_load_fail.replace("{}", &e.to_string())),
            }
        }
        self.crashes = engine::read_crash_history(&crash_path());
    }

    fn persist(&mut self) {
        let config = engine::Config { process: self.configs.clone() };
        match engine::save_config(&config_path(), &config) {
            Ok(()) => self.last_config_mtime = config_mtime(), // don't reload our own write
            Err(e) => self.set_toast(strings(self.lang).toast_save_fail.replace("{}", &e.to_string())),
        }
    }

    /// Pick up hand-edited guardian.toml without a restart. The GUI calls persist()
    /// after every change and refreshes the stored mtime, so only external edits land here.
    fn reload_config_if_changed(&mut self) {
        if self.last_config_check.elapsed() < std::time::Duration::from_millis(1_000) { return; }
        self.last_config_check = std::time::Instant::now();
        let mtime = config_mtime();
        if mtime == self.last_config_mtime { return; }
        self.last_config_mtime = mtime;
        let path = config_path();
        if !path.exists() {
            self.configs.clear();
        } else {
            match engine::load_config(&path) {
                Ok(config) => self.configs = config.process,
                Err(e) => {
                    self.set_toast(strings(self.lang).toast_load_fail.replace("{}", &e.to_string()));
                    return; // keep the current config on bad TOML
                }
            }
        }
        self.selected = None;
        self.apply_config();
        self.set_toast(strings(self.lang).toast_config_reloaded.to_string());
    }

    fn set_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(text.into());
        self.toast_at = std::time::Instant::now();
    }

    fn persist_settings(&mut self) {
        save_settings(&Settings { lang: self.lang, theme: self.theme });
    }

    fn spawn_watcher(&mut self) {
        if let Some(stop) = &self.watcher_stop { stop.store(true, Ordering::SeqCst); }
        let _ = engine::init_logs(&log_path(), &crash_path());
        let config = engine::Config { process: self.configs.clone() };
        let (mut watcher, tx) = Watcher::new(&config, self.sink.clone());
        self.watcher_stop = Some(watcher.stop_handle());
        self.config_tx = Some(tx);
        self.pause_handle = Some(watcher.pause_handle());
        if self.paused {
            if let Some(h) = &self.pause_handle { h.store(true, Ordering::SeqCst); }
        }
        std::thread::spawn(move || watcher.run());
    }

    /// Push the current config to the running watcher without killing healthy
    /// processes. Falls back to a full restart only when no watcher is alive.
    fn apply_config(&mut self) {
        let config = engine::Config { process: self.configs.clone() };
        match &self.config_tx {
            Some(tx) if tx.send(config).is_ok() => {}
            _ => self.spawn_watcher(),
        }
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
            self.set_toast(st.toast_add_fail.replace("{}", &e.to_string())); return;
        }
        self.configs.push(cfg);
        self.add.open = false;
        self.add.name.clear(); self.add.command.clear(); self.add.args.clear();
        self.persist(); self.apply_config();
        self.set_toast(st.toast_added.to_string());
    }

    fn add_from_process(&mut self) {
        let Some(idx) = self.add.selected else {
            self.set_toast(strings(self.lang).toast_select_first.to_string()); return;
        };
        let Some(proc) = self.add.processes.get(idx).cloned() else { return };
        let name = PathBuf::from(&proc.name).file_stem()
            .map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| proc.name.clone());
        let command = proc.path.clone().unwrap_or_else(|| proc.name.clone());
        let mut cfg = ProcessConfig::new(name, command);
        cfg.adopt = Some(proc.name);
        let st = strings(self.lang);
        if let Err(e) = cfg.validate() {
            self.set_toast(st.toast_add_fail.replace("{}", &e.to_string())); return;
        }
        self.configs.push(cfg);
        self.add.open = false; self.add.selected = None;
        self.persist(); self.apply_config();
        self.set_toast(st.toast_adopted.to_string());
    }

    fn remove_selected(&mut self) {
        if let Some(idx) = self.selected {
            if idx < self.configs.len() { self.configs.remove(idx); }
            self.selected = None;
            self.persist(); self.apply_config();
            self.set_toast(strings(self.lang).toast_removed.to_string());
        }
    }

    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if let Some(h) = &self.pause_handle { h.store(self.paused, Ordering::SeqCst); }
        self.set_toast(if self.paused {
            strings(self.lang).toast_paused.to_string()
        } else {
            strings(self.lang).toast_resumed.to_string()
        });
    }

    fn restart_all(&mut self) {
        self.spawn_watcher();
        self.set_toast(strings(self.lang).toast_restarted.to_string());
    }

    fn toggle_autostart(&mut self) {
        let result = if self.autostart { engine::uninstall_task() } else {
            engine::install_task(&std::env::current_exe().unwrap_or_default(), &config_path())
        };
        let st = strings(self.lang);
        match result {
            Ok(()) => {
                self.autostart = engine::task_installed();
                self.set_toast(if self.autostart { st.toast_auto_on.to_string() } else { st.toast_auto_off.to_string() });
            }
            Err(e) => self.set_toast(st.toast_auto_fail.replace("{}", &e.to_string())),
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn clear_history(&mut self) {
        let _ = engine::clear_crash_history(&crash_path());
        self.crashes.clear();
        self.set_toast(strings(self.lang).toast_history_cleared.to_string());
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
                for (i, c) in self.crashes.iter().rev().enumerate() {
                    if i > 0 { ui.separator(); }
                    // Prototype's .event: name + time on one row, detail on the next.
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&c.process).size(12.5).strong().color(p.ink));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(c.time.clone()).size(10.5).color(p.muted));
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.event_exit).size(11.5).color(p.muted));
                        ui.label(egui::RichText::new(&c.exit_code).size(11.5).color(p.red));
                        ui.label(egui::RichText::new("·").size(11.5).color(p.muted));
                        ui.label(egui::RichText::new(s.event_uptime).size(11.5).color(p.muted));
                        ui.label(egui::RichText::new(format_hms(c.uptime_secs)).size(11.5).color(p.ink));
                        ui.label(egui::RichText::new("·").size(11.5).color(p.muted));
                        ui.label(egui::RichText::new(
                            s.event_restart.replace("{}", &c.restart_number.to_string())
                        ).size(11.5).color(p.muted));
                    });
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

    /// Share of guarded entries that are running right now. No estimate.
    fn health_ratio(&self) -> f32 {
        if self.statuses.is_empty() { return 0.0; }
        let running = self.statuses.iter().filter(|x| x.running).count();
        running as f32 / self.statuses.len() as f32
    }

    /// The process with the most restarts today, for the subtitle finding.
    fn top_crasher_today(&self) -> Option<(String, usize)> {
        use chrono::{Local, NaiveDateTime};
        let today = Local::now().date_naive();
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for c in &self.crashes {
            if let Ok(t) = NaiveDateTime::parse_from_str(&c.time, "%Y-%m-%d %H:%M:%S") {
                if t.date() == today { *counts.entry(&c.process).or_insert(0) += 1; }
            }
        }
        counts.into_iter().max_by_key(|(_, n)| *n)
            .map(|(name, n)| (name.to_string(), n))
    }

    /// Honest counters for the health card: (running now, stopped now, crashes in 24h).
    fn health_counts(&self) -> (usize, usize, usize) {
        use chrono::{Local, NaiveDateTime};
        let running = self.statuses.iter().filter(|x| x.running).count();
        let stopped = self.statuses.iter().filter(|x| !x.running).count();
        let now = Local::now().naive_local();
        let crashes = self.crashes.iter().filter(|c| {
            NaiveDateTime::parse_from_str(&c.time, "%Y-%m-%d %H:%M:%S")
                .map(|t| (now - t).num_hours() < 24).unwrap_or(false)
        }).count();
        (running, stopped, crashes)
    }

    /// Guard entries adopted from an already-running process.
    fn adopted_count(&self) -> usize {
        self.configs.iter().filter(|c| c.adopt.is_some()).count()
    }

    /// Crashes in the last 20 minutes — the "cluster" KPI note.
    fn crash_cluster_count(&self) -> usize {
        use chrono::{Local, NaiveDateTime};
        let now = Local::now().naive_local();
        self.crashes.iter().filter(|c| {
            NaiveDateTime::parse_from_str(&c.time, "%Y-%m-%d %H:%M:%S")
                .map(|t| { let m = (now - t).num_seconds() / 60; m >= 0 && m <= 20 })
                .unwrap_or(false)
        }).count()
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

        // Keyboard shortcuts advertised in the menu: Ctrl+N add, Del remove, Esc close dialogs.
        // Skip Del while a text field has focus so editing is never swallowed.
        let editing = ctx.memory(|m| m.focused().is_some());
        let (ctrl_n, del, esc) = ctx.input(|i| (
            i.modifiers.ctrl && i.key_pressed(egui::Key::N),
            i.key_pressed(egui::Key::Delete),
            i.key_pressed(egui::Key::Escape),
        ));
        if ctrl_n {
            self.add.open = true;
            self.add.processes_loaded = false;
        } else if del && !editing {
            self.remove_selected();
        }
        if esc {
            self.add.open = false;
            self.about_open = false;
        }

        // Toasts are ephemeral; clear any that have been on screen for 5 seconds.
        // set_toast() refreshes toast_at whenever a new message arrives.
        if self.toast_at.elapsed() >= std::time::Duration::from_secs(5) {
            self.toast = None;
        }

        self.reload_config_if_changed();

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

        // ---- Custom title bar (frameless window) ----
        egui::TopBottomPanel::top("titlebar").exact_height(34.0).show(ctx, |ui| {
            let response = ui.interact(ui.max_rect(), ui.id().with("drag"), egui::Sense::drag());
            if response.double_clicked() {
                let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            } else if response.dragged() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                // Shield mark drawn from the product's own symbol.
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "◈",
                    egui::FontId::proportional(14.0), p.accent);
                ui.label(egui::RichText::new(s.brand_name).size(12.0).strong().color(p.ink));
                ui.label(egui::RichText::new(s.brand_scope).size(11.0).color(p.muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
                        let r = ui.add_sized([30.0, 30.0], egui::Button::new(
                            egui::RichText::new(glyph).size(12.0).color(p.muted)).frame(false));
                        r.on_hover_text(tip).clicked()
                    };
                    if btn(ui, "✕", s.win_close) { self.quit(ctx); }
                    let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                    if btn(ui, "□", if max { s.win_restore } else { s.win_maximize }) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                    if btn(ui, "─", s.win_minimize) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    ui.add_space(10.0);
                    let dot = if self.paused { p.amber } else { p.green };
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 3.5, dot);
                        ui.label(egui::RichText::new(s.live_active).size(11.0).color(p.muted));
                    });
                });
            });
        });

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
            ui.add_space((ui.available_height() - 44.0).max(0.0));
            let about = ui.add_sized([40.0, 38.0], egui::Button::new(
                egui::RichText::new("ⓘ").size(17.0).color(p.muted)).frame(false));
            if about.clicked() { self.about_open = true; }
            about.on_hover_text(s.menu_about);
        });

        // ---- Menu bar ----
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            // Menu row with a right-aligned key hint, like the prototype's `.key` span.
            let menu_item = |ui: &mut egui::Ui, label: &str, hint: &str| {
                let mut clicked = false;
                ui.horizontal(|ui| {
                    clicked = ui.button(label).clicked();
                    if !hint.is_empty() {
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(hint).size(11.0).color(p.muted));
                    }
                });
                clicked
            };
            egui::menu::bar(ui, |ui| {
                ui.menu_button(s.menu_file, |ui| {
                    if menu_item(ui, s.menu_add, "Ctrl+N") { self.add.open = true; self.add.processes_loaded = false; ui.close_menu(); }
                    if menu_item(ui, s.menu_remove, "Del") { self.remove_selected(); ui.close_menu(); }
                    ui.separator();
                    if ui.button(s.menu_open_folder).clicked() { self.open_folder(); ui.close_menu(); }
                    if ui.button(s.menu_open_log).clicked() { self.open_log(); ui.close_menu(); }
                    ui.separator();
                    if ui.button(s.menu_minimize_tray).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false)); ui.close_menu();
                    }
                    if menu_item(ui, s.menu_quit, "Alt+F4") { self.quit(ctx); }
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
                View::Processes => { egui::ScrollArea::vertical().show(ui, |ui| self.draw_processes(ui)); return; }
                View::History => { egui::ScrollArea::vertical().show(ui, |ui| self.draw_history(ui)); return; }
                View::Settings => { self.draw_settings(ui, ctx); return; }
                View::Overview => {}
            }

            // The dashboard is taller than the window; scroll so the table and
            // crash history are reachable even at 620px height.
            egui::ScrollArea::vertical().show(ui, |ui| {
            let running = self.statuses.iter().filter(|x| x.running).count();
            let total_restarts: i64 = self.statuses.iter().map(|x| x.restarts).sum();
            // Title row: heading + finding + prominent actions (matches prototype).
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading(egui::RichText::new(s.nav_overview).size(24.0).color(p.ink));
                    let running_fmt = fill2(s.subtitle_running, &running.to_string(), &self.configs.len().to_string());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(running_fmt).size(12.0).color(p.muted));
                        if let Some((name, n)) = self.top_crasher_today() {
                            let finding = fill2(s.subtitle_finding, &name, &n.to_string());
                            ui.label(egui::RichText::new(finding).size(12.0).color(p.amber).strong());
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::BOTTOM), |ui| {
                    if ui.button(egui::RichText::new(s.add_button).size(12.0)).clicked() {
                        self.add.open = true;
                        self.add.processes_loaded = false;
                    }
                    let pause_label = if self.paused { s.resume_button } else { s.pause_button };
                    if ui.button(egui::RichText::new(pause_label).size(12.0)).clicked() { self.toggle_pause(); }
                });
            });
            ui.add_space(12.0);

            // KPI strip: one container, hairline dividers. Notes are real counters (ponytail: no fabricated "14h 22m").
            let stopped = self.configs.len().saturating_sub(running);
            let note_stopped = s.note_stopped.replace("{}", &stopped.to_string());
            let note_adopted = s.note_adopted.replace("{}", &self.adopted_count().to_string());
            let note_cluster = s.note_crash_cluster.replace("{}", &self.crash_cluster_count().to_string());
            ui::stat_strip(ui, self.theme, &[
                (s.guarding, &running.to_string(), &note_stopped, p.green),
                (s.total, &self.configs.len().to_string(), &note_adopted, p.accent),
                (s.restarts_total, &total_restarts.to_string(), s.note_backoff, p.amber),
                (s.crash_records, &self.crashes.len().to_string(), &note_cluster, p.red),
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
            let (h_running, h_stopped, h_crashes) = self.health_counts();
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
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(s.label_online).size(11.0).color(p.muted));
                                    ui.label(egui::RichText::new(format!("{}", h_running)).size(11.0).color(p.green));
                                });
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(s.label_crashes_24h).size(11.0).color(p.muted));
                                    ui.label(egui::RichText::new(format!("{}", h_crashes)).size(11.0).color(p.amber));
                                });
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(s.label_offline).size(11.0).color(p.muted));
                                    ui.label(egui::RichText::new(format!("{}", h_stopped)).size(11.0).color(p.red));
                                });
                            });
                        });
                        if let Some(longest) = self.statuses.iter().filter(|x| x.running).max_by_key(|x| x.uptime_secs) {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(s.longest_running.replace("{}", &format_duration(longest.uptime_secs)))
                                .size(11.0).color(p.muted));
                        }
                        if let Some((name, n)) = self.top_crasher_today() {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(fill2(s.health_finding, &name, &n.to_string()))
                                .size(11.0).color(p.muted));
                        }
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
                    let pulse_axis = pulse_axis_labels();
                    ui::pulse_chart(ui, self.theme, &pulse, &pulse_axis);
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
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.status_bar).size(10.0).color(p.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(egui::RichText::new(s.view_all).size(10.0).color(p.accent)).frame(false)).clicked() {
                                self.view = View::Processes;
                            }
                        });
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
                            ui.label(egui::RichText::new(crash_line_text(s.crash_line, c))
                                .size(11.0).color(p.muted));
                        }
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let shown = self.crashes.len().min(6);
                            ui.label(egui::RichText::new(s.showing_latest.replace("{}", &shown.to_string())).size(10.0).color(p.muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(egui::RichText::new(s.menu_clear_history).size(10.0).color(p.accent)).frame(false)).clicked() {
                                    self.clear_history();
                                }
                            });
                        });
                    }
                });
            }
            }); // ScrollArea
        }); // CentralPanel

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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_title(title)
            .with_decorations(false),
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
    use super::{CrashEntry, fill2, format_duration, format_hms, should_minimize_to_tray};

    #[test]
    fn duration_formats_match_prototype() {
        assert_eq!(format_duration(41), "41s");
        assert_eq!(format_duration(537), "8m 57s");
        assert_eq!(format_duration(14 * 3600 + 22 * 60), "14h 22m");
        assert_eq!(format_hms(161), "00:02:41");
    }

    #[test]
    fn crash_line_uses_clock_uptime() {
        let c = CrashEntry {
            time: "14:51:07".into(), process: "app.exe".into(), pid: 1936,
            exit_code: "1".into(), uptime_secs: 161, restart_number: 6,
        };
        let line = super::crash_line_text("exit={code} uptime={up} restart#{n}", &c);
        assert_eq!(line, "exit=1 uptime=00:02:41 restart#6");
    }

    #[test]
    fn fill2_replaces_placeholders_in_order() {
        assert_eq!(fill2("{} of {} entries", "3", "5"), "3 of 5 entries");
        assert_eq!(fill2("{} restarted {} times", "Demo", "5"), "Demo restarted 5 times");
    }

    #[test]
    fn only_window_close_minimizes_to_tray() {
        assert!(should_minimize_to_tray(true, false));  // X button -> tray
        assert!(!should_minimize_to_tray(true, true));  // tray/menu Quit -> exit
        assert!(!should_minimize_to_tray(false, false)); // no request -> nothing
        assert!(!should_minimize_to_tray(false, true));  // quitting set, no request yet
    }
}
