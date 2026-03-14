# NEXT_SPRINTS — Dictator Platform Plan

> Last updated: 2026-03-10
> Planning mode: post-MVP reprioritization (Dictator + Contora convergence)

---

## 0) Reality Check (what is already done)

- Windows Dictator already has core dictation pipeline, embedded Whisper backend, model download/switch, history, updater, and native settings window baseline.
- macOS foundation is significantly advanced (modularized app, WhisperKit integration, streaming pipeline, hardening).
- In Contora, there is already practical groundwork we can reuse:
  - `HardwareDiagnosticsService` (GPU/CPU/RAM probing + CUDA recommendation)
  - `SharedModelConfigService` + `SharedModelConfig` (installed runtime/model registry)
  - `WhisperPaths` fallback chain and shared model root resolution.

Conclusion: next value is not another isolated feature. Next value is **shared adaptive platform**.

---

## 1) New Priority Stack (effective immediately)

1. Shared infrastructure between Dictator and Contora.
2. Reliable hardware-aware runtime selection (works on weak/mid/high-end machines).
3. Unified model catalog + shared model storage (no duplicate downloads).
4. Full settings UX for runtime/model/hardware control.
5. Optional cloud fallback architecture (design first, implementation later).

---

## 2) Sprint N — Shared Foundation Audit + Contracts (P0)

**Goal:** define and lock shared contracts before coding adapters.

### N1. Cross-project inventory (Dictator + Contora)
- Map overlapping concerns: model paths, model metadata, runtime selection, hardware diagnostics, settings semantics.
- Produce compatibility matrix: `Dictator Rust` vs `Contora .NET`.

### N2. Shared contract draft (`shared/contracts/runtime`)
- Define JSON schemas:
  - `hardware_profile.v1.json`
  - `runtime_policy.v1.json`
  - `model_catalog.v1.json`
  - `shared_model_store.v1.json`
- Include schema versioning + migration field.

### N3. ADR package (architecture decisions)
- Canonical shared root (desktop): `%LocalAppData%/AudioModels`.
- Atomic writes + lock strategy for cross-process updates.
- Conflict strategy when both apps change active model.

### Deliverables
- `docs/SHARED_RUNTIME_ADR.md`
- `shared/contracts/runtime/*.json`
- `docs/DICTATOR_CONTORA_COMPAT_MATRIX.md`

### Definition of Done
- Both apps can parse the same contract files without ambiguity.
- Storage contract includes corruption recovery rules.

---

## 3) Sprint O — Hardware Profiler Module (P0)

**Goal:** introduce reusable hardware capability module for all desktop targets.

### O1. Shared module design
- Build a reusable `hardware-profile` core (target: Rust core + per-platform adapters).
- Fields:
  - OS + architecture
  - CPU model, physical/logical cores, max frequency
  - RAM total/available
  - GPU vendor/model/VRAM
  - accelerator capabilities (CUDA/Metal/DirectML/none)
  - confidence score per metric

### O2. Adapter strategy
- Windows adapter: use Contora learnings (64-bit probing, robust VRAM detection, `nvidia-smi` fallback).
- llmfit alignment: borrow multi-GPU normalization strategy and backend capability flags from `llmfit-core/src/hardware.rs`.
- macOS adapter: Metal device capabilities + unified memory context.
- Linux adapter: conservative baseline (NVIDIA/AMD/Intel + RAM/CPU), robust fallbacks.

### O3. Scoring and tiering
- Produce deterministic output:
- scoring method: weighted profile inspired by `llmfit-core/src/fit.rs` with Dictator-specific latency constraints.
  - `tier = high | medium | low`
  - `recommended_device = cuda | metal | cpu`
  - `recommended_model_profile = quality | balanced | fast`

### Deliverables
- `shared/runtime/hardware-profile` module (or equivalent shared package)
- `docs/HARDWARE_SCORING_RULES.md`
- cross-platform sample output fixtures

### Definition of Done
- Same machine always returns stable profile output.
- Missing probe sources degrade gracefully, not crash.

---

## 4) Sprint P — Unified Model Catalog + Shared Store (P0)

**Goal:** one source of truth for models/runtimes used by both apps.

### P1. Canonical model catalog
- Build catalog with required metadata:
  - id, family, quantization/runtime compatibility
  - expected RAM/VRAM tiers
  - quality/speed labels
  - disk footprint
  - language/domain notes
- Include first supported families:
  - Whisper GGML/whisper-rs compatible models
  - faster-whisper model variants
  - Parakeet candidates (research-gated)

### P2. Shared model store service
- Read/write index of installed models/runtimes.
- Atomic register/unregister APIs.
- Health checks for partial or corrupted installations.
- File lock to prevent race between Dictator and Contora installers.

### P3. Discovery and migration
- Detect legacy app-local model folders and import into shared index.
- Preserve active selection where possible.

### Deliverables
- `shared/runtime/model-store` module
- `shared/runtime/model-catalog/catalog.v1.json`
- migration utility docs

### Definition of Done
- Model downloaded by app A is visible and selectable in app B without duplicate download.

---

## 5) Sprint Q — Adaptive Runtime Policy Engine (P0/P1)

**Goal:** ensure transcription works on any desktop with correct backend/model path.

