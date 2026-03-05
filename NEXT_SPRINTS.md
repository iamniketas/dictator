# Next Sprints — Dictator Development Plan

> Last updated: 2026-03-05
> Repository migrated to multi-platform structure.

---

## Current State

- **Windows (Rust):** v0.1.0-alpha in `apps/windows/`, build: `cd apps/windows && cargo build`
- **macOS (Swift):** Prototype in `apps/macos/`, single-file `main.swift` (~60KB)
- **Shared:** Whisper server in `shared/whisper-server/`, contracts in `shared/contracts/`
- **CI:** GitHub Actions for Windows (`apps/windows/` scope)

---

## Sprint A: Windows — Phase 1 MVP+ ✅ DONE (2026-03-05)

**Commit:** `c666d24`

### A1: Smart Hotkeys ✅
- LL keyboard hook (`SetWindowsHookExW`) — key down + key up events
- Hold >300ms = Push-to-Talk (stops on release)
- Tap <300ms = Toggle (stops on next press)
- Key suppressed from target app

### A2: Audio Waveform Overlay ✅
- Lock-free RMS amplitude via `AtomicU32` in audio callback
- 20-bar waveform, 30fps dedicated thread
- Replaces blinking dot

### A3: Flexible Text Injection ✅
- `[injection] method = direct|clipboard|clipboard_enter`
- clipboard mode preserves and restores original clipboard
- `clipboard_enter` appends Enter after paste

### A4: Model Selector in Tray ✅
- Scans `whisper.models_dir` (defaults to parent of `model_path`)
- Checkmark on active model, click saves to config.toml
- New optional config field `whisper.models_dir`

### Known issues found during testing
- Faster-whisper inserts double spaces between segments (upstream behavior)
  → Can fix with `.replace("  ", " ")` post-processing if needed

---

## Sprint B: macOS — MVP Foundation

**Goal:** Get macOS client to feature parity with Windows basic pipeline.

### B1: Modularize main.swift
- Split 60KB single file into proper Swift modules:
  - `AudioCaptureService.swift`
  - `TranscriptionService.swift`
  - `HotkeyManager.swift`
  - `TextInjectionService.swift`
  - `OverlayView.swift`
  - `SettingsStore.swift`
  - `AppState.swift`

### B2: WhisperKit Integration
- Replace HTTP server dependency with WhisperKit (CoreML/Metal/ANE)
- Evaluate performance on M1/M2/M3
- Keep HTTP fallback for older Macs without ANE

### B3: Text Injection Reliability
- Pasteboard + CGEvent (Cmd+V) as primary method
- Accessibility permission flow with onboarding guide
- Fallback: clipboard-only mode

### B4: Streaming Transcription
- Chunk-based pipeline matching Windows behavior
- Overlay/status panel for partial results

---

## Sprint C: Shared Infrastructure

### C1: Shared Whisper Models Directory ✅ DONE (2026-03-06)
- Both Dictator and Contora point to same model files
- Resolution order: config `models_dir` → `WHISPER_MODELS_DIR` env var → `%LocalAppData%\whisper-models\` (if exists) → `model_path` parent
- Environment variable override: `WHISPER_MODELS_DIR`

### C2: Memory Management ✅ DONE (2026-03-06)
- Auto-unload whisper server after idle (default: 5 min)
- Config: `[memory] idle_unload_minutes` (0 = never)
- Server stays running between recordings; idle timer stops it
- `stop_if_owned()` retained only on hard errors (audio/transcription failure)

### C3: History Module (P1) ✅ DONE (earlier)
- JSON metadata + WAV audio + TXT per recording in `recordings/YYYY-MM-DD/`
- Configurable retention_days (default 7), max_recent in tray (default 5)
- "Open Folder" and "Copy to Clipboard" tray actions

---

## Sprint D: Research & Evaluation

### D1: Parakeet V3 (NVIDIA NeMo)
- Evaluate as alternative/complement to Whisper
- CPU-friendly (~5x realtime), auto language detection
- Reference: Handy integration (MIT licensed)

### D2: Model Comparison Matrix
- Test on identical audio samples:
  - faster-whisper large-v2 (CUDA)
  - Parakeet V3 (CPU + CUDA)
  - WhisperKit (Apple Silicon)
  - Distil-Whisper (faster, lower quality)
- Metrics: latency, WER (word error rate), VRAM usage, CPU usage

### D3: Embedded Whisper (whisper-rs)
- Replace HTTP server with direct Rust binding
- Eliminates Python dependency on Windows
- Requires VS 2022 with C++ toolchain

---

## Questions Inspired by Handy Research

### Architecture
1. **Parakeet V3 integration** — Handy shows it works well for CPU-only users. Should we support it as a lightweight alternative for machines without NVIDIA GPU?
2. **Silero VAD** — Handy uses Silero for voice activity detection. Our current VAD relies on Whisper's built-in. Is dedicated VAD worth the complexity for better pause detection?
3. **whisper-rs vs HTTP server** — Handy embeds whisper.cpp directly via `whisper-rs`. This eliminates the Python dependency but requires C++ toolchain. When should we make this transition for Windows?

### UX
4. **Remote control via CLI** — Handy supports `--toggle`, `--stop` flags for scripting. Should Dictator expose a local socket/pipe for automation?
5. **Debug mode** — Handy has a keyboard shortcut to toggle debug overlay. Should we add similar diagnostics (latency, VRAM, model info)?

### Models
6. **Model auto-download** — Handy downloads models on demand from the app. Should we build a model manager (download, verify, switch) instead of requiring manual setup?
7. **Model size tiers** — Handy offers small/medium/turbo/large. Should our model selector show speed/quality/size comparison?

### Cross-platform
8. **Wayland support** — Handy documents significant issues with Wayland (hotkeys, text injection). For our Linux client, should we target X11 first and Wayland as best-effort?
9. **Single-instance enforcement** — Handy uses a plugin for this. We should implement named mutex (Win) / file lock (Unix) to prevent duplicate instances.

---

## Priority Order

1. **Sprint A** (Windows P0) ✅ DONE
2. **Sprint B1-B2** (macOS modularization + WhisperKit) — macOS machine only
3. **Sprint C1** (shared models) ✅ DONE
4. **Sprint C2** (memory management) ✅ DONE
5. **Sprint C3** (history) ✅ DONE (earlier)
6. **Sprint D1-D2** (model research) — informs future architecture
7. **Sprint D3** (embedded whisper-rs) — eliminates Python dependency
