# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Language Preferences
- ALWAYS respond to the user in Russian.
- Keep technical terms and code snippets in their original form (English).

---

## Project Overview

Dictator is a **multi-platform local voice dictation** app. Each platform has a fully native client.

- **Windows** (`apps/windows/`) — Rust + Win32, production-ready (v0.3.0)
- **macOS** (`apps/macos/`) — Swift + SwiftUI/AppKit, beta (core pipeline works, polish in progress)
- **Shared** (`shared/`) — Python whisper server (legacy) + cross-platform contracts

---

## Build & Development Commands

### macOS client (Swift)

```bash
cd apps/macos
swift build                          # Debug build
swift run                            # Run directly
swift package resolve                # Resolve/update dependencies (including WhisperKit)
```

Requires macOS 14 (Sonoma)+. WhisperKit downloads Core ML models on first use (~150 MB for base, ~3 GB for large-v3) to `~/Library/Application Support/huggingface/models/`.

**HTTP fallback**: if WhisperKit is not configured, falls back to HTTP transcription via the Python whisper server at `http://127.0.0.1:5500/transcribe`.

**Permissions required at runtime**: Microphone + Accessibility (for global hotkeys and text injection).

### Windows client (Rust)

```bash
cd apps/windows

cargo build                          # Debug build
cargo build --release                # Release (LTO, opt-level 3, strip)
cargo build --release --features cuda  # With CUDA GPU acceleration

cargo check --all-targets            # Fast syntax/type check (no binary)
cargo test --all-targets             # Run all unit + integration tests

cargo run --example test_overlay     # Test waveform overlay window
cargo run --example benchmark        # Transcription benchmark
```

**Requirement:** LLVM must be installed for `whisper-rs` bindgen.
- Windows: `choco install llvm` or via Visual Studio 2022 LLVM component
- Set `LIBCLANG_PATH` if not on PATH (`.cargo/config.toml` has a VS2022 fallback preset)

### Python whisper server (legacy)

```bash
python3 shared/whisper-server/whisper_server.py [model_path]
# or
bash shared/whisper-server/start_whisper_server.sh
```

Environment variables: `WHISPER_PORT` (default 5500), `WHISPER_MODEL_PATH`, `WHISPER_DEVICE` (auto/cpu/cuda), `WHISPER_COMPUTE_TYPE` (default int8_float16).

---

## Architecture

### Core pipeline (all platforms)

```
hotkey press → recording (cpal/AVAudioEngine, 16kHz mono)
             → transcribing (whisper-rs embedded OR HTTP to whisper-server)
             → correcting (optional Ollama LLM, /api/generate)
             → injecting (clipboard/SendInput → active window)
             → idle (optional VRAM unload after N minutes)
```

### macOS module map (`apps/macos/Sources/DictatorMac/`)

| Module | Responsibility |
|--------|----------------|
| `Models/AppModel.swift` | `@MainActor` state machine: recording pipeline, streaming, timers, metrics |
| `Services/AudioCaptureService.swift` | `AVAudioEngine` capture, resampling to 16kHz mono, thread-safe ring buffer |
| `Services/TranscriptionService.swift` | `TranscriptionService` protocol + `WhisperHTTPTranscriptionService` |
| `Services/WhisperKitService.swift` | On-device Apple Silicon transcription via WhisperKit (CoreML/Metal/ANE) |
| `Services/RecordingArchiveService.swift` | Save WAV + JSON to `~/Library/Application Support/Dictator/Recordings/`, keep last 5 |
| `Services/HotkeyManager.swift` | `CGEventTap` global hotkey; smart/toggle/PTT modes; Carbon fallback |
| `Services/TextInjectionService.swift` | Pasteboard + `CGEvent` Cmd+V with retry delays; AppleScript fallback |
| `Services/SettingsStore.swift` | `UserDefaults`-backed config: backend, language, model, streaming, endpoint |
| `Services/LLMService.swift` | Ollama client for grammar correction (optional post-processing) |
| `UI/AppDelegate.swift` | `NSStatusItem`, tray menu, ticker-window lifecycle |
| `UI/DashboardView.swift` | Floating borderless ticker (320×36 px, `.floating` level) |
| `UI/SettingsView.swift` | SwiftUI Settings: backend selector, WhisperKit model picker/download |

