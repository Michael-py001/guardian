//! Core guardian engine: config model, process watching, crash history and autostart.
//! Kept free of any GUI code so it stays testable and reusable.

use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(windows)]
pub const TASK_NAME: &str = "Guardian";
pub const POLL_INTERVAL_MS: u64 = 250;

/// Set by the GUI to stop all watch threads on exit.
pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);
static CRASH_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn log(msg: &str) {
    let line = format!("[{}] {}\n", Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    if let Ok(mut guard) = LOG_FILE.lock()
        && let Some(f) = guard.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

pub fn init_logs(log_path: &Path, crash_path: &Path) -> Result<()> {
    *LOG_FILE.lock().unwrap() = Some(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("failed to open log '{}'", log_path.display()))?,
    );
    *CRASH_FILE.lock().unwrap() = Some(crash_path.to_path_buf());
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CrashEntry {
    pub time: String,
    pub process: String,
    pub pid: u32,
    pub exit_code: String,
    pub uptime_secs: u64,
    pub restart_number: i64,
}

fn record_crash(entry: &CrashEntry) {
    let Some(path) = CRASH_FILE.lock().ok().and_then(|g| g.clone()) else {
        return;
    };
    if let Ok(mut line) = serde_json::to_string(entry) {
        line.push('\n');
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

pub fn clear_crash_history(path: &Path) -> std::io::Result<()> {
    match std::fs::write(path, "") {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn read_crash_history(path: &Path) -> Vec<CrashEntry> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub process: Vec<ProcessConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ProcessConfig {
    pub name: String,
    pub command: String,
    /// exe image name to adopt if already running (Windows, case-insensitive)
    #[serde(default)]
    pub adopt: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,
    #[serde(default = "default_max_restart_delay_ms")]
    pub max_restart_delay_ms: u64,
    #[serde(default = "default_backoff_factor")]
    pub backoff_factor: f64,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: i64,
    #[serde(default = "default_healthy_uptime_secs")]
    pub healthy_uptime_secs: u64,
}

pub fn default_restart_delay_ms() -> u64 { 1_000 }
pub fn default_max_restart_delay_ms() -> u64 { 30_000 }
pub fn default_backoff_factor() -> f64 { 2.0 }
pub fn default_max_restarts() -> i64 { -1 }
pub fn default_healthy_uptime_secs() -> u64 { 60 }

impl ProcessConfig {
    pub fn new(name: String, command: String) -> Self {
        Self {
            name,
            command,
            adopt: None,
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            restart_delay_ms: default_restart_delay_ms(),
            max_restart_delay_ms: default_max_restart_delay_ms(),
            backoff_factor: default_backoff_factor(),
            max_restarts: default_max_restarts(),
            healthy_uptime_secs: default_healthy_uptime_secs(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            bail!("process name must not be empty");
        }
        if self.command.trim().is_empty() {
            bail!("process '{}': command must not be empty", self.name);
        }
        // Only require the path to exist when it looks like a path (contains a
        // separator or drive). Bare exe names are resolved via PATH at launch,
        // and adopted processes may only have an image name.
        let looks_like_path = self.command.contains('\\')
            || self.command.contains('/')
            || self.command.contains(':');
        if looks_like_path && !Path::new(&self.command).exists() {
            bail!("process '{}': executable not found: {}", self.name, self.command);
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

pub fn load_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config '{}'", path.display()))?;
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("failed to parse config '{}'", path.display()))?;
    for p in &config.process {
        p.validate()?;
    }
    Ok(config)
}

pub fn save_config(path: &Path, config: &Config) -> Result<()> {
    let content = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(path, content).with_context(|| format!("failed to write '{}'", path.display()))
}

/// Compute the next backoff delay. Pure function so it can be tested in isolation.
pub fn next_delay(current_ms: u64, factor: f64, max_ms: u64) -> u64 {
    let grown = (current_ms as f64 * factor).ceil() as u64;
    grown.min(max_ms).max(1)
}

// ---------------- Win32 helpers for process adoption / listing ----------------

#[cfg(windows)]
mod win {
    pub const PROCESS_TERMINATE: u32 = 0x0001;
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    pub const SYNCHRONIZE: u32 = 0x0010_0000;
    pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    pub const WAIT_OBJECT_0: u32 = 0;
    pub const INVALID_HANDLE_VALUE: isize = -1;

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

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn CreateToolhelp32Snapshot(dw_flags: u32, th32_process_id: u32) -> isize;
        pub fn Process32FirstW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        pub fn Process32NextW(h_snapshot: isize, lppe: *mut ProcessEntry32W) -> i32;
        pub fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        pub fn CloseHandle(h_object: isize) -> i32;
        pub fn WaitForSingleObject(h_handle: isize, dw_milliseconds: u32) -> u32;
        pub fn TerminateProcess(h_process: isize, u_exit_code: u32) -> i32;
        pub fn QueryFullProcessImageNameW(
            h_process: isize,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
    }
}

/// A running process shown in the "add from running processes" tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    /// Full path to the executable when available (used to restart it later).
    pub path: Option<String>,
}

/// Every running process instance, including duplicate image names.
#[cfg(windows)]
fn list_running_processes_raw() -> Vec<RunningProcess> {
    unsafe {
        let snap = win::CreateToolhelp32Snapshot(win::TH32CS_SNAPPROCESS, 0);
        if snap == win::INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        let mut entry: win::ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<win::ProcessEntry32W>() as u32;
        let mut out = Vec::new();
        let mut ok = win::Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let len = entry
                .sz_exe_file
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.sz_exe_file.len());
            let name = String::from_utf16_lossy(&entry.sz_exe_file[..len]);
            let path = process_image_path(entry.th32_process_id);
            out.push(RunningProcess {
                pid: entry.th32_process_id,
                name,
                path,
            });
            ok = win::Process32NextW(snap, &mut entry);
        }
        win::CloseHandle(snap);
        out
    }
}

/// List running processes (name + pid), sorted by name. Used by the GUI add dialog.
pub fn list_running_processes() -> Vec<RunningProcess> {
    #[cfg(windows)]
    {
        let mut out = list_running_processes_raw();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
        out
    }
    #[cfg(not(windows))]
    Vec::new()
}

#[cfg(windows)]
fn process_image_path(pid: u32) -> Option<String> {
    unsafe {
        let h = win::OpenProcess(win::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = win::QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        win::CloseHandle(h);
        if ok == 0 || size as usize > buf.len() {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..size as usize]))
    }
}

/// All PIDs whose image name matches. `list_running_processes` dedupes by name
/// for the picker UI, so we enumerate raw entries here to see every instance.
#[cfg(windows)]
fn find_process_by_name(name: &str) -> Vec<u32> {
    list_running_processes_raw()
        .into_iter()
        .filter(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.pid)
        .collect()
}

#[cfg(windows)]
fn open_process(pid: u32) -> Option<isize> {
    let handle = unsafe { win::OpenProcess(win::PROCESS_TERMINATE | win::SYNCHRONIZE, 0, pid) };
    (handle != 0).then_some(handle)
}

// ---------------- Runtime watcher ----------------

enum Watched {
    Owned(Child),
    #[cfg(windows)]
    Adopted { handle: isize, pid: u32 },
}

impl Watched {
    fn pid(&self) -> u32 {
        match self {
            Watched::Owned(child) => child.id(),
            #[cfg(windows)]
            Watched::Adopted { pid, .. } => *pid,
        }
    }

    /// None = still running. Some(code) = exited (code unknown for adopted processes).
    fn try_wait(&mut self) -> Option<Option<i32>> {
        match self {
            Watched::Owned(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            },
            #[cfg(windows)]
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
            #[cfg(windows)]
            Watched::Adopted { handle, .. } => unsafe {
                let _ = win::TerminateProcess(*handle, 1);
                let _ = win::WaitForSingleObject(*handle, 3_000);
                let _ = win::CloseHandle(*handle);
            },
        }
    }
}

/// Live status of one watched entry, shown in the GUI (indexed by config order).
#[derive(Debug, Clone)]
pub struct EntryStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub restarts: i64,
    pub last_exit: Option<String>,
    /// Seconds this process has been continuously running (0 when stopped).
    pub uptime_secs: u64,
    /// True while a restart is scheduled and the process is not running.
    pub restarting: bool,
}

struct Runtime {
    cfg: ProcessConfig,
    child: Option<Watched>,
    started_at: Option<Instant>,
    pending_start_at: Option<Instant>,
    delay_ms: u64,
    restarts: i64,
    stopped: bool,
    last_exit: Option<String>,
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
            last_exit: None,
        }
    }

    fn status(&self) -> EntryStatus {
        EntryStatus {
            running: self.child.is_some(),
            pid: self.child.as_ref().map(|c| c.pid()),
            restarts: self.restarts,
            last_exit: self.last_exit.clone(),
            uptime_secs: self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0),
            restarting: self.child.is_none() && self.pending_start_at.is_some(),
        }
    }
}

