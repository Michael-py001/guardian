use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::{Duration, Instant};

const POLL_INTERVAL_MS: u64 = 250;
const TASK_NAME: &str = "Guardian";
const CONFIG_RELOAD_INTERVAL_MS: u64 = 1_000;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static QUIET_CONSOLE: AtomicBool = AtomicBool::new(false);
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);
static CRASH_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);
static TRAY_CONFIG_PATH: Mutex<Option<String>> = Mutex::new(None);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);

#[cfg(windows)]
const WM_APP_TRAY: u32 = 0x8000 + 1;
#[cfg(windows)]
const WM_QUIT: u32 = 0x0012;
#[cfg(windows)]
const WM_DESTROY: u32 = 0x0002;
#[cfg(windows)]
const WM_COMMAND: u32 = 0x0111;
#[cfg(windows)]
const WM_RBUTTONUP: u32 = 0x0205;
#[cfg(windows)]
const WM_LBUTTONDBLCLK: u32 = 0x0203;

#[cfg(windows)]
mod win {
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
    pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    pub const WAIT_OBJECT_0: u32 = 0;
    pub const INVALID_HANDLE_VALUE: isize = -1;
    pub const NIF_MESSAGE: u32 = 0x1;
    pub const NIF_ICON: u32 = 0x2;
    pub const NIF_TIP: u32 = 0x4;
    pub const NIM_ADD: u32 = 0;
    pub const NIM_DELETE: u32 = 2;

    #[repr(C)]
    pub struct ProcessEntry32W {
        pub dw_size: u32,
        pub cnt_usage: u32,
        pub th32_process_id: u32,
        pub th32_default_heap_id: usize,
        pub th32_module_id: u32,
        pub cnt_threads: u32,
        pub th32_parent_process_id: u32,
        pub pc_pri_class_base: i32,
        pub dw_flags: u32,
        pub sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    pub struct Msg {
        pub hwnd: isize,
        pub message: u32,
        pub w_param: usize,
        pub l_param: isize,
        pub time: u32,
        pub pt: Point,
    }

    #[repr(C)]
    pub struct Guid {
        pub data1: u32,
        pub data2: u16,
        pub data3: u16,
        pub data4: [u8; 8],
    }

    #[repr(C)]
    pub struct NotifyIconDataW {
        pub cb_size: u32,
        pub hwnd: isize,
        pub u_id: u32,
        pub u_flags: u32,
        pub u_callback_message: u32,
        pub h_icon: isize,
        pub sz_tip: [u16; 128],
        pub dw_state: u32,
        pub dw_state_mask: u32,
        pub sz_info: [u16; 256],
        pub u_timeout_or_version: u32,
        pub sz_info_title: [u16; 64],
        pub dw_info_flags: u32,
        pub guid_item: Guid,
        pub h_balloon_icon: isize,
    }

    pub type WndProc = unsafe extern "system" fn(isize, u32, usize, isize) -> isize;

    #[repr(C)]
    pub struct WndClassW {
        pub style: u32,
        pub lpfn_wnd_proc: Option<WndProc>,
        pub cb_cls_extra: i32,
        pub cb_wnd_extra: i32,
        pub h_instance: isize,
        pub h_icon: isize,
        pub h_cursor: isize,
        pub hbr_background: isize,
        pub lpsz_menu_name: *const u16,
        pub lpsz_class_name: *const u16,
    }

