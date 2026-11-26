# Changelog

All notable changes to Guardian.

## Unreleased

### Features

- **mac:** build and release macOS binaries (`5ae1ac4`)

## 0.3.2 - 2026-11-26

### Features

- **ui:** show the version in the About dialog (`9b09c7f`)
- **win:** embed a version resource from Cargo.toml (`4107095`)

### Bug Fixes

- **startup:** no console flash, center on primary monitor (`f62a274`)
- **exit:** graceful tray quit without ghost icon or killing guarded apps (`3ec9c7c`)
- **ui:** one plus on Add, and scroll bars that reserve space (`dd6cae1`)
- **window:** working tray restore, single instance, centered start (`54b33c5`)
- **window:** keep show/hide outside winit, and sleep while hidden (`7561680`)

### Styles

- **ui:** space out the health card legend and clear the ring (`ad15b50`)

### Other

- Revert "fix(window): keep show/hide outside winit, and sleep while hidden" (`2c34d83`)

## 0.3.1 - 2026-11-24

### Features

- embed shield icon into the Windows exe (`69e1b59`)
- **ui:** show keyboard hints in File menu like the prototype (`0fce137`)
- **ui:** show real uptime in health card; honest running-share ring (`bfc9c98`)
- restore config hot reload after GUI rewrite (`80950e4`)
- **ui:** default to dark theme; add process/crash card footers (`8609cf7`)
- **ui:** match prototype charts — peak annotation, pulse axis, health finding, title actions (`1a012d0`)
- **ui:** wire Ctrl+N / Del / Esc shortcuts and double-click maximize (`f5cc829`)
- **ui:** add About button to the icon rail footer (`c634ece`)
- **ui:** close remaining prototype gaps — KPI notes, live dot, scrollable dashboard (`f5b42f8`)
- **ui:** add frameless custom title bar matching the prototype (`ffa438e`)
- **ui:** highlight today's top crasher in the overview subtitle (`bf2cb21`)
- **ui:** show real counters in the uptime health card (`7a9ac11`)
- **ui:** add icon rail with 4 views, restart-pulse chart, remove notepad hardcoding (`3536ddc`)

### Bug Fixes

- **icon:** one shield everywhere, and a tray icon that reads (`8d5c218`)
- **tray:** use the embedded shield logo and make Show/Quit work while hidden (`01de6d7`)
- titlebar close hides to tray; add frameless resize edges; fix card right margin (`3f1a997`)
- **ui:** crash card fills column; remove deprecated egui call (`5cffa01`)
- **ui:** toasts now clear themselves after 5 seconds (`7cb896e`)
- adding/removing a guard no longer restarts unrelated processes (`c3178b3`)
- **ui:** make Processes and History views scrollable (`c7e8b0d`)
- **ui:** fill two placeholder templates in order (`142f308`)
- **tray:** quit now actually exits, and ship the dashboard UI redesign (`7f94f4e`)

### Documentation

- describe dashboard charts in README (`8233cd5`)

### Styles

- **ui:** soften selected toggles to a subtle tint (`d94ad13`)
- **ui:** center the About dialog close button (`df7c0e4`)
- **ui:** polish light theme, chrome alignment, About dialog, font clarity (`d6876d9`)
- **ui:** align both dashboard bands to one grid and matched card heights (`51cb4a8`)
- **ui:** vector icons, rounded buttons, prototype two-band layout (`d31ca74`)
- **ui:** use structured crash event cards in Overview too (`bda42ee`)
- **ui:** close 8 remaining prototype gaps from audit (`0b63b86`)
- **ui:** render crash history as structured event cards like prototype (`9b072ff`)
- **ui:** align brand, heading and window-control tooltips with prototype (`684a473`)
- **ui:** show crash uptime as HH:MM:SS like the prototype (`3a05fca`)

### CI

- include version tag in release asset filename (`8c9fc5c`)

## 0.3.0 - 2026-11-23

### Features

- **ui:** modern dashboard with menu bar, dark theme, pause/resume, search, about dialog (`9a86c60`)

### Bug Fixes

- clear stale last-exit message once a process is running again (`19eb441`)

## 0.2.0 - 2026-11-23

### Features

- **gui:** rewrite as visual egui app with tray, drop CLI mode (`38a3486`)
- native tray mode, history command, and tray-based autostart (`05f27fa`)
- file logging, crash history, hot reload, and release distribution (`35eef9d`)
- adopt running processes, logon auto-start, bilingual README and CI badge (`631083e`)
- initial process guardian with TOML config and exponential backoff (`ba813f6`)

### Documentation

- remove project-specific references from README and examples (`c2fb0f6`)
