# Dictator

**Local voice dictation** — converts speech to text using on-device AI models. No cloud, no subscriptions, no data leaves your machine.

Cross-platform project with native clients per OS.

## Platform Clients

| Platform | Stack | Status | Path |
|----------|-------|--------|------|
| **Windows** | Rust + Win32 API | v0.1.0-alpha | [`apps/windows/`](apps/windows/) |
| **macOS** | Swift + SwiftUI/AppKit | Prototype | [`apps/macos/`](apps/macos/) |
| **Linux** | TBD | Planned | — |

## Core Pipeline (all platforms)

```
Hotkey (start/stop)
    -> Audio Capture (platform-native)
    -> Transcription (local ML model)
    -> [Optional] LLM Correction (Ollama)
    -> Text Injection (into active window)
```

## Transcription Engines

| Engine | Platform | Acceleration | Notes |
|--------|----------|-------------|-------|
| faster-whisper (Python HTTP) | Windows, Linux | CUDA | Current default |
| WhisperKit | macOS | CoreML / Metal / ANE | Apple Silicon optimized |
| Parakeet V3 (NVIDIA) | Windows, Linux | CUDA / CPU | Research candidate |
| whisper.cpp | All | CPU / CUDA / Metal | Fallback option |

## Shared Infrastructure

- **Whisper HTTP Server** — [`shared/whisper-server/`](shared/whisper-server/) — Python Flask server wrapping faster-whisper, shared with other local projects
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

### Windows
```bash
cd apps/windows
cargo build --release
# See apps/windows/README.md for full instructions
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
    windows/          # Rust + Win32 native client
    macos/            # Swift + SwiftUI/AppKit native client
  shared/
    whisper-server/   # Python HTTP server for faster-whisper
    contracts/        # Cross-platform schemas and agreements
  docs/
    MACOS_ROADMAP.md
    REPO_STRUCTURE_PLAN.md
    archive/          # Historical session reports
  ROADMAP.md          # Feature prioritization and phases
  ARCHITECTURE.md     # Technical architecture overview
  CLAUDE.md           # AI assistant instructions
```

## Roadmap

See [ROADMAP.md](ROADMAP.md) for detailed feature plan.

**Phase 1 (MVP+):** Smart hotkeys, audio waveform overlay, flexible text injection, model selector GUI
**Phase 2 (Polish):** History, memory management, LLM post-processing toggle
**Phase 3 (Advanced):** Custom dictionary editor, Command Mode (AI text editing)

## Related Projects

- **Contora** — local audio recording and transcription app (shares whisper runtime and models on the same machine)

## License

Apache 2.0 — see [LICENSE](LICENSE) file for details.

## Credits

- [faster-whisper](https://github.com/SYSTRAN/faster-whisper) — Fast Whisper implementation
- [WhisperKit](https://github.com/argmaxinc/WhisperKit) — On-device speech recognition for Apple Silicon
- [cpal](https://github.com/RustAudio/cpal) — Cross-platform audio I/O
- [windows-rs](https://github.com/microsoft/windows-rs) — Rust bindings for Windows API
- [Handy](https://github.com/cjpais/Handy) — Open-source voice dictation (architecture reference)
