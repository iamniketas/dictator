# model-store

Shared module for Dictator + Contora that provides:

- canonical model catalog loading (`model_catalog.v1`),
- shared installed-state store loading/saving (`shared_model_store.v1`),
- atomic writes for store updates,
- runtime/model upsert helpers,
- basic local model discovery for GGML `.bin` files.

## Paths

- default root: `AudioModels` (from `hardware-profiler`),
- default state file: `AudioModels/shared_model_store.v1.json`,
- starter catalog: `catalog/catalog.v1.json`.

## Next

- add file-lock strategy for cross-process updates,
- add migration/import from legacy `whisper-models`,
- align with Contora write/read behavior.