    #[link(name = "user32")]
    #[link(name = "shell32")]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        pub fn Process32FirstW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        pub fn Process32NextW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        pub fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> isize;
        pub fn CloseHandle(h_object: isize) -> i32;
        pub fn WaitForSingleObject(h_handle: isize, dw_milliseconds: u32) -> u32;
        pub fn TerminateProcess(h_process: isize, u_exit_code: u32) -> i32;
        pub fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
        pub fn FreeConsole() -> i32;
        pub fn RegisterClassW(lp_wnd_class: *const WndClassW) -> u16;
        pub fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            hwnd_parent: isize,
            h_menu: isize,
            h_instance: isize,
            lp_param: *const std::ffi::c_void,
        ) -> isize;
        pub fn DestroyWindow(h_wnd: isize) -> i32;
        pub fn GetMessageW(lp_msg: *mut Msg, h_wnd: isize, msg_filter_min: u32, msg_filter_max: u32) -> i32;
        pub fn TranslateMessage(lp_msg: *const Msg) -> i32;
        pub fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        pub fn DefWindowProcW(h_wnd: isize, msg: u32, w_param: usize, l_param: isize) -> isize;
        pub fn PostMessageW(h_wnd: isize, msg: u32, w_param: usize, l_param: isize) -> i32;
        pub fn PostQuitMessage(exit_code: i32);
        pub fn LoadIconW(h_instance: isize, lp_icon_name: *const u16) -> isize;
        pub fn Shell_NotifyIconW(dw_message: u32, lp_data: *mut NotifyIconDataW) -> i32;
        pub fn CreatePopupMenu() -> isize;
        pub fn AppendMenuW(h_menu: isize, u_flags: u32, u_id_new_item: usize, lp_new_item: *const u16) -> i32;
        pub fn DestroyMenu(h_menu: isize) -> i32;
        pub fn GetCursorPos(lp_point: *mut Point) -> i32;
        pub fn SetForegroundWindow(h_wnd: isize) -> i32;
        pub fn TrackPopupMenu(
            h_menu: isize,
            u_flags: u32,
            x: i32,
            y: i32,
            n_reserved: i32,
            h_wnd: isize,
            lp_rect: *const std::ffi::c_void,
        ) -> i32;
        pub fn MessageBoxW(h_wnd: isize, lp_text: *const u16, lp_caption: *const u16, u_type: u32) -> i32;
    }
}

#[cfg(windows)]
unsafe extern "system" fn console_ctrl_handler(_ctrl_type: u32) -> i32 {
    SHUTDOWN.store(true, Ordering::SeqCst);
    1
}

#[cfg(windows)]
fn install_ctrl_handler() -> Result<()> {
    if unsafe { win::SetConsoleCtrlHandler(Some(console_ctrl_handler), 1) } == 0 {
        bail!("failed to install console control handler");
    }
    Ok(())
}

#[cfg(not(windows))]
fn install_ctrl_handler() -> Result<()> {
    Ok(())
}

fn log(msg: &str) {
    let line = format!("[{}] {}\n", Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    if !QUIET_CONSOLE.load(Ordering::SeqCst) {
        print!("{}", line);
        let _ = std::io::stdout().flush();
    }
    if let Ok(mut guard) = LOG_FILE.lock()
        && let Some(f) = guard.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

/// Append one JSON line to the crash history file.
fn record_crash(entry: &CrashEntry) {
    let Some(path) = CRASH_FILE.lock().ok().and_then(|g| g.clone()) else {
        return;
    };
    let mut line = match serde_json::to_string(entry) {
        Ok(l) => l,
        Err(_) => return,
    };
    line.push('\n');
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CrashEntry {
    time: String,
    process: String,
    pid: u32,
    exit_code: String,
    uptime_secs: u64,
    restart_number: i64,
}

#[derive(Debug, Deserialize, Clone)]
struct Config {
    #[serde(default)]
    process: Vec<ProcessConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ProcessConfig {
    name: String,
    command: String,
    /// If set and a process with this image name is already running, adopt it
    /// instead of spawning a new one. On Windows this is an exe file name like
    /// "baocode.exe" (case-insensitive).
    #[serde(default)]
    adopt: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default = "default_restart_delay_ms")]
    restart_delay_ms: u64,
    #[serde(default = "default_max_restart_delay_ms")]
    max_restart_delay_ms: u64,
    #[serde(default = "default_backoff_factor")]
    backoff_factor: f64,
    #[serde(default = "default_max_restarts")]
    max_restarts: i64,
    #[serde(default = "default_healthy_uptime_secs")]
    healthy_uptime_secs: u64,
}

fn default_restart_delay_ms() -> u64 {
    1_000
}
fn default_max_restart_delay_ms() -> u64 {
    30_000
}
fn default_backoff_factor() -> f64 {
    2.0
}
fn default_max_restarts() -> i64 {
    -1
}
fn default_healthy_uptime_secs() -> u64 {
    60
}

impl ProcessConfig {
    fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("process name must not be empty");
        }
        if self.command.trim().is_empty() {
            bail!("process '{}': command must not be empty", self.name);
        }
        if let Some(adopt) = &self.adopt {
            if adopt.trim().is_empty() {
                bail!("process '{}': adopt must not be empty", self.name);
            }
        }
        if !self.backoff_factor.is_finite() || self.backoff_factor <= 0.0 {
            bail!("process '{}': backoff_factor must be a positive number", self.name);
        }
        if self.max_restart_delay_ms < self.restart_delay_ms {
            bail!(
                "process '{}': max_restart_delay_ms must be >= restart_delay_ms",
                self.name
            );
        }
        Ok(())
    }
}

