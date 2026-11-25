# Build Guardian and stage the distributable entirely inside ./release.
# All local build artifacts go here — never copy the exe to an external folder.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build --release failed' }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
$release = Join-Path $root 'release'
New-Item -ItemType Directory -Force -Path $release | Out-Null

Copy-Item (Join-Path $root 'target/release/guardian.exe') `
          (Join-Path $release "guardian-v${version}-windows-x86_64.exe") -Force
Copy-Item (Join-Path $root 'README.md') (Join-Path $release 'README.md') -Force
Copy-Item (Join-Path $root 'LICENSE') (Join-Path $release 'LICENSE') -Force
Copy-Item (Join-Path $root 'config.example.toml') (Join-Path $release 'config.example.toml') -Force

Write-Host "Staged in $release (guardian-v${version}-windows-x86_64.exe)"
