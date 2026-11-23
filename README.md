# Guardian

[![build](https://github.com/Michael-py001/guardian/actions/workflows/build.yml/badge.svg)](https://github.com/Michael-py001/guardian/actions/workflows/build.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A small **visual** process guardian for Windows. It keeps the apps you choose
running: if one crashes, Guardian restarts it automatically with exponential
backoff.

No config files to hand-write and no terminal commands — pick programs in the
GUI and Guardian takes care of the rest. Built with Rust + egui; single native
window, tray icon, no runtime to install.

中文说明见下方 [中文文档](#中文文档)。

## Features

- **Full menu bar** (File / Guard / View / Help) and a clean, modern dashboard
  with live stat cards (guarding / total / restarts / crashes).
- **Two ways to add**: browse for an `.exe`, or pick from the list of currently
  running processes (adopt an app that is already open).
- Live status table with search, plus **pause / resume** and restart-all.
- **中文 / English toggle** and **light / dark theme**, both remembered across
  restarts.
- Crash history panel with counts and a clear-history action.
- Open the data folder / log file straight from the File menu.
- System-tray icon with **Show window / Autostart / Exit**; closing the window
  hides it to the tray and keeps guarding.
- Logon autostart toggle (Windows scheduled task, runs in your desktop session).
- Exponential backoff: 1s → 2s → 4s … capped at 30s, reset after a healthy run.
- Chinese text uses a system font, so the UI is legible on any modern Windows
  without bundling a large font file.

## Quick start

Download `guardian-vX.Y.Z-windows-x86_64.zip` (e.g. `guardian-v0.3.0-windows-x86_64.zip`) from
[Releases](https://github.com/Michael-py001/guardian/releases), extract it and
run `guardian.exe`. A window appears — no console, no setup.

Click **Add entry**, choose an executable (or a running process), and Guardian
starts watching it immediately.

To build from source (Rust + MSVC build tools):

```powershell
cargo build --release
.\target\release\guardian.exe
```

## Where files live

Guardian is a portable app: its config, log and crash history sit **next to the
executable**:

| File | Purpose |
|---|---|
| `guardian.toml` | The programs you added (written by the GUI; edit by hand only if you know TOML). |
| `guardian.log` | Timestamped log of starts, exits and restarts. |
| `crash-history.jsonl` | Append-only JSON record of every exit. |
| `settings.json` | Remembered language (`zh`/`en`) and theme (`light`/`dark`). |

Put the extracted folder somewhere writable (e.g. your Documents or a tools
folder), not `C:\Program Files`, so Guardian can save these files.

## How it behaves

- Every 250 ms Guardian checks each guarded process. On an unexpected exit it
  schedules a restart with growing backoff (1s → 2s → 4s … up to 30s).
- If a process stayed alive at least 60 seconds, it counts as healthy and the
  backoff resets — closing a long-running app normally is not a crash loop.
- A process adopted from the running-process list is treated as healthy on its
  first observed exit (its true age is unknown).
- Clicking the window's X hides it to the tray; Guardian keeps running. Use the
  tray's **Show window** to bring it back, or **Exit** to stop and terminate all
  guarded processes.

## Limitations

- Adopting a running process matches by exe image name; two processes with the
  same name are ambiguous (the first is chosen).
- Not a Windows Service: a service runs in Session 0 where GUI apps have no
  visible window. Logon autostart uses a scheduled task in your desktop session.

## License

MIT — see [LICENSE](LICENSE).

---

## 中文文档

一个轻量的**可视化** Windows 进程守护工具。你选中程序后由它守护：一旦崩溃，
就按指数退避自动重启。

不用手写配置文件、不用敲命令——在界面里选程序即可。Rust + egui 实现，单个
原生窗口，带托盘图标，无需运行时。

### 功能

- **完整菜单栏**（文件 / 守护 / 视图 / 帮助）与现代化仪表盘：实时统计卡片（守护中 / 总计 / 累计重启 / 崩溃记录）。
- **两种添加方式**：浏览选择 `.exe` 文件，或从当前运行进程列表里选（接管已打开的程序）。
- 实时状态表带搜索，支持**暂停 / 恢复**与重启全部。
- **中文 / English 一键切换**、**浅色 / 深色主题**，都会记住，下次启动保持。
- 崩溃历史：次数统计、可展开查看、可一键清空。
- 从「文件」菜单直接打开数据目录与日志文件。
- 托盘图标：**显示窗口 / 开机自启 / 退出**；关闭窗口只是缩到托盘，守护继续。
- 开机自启开关（Windows 计划任务，运行在桌面会话）。
- 指数退避：1s → 2s → 4s … 封顶 30s，健康运行后重置。
- 中文使用系统字体渲染，任何现代 Windows 都清晰，无需内置大字体文件。

### 快速开始

从 [Releases](https://github.com/Michael-py001/guardian/releases) 下载
`guardian-vX.Y.Z-windows-x86_64.zip`（如 `guardian-v0.3.0-windows-x86_64.zip`），解压后运行 `guardian.exe`。直接出现窗口——
没有控制台，无需安装。

点 **Add entry**，选一个可执行文件（或运行中的进程），Guardian 立即开始守护。

源码编译（需要 Rust + MSVC 构建工具）：

```powershell
cargo build --release
.\target\release\guardian.exe
```

### 文件位置

Guardian 是便携应用：配置、日志、崩溃历史都放在 **exe 同目录**：

| 文件 | 用途 |
|---|---|
| `guardian.toml` | 你添加的程序（由界面写入；懂 TOML 才需手改）。 |
| `guardian.log` | 启动、退出、重启的带时间戳日志。 |
| `crash-history.jsonl` | 每次退出的追加式 JSON 记录。 |

请把解压目录放在可写位置（如文档或工具目录），不要放 `C:\Program Files`，
否则 Guardian 无法保存这些文件。

### 行为

- 每 250 ms 检查一次守护进程。意外退出时按递增退避重启（1s → 2s → 4s … 封顶 30s）。
- 若进程存活至少 60 秒，视为健康并重置退避——正常关闭长跑程序不算崩溃循环。
- 从运行进程列表接管的程序，首次退出按健康处理（真实存活时长未知）。
- 点窗口 X 只是缩到托盘，守护继续。用托盘 **Show window** 恢复，**Exit** 才会停止并终止所有守护进程。

### 局限

- 按 exe 映像名接管运行中的进程；同名进程无法区分（选第一个）。
- 不是 Windows 服务：服务运行在 Session 0，GUI 程序在那里没有可见窗口。登录自启用的是桌面会话的计划任务。

### 许可证

MIT — 见 [LICENSE](LICENSE)。
