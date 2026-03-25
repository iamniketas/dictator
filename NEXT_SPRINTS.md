# Next Sprints — Dictator Development Plan

> Last updated: 2026-03-06

---

## Current State

- **Windows (Rust):** v0.3.0 in `apps/windows/`, build: `cd apps/windows && cargo build`
- **macOS (Swift):** Prototype in `apps/macos/`, single-file `main.swift` (~60KB)
- **Shared:** Whisper server in `shared/whisper-server/`, contracts in `shared/contracts/`
- **CI:** GitHub Actions — Windows CI on push, Release workflow on `v*` tags

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

---

## Sprint B: macOS — MVP Foundation ✅ DONE (2026-03-25)

**Goal:** Get macOS client to feature parity with Windows basic pipeline.

### B1: Modularize main.swift ✅
- Split monolithic 1400-line `main.swift` into proper Swift modules:
  - `Models/AppModel.swift` — main state machine
  - `Services/AudioCaptureService.swift`
  - `Services/TranscriptionService.swift` — protocol + HTTP impl
  - `Services/WhisperKitService.swift` — on-device WhisperKit impl
  - `Services/HotkeyManager.swift` — CGEventTap + Carbon fallback
  - `Services/TextInjectionService.swift`
  - `Services/RecordingArchiveService.swift`
  - `Services/SettingsStore.swift` — UserDefaults persistence
  - `UI/AppDelegate.swift`, `UI/DashboardView.swift`, `UI/SettingsView.swift`
  - `Extensions/DataExtensions.swift`
  - `DictatorMacApp.swift` — SwiftUI App entry

### B2: WhisperKit Integration ✅
- Added `WhisperKit` (0.17.0) to Package.swift dependencies
- `WhisperKitTranscriptionService`: on-device CoreML/Metal/ANE transcription
- Both backends implement `TranscriptionService` protocol
- `AppModel` switches between backends without restart
- Default backend: WhisperKit; HTTP server as fallback

### B3: Text Injection Reliability ✅
- `TextInjectionService`: extracted from AppModel, standalone class
- CGEvent Cmd+V (two taps) + AppleScript fallback
- Retry at 140ms / 320ms / 620ms / 1s / 2s after activation

### B4: Streaming Transcription ✅ (carried over from before, still working)
- Chunk-based pipeline (3/8/15 s) preserved in refactor

### Additional improvements in Sprint B:
- **HotkeyManager**: CGEventTap-based global hotkey with smart mode (tap=toggle, hold≥300ms=PTT). Carbon API as Accessibility fallback.
- **SettingsStore**: UserDefaults persistence for all settings (backend, language, model, endpoint, streaming, chunk size)
- **SettingsView**: full redesign with tabbed UI — Transcription (backend picker, WhisperKit model selector + load/unload), Recording (streaming, hotkey info, permissions), About

---

## Sprint C: Shared Infrastructure