#[cfg(windows)]
fn try_adopt(rt: &mut Runtime) -> bool {
    let Some(name) = rt.cfg.adopt.clone() else {
        return false;
    };
    for pid in find_process_by_name(&name) {
        let Some(handle) = open_process(pid) else { continue };
        // A zombie still appears in the process list and OpenProcess can succeed,
        // but its handle signals immediately. Skip it and try the next instance.
        if unsafe { win::WaitForSingleObject(handle, 0) } == win::WAIT_OBJECT_0 {
            unsafe { win::CloseHandle(handle) };
            continue;
        }
        log(&format!("[{}] adopted running {} (pid {})", rt.cfg.name, name, pid));
        rt.child = Some(Watched::Adopted { handle, pid });
        rt.started_at = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(rt.cfg.healthy_uptime_secs))
                .unwrap_or_else(Instant::now),
        );
        rt.pending_start_at = None;
        return true;
    }
    false
}

/// Adoption is Windows-only; on macOS Guardian only guards processes it starts.
#[cfg(not(windows))]
fn try_adopt(_rt: &mut Runtime) -> bool { false }

fn spawn(rt: &mut Runtime) {
    // A single-instance app may appear only after Guardian starts, or may be
    // reopened by the user after a crash. Re-check on every spawn instead of
    // giving up after the first attempt.
    if try_adopt(rt) {
        return;
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
            // Show the last exit only while stopped; a healthy running process
            // should not display a stale exit message.
            rt.last_exit = None;
        }
        Err(e) => {
            log(&format!("[{}] failed to start: {}", rt.cfg.name, e));
            rt.last_exit = Some(format!("start failed: {}", e));
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
    rt.last_exit = Some(format!("exit {} after {}s", status_str, uptime.as_secs()));

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

/// Shared status snapshot: the watcher publishes here each poll; the GUI reads it.
pub type StatusSink = Arc<Mutex<Vec<EntryStatus>>>;

/// The watcher owns all running processes. Call `run()` on a background thread;
/// call `stop()` to terminate everything. Live status is published to the sink.
pub struct Watcher {
    runtimes: Vec<Runtime>,
    sink: StatusSink,
    stop: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    config_rx: std::sync::mpsc::Receiver<Config>,
}

/// Send a fresh config to a running watcher so it can add/remove entries in place.
pub type ConfigSender = std::sync::mpsc::Sender<Config>;

impl Watcher {
    pub fn new(config: &Config, sink: StatusSink) -> (Self, ConfigSender) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self {
            runtimes: config.process.iter().cloned().map(Runtime::new).collect(),
            sink,
            stop: Arc::new(AtomicBool::new(false)),
            pause: Arc::new(AtomicBool::new(false)),
            config_rx: rx,
        }, tx)
    }

    /// A handle that can stop this watcher from another thread.
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// A handle to pause/resume auto-restarting from another thread.
    pub fn pause_handle(&self) -> Arc<AtomicBool> {
        self.pause.clone()
    }

    fn publish_statuses(&self) {
        let statuses: Vec<EntryStatus> = self.runtimes.iter().map(|r| r.status()).collect();
        if let Ok(mut s) = self.sink.lock() {
            *s = statuses;
        }
    }

    /// Main loop. Call on a dedicated thread. Returns when this watcher's stop
    /// flag or the global SHUTDOWN is set.
    pub fn run(&mut self) {
        let poll = Duration::from_millis(POLL_INTERVAL_MS);
        while !self.stop.load(Ordering::SeqCst) && !SHUTDOWN.load(Ordering::SeqCst) {
            // Apply config updates in place so untouched processes keep running.
            while let Ok(config) = self.config_rx.try_recv() {
                self.apply_config(&config);
            }
            if self.pause.load(Ordering::SeqCst) {
                // While paused, still detect exits but do not auto-restart.
                for rt in self.runtimes.iter_mut() {
                    if let Some(mut watched) = rt.child.take() {
                        match watched.try_wait() {
                            Some(_) => { /* leave stopped; no restart */ }
                            None => rt.child = Some(watched),
                        }
                    }
                }
                self.publish_statuses();
                std::thread::sleep(poll);
                continue;
            }
            for rt in self.runtimes.iter_mut() {
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
            self.publish_statuses();
            std::thread::sleep(poll);
        }
        // Leaving the guarded processes running is deliberate: Guardian exiting
        // (including a crash) must not take down the apps it was watching.
        self.publish_statuses();
    }

    /// Reconcile the running entries against a new config. Entries whose command
    /// and name are unchanged keep their live process; only added/removed/changed
    /// ones are started or stopped.
    fn apply_config(&mut self, config: &Config) {
        // Terminate and drop entries no longer present, or whose identity changed.
        self.runtimes.retain_mut(|rt| {
            let still_wanted = config.process.iter().any(|c| {
                c.name == rt.cfg.name && c.command == rt.cfg.command
            });
            if still_wanted { return true; }
            if let Some(mut child) = rt.child.take() {
                let pid = child.pid();
                child.terminate();
                log(&format!("[{}] removed from guarding (pid {} terminated)", rt.cfg.name, pid));
            }
            false
        });

        // Add new entries not already watched. Untouched runtimes keep their live process.
        for c in &config.process {
            let exists = self.runtimes.iter().any(|rt| {
                rt.cfg.name == c.name && rt.cfg.command == c.command
            });
            if !exists {
                log(&format!("[{}] added to guarding", c.name));
                self.runtimes.push(Runtime::new(c.clone()));
            }
        }
    }
}

// ---------------- Autostart (Windows scheduled task) ----------------

/// A `schtasks.exe` command that never pops a console window. Without
/// `CREATE_NO_WINDOW` each status check flashes a terminal on startup.
#[cfg(windows)]
fn schtasks() -> Command {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let mut cmd = Command::new("schtasks.exe");
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(windows)]
pub fn task_installed() -> bool {
    schtasks()
        .args(["/Query", "/TN", TASK_NAME])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
pub fn task_installed() -> bool { false }

#[cfg(windows)]
pub fn install_task(exe: &Path, config: &Path) -> Result<()> {
    let task_run = format!("\"{}\" \"{}\"", exe.display(), config.display());
    let status = schtasks()
        .args(["/Create", "/TN", TASK_NAME, "/TR", &task_run, "/SC", "ONLOGON", "/F"])
        .status()
        .context("failed to run schtasks.exe")?;
    if !status.success() {
        bail!("schtasks /Create failed");
    }
    Ok(())
}

#[cfg(windows)]
pub fn uninstall_task() -> Result<()> {
    let status = schtasks()
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .status()
        .context("failed to run schtasks.exe")?;
    if !status.success() {
        bail!("schtasks /Delete failed");
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn install_task(_exe: &Path, _config: &Path) -> Result<()> { bail!("Windows only") }
#[cfg(not(windows))]
pub fn uninstall_task() -> Result<()> { bail!("Windows only") }

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
    fn config_defaults() {
        let toml = r#"
            [[process]]
            name = "app"
            command = "app.exe"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let p = &cfg.process[0];
        assert_eq!(p.restart_delay_ms, 1000);
        assert_eq!(p.max_restart_delay_ms, 30_000);
        assert_eq!(p.backoff_factor, 2.0);
        assert_eq!(p.max_restarts, -1);
        assert_eq!(p.healthy_uptime_secs, 60);
    }

    #[test]
    fn config_roundtrip() {
        let cfg = Config {
            process: vec![ProcessConfig::new("app".into(), "app.exe".into())],
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.process[0].name, "app");
    }

    #[test]
    fn apply_config_preserves_unchanged_entries() {
        let a = ProcessConfig::new("a".into(), "a.exe".into());
        let b = ProcessConfig::new("b".into(), "b.exe".into());
        let sink: StatusSink = Arc::new(Mutex::new(Vec::new()));
        let (mut watcher, _tx) = Watcher::new(&Config { process: vec![a.clone(), b.clone()] }, sink.clone());

        // Dropping 'b' must leave 'a' in place rather than rebuilding everything.
        watcher.apply_config(&Config { process: vec![a.clone()] });
        assert_eq!(watcher.runtimes.len(), 1);
        assert_eq!(watcher.runtimes[0].cfg.name, "a");

        // Adding 'c' keeps 'a' and appends 'c'.
        let c = ProcessConfig::new("c".into(), "c.exe".into());
        watcher.apply_config(&Config { process: vec![a.clone(), c.clone()] });
        assert_eq!(watcher.runtimes.len(), 2);
        assert_eq!(watcher.runtimes[0].cfg.name, "a");
        assert_eq!(watcher.runtimes[1].cfg.name, "c");
    }

    #[test]
    fn runtime_reports_real_uptime() {
        let mut rt = Runtime::new(ProcessConfig::new("app".into(), "app.exe".into()));
        assert_eq!(rt.status().uptime_secs, 0); // not started yet
        assert!(!rt.status().restarting);
        rt.pending_start_at = Some(Instant::now() + Duration::from_secs(1));
        assert!(rt.status().restarting); // scheduled, not running
        rt.started_at = Some(Instant::now() - Duration::from_secs(42));
        assert!(rt.status().uptime_secs >= 42);
    }

    #[test]
    fn crash_entry_roundtrip() {
        let e = CrashEntry {
            time: "2026-11-23 14:24:42".into(),
            process: "app".into(),
            pid: 1234,
            exit_code: "7".into(),
            uptime_secs: 3,
            restart_number: 1,
        };
        let line = serde_json::to_string(&e).unwrap();
        let back: CrashEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.process, "app");
        assert_eq!(back.pid, 1234);
    }
}