/// A process we are watching: either one we spawned (std Child) or one that was
/// already running when guardian started (raw Windows handle).
enum Watched {
    Owned(Child),
    Adopted { handle: isize, pid: u32 },
}

impl Watched {
    fn pid(&self) -> u32 {
        match self {
            Watched::Owned(child) => child.id(),
            Watched::Adopted { pid, .. } => *pid,
        }
    }

    /// None = still running. Some(code) = exited; code may be unknown for adopted processes.
    fn try_wait(&mut self) -> Option<Option<i32>> {
        match self {
            Watched::Owned(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            },
            Watched::Adopted { handle, .. } => {
                let r = unsafe { win::WaitForSingleObject(*handle, 0) };
                if r == win::WAIT_OBJECT_0 {
                    unsafe { win::CloseHandle(*handle) };
                    Some(None)
                } else {
                    None
                }
            }
        }
    }

    fn terminate(&mut self) {
        match self {
            Watched::Owned(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Watched::Adopted { handle, .. } => unsafe {
                let _ = win::TerminateProcess(*handle, 1);
                let _ = win::WaitForSingleObject(*handle, 3_000);
                let _ = win::CloseHandle(*handle);
            },
        }
    }
}

#[cfg(windows)]
fn find_process_by_name(name: &str) -> Result<Option<u32>> {
    unsafe {
        let snap = win::CreateToolhelp32Snapshot(win::TH32CS_SNAPPROCESS, 0);
        if snap == win::INVALID_HANDLE_VALUE {
            bail!("CreateToolhelp32Snapshot failed");
        }
        let mut entry: win::ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<win::ProcessEntry32W>() as u32;
        let mut found = None;
        let mut ok = win::Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry
                .sz_exe_file
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.sz_exe_file.len());
            let exe = String::from_utf16_lossy(&entry.sz_exe_file[..len]);
            if exe.eq_ignore_ascii_case(name) {
                found = Some(entry.th32_process_id);
                break;
            }
            ok = win::Process32NextW(snap, &mut entry);
        }
        win::CloseHandle(snap);
        Ok(found)
    }
}

#[cfg(not(windows))]
fn find_process_by_name(_name: &str) -> Result<Option<u32>> {
    Ok(None)
}

#[cfg(windows)]
fn open_process(pid: u32) -> Option<isize> {
    let handle = unsafe { win::OpenProcess(win::PROCESS_TERMINATE | win::SYNCHRONIZE, 0, pid) };
    (handle != 0).then_some(handle)
}

#[cfg(not(windows))]
fn open_process(_pid: u32) -> Option<isize> {
    None
}

struct Runtime {
    cfg: ProcessConfig,
    child: Option<Watched>,
    started_at: Option<Instant>,
    pending_start_at: Option<Instant>,
    delay_ms: u64,
    restarts: i64,
    stopped: bool,
    adopt_attempted: bool,
}

/// Compute the next backoff delay. Pure function so it can be tested in isolation.
fn next_delay(current_ms: u64, factor: f64, max_ms: u64) -> u64 {
    let grown = (current_ms as f64 * factor).ceil() as u64;
    grown.min(max_ms).max(1)
}

impl Runtime {
    fn new(cfg: ProcessConfig) -> Self {
        let delay_ms = cfg.restart_delay_ms;
        Self {
            cfg,
            child: None,
            started_at: None,
            pending_start_at: None,
            delay_ms,
            restarts: 0,
            stopped: false,
            adopt_attempted: false,
        }
    }
}

/// Try to adopt an already-running process. Returns true if adopted.
fn try_adopt(rt: &mut Runtime) -> bool {
    let Some(name) = rt.cfg.adopt.clone() else {
        return false;
    };
    match find_process_by_name(&name) {
        Ok(Some(pid)) => match open_process(pid) {
            Some(handle) => {
                log(&format!(
                    "[{}] adopted running {} (pid {})",
                    rt.cfg.name, name, pid
                ));
                rt.child = Some(Watched::Adopted { handle, pid });
                // Its real age is unknown; treat the first observed exit as healthy
                // so we don't punish backoff for something we didn't witness.
                rt.started_at = Some(
                    Instant::now()
                        .checked_sub(Duration::from_secs(rt.cfg.healthy_uptime_secs))
                        .unwrap_or_else(Instant::now),
                );
                rt.pending_start_at = None;
                true
            }
            None => {
                log(&format!(
                    "[{}] found {} (pid {}) but could not open it; starting new",
                    rt.cfg.name, name, pid
                ));
                false
            }
        },
        Ok(None) => {
            log(&format!(
                "[{}] no running {} found; starting new",
                rt.cfg.name, name
            ));
            false
        }
        Err(e) => {
            log(&format!(
                "[{}] adoption lookup failed ({}); starting new",
                rt.cfg.name, e
            ));
            false
        }
    }
}

