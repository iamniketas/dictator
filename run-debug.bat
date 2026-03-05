@echo off
cd /d "%~dp0apps\windows"

echo Building Dictator (debug)...
cargo build
if errorlevel 1 (
    echo.
    echo BUILD FAILED - see errors above
    pause
    exit /b 1
)

echo.
echo Starting Dictator...
start "" "%~dp0apps\windows\target\debug\dictator.exe"
