# Dictator 🎤

**Voice dictation service for Windows** that converts speech to text using local AI models. The application runs as a background service with system tray integration and is activated via global hotkey.

## Features

- 🎤 **Global Hotkey** — Press `Ctrl+Shift+D` to start/stop recording
- 🔊 **Audio Capture** — Records from microphone (16 kHz mono, auto-conversion from device format)
- 🤖 **AI Transcription** — Uses [faster-whisper](https://github.com/SYSTRAN/faster-whisper) with CUDA support
- ✍️ **Text Injection** — Automatically inserts transcribed text into active window
- 🧠 **LLM Correction** — Optional text correction via Ollama (Qwen3 30B)
- 🪟 **System Tray** — Runs in background, minimal UI

## Architecture

```
Audio Capture (cpal)
    ↓
Resample to 16kHz mono
    ↓
HTTP → Whisper Server (Flask + faster-whisper)
    ↓
Transcribed Text
    ↓
[Optional] Ollama LLM Correction
    ↓
Text Injection (SendInput Win32 API)
```

### Why HTTP Server?

The Whisper model is **3 GB** and takes **5-10 seconds to load**. Running it as an HTTP server keeps the model in memory, reducing transcription time from ~10s to **~1-2 seconds**.

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
ollama pull qwen2.5-coder:32b
```

### 4. Rust Toolchain
Install from [rustup.rs](https://rustup.rs/)

## Installation

1. **Clone repository**
   ```bash
   git clone https://github.com/iamniketas/dictator.git
   cd dictator
   ```

2. **Configure**

   Edit `config.toml` in `%APPDATA%/dictator/` (auto-created on first run):
   ```toml
   [hotkey]
   modifiers = ["ctrl", "shift"]
   key = "D"

   [whisper]
   model_path = "C:\\path\\to\\faster-whisper-large-v2"
   language = "ru"

   [ollama]
   base_url = "http://localhost:11434"
   model = "qwen2.5-coder:32b"
   enabled = true
   ```

3. **Build**
   ```bash
   cargo build --release
   ```

## Usage

### 1. Start Whisper Server
```bash
cd target/release
start_whisper_server.bat
```

Wait ~10 seconds for model to load. You'll see:
```
INFO:__main__:Model loaded successfully on cuda
* Running on http://127.0.0.1:5000
```

### 2. Start Dictator
```bash
target\release\dictator.exe
```

The app will appear in system tray. Press `Ctrl+Shift+D` to:
- **First press** → Start recording (microphone icon appears)
- **Second press** → Stop recording, transcribe, and inject text

### 3. Exit
Right-click tray icon → **Exit**

## Project Structure

```
dictator/
├── src/
│   ├── main.rs          # Entry point, pipeline orchestration
│   ├── audio.rs         # Microphone capture with cpal
│   ├── transcribe.rs    # HTTP client for Whisper server
│   ├── llm.rs           # Ollama API client
│   ├── input.rs         # Global hotkey + text injection
│   ├── ui.rs            # System tray with Win32 API
│   └── config.rs        # TOML configuration
├── whisper_server.py    # Flask HTTP server for faster-whisper
├── start_whisper_server.bat  # Windows launcher
└── config.toml          # User configuration (in %APPDATA%)
```

## Technical Details

### Audio Pipeline
- **Capture:** cpal with device's native format (e.g., 48kHz stereo)
- **Conversion:** Stereo → mono (channel averaging), 48kHz → 16kHz (linear interpolation)
- **Output:** 16kHz mono f32 PCM WAV

### HTTP Communication
- **reqwest** with `.no_proxy()` (to bypass system proxy for localhost)
- **multipart/form-data** with WAV file + language parameter
- **Response:** JSON with transcribed text

### Text Injection
- Uses Windows `SendInput` API
- Types characters one-by-one with 1ms delay
- Supports Unicode via `KEYEVENTF_UNICODE`

## Known Issues

- **System Proxy:** If you have a system proxy enabled, the app automatically disables it for localhost requests to Whisper/Ollama
- **Console Window:** Release build hides console via `#![windows_subsystem = "windows"]`
- **VAD Filter:** Whisper's Voice Activity Detection may remove silence — actual transcription duration may differ from recording length

## Roadmap

- [ ] Overlay UI (show transcribed text near cursor)
- [ ] Customizable hotkey in UI
- [ ] macOS/Linux support
- [ ] Embedded Whisper (replace HTTP server with direct Rust binding)

## License

MIT

## Credits

- [faster-whisper](https://github.com/SYSTRAN/faster-whisper) — Fast Whisper implementation
- [cpal](https://github.com/RustAudio/cpal) — Cross-platform audio I/O
- [windows-rs](https://github.com/microsoft/windows-rs) — Rust bindings for Windows API