fn spawn(rt: &mut Runtime) {
    if !rt.adopt_attempted {
        rt.adopt_attempted = true;
        if try_adopt(rt) {
            return;
        }
    }

    let mut cmd = Command::new(&rt.cfg.command);
    cmd.args(&rt.cfg.args);
    if let Some(cwd) = &rt.cfg.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &rt.cfg.env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            log(&format!("[{}] started (pid {})", rt.cfg.name, child.id()));
            rt.child = Some(Watched::Owned(child));
            rt.started_at = Some(Instant::now());
            rt.pending_start_at = None;
        }
        Err(e) => {
            log(&format!("[{}] failed to start: {}", rt.cfg.name, e));
            on_exited(rt, None, 0);
        }
    }
}

fn on_exited(rt: &mut Runtime, status: Option<i32>, pid: u32) {
    let uptime = rt
        .started_at
        .map(|t| t.elapsed())
        .unwrap_or_else(|| Duration::from_secs(0));
    let status_str = status
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    log(&format!(
        "[{}] exited ({}) after {}s",
        rt.cfg.name,
        status_str,
        uptime.as_secs()
    ));

    // Every exit is a crash-history record (including healthy closes, marked via
    // the healthy flag in the log). The history file is an append-only audit.
    let healthy = uptime >= Duration::from_secs(rt.cfg.healthy_uptime_secs);
    record_crash(&CrashEntry {
        time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        process: rt.cfg.name.clone(),
        pid,
        exit_code: status_str,
        uptime_secs: uptime.as_secs(),
        restart_number: rt.restarts + 1,
    });

    if healthy {
        log(&format!("[{}] was healthy, resetting backoff", rt.cfg.name));
        rt.delay_ms = rt.cfg.restart_delay_ms;
        rt.restarts = 0;
    } else {
        rt.restarts += 1;
        if rt.cfg.max_restarts >= 0 && rt.restarts > rt.cfg.max_restarts {
            rt.stopped = true;
            log(&format!(
                "[{}] reached max_restarts ({}), giving up",
                rt.cfg.name, rt.cfg.max_restarts
            ));
            return;
        }
    }
    schedule_restart(rt, !healthy);
}

fn schedule_restart(rt: &mut Runtime, advance_backoff: bool) {
    let this_delay = rt.delay_ms;
    rt.started_at = None;
    rt.pending_start_at = Some(Instant::now() + Duration::from_millis(this_delay));
    if advance_backoff {
        rt.delay_ms = next_delay(rt.delay_ms, rt.cfg.backoff_factor, rt.cfg.max_restart_delay_ms);
    }
    log(&format!(
        "[{}] restart #{} scheduled in {}ms (next backoff {}ms)",
        rt.cfg.name, rt.restarts, this_delay, rt.delay_ms
    ));
}

#[derive(Debug)]
enum Action {
    Run { config_path: String, tray: bool },
    History { config_path: String },
    InstallTask { config_path: String },
    UninstallTask,
}

fn parse_action() -> Action {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config_path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "guardian.toml".to_string());
    if args.iter().any(|a| a == "--install-task") {
        return Action::InstallTask { config_path };
    }
    if args.iter().any(|a| a == "--uninstall-task") {
        return Action::UninstallTask;
    }
    if args.iter().any(|a| a == "--history") {
        return Action::History { config_path };
    }
    let tray = args.iter().any(|a| a == "--tray");
    Action::Run { config_path, tray }
}

fn load_config(config_path: &str) -> Result<Config> {
    let content = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config '{}'", config_path))?;
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("failed to parse config '{}'", config_path))?;
    if config.process.is_empty() {
        bail!("config has no [[process]] entries");
    }
    for p in &config.process {
        p.validate()?;
    }
    Ok(config)
}

