# macOS Sprint B Readiness Audit and Plan

Date: 2026-03-06
Branch: codex/macos-sprint-b-foundation

## Phase 0 status

Implemented in this branch:

- Module scaffold directories:
  - `apps/macos/Sources/DictatorMac/Core/`
  - `apps/macos/Sources/DictatorMac/Services/`
  - `apps/macos/Sources/DictatorMac/UI/`
  - `apps/macos/Sources/DictatorMac/Infrastructure/`
- Local smoke script:
  - `scripts/macos/smoke_build.sh`
- CI draft workflow:
  - `.github/workflows/macos-smoke.yml`
- Native Apple + overlay design constraints:
  - `docs/MACOS_UI_GUIDELINES.md`

## B1 status

Implemented in this branch:

- `DictatorMacApp.swift` reduced to composition root (`@main`).
- App state moved to `Core/AppState.swift`.
- Hotkey manager moved to `Core/HotkeyManager.swift`.
- Services extracted:
  - `Services/AudioCaptureService.swift`
  - `Services/TranscriptionService.swift`
  - `Services/TextInjectionService.swift`
- UI extracted:
  - `UI/AppDelegate.swift`
  - `UI/OverlayView.swift`
  - `UI/SettingsView.swift`
- Infrastructure extracted:
  - `Infrastructure/SettingsStore.swift`
  - `Infrastructure/RecordingArchiveService.swift`

Validation:

- `./scripts/macos/smoke_build.sh` passes.

## B2 status

Implemented in this branch:

- Added `WhisperKit` SwiftPM dependency in `apps/macos/Package.swift`.
- Added backend preference model: `auto | whisperKit | http`.
- Added `Services/WhisperKitTranscriptionService.swift`.
- Kept `Services/TranscriptionService.swift` as HTTP backend and unified protocol.
- Added runtime fallback behavior:
  - `auto` picks WhisperKit when available, otherwise HTTP.
  - if WhisperKit fails at runtime, app retries transcription via HTTP backend (when endpoint is valid).
- Added backend preference persistence in `Infrastructure/SettingsStore.swift`.
- Exposed backend selector in `UI/SettingsView.swift`.

Validation:

- `cd apps/macos && swift build` passes.

## B3 status

Implemented in this branch:

- Added text injection modes:
  - `pasteAndSend`
  - `clipboardOnly`
- Added deterministic fallback behavior in `Core/AppState.swift`:
  - if accessibility is not granted, transcript is copied to clipboard and auto-paste is skipped.
- Added injection status tracking in UI (`lastInjectionStatus`).
- Added permissions onboarding section in `UI/SettingsView.swift`:
  - live permission status,
  - request buttons,
  - direct links to macOS privacy settings.
- Added quick menu entry in tray menu:
  - `Permissions Setup...`

Validation:

- `cd apps/macos && swift build` passes.
- `./scripts/macos/smoke_build.sh` passes.

## B4 status

Implemented in this branch:

- Extracted streaming orchestration into dedicated service:
  - `Services/StreamingTranscriptionCoordinator.swift`
- Removed ad-hoc loop/chunk state machine from `Core/AppState.swift`.
- Added single event handler path in `AppState`:
  - `handleStreamingEvent(_:)`
  - all streaming status + partial transcript updates now flow through one event stream.
- Kept chunk-based behavior and finalization fallback:
  - final chunk flush on stop,
  - fallback to full transcription when streaming result is empty.

Validation:

- `cd apps/macos && swift build` passes.

## Context from project docs

- `NEXT_SPRINTS.md` defines Sprint B (B1-B4) as macOS MVP Foundation and parity target with Windows basic pipeline.
- `ROADMAP.md` P0 priorities: smart hotkeys, waveform overlay, flexible injection, model selection.
- `docs/MACOS_ROADMAP.md` sets native stack direction: SwiftUI/AppKit + WhisperKit-first + fallback + off-main-thread services.

## Current macOS implementation (fact check)

Previous monolith was split during B1. Composition root is:

- `apps/macos/Sources/DictatorMac/DictatorMacApp.swift`

What is already implemented:

- Menu bar app and status menu.
- Global hotkey registration (Cmd+Shift+D / Ctrl+Shift+D).
- Real microphone capture via `AVAudioEngine`.
- Chunk streaming loop with partial accumulation and final fallback.
- Archive of recordings and transcript JSON metadata.
- Pasteboard + synthetic Cmd+V text injection attempts.
- Permission checks for microphone and accessibility.

