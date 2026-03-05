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

### D3: Embedded Whisper (whisper-rs) ✅ DONE (2026-03-06)

**New default backend — no Python required.**

#### Architecture
- `whisper_engine.rs`: new module wrapping `whisper-rs 0.14`
- `SharedEngine = Arc<Mutex<Option<WhisperEngine>>>` — lazy load, idle unload
- `config.whisper.backend`: `"embedded"` (default) | `"server"` (legacy Python)

#### Changes
- `whisper_engine.rs`: `WhisperEngine::load()`, `transcribe()`, `SharedEngine` helpers
- `config.rs`: `WhisperBackend` enum, added to `WhisperConfig`
- `streaming.rs`: `StreamingTranscriber::new_embedded()` for embedded path
- `main.rs`: engine creation, backend-aware transcription, idle timer uses `unload_engine()`
- Model scan: `.bin` files for embedded, directories for server (legacy CTranslate2)
- `.cargo/config.toml`: `LIBCLANG_PATH` pointed to VS 2022 LLVM (for bindgen)

#### Prerequisites
- **Model**: download GGML `.bin` from https://huggingface.co/ggerganov/whisper.cpp
  - Place in `%LocalAppData%\whisper-models\` (shared with Contora)
  - Example: `ggml-large-v3.bin`, `ggml-medium.bin`
- **GPU**: build with `cargo build --features cuda` (requires CUDA Toolkit)
- **CPU**: default build, no extra dependencies

#### Migration from faster-whisper (server backend)
Add to `config.toml`:
```toml
[whisper]
backend = "server"  # keep using Python HTTP server
```
Or switch to embedded (recommended):
1. Download GGML model to `%LocalAppData%\whisper-models\`
2. Set `model_path` to the `.bin` file path
3. Remove or leave `backend = "embedded"` (it's the default)

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

## Sprint E: Windows Quality of Life ✅ DONE (2026-03-06)

### E1: Single-Instance Enforcement
- Named mutex `Global\DictatorSingleInstance` at startup
- If already running → MessageBox "Check system tray" + clean exit
- RAII guard `SingleInstanceGuard` releases mutex on app exit

### E2: Ollama Toggle in Tray
- `OLLAMA_ENABLED` AtomicBool in `ui.rs` — runtime toggle, independent of config.toml
- Initialized from `config.ollama.enabled` at startup
- "LLM Correction (Ollama)" menu item with checkmark
- Recording handler uses `ui::is_ollama_enabled()` (was `config.ollama.enabled`)

## Sprint F: UX Polish ✅ DONE (2026-03-06)

### F1: Whitespace Normalization (known faster-whisper bug)
- After transcription, `raw_text.split_whitespace().join(" ")` removes double spaces and trims
- Fixes upstream faster-whisper behavior of inserting double spaces between segments

### F2: Model Size in Tray Selector
- `model_dir_size_label()` computes total file size of model directory (single-level scan)
- `ModelMenuItem` now has `size_label: String` field, e.g. " (3.1 GB)"
- Displayed as "large-v3 (3.1 GB)" in tray model menu

## Sprint G: CLI Remote Control + About ✅ DONE (2026-03-06)

### G1: CLI --toggle / --stop via Named Windows Events
- `DictatorToggleEvent` and `DictatorStopEvent` named auto-reset events
- Main instance: `start_ipc_listener()` creates events and spawns thread, sends `RemoteToggle`/`RemoteStop` to hotkey channel
- CLI: parsed before single-instance check; `try_signal_remote()` opens + signals event
  - `dictator.exe --toggle` — toggles recording (shows dialog if not running)
  - `dictator.exe --stop`   — stops recording silently if active
- `RemoteToggle`/`RemoteStop` normalized to `RecordStart`/`RecordStop` in event loop

### G2: About Dialog in Tray
- "About Dictator" menu item above Exit
- Shows version, hotkey description, and "runs 100% locally" message via MessageBoxW

---

### F3: Long Recording Notification in Overlay
- After 30 seconds of active recording, overlay status appends:
  "Tip: tap hotkey again to stop"
- Helps users in PTT mode who forget to release

---

## Sprint H: In-App Model Downloader ✅ DONE (2026-03-06)

### H1: GGML Model Download from Tray
- `model_downloader.rs`: known models list (tiny/base/small/medium/large-v3-turbo/large-v3)
- Downloads from `huggingface.co/ggerganov/whisper.cpp` via `reqwest` blocking with streaming
- Atomic write: temp file + rename on completion (no partial/corrupt models)
- Tray: "Download Model ▶" popup submenu with checkmarks for already-downloaded models
- Progress shown in overlay: "Downloading large-v3 (47%)"
- After success: updates `config.toml model_path`, shows "restart" hint
- Startup hint: if no model found, overlay shows download instructions for 6 seconds
- `IS_DOWNLOADING` guard prevents concurrent downloads

---

### E3: Open Config File from Tray
- "Open Config File" menu item opens config.toml in Notepad
- Located between "Open Recordings Folder" and "Exit"

---

---

## Sprint I: Right Ctrl Hotkey + Hot Reload

### I1: Switch hotkey from Right Alt to Right Ctrl
- VK_RCONTROL (0xA3) instead of VK_RMENU (0xA5)
- Right Alt conflicts with Birman typographic keyboard layout

### I2: Hot reload after model switch (no restart)
- `Arc<RwLock<PathBuf>>` for active model path — shared between UI callbacks and event thread
- After model switch or download: update shared path + unload engine
- Next recording loads new model automatically — no restart needed

---

## Sprint K: Tray Cleanup

### K1: Slim down tray to quick actions only
- Remove streaming/chunk controls from tray (move to Settings window)
- Remove Download Model submenu from tray (move to Settings window)
- Keep: model selector (quick switch), last 3 recent recordings, Settings, Exit
- Add "Settings..." item that opens the settings window

---

## Sprint L: Settings Window

### L1: Native Win32 settings window
- Triggered from tray → "Settings..."
- Sections:
  - **Models** — list of downloaded models (name, size, active), download button, delete button
  - **Hotkey** — current hotkey display, picker
  - **General** — injection method, idle unload timeout, LLM toggle + Ollama URL/model
  - **About** — version, open logs folder, open config file, GitHub link
- Native Win32 controls (no external UI framework)

---

## Sprint M: Auto-Updater (Velopack)

### M1: Velopack integration
- Crate: `velopack` (same library Contora uses, has official Rust support)
- `VelopackApp::build().run()` at startup (required for update finalization)
- GitHub Releases as update channel (`https://github.com/iamniketas/dictator`)
- Background update check on startup (non-blocking thread)
- Tray: "Check for Updates" → "Update available (v1.x) — Install & Restart"
- After user approves: download + `apply_then_restart()`

### M2: Release packaging
- `vpk pack` script to build installer/package
- GitHub Actions workflow: build → pack → create release
- Distributable: single `.exe` installer (no manual build required)

---

## Priority Order

1. **Sprint A** (Windows P0) ✅ DONE
2. **Sprint B1-B2** (macOS modularization + WhisperKit) — macOS machine only
3. **Sprint C1** (shared models) ✅ DONE
4. **Sprint C2** (memory management) ✅ DONE
5. **Sprint C3** (history) ✅ DONE (earlier)
6. **Sprint E** (Windows quality of life) ✅ DONE
7. **Sprint F** (UX polish) ✅ DONE
8. **Sprint G** (CLI remote control + About) ✅ DONE
9. **Sprint D3** (embedded whisper-rs) ✅ DONE
10. **Sprint H** (in-app model downloader) ✅ DONE
11. **Sprint I** (Right Ctrl + hot reload) — in progress
12. **Sprint K** (tray cleanup) — depends on L
13. **Sprint L** (settings window) — unblocks K
14. **Sprint M** (Velopack auto-updater) — requires GitHub releases
15. **Sprint D1-D2** (model research) — informs future architecture
