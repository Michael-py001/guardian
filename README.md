# guardian

[![build](https://github.com/Michael-py001/guardian/actions/workflows/build.yml/badge.svg)](https://github.com/Michael-py001/guardian/actions/workflows/build.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

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
- Adopt an already-running process instead of always spawning a new one.
- Register a logon task for auto-start (runs in your desktop session, GUI visible).
- File logging and an append-only JSONL crash history.
- Hot config reload — edit the TOML without restarting guardian.
- Optional native system-tray mode (`--tray`), no extra dependencies.
- `--history` command to summarize recorded crashes.
- Ctrl+C / console-close gracefully terminates children.

中文说明见下方 [中文文档](#中文文档)。

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

### Don't want to build?

Grab `guardian-windows-x86_64.zip` from the [Releases](https://github.com/Michael-py001/guardian/releases)
page — it contains the exe plus example config, README and license. (A Release is
published automatically when a `v*` tag is pushed.)

## Config reference

See [config.example.toml](config.example.toml). Every field:

| Field | Default | Meaning |
|---|---|---|
| `name` | required | Label used in log lines. |
| `command` | required | Executable path or name on PATH. |
| `adopt` | none | If set and a process with this image name is already running, adopt it instead of spawning (Windows, case-insensitive, e.g. `"baocode.exe"`). |
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

## Adopting an already-running process

By default guardian starts its own copy of each process. If the app is already
open when guardian starts, set `adopt` to its executable image name:

```toml
[[process]]
name = "baocode"
command = "C:\\Program Files\\BaoCode\\baocode.exe"
adopt = "baocode.exe"
```

Guardian will look for `baocode.exe` on startup. If found, it watches that
process (and restarts it via `command` if it dies). If not found, it starts a
fresh one. Adoption is attempted only once, at first start.

## Auto-start at logon (Windows)

Register a scheduled task that runs guardian when you log in:

```powershell
guardian.exe --install-task guardian.toml
# Remove it later:
guardian.exe --uninstall-task
```

This uses `schtasks /SC ONLOGON`, so guardian runs in your **interactive desktop
session** — any GUI app it launches has a visible window. The task starts guardian
in `--tray` mode, so no console window appears. It stores the absolute path of the
config at install time; re-run `--install-task` if you move the file.

A Windows Service is intentionally **not** used: services run in Session 0 and
GUI apps started there have no visible window on your desktop.

## System tray mode

By default guardian runs as a console app. Add `--tray` to run it as a tray program:

```powershell
guardian.exe --tray guardian.toml
```

No console window; an icon appears in the system tray. Right-click it for:

- **Status** — shows the config path and whether autostart is on.
- **Toggle autostart** — install/remove the logon task without a command prompt.
- **Exit** — stops guardian and terminates the processes it owns.

The tray icon is built with native Win32 APIs — no GUI framework or extra
dependency, so the binary stays small. Logging still goes to `guardian.log`.

## Logs and crash history

Two files are written next to the config file:

- `guardian.log` — the same lines you see on the console, with timestamps.
- `crash-history.jsonl` — one JSON object per process exit (append-only), useful
  for counting crashes over time or feeding into other tools:

```json
{"time":"2026-11-23 14:24:42","process":"baocode","pid":93140,"exit_code":"7","uptime_secs":0,"restart_number":1}
```

`uptime_secs` is how long the process lived; a healthy close has a large value,
a fast crash has a small one.

Summarize the history without opening the file:

```powershell
guardian.exe --history guardian.toml
# prints record count, per-process counts, and the last 10 exits
```

## Hot config reload

Guardian checks the config file every second. Edit `guardian.toml` and changes
apply automatically, no restart needed:

- **Process added** → started immediately.
- **Process removed** → terminated.
- **Settings changed** (command, args, backoff, etc.) → process terminated and
  restarted with the new settings.
- **Invalid edit** → logged and ignored; the previous config stays active.

Processes are matched by `name`, so keep names stable when editing.

## How it behaves

For each process, guardian spawns it (or adopts it) and polls every 250 ms. On exit:

- If it lived at least `healthy_uptime_secs`, it is considered healthy — the
  backoff counter resets (a long-running app that eventually closes is not a
  crash loop).
- Otherwise the restart count goes up and the next delay grows by
  `backoff_factor` until it hits `max_restart_delay_ms`.
- When `max_restarts` is reached (if set), guardian stops restarting that
  process and leaves it alone.
- A failed `spawn()` (bad path, permission error) is treated the same as a
  crash and retried with backoff.
- An adopted process is treated as healthy on its first observed exit (its real
  age is unknown) so backoff is not punished for something guardian didn't witness.

On Ctrl+C or console close, guardian terminates every process it still owns,
including adopted ones.

## Limitations / honest notes

- **Adoption matches the exe image name**, not a full path or arguments. Two
  processes with the same image name are ambiguous — guardian adopts the first.
- **It is not a Windows Service** (see Auto-start above for why).
- **UniAccess caveat:** the injected DLL hooks `CreateProcessW`. Guardian also
  calls `CreateProcess` to spawn your app, so if the DLL crashes on a path with
  spaces, the guardian's own spawn could be affected. Its call surface is far
  smaller than a GUI app's, but the real fix is to add your app to UniAccess's
  whitelist. This tool is a safety net, not a cure.

## License

MIT — see [LICENSE](LICENSE).

---

## 中文文档

一个轻量的、配置驱动的 Windows 进程守护工具。在 TOML 配置里列出要守护的程序，
它负责启动、监控，并在崩溃时按指数退避自动重启——避免坏掉的程序空转烧 CPU。

起源：你依赖的桌面程序（如 BaoCode）被企业安全软件注入的 DLL 搞崩，无人值守时
需要它自动恢复。本工具完全通用，可指向任意可执行文件。

- 单个原生 exe，约 500 KB，无需运行时。
- 一个配置可守护多个进程。
- 可配置退避、最大重启次数、"健康存活时间"重置。
- 支持接管已在运行的进程，而不总是新起一个。
- 可注册登录自启计划任务（运行在桌面会话，GUI 窗口可见）。
- 日志落盘 + 追加式 JSONL 崩溃历史。
- 配置热重载——改 TOML 不用重启。
- 可选原生系统托盘模式（`--tray`），无额外依赖。
- `--history` 命令汇总历史崩溃。
- Ctrl+C / 关闭控制台时优雅终止子进程。

### 快速开始

```powershell
# 1. 编译（需要 Rust + MSVC 构建工具）
cargo build --release

# 2. 复制示例配置，改成你自己的程序路径
copy config.example.toml guardian.toml
# 编辑 guardian.toml，把 command 设为 exe 的完整路径

# 3. 运行
.\target\release\guardian.exe guardian.toml
```

不带参数时默认读取当前目录的 `guardian.toml`。

不想自己编译？到 [Releases](https://github.com/Michael-py001/guardian/releases)
下载 `guardian-windows-x86_64.zip`，内含 exe、示例配置、README 和许可证
（推送 `v*` 标签时自动发布）。

### 配置字段

见 [config.example.toml](config.example.toml)。字段说明：

| 字段 | 默认值 | 含义 |
|---|---|---|
| `name` | 必填 | 日志里显示的名字。 |
| `command` | 必填 | 可执行文件路径或 PATH 中的名字。 |
| `adopt` | 无 | 若设置了且同名进程已在运行，则接管它而不新起（Windows，不区分大小写，如 `"baocode.exe"`）。 |
| `args` | `[]` | 参数列表。 |
| `cwd` | 继承 | 工作目录。 |
| `env` | `{}` | 额外环境变量。 |
| `restart_delay_ms` | `1000` | 首次重试延迟。 |
| `max_restart_delay_ms` | `30000` | 重试延迟上限。 |
| `backoff_factor` | `2.0` | 每次快速崩溃后的延迟倍数。 |
| `max_restarts` | `-1` | 快速重启上限；`-1` 无限，`0` 从不。 |
| `healthy_uptime_secs` | `60` | 存活超过此时长会重置退避计数。 |

TOML 中 Windows 路径需用双反斜杠（`"...\\Program Files\\..."`）
或单引号字面量（`'C:\Program Files\...'`）。

### 接管已运行进程

默认 guardian 会自己新起进程。如果启动时程序已经开着，设置 `adopt` 为它的
exe 映像名即可接管：

```toml
[[process]]
name = "baocode"
command = "C:\\Program Files\\BaoCode\\baocode.exe"
adopt = "baocode.exe"
```

guardian 启动时会查找 `baocode.exe`：找到就接管（死了再用 `command` 重启）；
没找到就新起一个。接管只在首次启动时尝试一次。

### 登录自启（Windows）

注册登录时自动运行的计划任务：

```powershell
guardian.exe --install-task guardian.toml
# 之后卸载：
guardian.exe --uninstall-task
```

它使用 `schtasks /SC ONLOGON`，因此 guardian 运行在**交互式桌面会话**，
它启动的 GUI 程序窗口可见。计划任务以 `--tray` 模式启动 guardian，所以不会
弹出控制台窗口。安装时记录的是配置的绝对路径，移动文件后需重新执行
`--install-task`。

故意不做成 Windows 服务：服务运行在 Session 0，在那里启动的 GUI 程序在桌面
上看不到窗口。

### 系统托盘模式

默认 guardian 是控制台程序。加 `--tray` 即以托盘程序方式运行：

```powershell
guardian.exe --tray guardian.toml
```

无控制台窗口，托盘出现图标。右键菜单：

- **Status** —— 显示配置路径和自启是否开启。
- **Toggle autostart** —— 无需命令行即可安装/卸载登录任务。
- **Exit** —— 停止 guardian 并终止它持有的进程。

托盘图标用原生 Win32 API 实现，无 GUI 框架或额外依赖，二进制保持小巧。
日志照常写入 `guardian.log`。

### 日志与崩溃历史

配置文件所在目录会生成两个文件：

- `guardian.log` —— 与控制台相同的带时间戳日志。
- `crash-history.jsonl` —— 每次进程退出追加一条 JSON（只追加不覆盖），便于
  统计历史崩溃次数或交给其他工具分析：

```json
{"time":"2026-11-23 14:24:42","process":"baocode","pid":93140,"exit_code":"7","uptime_secs":0,"restart_number":1}
```

`uptime_secs` 是进程存活时长：正常关闭是大值，快速崩溃是小值。

不用打开文件即可汇总历史：

```powershell
guardian.exe --history guardian.toml
# 输出记录总数、各进程次数、最近 10 次退出
```

### 配置热重载

guardian 每秒检查一次配置文件。编辑 `guardian.toml` 后自动生效，无需重启：

- **新增进程** → 立即启动。
- **删除进程** → 终止。
- **设置变更**（command、args、退避等）→ 终止旧进程并用新设置重启。
- **改错了** → 记录日志并忽略，继续使用旧配置。

进程按 `name` 匹配，改配置时请保持名字稳定。

### 行为

guardian 每 250 ms 轮询一次。进程退出时：

- 若存活达到 `healthy_uptime_secs`，视为健康——重置退避计数（长时间运行后正常
  关闭的程序不算崩溃循环）。
- 否则重启次数加一，下次延迟乘以 `backoff_factor`，直到 `max_restart_delay_ms`。
- 若设置了 `max_restarts` 并达到上限，停止重启该进程。
- `spawn()` 失败（路径错误、权限问题）按崩溃处理，同样退避重试。
- 被接管的进程首次退出时按健康处理（真实存活时长未知），避免误伤退避。

Ctrl+C 或关闭控制台时，guardian 会终止它持有的所有进程，包括被接管的。

### 局限与说明

- **接管是按 exe 映像名匹配的**，不是完整路径或参数。两个同名进程无法区分——
  guardian 接管第一个。
- **不是 Windows 服务**（原因见"登录自启"）。
- **UniAccess 注意**：注入的 DLL 挂钩了 `CreateProcessW`。guardian 也调用
  `CreateProcess` 启动程序，所以若该 DLL 对带空格路径必崩，guardian 自身启动
  也可能受影响；但它的调用面比 GUI 程序小得多。真正的解法仍是把程序加入
  UniAccess 白名单。本工具是兜底，不是根治。

### 许可证

MIT — 见 [LICENSE](LICENSE)。
