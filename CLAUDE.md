# Guardian — agent rules

## Local build output

All local build artifacts go **only** in `./release/` inside this repository.
Never copy the built exe to an external directory such as
`D:\lili\my-project\GuardianApp`.

- Stage a release with `powershell -File build-release.ps1`.
- The staged exe is `release/guardian-v<version>-windows-x86_64.exe`.
- `release/` is gitignored; it is a local staging folder, not committed.

## Icon

The single source of truth is `assets/guardian.ico` (transparent shield + checkmark,
no dark background/border). Regenerate with `python assets/make_icon.py` (Pillow),
which also writes `assets/guardian.png`. The titlebar, Explorer/taskbar icon and
tray icon all use this same logo.
