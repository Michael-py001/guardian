// Hide the console window: this is a GUI application.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod engine;
mod ui;
mod ui_strings;

use eframe::egui;
use engine::{CrashEntry, EntryStatus, ProcessConfig, RunningProcess, StatusSink, Watcher};
#[cfg(windows)]
use raw_window_handle::HasWindowHandle;
use rfd::FileDialog;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};
use ui::ground;
use ui_strings::{Lang, Settings, Theme, strings};

/// A close request minimizes to tray only for the window's own X button.
/// Explicit Quit sets `quitting`, so the request is left uncancelled and eframe exits.
fn should_minimize_to_tray(close_requested: bool, quitting: bool) -> bool {
    close_requested && !quitting
}

/// Main window handle, stored as `isize` for the tray thread's `SetForegroundWindow`
/// call. Showing and hiding always go through `ViewportCommand` (see
/// `set_window_visible`) so winit's own `VISIBLE` flag never disagrees with the HWND.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// egui context, so the tray thread can drive the window through viewport commands
/// from outside `update()`.
static MAIN_CTX: Mutex<Option<egui::Context>> = Mutex::new(None);

/// Hidden-to-tray state, read by `update()` to decide whether to keep rendering.
static WINDOW_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Set by the tray autostart click; `update()` refreshes the menu label on its
/// next frame (when the window is shown again after being hidden).
static TRAY_AUTOSTART_CHANGED: AtomicBool = AtomicBool::new(false);

/// Fill two `{}` placeholders in order. `str::replace` would fill both with the first value.
fn fill2(template: &str, first: &str, second: &str) -> String {
    template.replacen("{}", first, 1).replacen("{}", second, 1)
}

