# Dictator macOS App (Skeleton)

This directory contains the native macOS client skeleton:

- `SwiftUI` for UI structure.
- `AppKit` integration for menu bar (`NSStatusItem`).
- Permissions bootstrap for microphone and accessibility.
- Real microphone capture via `AVAudioEngine`.

## Current Scope

- Menu bar app with checkable menu items.
- Dashboard window with runtime state and permission controls.
- Settings window skeleton.
- Start/stop recording now captures real audio buffer.
- Captured audio is converted to 16kHz mono for the next transcription step.
- Transcription backend supports `auto` (`WhisperKit` first, `HTTP` fallback), forced `WhisperKit`, or forced `HTTP`.
- Streaming mode now processes audio chunks during recording (3s / 8s / 15s).
- Text injection supports `pasteAndSend` and `clipboardOnly` with deterministic accessibility fallback.
- Streaming pipeline is coordinated by a dedicated orchestrator service with centralized event-driven UI updates.

## Run

```bash
cd apps/macos
swift run
```

For production development, open the package in Xcode and run as a macOS app target.

## Smoke Check (Phase 0)

```bash
./scripts/macos/smoke_build.sh
```

This script validates package resolution and compilation for `apps/macos`.

## Architecture (B1)

Source is now split into modules:

- `Sources/DictatorMac/Core/`
- `Sources/DictatorMac/Services/`
- `Sources/DictatorMac/UI/`
- `Sources/DictatorMac/Infrastructure/`

`DictatorMacApp.swift` is now the composition root (`@main`) and the runtime logic is extracted into modular files.

## UI Direction

macOS UI must follow native Apple aesthetics by default.
Overlay is the only stylistic exception and should remain minimalistic and delightful.

See full design constraints in:

- `docs/MACOS_UI_GUIDELINES.md`

## Next Milestones

1. Improve WhisperKit model management UX (download/select in-app).
2. Harden text injection reliability and onboarding UX.
3. Continue streaming pipeline polish and partial result UX.

## WhisperKit model path

WhisperKit backend looks for models in:

1. `WHISPERKIT_MODEL_DIR` environment variable (recommended for development)
2. `~/Library/Application Support/Dictator/WhisperKitModels/` (and its subfolders)

## Permissions onboarding

Settings now include a dedicated onboarding section with:

- live microphone/accessibility status,
- one-click permission prompts,
- shortcuts to system privacy pages.
