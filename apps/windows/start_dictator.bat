@echo off
setlocal
cd /d "%~dp0"

set "DICTATOR_EXE=target\release\dictator.exe"

if not exist "%DICTATOR_EXE%" (
    echo [Dictator] %DICTATOR_EXE% not found. Building release...
    cargo build --release --features cuda
    if errorlevel 1 (
        echo [Dictator] Build failed.
        exit /b 1
    )
)

tasklist /FI "IMAGENAME eq dictator.exe" | find /I "dictator.exe" >nul
if not errorlevel 1 (
    echo [Dictator] dictator.exe is already running.
    exit /b 0
)

echo [Dictator] Starting dictator.exe...
start "" "%DICTATOR_EXE%"
exit /b 0