/// Seconds to the prototype's "HH:MM:SS" uptime label.
fn format_hms(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
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

/// Install a clear Chinese-capable system font into egui so CJK is not tofu and
/// Latin text stays crisp. The CJK face goes last: egui falls back to it per-glyph,
/// so Latin uses the sharp built-in Ubuntu face and CJK uses Microsoft YaHei UI.
fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\msyh.ttc",       // Microsoft YaHei UI — clear, modern
        r"C:\Windows\Fonts\Deng.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];
    let found = candidates.iter().find_map(|p| std::fs::read(p).ok().map(|b| (p, b)));
    let mut fonts = egui::FontDefinitions::default();
    let Some((path, data)) = found else { ctx.set_fonts(fonts); return };
    let name = format!("CJK-{}", PathBuf::from(path).file_name().unwrap_or_default().to_string_lossy());
    // Slight upward scale makes the dense CJK glyphs read more clearly at small sizes.
    let data = egui::FontData::from_owned(data).tweak(egui::FontTweak {
        scale: 1.06,
        y_offset: -0.03,
        ..Default::default()
    });
    fonts.font_data.insert(name.clone(), Arc::new(data));
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Proportional) { list.push(name.clone()); }
    if let Some(list) = fonts.families.get_mut(&egui::FontFamily::Monospace) { list.push(name); }
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

    /// Frameless windows have no OS resize border. Detect the pointer near an edge
    /// and forward the drag to the backend via BeginResize so the window can still
    /// be resized from any side or corner.
    fn resize_edges(&self, ctx: &egui::Context) {
        use egui::viewport::ResizeDirection::{East, North, NorthEast, NorthWest, South, SouthEast, SouthWest, West};
        let t = 7.0_f32; // grab thickness in points
        let dir = ctx.input(|i| {
            let pointer = &i.pointer;
            let Some(pos) = pointer.hover_pos() else { return None };
            let r = i.screen_rect();
            let near_l = pos.x <= r.left() + t;
            let near_r = pos.x >= r.right() - t;
            let near_t = pos.y <= r.top() + t;
            let near_b = pos.y >= r.bottom() - t;
            if near_t && near_l { Some(NorthWest) }
            else if near_t && near_r { Some(NorthEast) }
            else if near_b && near_l { Some(SouthWest) }
            else if near_b && near_r { Some(SouthEast) }
            else if near_l { Some(West) }
            else if near_r { Some(East) }
            else if near_t { Some(North) }
            else if near_b { Some(South) }
            else { None }
        });
        if let Some(dir) = dir {
            ctx.set_cursor_icon(match dir {
                NorthWest | SouthEast => egui::CursorIcon::ResizeNwSe,
                NorthEast | SouthWest => egui::CursorIcon::ResizeNeSw,
                West | East => egui::CursorIcon::ResizeHorizontal,
                North | South => egui::CursorIcon::ResizeVertical,
            });
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
            }
        }
    }

    /// A text button with a leading vector icon.
    fn icon_button(ui: &mut egui::Ui, kind: ui::Icon, label: &str, color: egui::Color32) -> bool {
        let r = ui.button(egui::RichText::new(format!("  {}", label)).size(12.0));
        let ic = r.rect.left() + 9.0;
        ui::icon_at(ui.painter(), egui::pos2(ic, r.rect.center().y), kind, color, 12.0);
        r.clicked()
    }

    /// Paint a card surface across the full column at an exact height, then lay out
    /// its contents inside. Frames size to their content, which makes neighbouring
    /// cards ragged; this keeps every card in a band on the same baseline.
    fn card_surface(ui: &mut egui::Ui, height: f32, p: &ui::Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
        ui.painter().rect_filled(rect, 6.0, p.surface);
        ui.painter().rect_stroke(rect, 6.0, egui::Stroke::new(1.0_f32, p.border), egui::StrokeKind::Inside);
        let inner = rect.shrink2(egui::vec2(14.0, 12.0));
        // new_child does not advance the parent cursor — allocate_exact_size already did.
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::LEFT)),
        );
        add_contents(&mut child);
    }

    /// Render the restart-trend card, filling the width it is given.
    fn draw_trend_card(&self, ui: &mut egui::Ui, s: &ui_strings::Str, p: &ui::Palette, height: f32) {
        let (trend_values, trend_labels) = self.trend_data();
        Self::card_surface(ui, height, p, |ui| {
                ui.label(egui::RichText::new(s.chart_restart_trend).size(14.0).strong().color(p.ink));
                ui.label(egui::RichText::new(s.chart_trend_meta).size(11.0).color(p.muted));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let sw = ui.cursor().min;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(sw + egui::vec2(0.0, 3.0), egui::vec2(8.0, 8.0)), 2.0, p.accent);
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new(s.legend_all_entries).size(11.0).color(p.muted));
                });
                ui.add_space(2.0);
                let peak_note = trend_values.iter().enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                    .filter(|(_, v)| **v > 0.0)
                    .map(|(i, v)| {
                        let hour = trend_labels.get(i).and_then(|l| l.split(':').next()).unwrap_or("");
                        s.trend_peak.replace("{}", &format!("{:.0}", v)).replacen("{}", hour, 1)
                    });
                ui::line_chart(ui, self.theme, &trend_values, &trend_labels, peak_note.as_deref());
            });
    }

    /// Render the uptime-health card, filling the width it is given.
    fn draw_health_card(&self, ui: &mut egui::Ui, s: &ui_strings::Str, p: &ui::Palette, height: f32) {
        let health = self.health_ratio();
        let h_stopped = self.offline_count();
        Self::card_surface(ui, height, p, |ui| {
                ui.label(egui::RichText::new(s.chart_uptime).size(14.0).strong().color(p.ink));
                ui.label(egui::RichText::new(s.chart_uptime_meta).size(11.0).color(p.muted));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui::ring_chart(ui, self.theme, health, s.ring_online_rate);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        let online_secs = self.statuses.iter().filter(|x| x.running)
                            .map(|x| x.uptime_secs).max().unwrap_or(0);
                        ui.label(egui::RichText::new(format!("{}  {}", s.label_online,
                            format_duration(online_secs))).size(11.0).color(p.green));
                        let restarting = self.statuses.iter().filter(|x| x.restarting).count();
                        ui.label(egui::RichText::new(format!("{}  {}", s.label_restarting,
                            restarting)).size(11.0).color(p.amber));
                        ui.label(egui::RichText::new(format!("{}  {}", s.label_offline,
                            h_stopped)).size(11.0).color(p.red));
                    });
                });
                // Finding always occupies the footer so the card never leaves a blank gap.
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                if let Some(longest) = self.statuses.iter().filter(|x| x.running).max_by_key(|x| x.uptime_secs) {
                    ui.label(egui::RichText::new(s.longest_running.replace("{}", &format_duration(longest.uptime_secs)))
                        .size(11.0).color(p.muted));
                } else if h_stopped > 0 {
                    ui.label(egui::RichText::new(s.health_offline_finding.replace("{}", &h_stopped.to_string()))
                        .size(11.0).color(p.muted));
                } else {
                    ui.label(egui::RichText::new(s.health_empty).size(11.0).color(p.muted));
                }
            });
    }

    /// Render the restart-pulse card (signature view).
    fn draw_pulse_card(&self, ui: &mut egui::Ui, s: &ui_strings::Str, p: &ui::Palette, height: f32) {
        let pulse = self.pulse_data();
        Self::card_surface(ui, height, p, |ui| {
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
    }

    /// Guarded-process table. Always shows its frame and header, even when empty.
    fn draw_guard_table(&mut self, ui: &mut egui::Ui, s: &ui_strings::Str, p: &ui::Palette, height: f32) {
        let filter = self.search.to_lowercase();
        let visible: Vec<usize> = (0..self.configs.len())
            .filter(|&i| filter.is_empty() || self.configs[i].name.to_lowercase().contains(&filter))
            .collect();
        Self::card_surface(ui, height, p, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(s.nav_processes).size(14.0).strong().color(p.ink));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(
                            s.entries_count.replace("{}", &self.configs.len().to_string())
                        ).size(11.0).color(p.muted));
                    });
                });
                ui.add_space(6.0);
                egui::Grid::new("entries").num_columns(5).striped(false).spacing([16.0, 6.0]).show(ui, |ui| {
                    for h in [s.col_name, s.col_status, s.col_pid, s.col_restarts, s.col_last_exit] {
                        ui.label(egui::RichText::new(h).size(11.0).color(p.muted));
                    }
                    ui.end_row();
                    for idx in &visible {
                        let cfg = &self.configs[*idx];
                        let status = self.statuses.get(*idx);
                        if ui.selectable_label(self.selected == Some(*idx), &cfg.name).clicked() { self.selected = Some(*idx); }
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
                if self.configs.is_empty() {
                    ui.add_space(18.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(s.empty_title).size(14.0).color(p.muted));
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(s.empty_line1).size(11.0).color(p.muted));
                        ui.label(egui::RichText::new(s.empty_line2).size(11.0).color(p.muted));
                    });
                }
                ui.add_space(4.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(s.status_bar).size(10.0).color(p.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new(s.view_all).size(10.0).color(p.accent)).frame(false)).clicked() {
                            self.view = View::Processes;
                        }
                    });
                });
            });
    }

    /// Crash-history card with structured event rows and a footer.
    fn draw_crash_card(&mut self, ui: &mut egui::Ui, s: &ui_strings::Str, p: &ui::Palette, height: f32) {
        Self::card_surface(ui, height, p, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(s.crash_history).size(14.0).strong().color(p.ink));
                    ui.label(egui::RichText::new(format!("{} {}", self.crashes.len(), s.crash_count)).size(11.0).color(p.muted));
                });
                ui.add_space(4.0);
                if self.crashes.is_empty() {
                    ui.label(egui::RichText::new(s.no_crashes).size(11.0).color(p.muted));
                } else {
                    for (i, c) in self.crashes.iter().rev().take(6).enumerate() {
                        if i > 0 { ui.separator(); }
                        Self::draw_crash_event(ui, c, s, &p);
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

    /// One crash event as the prototype's two-row card: name+time, then details.
    fn draw_crash_event(ui: &mut egui::Ui, c: &CrashEntry, s: &ui_strings::Str, p: &ui::Palette) {
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
                    Self::draw_crash_event(ui, c, s, &p);
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

    /// Entries currently stopped (not running), for the health card's offline figure.
    fn offline_count(&self) -> usize {
        self.statuses.iter().filter(|x| !x.running).count()
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

        // Tray menu events are handled directly by a background callback thread
        // (see install_tray_handler): once the window is hidden, this update loop
        // stalls entirely, so menu clicks cannot be processed here.
        if TRAY_AUTOSTART_CHANGED.swap(false, Ordering::SeqCst) {
            self.autostart = engine::task_installed();
            self.update_tray_labels();
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
            set_window_visible(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        let s = strings(self.lang);
        let p = ui::palette(self.theme);

        // ---- Frameless window: invisible drag strips on the edges/corners so the
        // window can still be resized with the mouse (no OS border to grab).
        self.resize_edges(ctx);

        // ---- Custom title bar (frameless window) ----
        egui::TopBottomPanel::top("titlebar").exact_height(34.0).show(ctx, |ui| {
            let response = ui.interact(ui.max_rect(), ui.id().with("drag"), egui::Sense::drag());
            if response.double_clicked() {
                let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            } else if response.dragged() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.horizontal_centered(|ui| {
                ui.add_space(10.0);
                ui::logo(ui, 20.0);
                ui.add_space(6.0);
                ui.label(egui::RichText::new(s.brand_name).size(13.0).strong().color(p.ink));
                ui.add_space(8.0);
                ui.label(egui::RichText::new(s.brand_scope).size(11.0).color(p.muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Each control is a rectangle covering the full titlebar height, edge
                    // to edge with its neighbours, so anywhere inside it triggers the
                    // action — not just the 12 pt glyph. The icon is painted on top and
                    // a hover tint shows the live area.
                    let spacing = ui.spacing().item_spacing.x;
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let btn = |ui: &mut egui::Ui, icon: ui::Icon, tip: &str| {
                        let size = egui::vec2(46.0, ui.available_height());
                        let (rect, response) =
                            ui.allocate_exact_size(size, egui::Sense::click());
                        let hovered = response.hovered();
                        if hovered {
                            ui.painter().rect_filled(rect, 0.0, p.selection_bg);
                        }
                        let col = if hovered { p.ink } else { p.muted };
                        ui::icon_at(ui.painter(), rect.center(), icon, col, 12.0);
                        response.on_hover_text(tip).clicked()
                    };
                    // The titlebar X hides to tray; File/Quit and the tray menu's
                    // Quit are the only paths that actually exit the process.
                    if btn(ui, ui::Icon::Close, s.win_close) {
                        set_window_visible(false);
                    }
                    let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                    if btn(ui, if max { ui::Icon::Restore } else { ui::Icon::Maximize },
                           if max { s.win_restore } else { s.win_maximize }) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                    if btn(ui, ui::Icon::Minimize, s.win_minimize) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    ui.spacing_mut().item_spacing.x = spacing;
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
        // Custom frame on purpose: egui draws a dark default bg_stroke down the
        // panel edge, which reads as a black line in both themes. Match the palette.
        egui::SidePanel::left("rail").exact_width(48.0).resizable(false)
            .frame(egui::Frame::NONE.fill(ground(self.theme))
                .stroke(egui::Stroke::new(1.0_f32, p.border)))
            .show(ctx, |ui| {
            let mut rail_btn = |ui: &mut egui::Ui, kind: ui::Icon, tip: &str, view: View| {
                let active = self.view == view;
                let btn = ui.add_sized([40.0, 38.0], egui::Button::new("").frame(false));
                let hovered = btn.hovered();
                let col = if active { p.ink } else if hovered { p.ink } else { p.muted };
                ui::icon_at(ui.painter(), btn.rect.center(), kind, col, 18.0);
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
            rail_btn(ui, ui::Icon::Grid, s.nav_overview, View::Overview);
            rail_btn(ui, ui::Icon::List, s.nav_processes, View::Processes);
            rail_btn(ui, ui::Icon::Clock, s.nav_history, View::History);
            ui.add_space(6.0);
            ui.separator();
            rail_btn(ui, ui::Icon::Gear, s.nav_settings, View::Settings);
            ui.add_space((ui.available_height() - 44.0).max(0.0));
            let about = ui.add_sized([40.0, 38.0], egui::Button::new("").frame(false));
            ui::icon_at(ui.painter(), about.rect.center(), ui::Icon::Info,
                if about.hovered() { p.ink } else { p.muted }, 18.0);
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
                        set_window_visible(false); ui.close_menu();
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
                        set_window_visible(false); ui.close_menu();
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
                    if Self::icon_button(ui, ui::Icon::Plus, s.add_button, p.ink) {
                        self.add.open = true;
                        self.add.processes_loaded = false;
                    }
                    let pause_label = if self.paused { s.resume_button } else { s.pause_all };
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(
                        s.entries_count.replace("{}", &self.configs.len().to_string())
                    ).size(11.0).color(p.muted));
                });
            });
            if let Some(toast) = &self.toast {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(toast).color(p.amber));
            }
            ui.add_space(10.0);

            // Both bands share one column ratio so the divider lines up, and every
            // card is given an exact height so the two sides end on the same baseline.
            let gap = 16.0_f32;
            let band_w = ui.available_width();
            let left_w = (band_w - gap) * 0.62;
            let right_w = band_w - gap - left_w;
            let chart_h = 224.0_f32;

            // ---- Chart band: restart trend left, uptime health right ----
            if band_w >= 720.0 {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0; // widths already reserve the gap
                    ui.allocate_ui_with_layout(egui::vec2(left_w, chart_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(left_w);
                        self.draw_trend_card(ui, s, &p, chart_h);
                    });
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(egui::vec2(right_w, chart_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(right_w);
                        self.draw_health_card(ui, s, &p, chart_h);
                    });
                });
            } else {
                self.draw_trend_card(ui, s, &p, chart_h);
                ui.add_space(gap);
                self.draw_health_card(ui, s, &p, chart_h);
            }
            ui.add_space(16.0);

            // ---- Bottom band: guard table left; pulse + crash history stacked right ----
            let rows = self.configs.iter().filter(|c| self.search.is_empty()
                || c.name.to_lowercase().contains(&self.search.to_lowercase())).count();
            let table_h = (110.0 + rows as f32 * 24.0
                + if self.configs.is_empty() { 40.0 } else { 0.0 }).max(210.0);
            let pulse_h = self.pulse_data().len() as f32 * 22.0 + 28.0 + 92.0;
            let shown_crashes = self.crashes.len().min(6);
            let crash_min = if shown_crashes == 0 { 78.0 } else { 96.0 + shown_crashes as f32 * 44.0 };
            let bottom_h = table_h.max(pulse_h + gap + crash_min);
            let crash_h = bottom_h - pulse_h - gap;
            if band_w >= 720.0 {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0; // widths already reserve the gap
                    ui.allocate_ui_with_layout(egui::vec2(left_w, bottom_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(left_w);
                        self.draw_guard_table(ui, s, &p, bottom_h);
                    });
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(egui::vec2(right_w, bottom_h), egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.set_width(right_w);
                        self.draw_pulse_card(ui, s, &p, pulse_h);
                        ui.add_space(gap);
                        self.draw_crash_card(ui, s, &p, crash_h);
                    });
                });
            } else {
                self.draw_guard_table(ui, s, &p, bottom_h);
                ui.add_space(gap);
                self.draw_pulse_card(ui, s, &p, pulse_h);
                ui.add_space(gap);
                self.draw_crash_card(ui, s, &p, crash_h);
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
            // Both the titlebar X and the Close button close it. Never write the
            // Window's `open` straight back over our own Close click — that used to
            // undo the button (the old code assigned `open`, which was still true).
            let mut open = true;
            let mut close_clicked = false;
            let response = egui::Window::new(s.about_title)
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .open(&mut open).show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.about_version).size(11.0).color(p.muted));
                        ui.label(egui::RichText::new(env!("CARGO_PKG_VERSION")).size(11.0).strong().color(p.ink));
                    });
                    ui.add_space(6.0);
                    ui.label(s.about_body);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(s.about_github).size(11.0).color(p.muted));
                        ui.add(egui::Hyperlink::from_label_and_url(
                            "github.com/Michael-py001/guardian",
                            "https://github.com/Michael-py001/guardian",
                        ));
                    });
                    ui.add_space(8.0);
                    // Vertical centered layout keeps the button horizontally centred
                    // without the height inflation a right_to_left row caused.
                    ui.vertical_centered(|ui| {
                        if ui.button(s.close).clicked() { close_clicked = true; }
                    });
                });
            let _ = response;
            self.about_open = open && !close_clicked;
        }

        // Keep asking for frames even while hidden: this is the only thing that wakes
        // the event loop to run a viewport command sent from the tray thread. The idle
        // spin that costs CPU is eframe's own, not this call.
        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        engine::SHUTDOWN.store(true, Ordering::SeqCst);
    }
}

