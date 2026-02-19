@echo off
setlocal

REM Start Whisper HTTP Server for Dictator (Windows)
set SCRIPT_DIR=%~dp0
set DEFAULT_MODEL_PATH=%SCRIPT_DIR%models\faster-whisper-large-v2

if "%WHISPER_PORT%"=="" set WHISPER_PORT=5000

echo Starting Whisper HTTP Server on port %WHISPER_PORT%...
echo Default model path: %DEFAULT_MODEL_PATH%

if not "%~1"=="" (
    python whisper_server.py "%~1"
) else if not "%WHISPER_MODEL_PATH%"=="" (
    python whisper_server.py "%WHISPER_MODEL_PATH%"
) else (
    python whisper_server.py "%DEFAULT_MODEL_PATH%"
)

endlocal
