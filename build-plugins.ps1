# Build the reference WASM plugins and copy them to plugins/dist.
# Requires: rustup target add wasm32-unknown-unknown
$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path -LiteralPath (Join-Path $cargoBin "cargo.exe")) {
    $env:Path = "$cargoBin;$env:Path"
}

Push-Location "$PSScriptRoot\plugins"
try {
    rustup target add wasm32-unknown-unknown | Out-Null
    cargo build --release --target wasm32-unknown-unknown
    New-Item -ItemType Directory -Force -Path dist | Out-Null
    Copy-Item "target\wasm32-unknown-unknown\release\image_dimensions.wasm" dist\ -Force
    Copy-Item "target\wasm32-unknown-unknown\release\crc32_command.wasm" dist\ -Force
    Write-Host "Plugins built to plugins\dist\" -ForegroundColor Green
} finally {
    Pop-Location
}