fn make_tray_icon() -> tray_icon::Icon {
    // Use the same shield logo embedded into the EXE (assets/guardian.rc, ID 1)
    // that Explorer and the taskbar show, so the tray matches everywhere.
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
        // The size Windows draws the tray at, not the 32 px default: loading a
        // bigger frame makes Windows shrink it, which blurs the logo.
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
        tray_icon::Icon::from_resource(1, Some((size, size)))
            .or_else(|_| tray_icon::Icon::from_resource(1, None))
            .expect("load embedded guardian icon for tray")
    }
    #[cfg(not(windows))]
    {
        let size = 32u32;
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for _ in 0..size * size { rgba.extend_from_slice(&[54, 132, 232, 255]); }
        tray_icon::Icon::from_rgba(rgba, size, size).expect("tray icon")
    }
}

/// Stable tray menu IDs so the background click handler can recognize each item.
const TRAY_SHOW: &str = "guardian.tray.show";
const TRAY_AUTOSTART: &str = "guardian.tray.autostart";
const TRAY_QUIT: &str = "guardian.tray.quit";

fn build_tray(lang: Lang) -> Option<TrayState> {
    let s = strings(lang);
    let menu = Menu::new();
    let show = MenuItem::with_id(TRAY_SHOW, s.tray_show, true, None);
    let auto_label = if engine::task_installed() { s.tray_autostart_on } else { s.tray_autostart_off };
    let autostart = MenuItem::with_id(TRAY_AUTOSTART, auto_label, true, None);
    let quit = MenuItem::with_id(TRAY_QUIT, s.tray_quit, true, None);
    menu.append(&show).ok()?; menu.append(&autostart).ok()?; menu.append(&quit).ok()?;
    let icon = TrayIconBuilder::new().with_tooltip(s.app_title).with_icon(make_tray_icon())
        .with_menu(Box::new(menu)).build().ok()?;
    Some(TrayState { _icon: icon, show, autostart, quit })
}

