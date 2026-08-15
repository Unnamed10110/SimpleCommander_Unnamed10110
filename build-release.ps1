# Optimized release build of SimpleCommander (LTO, stripped).
# Usage: ./build-release.ps1 [-Run]
param(
    [switch]$Run
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "Building release binary..." -ForegroundColor Cyan
cargo build --release -p sc-app
if ($LASTEXITCODE -ne 0) { throw "release build failed" }

$exe = Join-Path $PSScriptRoot "target\release\simplecommander.exe"
$size = [math]::Round((Get-Item $exe).Length / 1MB, 2)
Write-Host "Release binary: $exe ($size MB)" -ForegroundColor Green

if ($Run) {
    Write-Host "Launching..." -ForegroundColor Cyan
    & $exe
}
