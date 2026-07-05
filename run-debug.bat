@echo off
cd /d "%~dp0apps\windows"

echo Building Dictator (release + CUDA/GPU)...
echo This compiles optimized code with GPU acceleration. First build is slow;
echo later rebuilds are incremental and fast.
cargo build --release --features cuda
if errorlevel 1 (
    echo.
    echo BUILD FAILED - see errors above
    pause
    exit /b 1
)

echo.
echo Stopping any running instance...
taskkill /IM dictator.exe /F >nul 2>&1

echo.
echo Starting Dictator (GPU build)...
start "" "%~dp0apps\windows\target\release\dictator.exe"

echo.
echo Done. Press any key to close this window.
pause >nul