/// Handle tray clicks on the menu event thread, independent of egui's update loop.
/// `update()` stops running while the window is hidden in the tray, so acting there
/// makes "Show"/"Quit" appear dead. These actions touch only globals / Win32 calls.
fn install_tray_handler() {
    MenuEvent::set_event_handler(Some(move |event: tray_icon::menu::MenuEvent| {
        match event.id.as_ref() {
            TRAY_SHOW => show_main_window(),
            TRAY_AUTOSTART => {
                if engine::task_installed() {
                    let _ = engine::uninstall_task();
                } else {
                    let _ = engine::install_task(
                        &std::env::current_exe().unwrap_or_default(),
                        &config_path(),
                    );
                }
                TRAY_AUTOSTART_CHANGED.store(true, Ordering::SeqCst);
            }
            TRAY_QUIT => {
                engine::SHUTDOWN.store(true, Ordering::SeqCst);
                std::process::exit(0);
            }
            _ => {}
        }
    }));
}

/// Show or hide the window through egui's own viewport command, whether it comes
/// from `update()` or from the tray thread.
///
/// Both paths must use this one: `ViewportCommand::Visible` is what keeps winit's
/// `VISIBLE` flag in step with the real window. A raw `ShowWindow(SW_SHOW)` from the
/// tray left that flag false, so every later flag diff ran with `new` lacking
/// `VISIBLE` and finished by hiding the window — the X button looked dead after a
/// tray restore, and maximizing or minimizing made the window vanish.
fn set_window_visible(visible: bool) {
    WINDOW_HIDDEN.store(!visible, Ordering::SeqCst);
    if let Some(ctx) = MAIN_CTX.lock().ok().and_then(|c| c.clone()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
    }
    if visible {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOW};
            let hwnd = MAIN_HWND.load(Ordering::SeqCst);
            if hwnd != 0 { ShowWindow(hwnd, SW_SHOW); }
        }
    }
}

