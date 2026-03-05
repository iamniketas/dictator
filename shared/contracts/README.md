# Shared Contracts

Stable agreements between platform clients and related projects (Dictator, Contora).

## Why this exists

Without contracts, each client drifts:
- same setting gets different names,
- defaults diverge,
- shared resources (models, whisper server) become incompatible.

Contracts prevent that by defining one canonical format.

## Contracts

### 1. Pipeline State Machine (`pipeline_states.md`)

All clients implement the same state transitions:

```
idle -> recording -> transcribing -> correcting -> injecting -> idle
                                                 \-> idle (if LLM disabled)
```

### 2. Config Schema (`config.schema.json`)

Cross-platform shape for core settings:

| Section | Keys | Notes |
|---------|------|-------|
| `hotkey` | modifiers, key | Platform-native key names |
| `audio` | device, sample_rate | Default: 16kHz mono |
| `whisper` | model_path, language, server_url | Server URL for HTTP mode |
| `ollama` | url, model, enabled | Default: enabled=false |
| `streaming` | enabled, chunk_duration_sec | Default: 8s |
| `memory` | idle_unload_minutes | Default: 5, 0=never |
| `injection` | method (direct/clipboard/auto) | Platform-dependent default |
| `history` | max_entries, store_audio | Default: 50, false |

### 3. Whisper Server API (`whisper_api.md`)

HTTP interface shared between Dictator and Contora:

```
POST /transcribe
Content-Type: multipart/form-data
  - file: WAV (16kHz mono f32)
  - language: "ru" | "en" | "auto"
  - task: "transcribe"
Response: {"text": "...", "segments": [...], "language": "ru"}
```

### 4. History Format (`history.schema.json`)

```json
{
  "id": "uuid",
  "timestamp": "ISO8601",
  "text": "transcribed text",
  "raw_text": "before LLM correction",
  "language": "ru",
  "duration_sec": 5.2,
  "audio_path": "optional/path.wav",
  "source": "dictator|contora"
}
```

### 5. Shared Resources (local machine)

Multiple apps on the same machine can share:

| Resource | Default Location (Windows) | Default Location (macOS) |
|----------|---------------------------|--------------------------|
| Whisper models | `%LocalAppData%\whisper-models\` | `~/Library/Application Support/whisper-models/` |
| Whisper server | `shared/whisper-server/` | (or WhisperKit embedded) |
| Ollama | `http://localhost:11434` | Same |
| Custom dictionary | `%AppData%\dictator\dictionary.json` | `~/Library/Application Support/dictator/` |

## Versioning rule

- Backward compatible change: add optional field with default.
- Breaking change: new schema version and migration notes.

## How it works in practice

1. We define schemas in this folder.
2. Each platform maps its local models to this schema.
3. On load/save, each client validates against the schema.
4. If schema changes, we bump version and update all clients intentionally.
