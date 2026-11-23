use anyhow::{Context, Result, bail};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const POLL_INTERVAL_MS: u64 = 250;
const TASK_NAME: &str = "Guardian";
const CONFIG_RELOAD_INTERVAL_MS: u64 = 1_000;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);
static CRASH_FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(windows)]
mod win {
    pub const PROCESS_TERMINATE: u32 = 0x0001;
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
    print!("{}", line);
    let _ = std::io::stdout().flush();
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
    Run { config_path: String },
    InstallTask { config_path: String },
    UninstallTask,
}

fn parse_action() -> Action {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--install-task") => Action::InstallTask {
            config_path: args.next().unwrap_or_else(|| "guardian.toml".to_string()),
        },
        Some("--uninstall-task") => Action::UninstallTask,
        Some(path) => Action::Run {
            config_path: path.to_string(),
        },
        None => Action::Run {
            config_path: "guardian.toml".to_string(),
        },
    }
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

#[cfg(windows)]
fn install_task(config_path: &str) -> Result<()> {
    if !Path::new(config_path).exists() {
        bail!("config file '{}' does not exist", config_path);
    }
    let exe = std::env::current_exe().context("failed to locate guardian.exe")?;
    let cfg_abs = std::path::absolute(config_path).context("failed to resolve config path")?;
    let task_run = format!("\"{}\" \"{}\"", exe.display(), cfg_abs.display());
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
                // Config changed if any tuning field differs (comparison by TOML value).
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

fn run(config_path: &str) -> Result<()> {
    let config = load_config(config_path)?;

    // Log + crash history paths live next to the config file.
    let cfg_dir = Path::new(config_path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let log_path = cfg_dir.join("guardian.log");
    let crash_path = cfg_dir.join("crash-history.jsonl");
    *LOG_FILE.lock().unwrap() = Some(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("failed to open log '{}'", log_path.display()))?,
    );
    *CRASH_FILE.lock().unwrap() = Some(crash_path.clone());

    install_ctrl_handler()?;

    let mut runtimes: Vec<Runtime> = config.process.iter().cloned().map(Runtime::new).collect();
    log(&format!(
        "guardian started, watching {} process(es). Ctrl+C to stop.",
        runtimes.len()
    ));
    log(&format!("log: {}", log_path.display()));
    log(&format!("crash history: {}", crash_path.display()));

    let poll = Duration::from_millis(POLL_INTERVAL_MS);
    let mut last_config_check = Instant::now();
    let mut last_config_mtime = fs::metadata(config_path).and_then(|m| m.modified()).ok();

    while !SHUTDOWN.load(Ordering::SeqCst) {
        // --- Hot reload: poll config mtime, apply changes in place ---
        if last_config_check.elapsed() >= Duration::from_millis(CONFIG_RELOAD_INTERVAL_MS) {
            last_config_check = Instant::now();
            let mtime = fs::metadata(config_path).and_then(|m| m.modified()).ok();
            if mtime != last_config_mtime {
                last_config_mtime = mtime;
                match load_config(config_path) {
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
                                // Terminate the old process so it restarts with new settings.
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
    Ok(())
}

fn main() -> Result<()> {
    match parse_action() {
        Action::Run { config_path } => run(&config_path),
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
    fn action_parsing() {
        let a = Action::Run {
            config_path: "x.toml".to_string(),
        };
        assert!(matches!(a, Action::Run { .. }));
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
}
