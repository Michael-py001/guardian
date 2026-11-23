use anyhow::{bail, Context, Result};
use chrono::Local;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const POLL_INTERVAL_MS: u64 = 250;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

#[cfg(windows)]
unsafe extern "system" fn console_ctrl_handler(_ctrl_type: u32) -> i32 {
    SHUTDOWN.store(true, Ordering::SeqCst);
    1
}

#[cfg(windows)]
extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
}

#[cfg(windows)]
fn install_ctrl_handler() -> Result<()> {
    if unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), 1) } == 0 {
        bail!("failed to install console control handler");
    }
    Ok(())
}

#[cfg(not(windows))]
fn install_ctrl_handler() -> Result<()> {
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default)]
    process: Vec<ProcessConfig>,
}

#[derive(Debug, Deserialize)]
struct ProcessConfig {
    name: String,
    command: String,
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

struct Runtime {
    cfg: ProcessConfig,
    child: Option<Child>,
    started_at: Option<Instant>,
    pending_start_at: Option<Instant>,
    delay_ms: u64,
    restarts: i64,
    stopped: bool,
}

fn log(msg: &str) {
    println!("[{}] {}", Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
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
        }
    }
}

fn spawn(rt: &mut Runtime) {
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
            log(&format!(
                "[{}] started (pid {})",
                rt.cfg.name,
                child.id()
            ));
            rt.child = Some(child);
            rt.started_at = Some(Instant::now());
            rt.pending_start_at = None;
        }
        Err(e) => {
            log(&format!("[{}] failed to start: {}", rt.cfg.name, e));
            on_exited(rt, None);
        }
    }
}

fn on_exited(rt: &mut Runtime, status: Option<i32>) {
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

    let healthy = uptime >= Duration::from_secs(rt.cfg.healthy_uptime_secs);
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

fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "guardian.toml".to_string());
    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read config '{}'", config_path))?;
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("failed to parse config '{}'", config_path))?;

    if config.process.is_empty() {
        bail!("config has no [[process]] entries");
    }
    for p in &config.process {
        p.validate()?;
    }

    install_ctrl_handler()?;

    let mut runtimes: Vec<Runtime> = config.process.into_iter().map(Runtime::new).collect();
    log(&format!(
        "guardian started, watching {} process(es). Ctrl+C to stop.",
        runtimes.len()
    ));

    let poll = Duration::from_millis(POLL_INTERVAL_MS);
    while !SHUTDOWN.load(Ordering::SeqCst) {
        for rt in runtimes.iter_mut() {
            if rt.stopped {
                continue;
            }

            if let Some(mut child) = rt.child.take() {
                match child.try_wait() {
                    Ok(Some(status)) => on_exited(rt, status.code()),
                    Ok(None) => rt.child = Some(child),
                    Err(e) => {
                        log(&format!("[{}] try_wait error: {}", rt.cfg.name, e));
                        on_exited(rt, None);
                    }
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
        if let Some(mut child) = rt.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            log(&format!("[{}] terminated", rt.cfg.name));
        }
    }
    log("guardian stopped.");
    Ok(())
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
        assert_eq!(a.args, vec!["--flag"]);
        assert_eq!(a.restart_delay_ms, 1000);
        assert_eq!(a.max_restart_delay_ms, 30_000);
        assert_eq!(a.backoff_factor, 2.0);
        assert_eq!(a.max_restarts, -1);
        assert_eq!(a.healthy_uptime_secs, 60);
        let b = &cfg.process[1];
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
}