fn config_dir(config_path: &str) -> PathBuf {
    Path::new(config_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn crash_history_path(config_path: &str) -> PathBuf {
    config_dir(config_path).join("crash-history.jsonl")
}

/// Print a summary of the crash history next to the given config.
fn show_history(config_path: &str) -> Result<()> {
    let path = crash_history_path(config_path);
    let content = fs::read_to_string(&path).with_context(|| {
        format!(
            "no crash history at '{}' (guardian has not recorded any exits yet)",
            path.display()
        )
    })?;
    let entries: Vec<CrashEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    println!("Crash history: {} ({} records)", path.display(), entries.len());

    let mut by_process: HashMap<&str, usize> = HashMap::new();
    for e in &entries {
        *by_process.entry(&e.process).or_insert(0) += 1;
    }
    let mut names: Vec<_> = by_process.iter().collect();
    names.sort_by(|a, b| b.1.cmp(a.1));
    println!("\nBy process:");
    for (name, count) in names {
        println!("  {:20} {}", name, count);
    }

    println!("\nLast 10 exits:");
    for e in entries.iter().rev().take(10) {
        println!(
            "  {}  {:16} pid={:<8} exit={:<8} uptime={}s restart#{}",
            e.time, e.process, e.pid, e.exit_code, e.uptime_secs, e.restart_number
        );
    }
    Ok(())
}

#[cfg(windows)]
fn query_task() -> bool {
    Command::new("schtasks.exe")
        .args(["/Query", "/TN", TASK_NAME])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn install_task(config_path: &str) -> Result<()> {
    if !Path::new(config_path).exists() {
        bail!("config file '{}' does not exist", config_path);
    }
    let exe = std::env::current_exe().context("failed to locate guardian.exe")?;
    let cfg_abs = std::path::absolute(config_path).context("failed to resolve config path")?;
    // Autostart launches guardian in tray mode so no console window appears.
    let task_run = format!("\"{}\" --tray \"{}\"", exe.display(), cfg_abs.display());
    let status = Command::new("schtasks.exe")
        .args(["/Create", "/TN", TASK_NAME, "/TR", &task_run, "/SC", "ONLOGON", "/F"])
        .status()
        .context("failed to run schtasks.exe")?;
    if !status.success() {
        bail!("schtasks /Create failed");
    }
    println!("installed logon task '{}': {}", TASK_NAME, task_run);
    Ok(())
}

#[cfg(windows)]
fn uninstall_task() -> Result<()> {
    let status = Command::new("schtasks.exe")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .status()
        .context("failed to run schtasks.exe")?;
    if !status.success() {
        bail!("schtasks /Delete failed");
    }
    println!("removed logon task '{}'", TASK_NAME);
    Ok(())
}

#[cfg(not(windows))]
fn install_task(_config_path: &str) -> Result<()> {
    bail!("scheduled tasks are only supported on Windows")
}

#[cfg(not(windows))]
fn uninstall_task() -> Result<()> {
    bail!("scheduled tasks are only supported on Windows")
}

// ---------------- tray UI (native Win32, no extra dependencies) ----------------

#[cfg(windows)]
fn message_box(text: &str) {
    let body: Vec<u16> = format!("{}\0", text).encode_utf16().collect();
    let caption: Vec<u16> = "Guardian\0".encode_utf16().collect();
    unsafe {
        win::MessageBoxW(0, body.as_ptr(), caption.as_ptr(), 0);
    }
}

#[cfg(windows)]
fn show_tray_status() {
    let cfg = TRAY_CONFIG_PATH
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "guardian.toml".to_string());
    let autostart = if query_task() { "on" } else { "off" };
    message_box(&format!(
        "Guardian is running.\nConfig: {}\nAutostart: {}\nSee guardian.log for details.",
        cfg, autostart
    ));
}

#[cfg(windows)]
fn toggle_autostart() {
    let cfg = TRAY_CONFIG_PATH
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_else(|| "guardian.toml".to_string());
    let was_on = query_task();
    let result = if was_on {
        uninstall_task()
    } else {
        install_task(&cfg)
    };
    match result {
        Ok(()) => {
            let now_on = query_task();
            message_box(&format!(
                "Autostart is now {}.",
                if now_on { "on" } else { "off" }
            ));
        }
        Err(e) => message_box(&format!("Failed to change autostart: {}", e)),
    }
}

#[cfg(windows)]
fn show_tray_menu(hwnd: isize) {
    unsafe {
        let menu = win::CreatePopupMenu();
        if menu == 0 {
            return;
        }
        for (text, id) in [("Status", 1usize), ("Toggle autostart", 2), ("Exit", 3)] {
            let wide: Vec<u16> = format!("{}\0", text).encode_utf16().collect();
            win::AppendMenuW(menu, 0, id, wide.as_ptr());
        }
        let mut pt = win::Point { x: 0, y: 0 };
        win::GetCursorPos(&mut pt);
        win::SetForegroundWindow(hwnd);
        win::TrackPopupMenu(menu, 2, pt.x, pt.y, 0, hwnd, std::ptr::null());
        win::DestroyMenu(menu);
    }
}

#[cfg(windows)]
unsafe extern "system" fn tray_wnd_proc(
    hwnd: isize,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    unsafe {
        match msg {
            WM_APP_TRAY => {
                match (lparam & 0xffff) as u32 {
                    WM_RBUTTONUP | WM_LBUTTONDBLCLK => show_tray_menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_COMMAND => {
                match (wparam & 0xffff) as u32 {
                    1 => show_tray_status(),
                    2 => toggle_autostart(),
                    3 => {
                        SHUTDOWN.store(true, Ordering::SeqCst);
                        win::PostMessageW(hwnd, WM_QUIT, 0, 0);
                    }
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                win::PostQuitMessage(0);
                0
            }
            _ => win::DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(windows)]
fn tray_message_loop() -> Result<()> {
    unsafe {
        let class_name: Vec<u16> = "GuardianTrayClass\0".encode_utf16().collect();
        let wc = win::WndClassW {
            style: 0,
            lpfn_wnd_proc: Some(tray_wnd_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: 0,
            h_icon: 0,
            h_cursor: 0,
            hbr_background: 0,
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: class_name.as_ptr(),
        };
        if win::RegisterClassW(&wc) == 0 {
            bail!("RegisterClassW failed");
        }
        let hwnd = win::CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            std::ptr::null(),
        );
        if hwnd == 0 {
            bail!("CreateWindowExW failed");
        }
        TRAY_HWND.store(hwnd, Ordering::SeqCst);

        let mut nid: win::NotifyIconDataW = std::mem::zeroed();
        nid.cb_size = std::mem::size_of::<win::NotifyIconDataW>() as u32;
        nid.hwnd = hwnd;
        nid.u_id = 1;
        nid.u_flags = win::NIF_MESSAGE | win::NIF_ICON | win::NIF_TIP;
        nid.u_callback_message = WM_APP_TRAY;
        nid.h_icon = win::LoadIconW(0, 32512usize as *const u16); // IDI_APPLICATION
        let tip: Vec<u16> = "Guardian\0".encode_utf16().collect();
        nid.sz_tip[..tip.len()].copy_from_slice(&tip);
        if win::Shell_NotifyIconW(win::NIM_ADD, &mut nid) == 0 {
            bail!("Shell_NotifyIconW failed");
        }
        log("tray icon active (right-click for menu)");

        let mut msg: win::Msg = std::mem::zeroed();
        while win::GetMessageW(&mut msg, 0, 0, 0) > 0 {
            win::TranslateMessage(&msg);
            win::DispatchMessageW(&msg);
        }

        win::Shell_NotifyIconW(win::NIM_DELETE, &mut nid);
        win::DestroyWindow(hwnd);
        TRAY_HWND.store(0, Ordering::SeqCst);
    }
    Ok(())
}

// ---------------- monitor loop ----------------

/// Ordered diff of old vs new config by process name. Returns (to_remove, to_add, to_update).
fn diff_config(
    old: &[Runtime],
    new: &Config,
) -> (Vec<String>, Vec<ProcessConfig>, Vec<(String, ProcessConfig)>) {
    let new_by_name: HashMap<&str, &ProcessConfig> =
        new.process.iter().map(|p| (p.name.as_str(), p)).collect();
    let old_names: HashMap<&str, &Runtime> =
        old.iter().map(|r| (r.cfg.name.as_str(), r)).collect();

    let mut to_remove = Vec::new();
    for r in old {
        if !new_by_name.contains_key(r.cfg.name.as_str()) {
            to_remove.push(r.cfg.name.clone());
        }
    }
    let mut to_add = Vec::new();
    let mut to_update = Vec::new();
    for p in &new.process {
        match old_names.get(p.name.as_str()) {
            None => to_add.push(p.clone()),
            Some(r) => {
                let old_val = toml::to_string(&r.cfg).unwrap_or_default();
                let new_val = toml::to_string(p).unwrap_or_default();
                if old_val != new_val {
                    to_update.push((p.name.clone(), p.clone()));
                }
            }
        }
    }
    (to_remove, to_add, to_update)
}

fn monitor_loop(mut runtimes: Vec<Runtime>, config_path: &str) {
    let poll = Duration::from_millis(POLL_INTERVAL_MS);
    let mut last_config_check = Instant::now();
    let mut last_config_mtime = fs::metadata(config_path).and_then(|m| m.modified()).ok();

    while !SHUTDOWN.load(Ordering::SeqCst) {
        // --- Hot reload: poll config mtime, apply changes in place ---
        if last_config_check.elapsed() >= Duration::from_millis(CONFIG_RELOAD_INTERVAL_MS) {
            last_config_check = Instant::now();
            let mtime = fs::metadata(&config_path).and_then(|m| m.modified()).ok();
            if mtime != last_config_mtime {
                last_config_mtime = mtime;
                match load_config(&config_path) {
                    Ok(new_config) => {
                        let (to_remove, to_add, to_update) = diff_config(&runtimes, &new_config);

                        for name in to_remove {
                            if let Some(pos) = runtimes.iter().position(|r| r.cfg.name == name) {
                                let mut rt = runtimes.remove(pos);
                                if let Some(mut w) = rt.child.take() {
                                    let pid = w.pid();
                                    w.terminate();
                                    log(&format!("[{}] config removed, terminated (pid {})", name, pid));
                                }
                            }
                        }
                        for (name, new_cfg) in to_update {
                            if let Some(rt) = runtimes.iter_mut().find(|r| r.cfg.name == name) {
                                rt.cfg = new_cfg;
                                rt.delay_ms = rt.cfg.restart_delay_ms;
                                rt.restarts = 0;
                                rt.stopped = false;
                                if let Some(mut w) = rt.child.take() {
                                    w.terminate();
                                }
                                rt.pending_start_at = Some(Instant::now());
                                rt.adopt_attempted = false;
                                log(&format!("[{}] config updated, restarting with new settings", name));
                            }
                        }
                        for cfg in to_add {
                            log(&format!("[{}] config added, starting", cfg.name));
                            runtimes.push(Runtime::new(cfg));
                        }
                    }
                    Err(e) => {
                        log(&format!("config reload failed ({}); keeping previous config", e));
                    }
                }
            }
        }

        for rt in runtimes.iter_mut() {
            if rt.stopped {
                continue;
            }

            if let Some(mut watched) = rt.child.take() {
                let pid = watched.pid();
                match watched.try_wait() {
                    Some(code) => on_exited(rt, code, pid),
                    None => rt.child = Some(watched),
                }
            } else if let Some(at) = rt.pending_start_at {
                if Instant::now() >= at {
                    spawn(rt);
                }
            } else {
                spawn(rt);
            }
        }
        std::thread::sleep(poll);
    }

    log("shutdown requested, terminating children...");
    for rt in runtimes.iter_mut() {
        if let Some(mut watched) = rt.child.take() {
            let pid = watched.pid();
            watched.terminate();
            log(&format!("[{}] terminated (pid {})", rt.cfg.name, pid));
        }
    }
    log("guardian stopped.");

    // In tray mode, wake the main thread's message loop so it can clean up.
    #[cfg(windows)]
    {
        let hwnd = TRAY_HWND.load(Ordering::SeqCst);
        if hwnd != 0 {
            unsafe {
                win::PostMessageW(hwnd, WM_QUIT, 0, 0);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = &TRAY_HWND;
}

fn run(config_path: &str, tray: bool) -> Result<()> {
    let config = load_config(config_path)?;

    // Capture the config path for hot reload (and for the tray menu).
    *TRAY_CONFIG_PATH.lock().unwrap() = Some(config_path.to_string());

    let cfg_dir = config_dir(config_path);
    let log_path = cfg_dir.join("guardian.log");
    let crash_path = crash_history_path(config_path);
    *LOG_FILE.lock().unwrap() = Some(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open log '{}'", log_path.display()))?,
    );
    *CRASH_FILE.lock().unwrap() = Some(crash_path.clone());

    install_ctrl_handler()?;

    let runtimes: Vec<Runtime> = config.process.iter().cloned().map(Runtime::new).collect();
    log(&format!(
        "guardian started, watching {} process(es). Ctrl+C to stop.",
        runtimes.len()
    ));
    log(&format!("log: {}", log_path.display()));
    log(&format!("crash history: {}", crash_path.display()));

    if tray {
        #[cfg(windows)]
        {
            let owned_path = config_path.to_string();
            let handle = std::thread::spawn(move || monitor_loop(runtimes, &owned_path));
            let tray_result = tray_message_loop();
            SHUTDOWN.store(true, Ordering::SeqCst);
            let _ = handle.join();
            tray_result?;
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            bail!("tray mode is only supported on Windows");
        }
    }

    monitor_loop(runtimes, config_path);
    Ok(())
}

fn main() -> Result<()> {
    match parse_action() {
        Action::Run { config_path, tray } => {
            if tray {
                #[cfg(windows)]
                {
                    QUIET_CONSOLE.store(true, Ordering::SeqCst);
                    unsafe {
                        win::FreeConsole();
                    }
                }
            }
            run(&config_path, tray)
        }
        Action::History { config_path } => show_history(&config_path),
        Action::InstallTask { config_path } => install_task(&config_path),
        Action::UninstallTask => uninstall_task(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(next_delay(1000, 2.0, 30_000), 2000);
        assert_eq!(next_delay(2000, 2.0, 30_000), 4000);
        assert_eq!(next_delay(20_000, 2.0, 30_000), 30_000);
        assert_eq!(next_delay(100, 1.5, 30_000), 150);
        assert_eq!(next_delay(0, 2.0, 30_000), 1);
    }

    #[test]
    fn config_defaults_and_values() {
        let toml = r#"
            [[process]]
            name = "baocode"
            command = "C:\\Program Files\\BaoCode\\baocode.exe"
            adopt = "baocode.exe"
            args = ["--flag"]

            [[process]]
            name = "other"
            command = "notepad.exe"
            restart_delay_ms = 500
            max_restarts = 3
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.process.len(), 2);
        let a = &cfg.process[0];
        assert_eq!(a.name, "baocode");
        assert_eq!(a.adopt.as_deref(), Some("baocode.exe"));
        assert_eq!(a.args, vec!["--flag"]);
        assert_eq!(a.restart_delay_ms, 1000);
        assert_eq!(a.max_restart_delay_ms, 30_000);
        assert_eq!(a.backoff_factor, 2.0);
        assert_eq!(a.max_restarts, -1);
        assert_eq!(a.healthy_uptime_secs, 60);
        let b = &cfg.process[1];
        assert_eq!(b.adopt, None);
        assert_eq!(b.restart_delay_ms, 500);
        assert_eq!(b.max_restarts, 3);
    }

    #[test]
    fn validation_rejects_bad_config() {
        let toml = r#"
            [[process]]
            name = ""
            command = "x.exe"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.process[0].validate().is_err());
    }

    #[test]
    fn action_parsing_shape() {
        let a = Action::Run {
            config_path: "x.toml".to_string(),
            tray: false,
        };
        assert!(matches!(a, Action::Run { tray: false, .. }));
        assert!(matches!(Action::UninstallTask, Action::UninstallTask));
    }

    #[test]
    fn diff_detects_changes() {
        let old_toml = r#"
            [[process]]
            name = "a"
            command = "a.exe"
            restart_delay_ms = 1000
            [[process]]
            name = "b"
            command = "b.exe"
        "#;
        let old_cfg: Config = toml::from_str(old_toml).unwrap();
        let old: Vec<Runtime> = old_cfg.process.iter().cloned().map(Runtime::new).collect();

        let new_toml = r#"
            [[process]]
            name = "a"
            command = "a.exe"
            restart_delay_ms = 5000
            [[process]]
            name = "c"
            command = "c.exe"
        "#;
        let new_cfg: Config = toml::from_str(new_toml).unwrap();
        let (remove, add, update) = diff_config(&old, &new_cfg);
        assert_eq!(remove, vec!["b"]);
        assert_eq!(add.len(), 1);
        assert_eq!(add[0].name, "c");
        assert_eq!(update.len(), 1);
        assert_eq!(update[0].0, "a");
    }

    #[test]
    fn crash_entry_json_roundtrip() {
        let entry = CrashEntry {
            time: "2026-11-23 14:24:42".to_string(),
            process: "baocode".to_string(),
            pid: 1234,
            exit_code: "7".to_string(),
            uptime_secs: 3,
            restart_number: 1,
        };
        let line = serde_json::to_string(&entry).unwrap();
        let back: CrashEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.process, "baocode");
        assert_eq!(back.pid, 1234);
        assert_eq!(back.restart_number, 1);
    }
}
