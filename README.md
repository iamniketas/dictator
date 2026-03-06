# Dictator

**Local voice dictation** — converts speech to text using on-device AI models. No cloud, no subscriptions, no data leaves your machine.

Cross-platform project with native clients per OS.

## Platform Clients

| Platform | Stack | Status | Path |
|----------|-------|--------|------|
| **Windows** | Rust + Win32 API | v0.3.0 | [`apps/windows/`](apps/windows/) |
| **macOS** | Swift + SwiftUI/AppKit | Prototype | [`apps/macos/`](apps/macos/) |
| **Linux** | TBD | Planned | — |

## Windows — Quick Install

Download `dictator-win-Setup.exe` from [Releases](https://github.com/iamniketas/dictator/releases/latest) and run it.

The app lives in the system tray. Hold **Right Ctrl** to record (Push-to-Talk), or tap to toggle. Text is injected into the active window. Updates are delivered automatically.

## Core Pipeline (all platforms)

```
Hotkey (start/stop)
    -> Audio Capture (platform-native)
    -> Transcription (local ML model)
    -> [Optional] LLM Correction (Ollama)
    -> Text Injection (into active window)
```

## Transcription Engines

| Engine | Platform | Notes |
|--------|----------|-------|
| **whisper-rs (GGML)** | Windows | **Default.** Embedded, no Python required. CPU + CUDA. |
| WhisperKit | macOS | CoreML / Metal / ANE — Apple Silicon optimized |
| faster-whisper (Python HTTP) | Windows (legacy) | `backend = "server"` in config |
| Parakeet V3 (NVIDIA) | Windows, Linux | Research candidate |

## Shared Infrastructure

- **Whisper HTTP Server** — [`shared/whisper-server/`](shared/whisper-server/) — Python Flask server wrapping faster-whisper, shared with other local projects (legacy)
- **Contracts** — [`shared/contracts/`](shared/contracts/) — cross-platform config schema, pipeline states, history format
- **Documentation** — [`docs/`](docs/)

## Design Principles

1. **Local-first** — all processing on-device, zero cloud dependency
2. **Native UX** — platform-specific UI following each OS guidelines
3. **Minimal footprint** — tray/menubar app, overlay only during recording
4. **Resource-aware** — auto-unload models from VRAM after idle (default: 5 min)
5. **Open** — Apache 2.0 license, forkable architecture

## Anti-patterns (what we deliberately avoid)

- Persistent desktop widgets
- Paywalls on local features
- Hidden cloud processing
- Excessive system permissions
- In-app marketing

## Quick Start

### Windows (from source)
```bash
cd apps/windows
cargo build --release
# Requires LLVM for whisper-rs bindgen (see apps/windows/README.md)
```

### macOS
```bash
cd apps/macos
swift build
# See apps/macos/README.md for full instructions
```

## Project Structure

```
dictator/
  apps/
    windows/          # Rust + Win32 native client (v0.3.0)
    macos/            # Swift + SwiftUI/AppKit native client (prototype)
  shared/
    whisper-server/   # Python HTTP server for faster-whisper (legacy)
    contracts/        # Cross-platform schemas and agreements
  docs/
    MACOS_ROADMAP.md
    archive/          # Historical session reports
  ROADMAP.md          # Feature prioritization and phases
  ARCHITECTURE.md     # Technical architecture overview
  NEXT_SPRINTS.md     # Development sprint plan
  CLAUDE.md           # AI assistant instructions
```

## Roadmap

See [NEXT_SPRINTS.md](NEXT_SPRINTS.md) for the detailed sprint plan.

**Phase 1 (MVP+):** ✅ Smart hotkeys, waveform overlay, flexible injection, model selector, embedded whisper, settings window, auto-updater
**Phase 2 (Polish):** History ✅, dictionary editor, LLM post-processing improvements
**Phase 3 (Advanced):** Custom dictionary editor GUI, Command Mode (AI text editing)

## Related Projects

- **Contora** — local audio recording and transcription app (shares whisper models directory on the same machine)

## License

Apache 2.0 — see [LICENSE](LICENSE) file for details.

## Credits

- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) / [whisper-rs](https://github.com/tazz4843/whisper-rs) — Embedded Whisper inference
- [faster-whisper](https://github.com/SYSTRAN/faster-whisper) — Fast Whisper implementation (legacy backend)
- [WhisperKit](https://github.com/argmaxinc/WhisperKit) — On-device speech recognition for Apple Silicon
- [cpal](https://github.com/RustAudio/cpal) — Cross-platform audio I/O
- [windows-rs](https://github.com/microsoft/windows-rs) — Rust bindings for Windows API
- [Velopack](https://velopack.io) — Auto-update framework
- [Handy](https://github.com/cjpais/Handy) — Open-source voice dictation (architecture reference)
