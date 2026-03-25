# hardware-profiler

Cross-platform shared module for Dictator and Contora.

## Purpose

- detect machine capabilities (CPU/GPU/RAM/OS),
- classify hardware tier (`high|medium|low|unknown`),
- expose deterministic JSON output that can be consumed by apps on different stacks.

## Why CLI + Library

- Rust clients can use the library directly.
- Swift/.NET clients can call the CLI and parse JSON.
- One implementation, multiple consumers.

## Output Contract

Current output maps to:
- `shared/contracts/runtime/hardware_profile.v1.json`

## Shared Models Default Path

`default_audio_models_dir()` returns:
- Windows: `%LOCALAPPDATA%\\AudioModels`
- macOS: `~/Library/Application Support/AudioModels`
- Linux: `$XDG_DATA_HOME/audio-models` or `~/.local/share/audio-models`

## Usage

```bash
cd shared/runtime/hardware-profiler
cargo run -- --pretty
```

Optional env var:

```bash
HARDWARE_PROFILE_SOURCE=dictator
```

