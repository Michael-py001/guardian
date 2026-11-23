// Hide the console window: this is a GUI application.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod engine;

use eframe::egui;
use engine::{CrashEntry, EntryStatus, ProcessConfig, RunningProcess, StatusSink, Watcher};
use rfd::FileDialog;
use tray_icon::{TrayIcon, TrayIconBuilder};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn config_path() -> PathBuf {
    exe_dir().join("guardian.toml")
}
fn crash_path() -> PathBuf {
    exe_dir().join("crash-history.jsonl")
}
fn log_path() -> PathBuf {
    exe_dir().join("guardian.log")
}

#[derive(Default)]
struct AddDialog {
    open: bool,
    tab: usize,
    name: String,
    command: String,
    args: String,
    processes: Vec<RunningProcess>,
    search: String,
    selected: Option<usize>,
}

/// Tray icon and its menu items, shared into the app by the creator closure.
struct TrayState {
    _icon: TrayIcon,
    show: MenuItem,
    autostart: MenuItem,
    quit: MenuItem,
}

fn make_tray_icon() -> tray_icon::Icon {
    // Simple 32x32 solid shield-ish icon generated in code (no asset file needed).
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let edge = x < 2 || y < 2 || x >= size - 2 || y >= size - 2;
            let (r, g, b, a) = if edge {
                (40, 44, 52, 255)
            } else {
                (64, 158, 255, 255)
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
    }
    tray_icon::Icon::from_rgba(rgba, size, size).expect("failed to build tray icon")
}

struct GuardianApp {
    configs: Vec<ProcessConfig>,
    statuses: Vec<EntryStatus>,
    crashes: Vec<CrashEntry>,
    sink: StatusSink,
    selected: Option<usize>,
    add: AddDialog,
    watcher_stop: Option<Arc<AtomicBool>>,
    autostart: bool,
    toast: Option<String>,
    last_crash_refresh: std::time::Instant,
    tray: Option<TrayState>,
}

impl GuardianApp {
    fn new(tray: Option<TrayState>) -> Self {
        let mut app = Self {
            configs: Vec::new(),
            statuses: Vec::new(),
            crashes: Vec::new(),
            sink: Arc::new(Mutex::new(Vec::new())),
            selected: None,
            add: AddDialog::default(),
            watcher_stop: None,
            autostart: engine::task_installed(),
            toast: None,
            last_crash_refresh: std::time::Instant::now(),
            tray,
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
                Err(e) => self.toast = Some(format!("config load failed: {}", e)),
            }
        }
        self.crashes = engine::read_crash_history(&crash_path());
    }

    fn persist(&mut self) {
        let config = engine::Config {
            process: self.configs.clone(),
        };
        if let Err(e) = engine::save_config(&config_path(), &config) {
            self.toast = Some(format!("save failed: {}", e));
        }
    }

    /// (Re)start the watcher with the current config.
    fn spawn_watcher(&mut self) {
        if let Some(stop) = &self.watcher_stop {
            stop.store(true, Ordering::SeqCst);
        }
        let _ = engine::init_logs(&log_path(), &crash_path());

        let config = engine::Config {
            process: self.configs.clone(),
        };
        let mut watcher = Watcher::new(&config, self.sink.clone());
        self.watcher_stop = Some(watcher.stop_handle());
        std::thread::spawn(move || watcher.run());
    }

    fn add_from_file(&mut self) {
        let name = if self.add.name.trim().is_empty() {
            PathBuf::from(&self.add.command)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "app".to_string())
        } else {
            self.add.name.trim().to_string()
        };
        let mut cfg = ProcessConfig::new(name, self.add.command.trim().to_string());
        if !self.add.args.trim().is_empty() {
            cfg.args = self
                .add
                .args
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
        }
        if let Err(e) = cfg.validate() {
            self.toast = Some(format!("invalid entry: {}", e));
            return;
        }
        self.configs.push(cfg);
        self.add.open = false;
        self.add.name.clear();
        self.add.command.clear();
        self.add.args.clear();
        self.persist();
        self.spawn_watcher();
    }

    fn add_from_process(&mut self) {
        let Some(idx) = self.add.selected else {
            self.toast = Some("select a process first".to_string());
            return;
        };
        let Some(proc) = self.add.processes.get(idx).cloned() else {
            return;
        };
        let name = PathBuf::from(&proc.name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| proc.name.clone());
        // Prefer the full image path so the process can be restarted even after
        // it closes; fall back to the bare name (resolved via PATH).
        let command = proc.path.clone().unwrap_or_else(|| proc.name.clone());
        let mut cfg = ProcessConfig::new(name, command);
        cfg.adopt = Some(proc.name);
        if let Err(e) = cfg.validate() {
            self.toast = Some(format!("invalid entry: {}", e));
            return;
        }
        self.configs.push(cfg);
        self.add.open = false;
        self.add.selected = None;
        self.persist();
        self.spawn_watcher();
    }

    fn remove_selected(&mut self) {
        if let Some(idx) = self.selected {
            if idx < self.configs.len() {
                self.configs.remove(idx);
            }
            self.selected = None;
            self.persist();
            self.spawn_watcher();
        }
    }

    fn toggle_autostart(&mut self) {
        let result = if self.autostart {
            engine::uninstall_task()
        } else {
            engine::install_task(&std::env::current_exe().unwrap_or_default(), &config_path())
        };
        match result {
            Ok(()) => self.autostart = engine::task_installed(),
            Err(e) => self.toast = Some(format!("autostart failed: {}", e)),
        }
    }
}