/// Bring the window back and focus it. Called from the tray thread.
#[cfg(windows)]
fn show_main_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    set_window_visible(true);
    let hwnd = MAIN_HWND.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe { SetForegroundWindow(hwnd) };
    }
}

#[cfg(not(windows))]
fn show_main_window() { set_window_visible(true); }

/// Keeps this process the only Guardian instance. Windows destroys the mutex when
/// the process exits, so a stale handle can never block a later launch.
#[cfg(windows)]
static INSTANCE_MUTEX: AtomicIsize = AtomicIsize::new(0);

/// Take the single-instance lock. Returns false when another Guardian already holds
/// it, in which case that instance is asked to show its window instead of a second
/// copy opening.
#[cfg(windows)]
fn claim_single_instance() -> bool {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    // Stable per-user name: the local session namespace keeps it out of other logins.
    let mut name: Vec<u16> = "Local\\GuardianSingleInstance\0".encode_utf16().collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_mut_ptr()) };
    if handle == 0 { return true; } // cannot create it: fail open, never block startup
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    INSTANCE_MUTEX.store(handle, Ordering::SeqCst);
    if already {
        // Hand off to the running copy: it may be sitting in the tray.
        wake_existing_instance();
        return false;
    }
    true
}

#[cfg(not(windows))]
fn claim_single_instance() -> bool { true }

