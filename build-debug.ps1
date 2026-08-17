# Fast debug build of SimpleCommander (no LTO, unoptimized app code).
# Usage: ./build-debug.ps1 [-Run]
param(
    [switch]$Run
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path -LiteralPath (Join-Path $cargoBin "cargo.exe")) {
    $env:Path = "$cargoBin;$env:Path"
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo not found. Install Rust with: winget install --id Rustlang.Rustup -e"
}

Write-Host "Building debug binary..." -ForegroundColor Cyan
cargo build -p sc-app
if ($LASTEXITCODE -ne 0) { throw "debug build failed" }

$exe = Join-Path $PSScriptRoot "target\debug\simplecommander.exe"
Write-Host "Debug binary: $exe" -ForegroundColor Green

if ($Run) {
    Write-Host "Launching..." -ForegroundColor Cyan
    & $exe
}
