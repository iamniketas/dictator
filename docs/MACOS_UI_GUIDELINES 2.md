# Dictator macOS UI Guidelines

Date: 2026-03-06
Scope: Sprint B and beyond

## Product UI direction

Dictator macOS must feel like a first-party Apple utility:

- native controls first (`SwiftUI` + `AppKit`),
- clean typography and spacing,
- restrained animation,
- clear permission onboarding,
- zero visual noise when idle.

## Native Apple principles (required)

1. Prefer platform defaults over custom widgets unless there is clear UX value.
2. Keep menu bar interactions concise and predictable.
3. Use semantic colors and SF Symbols as baseline language.
4. Respect macOS conventions for settings, focus, window behavior, and accessibility.
5. Avoid persistent UI clutter; app should disappear when user is not recording.

## Overlay exception: "Joyful Minimal Overlay"

Overlay is the only intentional stylistic accent. It should still feel native, but emotionally warmer.

Required qualities:

- Minimal footprint: compact, non-blocking, no desktop pollution.
- Immediate trust signal: clear recording/listening state in <150ms.
- Soft delight: subtle motion and polish that feels responsive, not flashy.
- High legibility: transcript/status always readable over mixed backgrounds.
- Calm disappearance: fades out quickly after completion.

Visual direction:

- Rounded glass-like surface (`ultraThinMaterial` style baseline).
- One accent color for "listening" state, one for "processing" state.
- Micro-animations for state transitions only (no constant pulsing noise).
- Optional waveform/level indicator should be clean and low-amplitude by default.

Anti-patterns (forbidden):

- Bulky floating bars that stay visible when idle.
- Aggressive neon or game-like visual language.
- Excessive motion that distracts from dictation flow.

## Engineering implications

- Overlay rendering and animation must stay off critical audio/transcription paths.
- Any visual effect must degrade gracefully on low-power devices.
- Overlay state should be driven by a single app-state source of truth.
