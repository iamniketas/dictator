@echo off
setlocal
cd /d "%~dp0"

set "DICTATOR_EXE=target\release\dictator.exe"
set "WHISPER_STARTER=start_whisper_server.bat"

if not exist "%DICTATOR_EXE%" (
    echo [Dictator] %DICTATOR_EXE% not found. Building release...
    cargo build --release
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

powershell -NoProfile -Command "if (Get-NetTCPConnection -LocalPort 5000 -State Listen -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }"
if errorlevel 1 (
    echo [Dictator] Whisper server is not running. Starting...
    if not exist "%WHISPER_STARTER%" (
        echo [Dictator] %WHISPER_STARTER% not found.
        exit /b 1
    )
    start "Whisper Server" cmd /k "%WHISPER_STARTER%"
    timeout /t 8 /nobreak >nul
) else (
    echo [Dictator] Whisper server is already running.
)

echo [Dictator] Starting dictator.exe...
start "" "%DICTATOR_EXE%"
exit /b 0