Main architectural risks right now:

1. Monolith in one file couples UI, state machine, audio, transport, persistence, and injection.
2. Transcription is HTTP-only (`WhisperHTTPTranscriptionService`) and still depends on server endpoint config.
3. No backend abstraction for WhisperKit vs HTTP fallback.
4. Hotkey behavior is single toggle path only; no hold-vs-tap strategy like Windows.
5. Injection path is best-effort and lacks explicit strategy modes and deterministic fallback state.
6. Settings are runtime-only (`@Published`) without proper persistent `SettingsStore`.

## Parity gap vs Windows MVP baseline

Windows (`apps/windows`) already has:

- Smart hotkey logic (hold=PTT, tap=toggle).
- Overlay with waveform amplitude updates.
- Configurable injection methods (`direct|clipboard|clipboard_enter`).
- Mature settings/config surface and runtime toggles.

macOS currently lacks full parity in:

- Hotkey semantics.
- Reliable, strategy-based injection pipeline.
- Native model backend (WhisperKit) with fallback abstraction.
- Modular architecture required to scale Sprint B safely.

## Readiness verdict

Current macOS state is functionally promising but architecturally not ready for direct feature growth without refactor.

Sprint B should start with architecture-first stabilization (B1), then backend and reliability work (B2/B3), and only then finalize behavior parity polish under B4.

## Execution plan (to make macOS Sprint-ready)

### Phase 0: Stabilization gate (pre-B1, 0.5-1 day)

- Freeze feature additions in `DictatorMacApp.swift`.
- Introduce folder structure under `apps/macos/Sources/DictatorMac/`:
  - `Services/`
  - `Core/`
  - `UI/`
  - `Infrastructure/`
- Add minimal compile-time smoke workflow for macOS target (local script or CI job draft).

Exit criteria:

- Build is green after pure file moves + zero behavior change.

### Phase 1: B1 Modularization (1-2 days)

Refactor into explicit modules:

- `Services/AudioCaptureService.swift`
- `Services/TranscriptionService.swift`
- `Services/TextInjectionService.swift`
- `Core/HotkeyManager.swift`
- `Core/AppState.swift`
- `Infrastructure/SettingsStore.swift`
- `UI/OverlayView.swift`
- `UI/AppDelegate.swift`

Rules:

- `AppState` owns only orchestration/state transitions.
- Services are protocol-driven and testable in isolation.
- No direct UI API calls from low-level services.

Exit criteria:

- Functional behavior matches current app.
- `DictatorMacApp.swift` becomes composition root only.

### Phase 2: B2 WhisperKit integration with fallback (2-3 days)

- Define backend protocol, e.g. `TranscriptionBackend`.
- Implement:
  - `WhisperKitBackend` (primary on Apple Silicon).
  - `HTTPWhisperBackend` (fallback/legacy).
- Add runtime backend selection in settings.
- Add capability check and safe fallback if WhisperKit unavailable.

Exit criteria:

- Local transcription works without external Python server on supported Apple Silicon.
- HTTP fallback remains operational.

### Phase 3: B3 Injection reliability (1-2 days)

- Formalize injection modes:
  - `pasteAndSend`
  - `clipboardOnly`
- Add permission onboarding flow with explicit state and user guidance.
- Keep current pasteboard content restoration strategy documented and deterministic.
- Add retry policy boundaries and completion signal in `AppState`.

Exit criteria:

- Predictable text output behavior across allowed/denied accessibility states.

### Phase 4: B4 Streaming hardening (1-2 days)

- Move streaming loop logic into dedicated service/orchestrator.
- Keep chunk modes (3/8/15s), but isolate timing and cancellation logic.
- Overlay/status partial results should be fed from a single event stream.

Exit criteria:

- Stable streaming with cancellation safety and no UI-thread stalls.

## Suggested branch strategy for macOS work

- Base branch for Sprint B foundation:
  - `codex/macos-sprint-b-foundation`
- Optional child branches per scope:
  - `codex/macos-b1-modularization`
  - `codex/macos-b2-whisperkit`
  - `codex/macos-b3-injection-reliability`
  - `codex/macos-b4-streaming-hardening`

## MVP-not-blocking items (defer after Sprint B)

- Dictionary editor UI.
- Command mode.
- Advanced model benchmarking matrix.