impl eframe::App for GuardianApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Process tray menu clicks. Copy the ids first so we don't hold a borrow
        // on self.tray while calling &mut self methods.
        let tray_ids = self.tray.as_ref().map(|t| {
            (
                t.show.id().clone(),
                t.autostart.id().clone(),
                t.quit.id().clone(),
            )
        });
        if let (Some(ids), Ok(event)) = (tray_ids, MenuEvent::receiver().try_recv()) {
            let (show_id, auto_id, quit_id) = ids;
            if event.id == show_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if event.id == auto_id {
                self.toggle_autostart();
                let label = if self.autostart {
                    "Disable autostart"
                } else {
                    "Enable autostart"
                };
                if let Some(tray) = &self.tray {
                    tray.autostart.set_text(label);
                }
            } else if event.id == quit_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Read the live status snapshot published by the watcher thread.
        if let Ok(s) = self.sink.lock() {
            self.statuses = s.clone();
        }
        if self.last_crash_refresh.elapsed() >= std::time::Duration::from_millis(500) {
            self.crashes = engine::read_crash_history(&crash_path());
            self.last_crash_refresh = std::time::Instant::now();
        }

        // Minimize to tray (hide window) on close button; keep guardian running.
        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Guardian — Process Watch");
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("Add entry").clicked() {
                    self.add.open = true;
                    self.add.processes = engine::list_running_processes();
                }
                if ui.button("Remove").clicked() {
                    self.remove_selected();
                }
                let label = if self.autostart {
                    "Autostart: ON"
                } else {
                    "Autostart: OFF"
                };
                if ui.button(label).clicked() {
                    self.toggle_autostart();
                }
                ui.label(format!("| {} watched", self.configs.len()));
            });

            if let Some(toast) = &self.toast {
                ui.colored_label(egui::Color32::YELLOW, toast);
            }

            ui.separator();

            egui::Grid::new("entries")
                .num_columns(5)
                .striped(true)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.strong("Name");
                    ui.strong("Status");
                    ui.strong("PID");
                    ui.strong("Restarts");
                    ui.strong("Last exit");
                    ui.end_row();

                    for (idx, cfg) in self.configs.iter().enumerate() {
                        let status = self.statuses.get(idx);
                        let is_sel = self.selected == Some(idx);
                        if ui.selectable_label(is_sel, &cfg.name).clicked() {
                            self.selected = Some(idx);
                        }
                        let running = status.map(|s| s.running).unwrap_or(false);
                        ui.colored_label(
                            if running {
                                egui::Color32::GREEN
                            } else {
                                egui::Color32::RED
                            },
                            if running { "running" } else { "stopped" },
                        );
                        ui.label(
                            status
                                .and_then(|s| s.pid)
                                .map(|p| p.to_string())
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label(
                            status
                                .map(|s| s.restarts.to_string())
                                .unwrap_or_else(|| "0".into()),
                        );
                        ui.label(
                            status
                                .and_then(|s| s.last_exit.clone())
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.end_row();
                    }
                });

            ui.separator();
            ui.collapsing("Crash history", |ui| {
                ui.label(format!("{} recorded exits", self.crashes.len()));
                for c in self.crashes.iter().rev().take(20) {
                    ui.label(format!(
                        "{}  {}  pid={}  exit={}  uptime={}s  restart#{}",
                        c.time, c.process, c.pid, c.exit_code, c.uptime_secs, c.restart_number
                    ));
                }
            });
        });

        if self.add.open {
            let mut open = self.add.open;
            egui::Window::new("Add process")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.add.tab, 0, "Choose file");
                        ui.selectable_value(&mut self.add.tab, 1, "Running process");
                    });
                    ui.separator();

                    if self.add.tab == 0 {
                        ui.horizontal(|ui| {
                            ui.label("Executable:");
                            ui.text_edit_singleline(&mut self.add.command);
                            if ui.button("Browse...").clicked() {
                                if let Some(path) = FileDialog::new().pick_file() {
                                    self.add.command = path.to_string_lossy().into_owned();
                                    if self.add.name.is_empty() {
                                        self.add.name = path
                                            .file_stem()
                                            .map(|s| s.to_string_lossy().into_owned())
                                            .unwrap_or_default();
                                    }
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Name:");
                            ui.text_edit_singleline(&mut self.add.name);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Args:");
                            ui.text_edit_singleline(&mut self.add.args);
                        });
                        if ui.button("Add").clicked() {
                            self.add_from_file();
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("Filter:");
                            ui.text_edit_singleline(&mut self.add.search);
                            if ui.button("Refresh").clicked() {
                                self.add.processes = engine::list_running_processes();
                            }
                        });
                        let filtered: Vec<(usize, &RunningProcess)> = self
                            .add
                            .processes
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| {
                                self.add.search.is_empty()
                                    || p.name
                                        .to_lowercase()
                                        .contains(&self.add.search.to_lowercase())
                            })
                            .collect();
                        egui::ScrollArea::vertical()
                            .max_height(260.0)
                            .show(ui, |ui| {
                                for (idx, proc) in filtered {
                                    let label = format!("{}  (pid {})", proc.name, proc.pid);
                                    if ui
                                        .selectable_label(self.add.selected == Some(idx), label)
                                        .clicked()
                                    {
                                        self.add.selected = Some(idx);
                                    }
                                }
                            });
                        if ui.button("Add selected").clicked() {
                            self.add_from_process();
                        }
                    }
                });
            self.add.open = open;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(300));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        engine::SHUTDOWN.store(true, Ordering::SeqCst);
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([780.0, 520.0])
            .with_title("Guardian — Process Watch"),
        ..Default::default()
    };
    eframe::run_native(
        "Guardian",
        options,
        Box::new(|_cc| {
            let tray = build_tray();
            Ok(Box::new(GuardianApp::new(tray)))
        }),
    )
}

fn build_tray() -> Option<TrayState> {
    let menu = Menu::new();
    let show = MenuItem::new("Show window", true, None);
    let autostart_label = if engine::task_installed() {
        "Disable autostart"
    } else {
        "Enable autostart"
    };
    let autostart = MenuItem::new(autostart_label, true, None);
    let quit = MenuItem::new("Exit", true, None);
    menu.append(&show).ok()?;
    menu.append(&autostart).ok()?;
    menu.append(&quit).ok()?;
    let icon = TrayIconBuilder::new()
        .with_tooltip("Guardian — Process Watch")
        .with_icon(make_tray_icon())
        .with_menu(Box::new(menu))
        .build()
        .ok()?;
    Some(TrayState {
        _icon: icon,
        show,
        autostart,
        quit,
    })
}