/// Ask the already-running Guardian to come to the front. The tray often leaves it
/// hidden, so this looks for the other instance's window even when it is not visible
/// and restores it. Matching by process name avoids depending on the window title,
/// which changes with the language setting.
#[cfg(windows)]
fn wake_existing_instance() {
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    struct Search {
        me: u32,
        found: isize,
    }

    unsafe extern "system" fn visit(hwnd: isize, data: isize) -> i32 {
        let search = unsafe { &mut *(data as *mut Search) };
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != 0 && pid != search.me && same_process_name(pid) {
            search.found = hwnd;
            return 0; // the first match is the other Guardian; stop enumerating
        }
        1
    }

    let mut search = Search { me: unsafe { GetCurrentProcessId() }, found: 0 };
    unsafe {
        EnumWindows(Some(visit), &mut search as *mut Search as isize);
        if search.found != 0 {
            ShowWindow(search.found, SW_RESTORE);
            SetForegroundWindow(search.found);
        }
    }
}

/// Whether `pid` runs an executable with this process's own file name, so helper
/// windows of unrelated applications are never mistaken for Guardian.
#[cfg(windows)]
fn same_process_name(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    let me = match std::env::current_exe() {
        Ok(p) => p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default(),
        Err(_) => return false,
    };
    if me.is_empty() { return false; }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 { return false; }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 { return false; }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase() == me)
            .unwrap_or(false)
    }
}

