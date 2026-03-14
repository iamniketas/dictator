# ROADMAP — Dictator (Updated Product Direction)

> Last updated: 2026-03-10
> Status: active rewrite after macOS + Windows baseline completion

## 1) Product North Star

Dictator evolves from a "voice typing app" into a **hardware-adaptive local transcription platform** that:
- runs reliably on weak, mid, and high-end desktops,
- shares runtime/model infrastructure with Contora,
- avoids duplicate model downloads across apps,
- keeps local-first behavior by default,
- is ready for future cloud fallback and mobile clients.

## 2) What Changed (Priority Reset)

Previous roadmap focused on MVP dictation UX features. Most of that baseline already exists.
Current bottleneck is no longer "another feature" (e.g. dictionary), but **platform reliability + shared infrastructure**:
- common model space for Dictator + Contora,
- cross-platform hardware detection and scoring,
- adaptive backend/model routing by machine capabilities,
- scalable settings UX for many options.

## 3) Strategic Goals (Now)

### G1. Shared Foundation Across Dictator + Contora (P0)
Build shared modules and contracts so both apps can reuse one stack for:
- hardware profiling,
- model catalog and installed-state index,
- model storage location,
- common metadata/config compatibility.

### G2. "Works on Any Desktop" Reliability (P0)
Define runtime strategy for:
- NVIDIA GPU systems (fast local GPU path),
- CPU-only / weak GPU systems (optimized local CPU path),
- low-resource systems (graceful degraded mode now, cloud fallback later).

### G3. Unified Model Experience (P0)
User downloads model once, both apps see it.
Both apps can:
- discover installed models,
- validate compatibility,
- show recommended options for current hardware.

### G4. Full Settings UX (P0/P1)
Move from tray-centric controls to a full settings window with sections for:
- Models & Runtime,
- Hardware profile,
- Recording/Transcription behavior,
- Shared storage,
- History,
- Hotkeys,
- About/Diagnostics.

### G5. Future-Ready Architecture (P1/P2)
Prepare extension points for:
- optional cloud acceleration fallback,
- account-level sync (dictionary/preferences) in future,
- native iOS/Android clients.

## 4) Product Principles (Must Keep)

- Local-first by default.
- No hidden cloud processing.
- No duplicate model downloads when both apps are installed.
- Transparent runtime choice (show why backend/model was selected).
- Minimal required permissions.

## 5) Architecture Priorities

### A. Shared Module Layer (Dictator + Contora)
Create a reusable shared runtime package (naming TBD, e.g. `shared/runtime-core`) containing:
- `hardware_profile` (detect CPU/GPU/RAM/OS + capabilities),
- `hardware_scoring` (tier classification and confidence),
- `model_catalog` (canonical list of supported models/backends),
- `model_store` (discover/install/remove/validate shared model files),
- `runtime_policy` (rules to choose backend/model by hardware + user preference).

Implementation references for current cycle:
- Contora: HardwareDiagnosticsService, SharedModelConfigService, WhisperPaths.
- llmfit: cross-platform hardware.rs (multi-GPU, backend detection), fit.rs (Q/S/F/C scoring), providers.rs (provider abstraction: Ollama/llama.cpp/MLX).

### B. Capability-Aware Runtime Orchestrator
At startup and before heavy tasks:
1. Probe hardware.
2. Score machine tier.
3. Resolve recommended backend + model.
4. Apply user override if set.
5. Fallback safely if chosen path fails.

Scoring baseline:
- adopt weighted multi-axis scoring inspired by llmfit (quality/speed/fit/context),
- keep Dictator-specific constraints for speech-to-text latency and transcription quality targets.

### C. Shared Storage Contract
Define stable cross-app locations and metadata:
- shared model directory,
- shared installed-model index file,
- optional shared dictionary file (later),
- optional shared presets (later).

Contract must support:
- app A installs model -> app B sees it without rescan issues,
- partial/corrupt downloads -> clear health state,
- versioned metadata migrations.

