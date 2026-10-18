# Build a release binary of Lumen into dist\ (Windows, PowerShell).
# Usage:  powershell -ExecutionPolicy Bypass -File .\build.ps1
$ErrorActionPreference = "Stop"
Set-Location -Path $PSScriptRoot

# Windows can't replace dist\lumen.exe while it's running, so close it first.
Get-Process -Name lumen -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Transcript -Path (Join-Path $PSScriptRoot 'build.log') -Force | Out-Null

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust is not installed. Installing via rustup (non-interactive)..."
    $installer = Join-Path $env:TEMP "rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $installer
    & $installer -y --profile minimal --default-toolchain stable --default-host x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "Rust installation failed" }
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
}

# The MSVC toolchain needs the "Desktop development with C++" build tools
# (for the vendored Lua and other C code).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere) -or -not (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)) {
        Write-Warning "Visual Studio C++ Build Tools not found. Install them with:"
        Write-Warning "  winget install Microsoft.VisualStudio.2022.BuildTools --override `"--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended`""
    }
}

# Bundled libopus is compiled with CMake. Visual Studio's recommended C++
# components include CMake, but its executable need not be in the user's PATH.
if (-not (Get-Command cmake -ErrorAction SilentlyContinue) -and (Test-Path -LiteralPath $vswhere)) {
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vsPath) {
        $cmakeBin = Join-Path $vsPath 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
        if (Test-Path -LiteralPath (Join-Path $cmakeBin 'cmake.exe')) { $env:Path = "$cmakeBin;$env:Path" }
    }
}
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) { throw 'CMake is required for local Opus playback. Install the Visual Studio C++ CMake tools or CMake, then rebuild.' }

# LLVM's archiver also works with MSVC objects and avoids lib.exe's in-place
# archive replacement, which is denied in some restricted Windows environments.
# Install it with `rustup component add llvm-tools` when needed.
if (-not $env:AR -and (Get-Command rustc -ErrorAction SilentlyContinue)) {
    $sysroot = & rustc --print sysroot
    $llvmAr = Join-Path $sysroot 'lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-ar.exe'
    if (Test-Path -LiteralPath $llvmAr) { $env:AR = $llvmAr }
}

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path dist | Out-Null
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
Copy-Item (Join-Path $targetDir 'release\lumen.exe') dist\lumen.exe -Force
Copy-Item plugins dist\example-plugins -Recurse -Force
$size = [math]::Round((Get-Item dist\lumen.exe).Length / 1MB, 1)
Write-Host "Built dist\lumen.exe ($size MB)" -ForegroundColor Green
Stop-Transcript | Out-Null

# Start the new build (skip with -NoRun).
if ($args -notcontains '-NoRun') { Start-Process (Join-Path $PSScriptRoot 'dist\lumen.exe') }