/// Place the window in the middle of the primary monitor's work area.
///
/// With more than one monitor Windows picks a position itself, and that lands the
/// window on whichever screen it likes (observed on a secondary one). The work area
/// is used rather than the full monitor rect so the window never starts under the
/// taskbar.
#[cfg(windows)]
fn center_on_primary_screen(hwnd: isize) {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, SetWindowPos, SWP_NOSIZE, SWP_NOZORDER, SWP_NOACTIVATE,
    };
    use windows_sys::Win32::Foundation::POINT;

    let origin = POINT { x: 0, y: 0 };
    unsafe {
        let monitor = MonitorFromPoint(origin, MONITOR_DEFAULTTOPRIMARY);
        if monitor == 0 { return; }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 { return; }

        let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 { return; }
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + ((work.bottom - work.top) - h) / 2;
        SetWindowPos(hwnd, 0, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    if !claim_single_instance() { return Ok(()); }
    let st = load_settings();
    let title = strings(st.lang).app_title;
    let options = eframe::NativeOptions {
        // Logical points; keep it DPI-safe so all columns fit at 150% scaling.
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 620.0])
            .with_title(title)
            .with_icon(ui::window_icon())
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Guardian", options, Box::new(move |cc| {
        // Apply saved theme + CJK font before first frame.
        install_cjk_font(&cc.egui_ctx);
        ui::apply_visuals(&cc.egui_ctx, st.theme);
        // Remember the HWND so the tray callback thread can show/focus the window
        // even while egui's update loop is stalled (hidden in tray).
        #[cfg(windows)]
        if let Ok(handle) = cc.window_handle() {
            if let raw_window_handle::RawWindowHandle::Win32(w) = handle.as_raw() {
                MAIN_HWND.store(w.hwnd.get(), Ordering::SeqCst);
                center_on_primary_screen(w.hwnd.get() as isize);
            }
        }
        if let Ok(mut ctx) = MAIN_CTX.lock() { *ctx = Some(cc.egui_ctx.clone()); }
        install_tray_handler();
        let tray = build_tray(st.lang);
        let mut app = GuardianApp::new(tray);
        app.font_installed = true;
        Ok(Box::new(app))
    }))
}

#[cfg(test)]
mod close_tests {
    use super::{fill2, format_duration, format_hms, should_minimize_to_tray};

    #[test]
    fn duration_formats_match_prototype() {
        assert_eq!(format_duration(41), "41s");
        assert_eq!(format_duration(537), "8m 57s");
        assert_eq!(format_duration(14 * 3600 + 22 * 60), "14h 22m");
        assert_eq!(format_hms(161), "00:02:41");
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
