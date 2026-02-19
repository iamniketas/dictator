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
- After stop, audio is sent to Whisper HTTP endpoint for real transcription.
- Streaming mode now processes audio chunks during recording (3s / 8s / 15s).

## Run

```bash
cd apps/macos
swift run
```

For production development, open the package in Xcode and run as a macOS app target.

## Next Milestones

1. Replace HTTP transcription with on-device Apple Silicon engine (WhisperKit-first).
2. Add global hotkey and text injection pipeline.
3. Add streaming chunk pipeline with partial results.