**Transcription backends (macOS):**
- `whisperkit` (default) — on-device, no server needed; requires model download (~150 MB – 3 GB)
- `http` — sends WAV via multipart POST to Python whisper server

**Hotkey modes:**
- `smart` (default) — tap < 300ms = toggle, hold ≥ 300ms = push-to-talk
- `toggle` — every tap flips recording state
- `ptt` — key-down starts, key-up stops

### macOS: known issues & incomplete areas

| Area | Status |
|------|--------|
| WhisperKit download progress | Shows "0%" indefinitely — WhisperKit API gives no granular progress |
| Waveform overlay | Not implemented (exists on Windows via `overlay_win32.rs`) |
| Ollama LLM correction | Not implemented (exists on Windows via `llm.rs`) |
| Permission re-check | Checked only at startup; no re-check after user grants permissions |
| Ticker window position | Hardcoded top-right corner, not persisted after user moves it |

### Windows module map (`apps/windows/src/`)

| Module | Responsibility |
|--------|----------------|
| `main.rs` | Pipeline orchestration, single-instance guard, IPC listener |
| `whisper_engine.rs` | **Default backend**: embedded whisper-rs (GGML .bin), CUDA support |
| `audio.rs` | Microphone capture via cpal; lock-free `AtomicU32` RMS amplitude |
| `input.rs` | Global low-level keyboard hook; PTT / toggle modes |
| `overlay_win32.rs` | Recording overlay window (winit + softbuffer, waveform visualization) |
| `settings_window.rs` | Native Win32 settings dialog (model management, download, LLM, memory) |
| `ui.rs` | System tray (Shell_NotifyIcon), menu, history entries |
| `llm.rs` | Ollama client for grammar correction |
| `streaming.rs` | Chunked real-time transcription (3/8/15 sec intervals) |
| `history.rs` | Transcription storage: JSON metadata + WAV + TXT, retention policy |
| `model_downloader.rs` | Download GGML models from HuggingFace with progress tracking |
| `updater.rs` | Velopack auto-update from GitHub Releases feed |
| `config.rs` | TOML config parsing + typed structs with defaults |

### Shared contracts (`shared/contracts/`)

- `config.schema.json` — JSON Schema (draft 2020-12) for cross-platform config
- `pipeline_states.json` — canonical state machine definition
- `history_format.json` — history entry schema

### Config file location

- Windows: `%APPDATA%\dictator\config.toml`
- macOS: `~/Library/Application Support/dictator/` (UserDefaults, no TOML)
- Model files shared at `%LocalAppData%\whisper-models\` (Windows)

---

## CI/CD

- **`.github/workflows/windows.yml`** — runs on push/PR to `main` affecting `apps/windows/**` or `shared/**`; installs LLVM, runs `cargo check` then `cargo test --all-targets`
- **`.github/workflows/release.yml`** — triggered by `v*` tags; builds release binary, packages with Velopack (`vpk pack`), uploads `*-Setup.exe`, `*-full.nupkg`, `releases.stable.json` to GitHub Releases

---

## Design Principles (Anti-patterns to avoid)

- Overlay is shown **only during recording/transcription** — never as a persistent desktop widget
- Local mode must be **fully free and unlimited** — no subscription gates on offline features
- Request only **Microphone + Accessibility** — no screen capture unless explicitly opt-in
- **No hidden cloud processing** — clear visual separation between "Local Whisper" and "HTTP server"

---

## Related Projects

- **Contora** (`../contora/`) — .NET/WinUI 3 audio recorder. Shares the whisper server and model files on the same machine.
