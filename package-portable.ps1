# Builds the portable distribution: exe + reference plugins + docs in a zip.
# Usage: ./package-portable.ps1 [-OutDir dist]
param(
    [string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

Write-Host "Building release binary..." -ForegroundColor Cyan
cargo build --release -p sc-app
if ($LASTEXITCODE -ne 0) { throw "release build failed" }

Write-Host "Building plugins..." -ForegroundColor Cyan
& .\build-plugins.ps1
if ($LASTEXITCODE -ne 0) { throw "plugin build failed" }

$stage = Join-Path $OutDir "SimpleCommander"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $stage "plugins") -Force | Out-Null

Copy-Item "target\release\simplecommander.exe" $stage
Copy-Item "plugins\dist\*.wasm" (Join-Path $stage "plugins")
Copy-Item "README.md" $stage

$zip = Join-Path $OutDir "SimpleCommander-portable.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip

$size = [math]::Round((Get-Item $zip).Length / 1MB, 2)
Write-Host "Portable package: $zip ($size MB)" -ForegroundColor Green
