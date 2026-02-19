# Shared Contracts

`Shared contract` is a stable agreement between platform clients.

In this repository, that means:

- Windows and macOS clients may use different UI/frameworks.
- But they keep the same data shapes and behavior rules for key flows.

## Why this exists

Without contracts, each client drifts:

- same setting gets different names,
- defaults diverge,
- telemetry and automation become inconsistent.

Contracts prevent that by defining one canonical format.

## How it works in practice

1. We define schemas in this folder (starting with config schema).
2. Each platform maps its local models to this schema.
3. On load/save, each client validates data against the schema (or equivalent typed model checks).
4. If schema changes, we bump version and update both clients intentionally.

## First contract

- `config.schema.json`: cross-platform shape for core settings.

Current keys align with existing Windows app sections:

- `hotkey`
- `audio`
- `whisper`
- `ollama`
- `streaming`

## Versioning rule

- Backward compatible change: add optional field with default.
- Breaking change: new schema version and migration notes.
