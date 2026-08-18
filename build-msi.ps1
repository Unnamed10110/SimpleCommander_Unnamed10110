# Builds the SimpleCommander MSI installer (release binary + WiX v3).
# Usage: ./build-msi.ps1 [-SkipBuild]
# Output: target/wix/SimpleCommander-<version>.msi
param(
    [switch]$SkipBuild
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

function Get-WixBinDir {
    $candidates = @()
    if ($env:WIX) {
        $candidates += (Join-Path $env:WIX "bin")
    }
    $candidates += @(
        (Join-Path ${env:ProgramFiles(x86)} "WiX Toolset v3.14\bin"),
        (Join-Path ${env:ProgramFiles(x86)} "WiX Toolset v3.11\bin"),
        (Join-Path $env:LOCALAPPDATA "wix314\bin")
    )
    foreach ($dir in $candidates) {
        if ($dir -and (Test-Path -LiteralPath (Join-Path $dir "candle.exe"))) {
            return $dir
        }
    }
    return $null
}

function Install-WixBinaries {
    $destRoot = Join-Path $env:LOCALAPPDATA "wix314"
    $binDir = Join-Path $destRoot "bin"
    $zip = Join-Path $env:TEMP "wix314-binaries.zip"
    New-Item -ItemType Directory -Force -Path $binDir | Out-Null
    Write-Host "Downloading WiX Toolset v3.14.1 binaries..." -ForegroundColor Cyan
    Invoke-WebRequest -Uri "https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip" -OutFile $zip -UseBasicParsing
    Expand-Archive -LiteralPath $zip -DestinationPath $binDir -Force
    if (-not (Test-Path -LiteralPath (Join-Path $binDir "candle.exe"))) {
        throw "WiX download did not contain candle.exe"
    }
    return $binDir
}

$exe = Join-Path $PSScriptRoot "target\release\simplecommander.exe"
if (-not $SkipBuild -or -not (Test-Path -LiteralPath $exe)) {
    Write-Host "Building release binary..." -ForegroundColor Cyan
    cargo build --release -p sc-app
    if ($LASTEXITCODE -ne 0) { throw "release build failed" }
}
if (-not (Test-Path -LiteralPath $exe)) {
    throw "Missing $exe — run without -SkipBuild"
}

if (-not (Get-Command cargo-wix -ErrorAction SilentlyContinue)) {
    Write-Host "Installing cargo-wix..." -ForegroundColor Cyan
    cargo install cargo-wix
    if ($LASTEXITCODE -ne 0) { throw "cargo install cargo-wix failed" }
}

$wixBin = Get-WixBinDir
if (-not $wixBin) {
    $wixBin = Install-WixBinaries
}

$releaseDir = Join-Path $PSScriptRoot "target\release"
Write-Host "Building MSI installer..." -ForegroundColor Cyan
cargo wix -p sc-app --include wix/main.wxs --no-build --target-bin-dir $releaseDir --bin-path $wixBin --name SimpleCommander --nocapture
if ($LASTEXITCODE -ne 0) { throw "MSI build failed" }

$wixOut = Join-Path $PSScriptRoot "target\wix"
$msi = Get-ChildItem -LiteralPath $wixOut -Filter "SimpleCommander-*.msi" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $msi) { throw "MSI was not created in target\wix" }
$msiSize = [math]::Round($msi.Length / 1MB, 2)
Write-Host "MSI installer: $($msi.FullName) ($msiSize MB)" -ForegroundColor Green
