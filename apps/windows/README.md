# Dictator — Windows Client

Native Windows voice dictation built with Rust + Win32 API. No Python, no cloud.

## Installation

Download `dictator-win-Setup.exe` from the [latest release](https://github.com/iamniketas/dictator/releases/latest) and run it.

The app updates itself automatically when a new version is available — you'll see a notification in the tray.

---

## Prerequisites (for building from source)

### 1. Rust Toolchain
Install from [rustup.rs](https://rustup.rs/)

### 2. LLVM / libclang
Required by `whisper-rs` (bindgen). Install via [LLVM releases](https://releases.llvm.org/) or use the one bundled with Visual Studio 2022.

Set the environment variable (or configure `.cargo/config.toml`):
```
LIBCLANG_PATH=C:\Program Files\LLVM\bin
```

### 3. Whisper Model (GGML format)
Download a `.bin` model from [ggerganov/whisper.cpp on Hugging Face](https://huggingface.co/ggerganov/whisper.cpp):

| Model | Size | Speed | Quality |
|-------|------|-------|---------|
| `ggml-tiny.bin` | 75 MB | Fastest | Low |
| `ggml-small.bin` | 466 MB | Fast | Good |
| `ggml-medium.bin` | 1.5 GB | Moderate | Very good |
| `ggml-large-v3-turbo.bin` | 1.6 GB | Fast | Best |
| `ggml-large-v3.bin` | 3.1 GB | Slow | Best |

Place models in `%LocalAppData%\whisper-models\` (shared with Contora if installed).

You can also download models directly from the app: tray → **Settings** → **Download Model**.

### 4. Ollama (Optional)
For LLM post-processing (grammar correction, formatting):
```bash
ollama pull llama3.2
```

---

## Build

```bash
cd apps/windows
cargo build --release
# With CUDA GPU acceleration:
cargo build --release --features cuda
```

---

## Configure

Config file is auto-created on first run at `%APPDATA%\dictator\config.toml`.

Key settings:

```toml
[hotkey]
key = "right_ctrl"   # Push-to-talk or Toggle with Right Ctrl

[whisper]
backend = "embedded"                                    # "embedded" (default) or "server" (legacy Python)
model_path = "C:\\Users\\you\\AppData\\Local\\whisper-models\\ggml-large-v3-turbo.bin"
models_dir = "C:\\Users\\you\\AppData\\Local\\whisper-models\\"
language = "ru"

[ollama]
enabled = false
url = "http://localhost:11434"
model = "llama3.2"

[injection]
method = "clipboard"   # "direct" | "clipboard" | "clipboard_enter"

[memory]
idle_unload_minutes = 5   # 0 = never unload
```

---

## Usage

1. Launch `dictator.exe` — it appears in the system tray
2. **Hold Right Ctrl** → Push-to-Talk (releases on key up)
   **Tap Right Ctrl** → Toggle (tap again to stop)
3. Transcribed text is injected into the active window

### Tray Menu

- **Model selector** — switch between downloaded models (hot-swap, no restart)
- **Recent recordings** — last 3, click to copy
- **Open Recordings Folder**
- **Settings...** — full settings window (models, download, general, about)
- **Exit**

### Settings Window

Opened from tray → **Settings...**

| Section | Controls |
|---------|----------|
| **Models** | List of downloaded models, active indicator, Use / Delete buttons |
| **Download** | Model selector dropdown, Download button with progress |
| **General** | Text injection method, LLM toggle + Ollama URL/model, idle unload timer |
| **About** | Version, hotkey, Open Logs, Open Config |

### CLI Remote Control

```bash
dictator.exe --toggle   # Start/stop recording
dictator.exe --stop     # Stop if recording, otherwise no-op
```

---

## Architecture

```
Hotkey (Right Ctrl — global low-level hook)
    -> Audio Capture (cpal, 16kHz mono)
    -> Embedded Whisper (whisper-rs / whisper.cpp GGML)
    -> [Optional] LLM Correction (Ollama HTTP)
    -> Text Injection (SendInput / Clipboard — Win32)
```

Legacy backend (Python HTTP server) is still supported via `backend = "server"` in config.

---

## Source Structure

```
apps/windows/
  src/
    main.rs               # Entry point, pipeline orchestration
    audio.rs              # Microphone capture (cpal)
    streaming.rs          # Streaming / chunked transcription
    transcribe.rs         # HTTP client for legacy whisper server
    whisper_engine.rs     # Embedded whisper-rs engine (default)
    whisper_server.rs     # Legacy Python server management
    model_downloader.rs   # GGML model download from Hugging Face
    llm.rs                # Ollama API client
    input.rs              # Global hotkey (Right Ctrl) + text injection
    ui.rs                 # System tray (Win32 Shell_NotifyIcon)
    overlay_win32.rs      # Overlay window during recording
    settings_window.rs    # Native Win32 settings window
    updater.rs            # Velopack auto-updater
    history.rs            # Transcription history (JSON + WAV + TXT)
    config.rs             # TOML configuration
    lib.rs                # Module declarations
  examples/
    test_overlay.rs
    benchmark.rs
  assets/
    dictator.ico
  build.rs                # Windows resource embedding
  dictator.rc             # Icon resource
  Cargo.toml
```

---

## Known Issues

- **Console Window:** Release build hides console via `#![windows_subsystem = "windows"]`
- **Double spaces:** faster-whisper (server backend) may insert double spaces between segments — normalized automatically
- **Auto-update requires installer:** Velopack updater only works when installed via Setup.exe; portable/dev builds skip update checks silently
