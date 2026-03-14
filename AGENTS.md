# AGENTS.md

Instructions for Codex (Codex.ai/code) working in this repository.

## Language Preferences
- ALWAYS respond to the user in Russian.
- Keep technical terms and code snippets in their original form (English).

---

## Project Overview

Dictator is a **multi-platform local voice dictation** app. Each platform has a native client.

### Repository Structure

```
dictator/
  apps/
    windows/          # Rust + Win32 native client (v0.1.0-alpha)
      Cargo.toml
      src/
      examples/
      assets/
    macos/            # Swift + SwiftUI/AppKit (prototype)
      Package.swift
      Sources/
  shared/
    whisper-server/   # Python HTTP server for faster-whisper
    contracts/        # Cross-platform schemas
  docs/
    MACOS_ROADMAP.md
    REPO_STRUCTURE_PLAN.md
    archive/          # Historical session reports
  ROADMAP.md          # Feature prioritization
  ARCHITECTURE.md     # Technical architecture
  README.md           # Project hub
```

### Build Commands

```bash
# Windows client
cd apps/windows && cargo build

# macOS client
cd apps/macos && swift build

# Whisper server
python shared/whisper-server/whisper_server.py
```

---

## Roadmap Reference

See [ROADMAP.md](ROADMAP.md) for feature priorities:
- **Phase 1 (MVP+):** Smart hotkeys, waveform overlay, flexible injection, model selector
- **Phase 2 (Polish):** History, memory management (5 min default, configurable), LLM toggle
- **Phase 3 (Advanced):** Dictionary editor, Command Mode

For competitor details: see [COMPETITORS_ANALYSIS.md](COMPETITORS_ANALYSIS.md)

---

## Related Projects

- **Contora** (`../contora/`) — local audio recording and transcription (.NET/WinUI 3). Shares whisper runtime and models on Windows.
