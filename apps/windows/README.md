# Dictator — Windows Client

Native Windows voice dictation built with Rust + Win32 API.

## Prerequisites

### 1. Python 3.13+ with Dependencies
```bash
pip install flask faster-whisper
```

### 2. Whisper Model
Download a [faster-whisper model](https://huggingface.co/Systran/faster-whisper-large-v2) (e.g., `faster-whisper-large-v2`).

### 3. Ollama (Optional)
For text correction:
```bash
ollama pull glm-4.7-flash
```

### 4. Rust Toolchain
Install from [rustup.rs](https://rustup.rs/)

## Build

```bash
cd apps/windows
cargo build --release
```

## Configure

Edit `config.toml` in `%APPDATA%/dictator/` (auto-created on first run):
```toml
[hotkey]
modifiers = ["ctrl", "shift"]
key = "D"

[whisper]
model_path = "C:\\path\\to\\faster-whisper-large-v2"
language = "ru"

[ollama]
url = "http://localhost:11434"
model = "glm-4.7-flash"
```

## Usage

### 1. Start Whisper Server
```bash
# From repo root:
python shared/whisper-server/whisper_server.py
# Or use the launcher:
start_whisper_server.bat
```

Wait ~10 seconds for model to load.

### 2. Start Dictator
```bash
target\release\dictator.exe
```

Press `Ctrl+Shift+D` to start/stop recording. Text is injected into the active window.

### 3. Exit
Right-click tray icon -> **Exit**

## Architecture

```
Audio Capture (cpal, 16kHz mono)
    -> HTTP -> Whisper Server (Flask + faster-whisper, CUDA)
    -> Transcribed Text
    -> [Optional] Ollama LLM Correction
    -> Text Injection (SendInput Win32 API)
```

## Source Structure

```
apps/windows/
  src/
    main.rs           # Entry point, pipeline orchestration
    audio.rs          # Microphone capture with cpal
    streaming.rs      # Streaming transcription (chunked)
    transcribe.rs     # HTTP client for Whisper server
    whisper_server.rs # Whisper server management
    llm.rs            # Ollama API client
    input.rs          # Global hotkey + text injection
    ui.rs             # System tray with Win32 API
    overlay_win32.rs  # Overlay window (streaming text display)
    history.rs        # Transcription history
    config.rs         # TOML configuration
    lib.rs            # Module declarations
  examples/
    test_overlay.rs
    benchmark.rs
  assets/
    dictator.ico
  build.rs            # Windows resource embedding
  dictator.rc         # Icon resource
  Cargo.toml
```

## Technical Details

### Audio Pipeline
- **Capture:** cpal with device's native format (e.g., 48kHz stereo)
- **Conversion:** Stereo -> mono (channel averaging), 48kHz -> 16kHz (linear interpolation)
- **Output:** 16kHz mono f32 PCM WAV

### Text Injection
- Uses Windows `SendInput` API
- Types characters one-by-one with 1ms delay
- Supports Unicode via `KEYEVENTF_UNICODE`

## Known Issues

- **System Proxy:** Auto-disabled for localhost requests to Whisper/Ollama
- **Console Window:** Release build hides console via `#![windows_subsystem = "windows"]`
