# guardian

A tiny, config-driven process guardian for Windows. It launches the processes
you list in a TOML config, watches them, and restarts them if they crash — with
exponential backoff so a broken app doesn't spin your CPU.

Built for the case where a desktop app you rely on (e.g. BaoCode) gets taken
down by buggy injected DLLs from enterprise security software and you need it
to come back automatically during unattended runs. It is fully generic: point
it at any executable.

- Single native binary, ~500 KB, no runtime to install.
- Multiple processes in one config.
- Configurable backoff, max-restart cap, and "healthy uptime" reset.
- Ctrl+C / console-close gracefully terminates children.

## Quick start

```powershell
# 1. build (needs Rust + MSVC build tools)
cargo build --release

# 2. copy the example config and point it at your app
copy config.example.toml guardian.toml
# edit guardian.toml: set `command` to the full path of your exe

# 3. run
.\target\release\guardian.exe guardian.toml
```

With no argument it looks for `guardian.toml` in the current directory.

## Config reference

See [config.example.toml](config.example.toml). Every field:

| Field | Default | Meaning |
|---|---|---|
| `name` | required | Label used in log lines. |
| `command` | required | Executable path or name on PATH. |
| `args` | `[]` | Argument list. |
| `cwd` | inherit | Working directory. |
| `env` | `{}` | Extra environment variables. |
| `restart_delay_ms` | `1000` | First retry delay. |
| `max_restart_delay_ms` | `30000` | Retry delay ceiling. |
| `backoff_factor` | `2.0` | Delay multiplier after each fast death. |
| `max_restarts` | `-1` | Fast-restart cap; `-1` = unlimited, `0` = never. |
| `healthy_uptime_secs` | `60` | Surviving this long resets the backoff counter. |

Windows paths in TOML need doubled backslashes (`"C:\\Program Files\\..."`)
or single-quoted literal strings (`'C:\Program Files\...'`).

## How it behaves

For each process, guardian spawns it and polls every 250 ms. On exit:

- If it lived at least `healthy_uptime_secs`, it is considered healthy — the
  backoff counter resets (a long-running app that eventually closes is not a
  crash loop).
- Otherwise the restart count goes up and the next delay grows by
  `backoff_factor` until it hits `max_restart_delay_ms`.
- When `max_restarts` is reached (if set), guardian stops restarting that
  process and leaves it alone.
- A failed `spawn()` (bad path, permission error) is treated the same as a
  crash and retried with backoff.

On Ctrl+C or console close, guardian kills every child it still owns.

## Limitations / honest notes

- **It manages processes it starts.** If you want to drop in on an app that is
  *already running* (e.g. BaoCode is open when the guardian starts), that
  "adopt existing process" mode is not implemented — close the app first and
  let guardian own it, or ask for the mode.
- **It is not a Windows Service.** It runs as a console app. If you need it to
  survive logout or start at boot, that's a service wrapper (a future addition).
- **UniAccess caveat:** the injected DLL hooks `CreateProcessW`. Guardian also
  calls `CreateProcess` to spawn your app, so if the DLL crashes on a path with
  spaces, the guardian's own spawn could be affected. Its call surface is far
  smaller than a GUI app's, but the real fix is to add your app to UniAccess's
  whitelist. This tool is a safety net, not a cure.

## License

MIT — see [LICENSE](LICENSE).