### Q1. Policy engine
- Inputs: hardware profile + installed models + user preference.
- Output:
  - backend selection (embedded/server/faster-whisper/etc.)
  - device (cuda/metal/cpu)
  - model candidate order
  - fallback chain.

### Q2. Failure-aware fallback
- If selected backend fails, fallback deterministically.
- Store diagnostics reason for next launch.

### Q3. User control modes
- `Auto` (recommended)
- `Force GPU`
- `Force CPU`
- `Force specific model`
- clear warning when override reduces reliability.

### Deliverables
- `shared/runtime/policy-engine`
- policy tests for High/Mid/Low hardware fixtures

### Definition of Done
- First transcription succeeds across representative low/mid/high hardware fixtures (local test matrix).

---

## 6) Sprint R — Full Settings Window 2.0 (P0/P1)

**Goal:** move from tray-first control to scalable settings UX.

### R1. Information architecture
Sections:
- Models
- Runtime & Device
- Hardware Diagnostics
- Shared Storage
- Dictation/Injection
- History
- Hotkeys
- About & Diagnostics

### R2. Models UX
- install / delete / set active
- disk usage
- compatibility badges by current hardware tier
- "download once, available in both apps" explanation

### R3. Runtime UX
- show current auto decision and why (e.g. `Auto -> CPU`, no CUDA-compatible GPU)
- expose fallback status and last errors

### Deliverables
- updated native settings window in Dictator
- shared UI copy/spec for Contora parity

### Definition of Done
- User can fully configure runtime/model flow without editing config files.

---

## 7) Sprint S — Contora Integration Pass (P1)

**Goal:** wire shared modules into Contora and validate cross-app behavior.

### S1. Integrate shared contracts
- Contora reads/writes same model store/index schema.

### S2. Integrate hardware profile output
- Replace ad-hoc hardware recommendation path with shared scoring output.

### S3. End-to-end scenarios
- Install model in Dictator -> visible in Contora.
- Change active model in Contora -> visible in Dictator.
- Broken model files -> both apps surface same health warning.

### Deliverables
- `docs/DICTATOR_CONTORA_E2E_TESTS.md`
- compatibility test checklist

---

## 8) Sprint T — Research Track (parallel, non-blocking)

### T1. External hardware-profiling references
- Evaluate llmfit (https://github.com/AlexsJones/llmfit) as primary benchmark for cross-platform detection and model recommendation heuristics.
- Map reusable ideas into Dictator/Contora modules: hardware capability graph (`hardware.rs`), recommendation scoring (`fit.rs`), provider abstraction (`providers.rs`).
- Extract only portable, license-safe ideas.

### T2. Cloud fallback architecture (design only)
- define provider abstraction and privacy model
- cloud remains optional/off by default

---

## 9) Immediate Execution Order (what we start first)

1. Sprint N (contracts + ADR + compatibility matrix)
2. Sprint O (hardware profiler module)
3. Sprint P (shared model catalog/store)
4. Sprint U (expanded model stack: Canary/Granite/cloud candidate track)
5. Sprint Q (adaptive runtime policy engine with expanded catalog)
6. Sprint R (settings window 2.0 integration)

---

## 10) Open Inputs Needed

- Confirm which llmfit-derived scoring dimensions we lock first for v1 (`quality/speed/fit/context` or reduced subset).
- Confirm whether shared dictionary should be included in current cycle (or moved to next cycle after model/runtime stabilization).
- Confirm first Linux support scope (full app vs runtime module only).




## 11) Sprint U — Expanded STT Model Stack (P0/P1)

**Goal:** add new model families for stronger accuracy/speed coverage and keep one unified catalog for Dictator + Contora.

### U1. Candidate validation batch
- Evaluate candidates from current benchmark signals:
  - ElevenLabs Scribe v2 (cloud, optional track)
  - NVIDIA Canary Qwen 2.5B (local, high-accuracy GPU track)
  - IBM Granite Speech 3.3 8B (local, heavy high-accuracy track)
  - Keep Parakeet TDT 0.6B V2 as speed baseline.
- Produce reproducible internal comparison (WER-like proxy, latency, memory footprint, stability).

### U2. Runtime/backend feasibility for Windows-first
- Define backend integration path per candidate (local runtime, dependencies, packaging constraints).
- Lock minimum hardware gates (VRAM/RAM/CPU class) for each model family.

### U3. Catalog + policy integration
- Extend unified catalog metadata with new fields:
  - `min_vram_gb`, `min_ram_gb`, `language_scope`, `quality_tier`, `speed_tier`, `deployment_mode(local|cloud)`.
- Wire candidates into adaptive policy recommendations by hardware tier.

### U4. Settings UX integration
- Show per-model cards with clear labels:
  - hardware requirements,
  - expected speed/quality profile,
  - local/cloud indicator,
  - recommended/not recommended message for current machine.

### Deliverables
- Updated `shared/runtime/model-catalog/catalog.v1.json`
- `docs/MODEL_EVALUATION_MATRIX.md`
- Policy rules update for new families

### Definition of Done
- At least one non-Whisper local high-accuracy family is integrated end-to-end in Windows flow.
- Model recommendations in Settings are hardware-aware and explainable.