## 6) Platform Runtime Matrix (Target)

### Tier H (High-end, e.g. strong NVIDIA GPU)
- Preferred: GPU-accelerated backend + larger/faster-accurate models.
- Goal: real-time or near-real-time transcription.

### Tier M (Mid-range CPU/GPU)
- Preferred: efficient local models on CPU or mixed path.
- Goal: stable latency with acceptable quality.

### Tier L (Low-end machines)
- Preferred: smallest local models + conservative defaults.
- Goal: always functional local mode, lower quality tolerated.
- Future: optional cloud acceleration path (off by default).

## 7) Phased Plan

### Phase 1 — Shared Runtime Foundation (Now, P0)
- Shared module boundaries and contracts.
- Common model storage/index.
- Canonical model catalog draft.
- Hardware profiling MVP integrated in Dictator.

### Phase 2 — Adaptive Runtime & UX Integration (P0/P1)
- Runtime policy engine + fallback chain.
- Settings window expansion for hardware/runtime/model controls.
- Same shared modules integrated into Contora.

### Phase 3 — Robustness and Data Sharing (P1)
- Telemetry/logging for backend choice and failures (local logs).
- Shared dictionary contract (if approved).
- Shared/portable user presets (optional).

### Phase 4 — Cloud & Mobile Readiness (P2)
- Optional cloud transcription provider abstraction.
- Sync strategy design for multi-device scenarios.
- Reuse shared contracts for iOS/Android native clients.

## 8) Decisions Locked For Current Cycle

- Priority is **not** dictionary editor first.
- Priority is shared infra + hardware-aware reliability.
- Dictator and Contora must converge on one model ecosystem.
- Cloud is planned, but local compute remains first-class default.

## 9) Success Metrics

- Cold start to "ready to transcribe" within target by hardware tier.
- First transcription success rate across supported hardware tiers.
- Zero duplicate downloads for same model across Dictator/Contora on one machine.
- Deterministic runtime selection logs for troubleshooting.
- Settings discoverability: users can configure model/runtime without editing config files.

## 10) Risks and Mitigations

- Risk: hardware detection inconsistency across OS.
  - Mitigation: capability abstraction + per-OS adapters + confidence flags.
- Risk: model metadata drift between apps.
  - Mitigation: single shared catalog and schema versioning.
- Risk: fallback complexity causes brittle UX.
  - Mitigation: explicit fallback chain and user-visible status.
- Risk: over-expanding scope too early (cloud/mobile).
  - Mitigation: keep them architecture-ready, not delivery-critical for current cycle.



## 11) Model Expansion Track (New Priority)

Goal: expand the supported STT model set beyond current Whisper-centric defaults to improve
accuracy/speed coverage by hardware tier and user profile.

Input signal (community benchmark note, to validate in our pipeline):
- ElevenLabs Scribe v2 (cloud): very high benchmark accuracy claim.
- NVIDIA Canary Qwen 2.5B (local): high-accuracy local candidate, requires strong GPU (about 8GB VRAM class).
- IBM Granite Speech 3.3 8B (local): high-accuracy/noise-resilience candidate, heavy runtime profile.
- Parakeet TDT 0.6B V2 remains speed-oriented baseline.

Benchmark references to include in evaluation docs:
- https://artificialanalysis.ai/speech-to-text
- https://huggingface.co/spaces/hf-audio/open_asr_leaderboard

Acceptance policy for adding new model families:
1. Reproducible internal validation on Dictator test set (accuracy + latency + robustness).
2. Hardware-fit mapping (High/Mid/Low tiers) with explicit VRAM/RAM thresholds.
3. Runtime/backend feasibility for Windows-first delivery.
4. Licensing/commercial usage check.
5. UX readiness in Settings (clear trade-offs and recommended hardware).

Near-term target outcomes:
- Add at least one new high-accuracy local family beyond Whisper.
- Define optional cloud accuracy tier (off by default, explicit opt-in).
- Ship an updated unified model catalog with per-model capability requirements and recommendation rules.
