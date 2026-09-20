# Developer setup for the code-CAD engine (build123d).
#
# Creates a private Python virtual environment under the app's local data folder and installs
# build123d into it. The app looks for the engine there, so nothing has to be configured:
#
#   %LOCALAPPDATA%\ai.printpilot\cad-engine\venv\Scripts\python.exe
#
#   .\scripts\setup-cad-engine.ps1              # install from PyPI
#   .\scripts\setup-cad-engine.ps1 -Mirror      # install through the TUNA mirror (faster in mainland China)
#   .\scripts\setup-cad-engine.ps1 -Force       # delete and recreate the environment
#
# This is the DEV path and needs a system Python 3.10+ (`py` launcher). End users never run this:
# they get a prebuilt "engine pack" (embedded Python + build123d) that the app downloads on first
# use. See docs/adr/0003-code-cad-build123d.md.
#
# NOTE: keep this file ASCII-only (Windows PowerShell 5.1 reads BOM-less .ps1 files as ANSI).
param(
    [switch]$Mirror,
    [switch]$Force,
    [string]$Version = '0.12.0'
)

$ErrorActionPreference = 'Stop'
$engineDir = Join-Path $env:LOCALAPPDATA 'ai.printpilot\cad-engine'
$venv = Join-Path $engineDir 'venv'
$python = Join-Path $venv 'Scripts\python.exe'

if ($Force -and (Test-Path $venv)) {
    Write-Host "removing $venv"
    Remove-Item -Recurse -Force $venv -Confirm:$false
}

if (-not (Test-Path $python)) {
    New-Item -ItemType Directory -Force $engineDir | Out-Null
    # Prefer the newest interpreter build123d supports; fall back to whatever `py -3` gives us.
    $created = $false
    foreach ($v in '-3.13', '-3.12', '-3.11', '-3.10', '-3') {
        # Probe through cmd.exe: in Windows PowerShell 5.1 a native command that writes to stderr
        # (here: "Python 3.13 not found!") becomes a terminating error under ErrorActionPreference=Stop,
        # even with 2>$null.
        cmd /c "py $v -c ""import sys; assert (3,10) <= sys.version_info[:2] <= (3,14)"" >nul 2>&1"
        if ($LASTEXITCODE -eq 0) {
            Write-Host "creating virtual environment with: py $v"
            & py $v -m venv $venv
            if ($LASTEXITCODE -ne 0) { throw "python -m venv failed" }
            $created = $true
            break
        }
    }
    if (-not $created) { throw 'no suitable Python (3.10 - 3.14) found via the py launcher' }
}

$pipArgs = @('-m', 'pip', 'install', '--disable-pip-version-check', "build123d==$Version")
if ($Mirror) { $pipArgs += @('-i', 'https://pypi.tuna.tsinghua.edu.cn/simple') }

Write-Host "installing build123d $Version (this downloads a few hundred MB the first time)"
& $python @pipArgs
if ($LASTEXITCODE -ne 0) { throw 'pip install failed' }

& $python -c "import build123d, OCP, sys; print('build123d', build123d.__version__, '| python', sys.version.split()[0])"
if ($LASTEXITCODE -ne 0) { throw 'build123d does not import' }

$size = (Get-ChildItem $venv -Recurse -File | Measure-Object Length -Sum).Sum / 1MB
Write-Host ("engine ready: {0}  ({1:N0} MB)" -f $python, $size)