### C1: Shared Whisper Models Directory ✅ DONE (2026-03-06)
- Both Dictator and Contora point to same model files
- Resolution order: config `models_dir` → `WHISPER_MODELS_DIR` env var → `%LocalAppData%\whisper-models\` (if exists) → `model_path` parent
- Environment variable override: `WHISPER_MODELS_DIR`

### C2: Memory Management ✅ DONE (2026-03-06)
- Auto-unload whisper engine after idle (default: 5 min)
- Config: `[memory] idle_unload_minutes` (0 = never)
- `idle_unload_minutes` configurable via Settings window

### C3: History Module ✅ DONE (earlier)
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
  - whisper-rs GGML large-v3 (CPU + CUDA)
  - Parakeet V3 (CPU + CUDA)
  - WhisperKit (Apple Silicon)
  - Distil-Whisper (faster, lower quality)
- Metrics: latency, WER (word error rate), VRAM usage, CPU usage

### D3: Embedded Whisper (whisper-rs) ✅ DONE (2026-03-06)

**New default backend — no Python required.**

#### Architecture
- `whisper_engine.rs`: module wrapping `whisper-rs 0.14`
- `SharedEngine = Arc<Mutex<Option<WhisperEngine>>>` — lazy load, idle unload
- `config.whisper.backend`: `"embedded"` (default) | `"server"` (legacy Python)

#### Prerequisites
- **Model**: download GGML `.bin` from https://huggingface.co/ggerganov/whisper.cpp
  - Place in `%LocalAppData%\whisper-models\` (shared with Contora)
  - Or download from within the app: Settings → Download Model
- **GPU**: build with `cargo build --features cuda` (requires CUDA Toolkit)
- **CPU**: default build, no extra dependencies

---

## Sprint E: Windows Quality of Life ✅ DONE (2026-03-06)

### E1: Single-Instance Enforcement
- Named mutex `Global\DictatorSingleInstance` at startup
- If already running → MessageBox "Check system tray" + clean exit

### E2: Ollama Toggle in Tray → moved to Settings window
- Runtime toggle for LLM correction, independent of config.toml

### E3: Open Config File from Tray → moved to Settings window (About section)

---

## Sprint F: UX Polish ✅ DONE (2026-03-06)

### F1: Whitespace Normalization
- `raw_text.split_whitespace().join(" ")` after transcription
- Fixes faster-whisper double-space issue

### F2: Model Size in Tray Selector
- Shows e.g. "large-v3 (3.1 GB)" in tray model menu

### F3: Long Recording Notification
- After 30s of recording, overlay appends: "Tip: tap hotkey again to stop"

---

## Sprint G: CLI Remote Control + About ✅ DONE (2026-03-06)

### G1: CLI --toggle / --stop via Named Windows Events
- `dictator.exe --toggle` — toggles recording
- `dictator.exe --stop` — stops recording silently

### G2: About Dialog
- Moved to Settings window → About section

---

## Sprint H: In-App Model Downloader ✅ DONE (2026-03-06)

### H1: GGML Model Download
- `model_downloader.rs`: known models list (tiny/base/small/medium/large-v3-turbo/large-v3)
- Downloads from `huggingface.co/ggerganov/whisper.cpp` via streaming HTTP
- Atomic write: temp file + rename on completion
- Progress shown in overlay: "Downloading large-v3 (47%)"
- After success: hot-switches to new model (no restart needed)
- Startup hint: if no model found, overlay shows instructions for 6 seconds

---

## Sprint I: Right Ctrl Hotkey + Hot Reload ✅ DONE (2026-03-06)

### I1: Switch hotkey from Right Alt to Right Ctrl
- VK_RCONTROL (0xA3) instead of VK_RMENU (0xA5)
- Right Alt conflicts with Birman typographic keyboard layout

### I2: Hot model reload without restart
- `Arc<RwLock<PathBuf>>` shared between UI and event thread
- Model switch or download → update path + unload engine → next recording auto-loads new model

---

## Sprint K: Tray Cleanup ✅ DONE (2026-03-06)

### K1: Slim tray — quick actions only
- Removed: streaming controls, chunk size, download submenu
- Kept: model quick-switch, last 3 recordings, Open Recordings Folder, Settings, Exit
- Added "Settings..." → opens settings window
- Added "Install Update vX.X" (shown only when update is available)

---

## Sprint L: Native Win32 Settings Window ✅ DONE (2026-03-06)

### L1: Settings window
- `settings_window.rs` — native Win32 window, no external UI framework
- Triggered from tray → "Settings..."
- Sections:
  - **Models** — installed list, Use / Delete buttons
  - **Download** — model dropdown + Download button
  - **General** — injection method, idle unload, LLM toggle, Ollama URL/model
  - **About** — version, hotkey, Open Logs, Open Config
- `GWLP_USERDATA` pattern for per-window state, `WM_SETFONT` for system font

---

## Sprint M: Auto-Updater (Velopack) ✅ DONE (2026-03-06)

### M1: Velopack integration
- `updater.rs` with `velopack::sources::HttpSource` → GitHub Releases feed
- `VelopackApp::build().run()` as first call in `main()`
- Background update check on startup
- Tray: "Install Update vX.X" shown when update available
- On click: download + `apply_updates_and_restart()`

### M2: GitHub Actions release workflow
- `.github/workflows/release.yml` — triggers on `v*` tag push
- Steps: checkout → LLVM install → `cargo build --release` → `vpk pack` → GitHub Release
- Publishes: `dictator-win-Setup.exe`, `dictator-win-Portable.zip`, `dictator-X.X.X-full.nupkg`, `releases.win.json`
- First release: v0.3.0

---

## Sprint N: macOS — MLX Backend + Model Catalog ✅ DONE (2026-03-25)

**Commit:** `57feafb`

### N1: MLX Transcription Backend ✅
- `MLXTranscriptionService`: `POST /v1/audio/transcriptions` (OpenAI-compatible multipart)
- Auto-reads Contora shared config from `~/Library/Application Support/NiketasAI/runtime/transcription-server.json`
- `SharedRuntimePaths` + `SharedTranscriptionServerConfig` mirror Contora's path resolution
- `AppModel.probeMlxServer()`: async `GET /v1/models` reachability check
- `TranscriptionBackend` enum: `whisperkit | mlx | http`
- Settings → Transcription: MLX section with live indicator, model picker, Check button
- Endpoint URL + model ID stored in `UserDefaults`, auto-populated from Contora config on first launch

### N2: WhisperKit Model Catalog Expansion ✅
- 7 → 12 models: added `tiny.en`, `base.en`, `small.en`, `medium.en` (English-only, faster)
- Added `distil-whisper/distil-large-v3 ⚡` (~2× faster than large-v3, minimal quality tradeoff)
- Sorted smallest → largest

### N3: MLX Recommended Model Presets ✅
Four presets in Settings:
- `mlx-community/whisper-large-v3-turbo-asr-fp16` ✦ — Contora default, best quality/speed
- `mlx-community/distil-whisper-large-v3` — 2× faster
- `mlx-community/whisper-large-v3-mlx-fp16` — maximum accuracy
- `mlx-community/whisper-small-mlx-q4` — ultra-fast, older M1

---

## Up Next

### Sprint O: macOS — History View
- SwiftUI window listing past transcriptions from `RecordingArchiveService` (WAV + JSON already saved)
- Columns: timestamp, duration, transcript text preview
- Actions: Copy Text, Play Audio, Delete
- Accessible from tray menu: "History..."

### Sprint P: macOS — Memory Auto-Unload + Permission Re-Check
- WhisperKit idle unload timer (configurable: 0/5/10/30 min), mirrors Windows `C2`
- Persist ticker window position in `UserDefaults` on `windowDidMove`
- Accessibility permission re-check: subscribe to `NSWorkspace` app-switch notifications

### Sprint Q: macOS — Injection Method Choice
- Settings → "After transcription": Clipboard + Cmd+V (current) / Clipboard only / Clipboard + Enter
- `clipboard_enter` useful for messengers (auto-send)

### Sprint R: Custom Dictionary Editor (cross-platform)
- macOS: SwiftUI editor for custom word substitutions passed to WhisperKit initial prompt
- Windows: extend existing Settings window

### Sprint S: Command Mode (Research)
- Capture selected text + voice command → LLM rewrite → inject result
- Investigate Accessibility API for selected-text capture on both platforms

---

## Priority Order

1. **Sprint A** ✅ DONE
2. **Sprint C1** ✅ DONE
3. **Sprint C2** ✅ DONE
4. **Sprint C3** ✅ DONE
5. **Sprint E** ✅ DONE
6. **Sprint F** ✅ DONE
7. **Sprint G** ✅ DONE
8. **Sprint D3** ✅ DONE
9. **Sprint H** ✅ DONE
10. **Sprint I** ✅ DONE
11. **Sprint K** ✅ DONE
12. **Sprint L** ✅ DONE
13. **Sprint M** ✅ DONE
14. **Sprint B** ✅ DONE (macOS MVP)
15. **Sprint N** ✅ DONE (macOS MLX + model catalog)
16. **Sprint O** — macOS History View ← next
17. **Sprint P** — macOS Memory Unload + Permission Re-Check
18. **Sprint Q** — macOS Injection Method Choice
19. **Sprint R** — Custom Dictionary Editor
20. **Sprint S** — Command Mode (research)
