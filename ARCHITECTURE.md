# Dictator — Architecture

## Multi-Platform Strategy: Native per Platform

Each platform gets a fully native client. Shared behavior is enforced through contracts, not shared code.

```
dictator/
  apps/
    windows/    Rust + Win32 API + cpal + faster-whisper (CUDA)
    macos/      Swift + SwiftUI/AppKit + WhisperKit (CoreML/Metal/ANE)
    linux/      (planned) Rust or C++ + GTK4 + faster-whisper (CUDA/CPU)
  shared/
    whisper-server/   Python HTTP server (transitional, shared with Contora)
    contracts/        Config schema, pipeline states, history format
```

### Why not a single codebase (Tauri, Electron, etc.)?

1. **UI** — Win32, SwiftUI, GTK are fundamentally different paradigms. Cross-platform wrappers produce non-native UX.
2. **ML acceleration** — CUDA (Win/Linux) vs Metal/ANE (macOS) require different engines and optimization paths.
3. **System integration** — Text injection (SendInput vs CGEvent vs XTest), hotkeys (Win32 hooks vs EventTap vs X11), tray (Win32 vs NSStatusItem vs StatusNotifier) are all platform-specific.
4. **Performance** — Native code eliminates runtime overhead. Dictator must be instant and lightweight.

### What IS shared (through contracts):

- **Pipeline state machine:** idle -> recording -> transcribing -> correcting -> injecting
- **Config schema:** unified keys (hotkey, whisper, ollama, streaming, memory)
- **History format:** JSON with text, audio path, timestamps, language
- **Whisper Server API:** HTTP multipart (WAV + params) -> JSON response
- **Ollama API:** standard Ollama HTTP protocol

## Platform Architecture Details

### Windows (Rust + Win32)

```
System Tray (Win32 Shell_NotifyIcon)
  -> Global Hotkey (RegisterHotKey / low-level keyboard hook)
  -> Audio Capture (cpal, 16kHz mono)
  -> Streaming chunks -> HTTP -> Whisper Server (faster-whisper, CUDA)
  -> [Optional] Ollama LLM correction
  -> Overlay Window (Win32 Layered Window, softbuffer rendering)
  -> Text Injection (SendInput, KEYEVENTF_UNICODE)
```

**Tech stack:** Rust, windows-rs 0.58, cpal, tokio, reqwest, crossbeam-channel

### macOS (Swift + SwiftUI/AppKit)

```
Menu Bar App (NSStatusItem)
  -> Global Hotkey (Carbon EventTap)
  -> Audio Capture (AVAudioEngine)
  -> TranscriptionService (WhisperKit / CoreML / Metal / ANE)
  -> [Optional] Ollama LLM correction
  -> Text Injection (Pasteboard + CGEvent Cmd+V)
```

**Tech stack:** Swift, SwiftUI, AppKit, WhisperKit, AVFoundation

### Linux (planned)

```
System Tray (StatusNotifierItem / libappindicator)
  -> Global Hotkey (X11 XGrabKey / Wayland portal)
  -> Audio Capture (PipeWire / PulseAudio via cpal)
  -> Transcription (faster-whisper CUDA or Parakeet CPU)
  -> Text Injection (xdotool / wtype / ydotool)
```

## Transcription Engine Strategy

| Platform | Primary | Fallback | Acceleration |
|----------|---------|----------|-------------|
| Windows | faster-whisper (HTTP) | whisper.cpp | CUDA (RTX series) |
| macOS | WhisperKit | whisper.cpp (Metal) | CoreML + ANE (Apple Silicon) |
| Linux | faster-whisper (HTTP) | Parakeet V3 (CPU) | CUDA or CPU-only |

### Model candidates for evaluation:

- **faster-whisper large-v2/v3** — current default, CUDA, high quality
- **WhisperKit** — Apple Silicon native, CoreML/ANE optimized
- **Parakeet V3** (NVIDIA NeMo) — CPU-friendly, auto language detection, ~5x realtime
- **Distil-Whisper** — faster inference, slightly lower quality
- **whisper.cpp** — universal fallback, supports CPU/CUDA/Metal

## Shared Whisper Server

The Python HTTP server (`shared/whisper-server/`) is a transitional component:

- Used by Dictator (Windows) and Contora for transcription
- Wraps faster-whisper with Flask
- Keeps model loaded in VRAM for fast response
- Both apps can share the same server instance and model files

Long-term: platform clients may embed transcription directly (whisper-rs, WhisperKit).

## Resource Management

- Models auto-unload from VRAM/RAM after configurable idle timeout (default: 5 min)
- Option to disable auto-unload ("never" setting)
- Pre-warm on hotkey press to minimize latency after sleep
- Memory budget monitoring per platform

## Development Coordination

Each platform is developed independently with:
- Separate CI pipelines (`.github/workflows/`)
- Platform-specific branches (`feature/windows-*`, `feature/macos-*`)
- Independent release tags (`windows/vX.Y.Z`, `macos/vX.Y.Z`)
- Shared contracts versioned in `shared/contracts/`
