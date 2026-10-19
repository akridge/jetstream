# Build JetStream Desktop on Windows: installers (.msi, setup .exe) and a portable .zip.
#
#   powershell -ExecutionPolicy Bypass -File desktop\scripts\build_windows.ps1
#   powershell -ExecutionPolicy Bypass -File desktop\scripts\build_windows.ps1 -PortableOnly
#
# Needs: Python 3.10+ (python on PATH), Rust (https://rustup.rs), Node.js 18+, and the
# Visual Studio C++ Build Tools ("Desktop development with C++"). Output: desktop\release\
param(
    [switch]$PortableOnly,
    [switch]$PackageOnly,
    [switch]$SkipPythonDeps
)
$ErrorActionPreference = "Stop"

foreach ($tool in @("python", "cargo", "node", "npm")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Error "$tool was not found on PATH. See the requirements at the top of this script."
    }
}

$argsList = @()
if ($PortableOnly) { $argsList += "--portable-only" }
if ($PackageOnly) { $argsList += "--package-only" }
if ($SkipPythonDeps) { $argsList += "--skip-python-deps" }

python "$PSScriptRoot\build_desktop.py" @argsList
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "`nDone. Look in desktop\release\" -ForegroundColor Green
