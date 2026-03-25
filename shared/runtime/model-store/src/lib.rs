use anyhow::{Context, Result};
use hardware_profiler::default_audio_models_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_VERSION: &str = "shared_model_store.v1";
pub const CATALOG_SCHEMA_VERSION: &str = "model_catalog.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalog {
    pub schema_version: String,
    pub catalog_version: String,
    pub updated_at: String,
    pub models: Vec<CatalogModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    pub family: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub quality_profile: String,
    pub speed_profile: String,
    pub supported_backends: Vec<String>,
    pub recommended_tiers: Vec<String>,
    #[serde(default)]
    pub download_filename: Option<String>,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub required_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedModelStore {
    pub schema_version: String,
    pub store_version: u64,
    pub models_root_path: String,
    pub active_runtime_id: Option<String>,
    pub active_model_id: Option<String>,
    pub updated_at: Option<String>,
    pub updated_by: String,
    pub installed_runtimes: Vec<InstalledRuntime>,
    pub installed_models: Vec<InstalledModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledRuntime {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub version: Option<String>,
    pub entry_path: String,
    pub disk_usage_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModel {
    pub id: String,
    pub runtime_id: String,
    pub directory_path: String,
    pub size_bytes: Option<u64>,
    pub is_default: Option<bool>,
    pub health: String,
    pub required_files: Option<Vec<String>>,
    pub registered_at: Option<String>,
}

pub fn default_store_path() -> PathBuf {
    default_audio_models_dir().join("shared_model_store.v1.json")
}

pub fn load_catalog_from_file(path: &Path) -> Result<ModelCatalog> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read model catalog: {}", path.display()))?;
    let catalog: ModelCatalog = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse model catalog: {}", path.display()))?;
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        anyhow::bail!(
            "unexpected catalog schema version: {}",
            catalog.schema_version
        );
    }
    Ok(catalog)
}

pub fn load_embedded_catalog() -> Result<ModelCatalog> {
    let raw = include_str!("../catalog/catalog.v1.json");
    let catalog: ModelCatalog =
        serde_json::from_str(raw).context("failed to parse embedded model catalog")?;
    if catalog.schema_version != CATALOG_SCHEMA_VERSION {
        anyhow::bail!(
            "unexpected embedded catalog schema version: {}",
            catalog.schema_version
        );
    }
    Ok(catalog)
}

pub fn load_or_default_store(path: &Path) -> Result<SharedModelStore> {
    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read model store: {}", path.display()))?;
        let store: SharedModelStore = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse model store: {}", path.display()))?;
        if store.schema_version != STORE_SCHEMA_VERSION {
            anyhow::bail!("unexpected store schema version: {}", store.schema_version);
        }
        return Ok(store);
    }

    Ok(SharedModelStore {
        schema_version: STORE_SCHEMA_VERSION.to_string(),
        store_version: 1,
        models_root_path: default_audio_models_dir().display().to_string(),
        active_runtime_id: None,
        active_model_id: None,
        updated_at: None,
        updated_by: String::from("unknown"),
        installed_runtimes: Vec::new(),
        installed_models: Vec::new(),
    })
}

pub fn save_store_atomic(path: &Path, store: &SharedModelStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create model store parent dir: {}",
                parent.display()
            )
        })?;
    }

    let tmp_path = path.with_extension("tmp");
    let content = serde_json::to_string_pretty(store).context("failed to serialize model store")?;
    fs::write(&tmp_path, content)
        .with_context(|| format!("failed to write temp store file: {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "failed to atomically replace store file {} with {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

pub fn upsert_runtime(store: &mut SharedModelStore, runtime: InstalledRuntime) {
    if let Some(existing) = store
        .installed_runtimes
        .iter_mut()
        .find(|r| r.id.eq_ignore_ascii_case(&runtime.id))
    {
        *existing = runtime;
    } else {
        store.installed_runtimes.push(runtime);
    }
}

pub fn upsert_model(store: &mut SharedModelStore, model: InstalledModel) {
    if let Some(existing) = store
        .installed_models
        .iter_mut()
        .find(|m| m.id.eq_ignore_ascii_case(&model.id))
    {
        *existing = model;
    } else {
        store.installed_models.push(model);
    }
}

pub fn evaluate_model_health(model_dir: &Path, required_files: &[String]) -> String {
    if !model_dir.exists() {
        return String::from("missing_files");
    }

    for file in required_files {
        if !model_dir.join(file).exists() {
            return String::from("missing_files");
        }
    }

    String::from("ok")
}

pub fn discover_local_ggml_models(models_root: &Path) -> Result<Vec<PathBuf>> {
    if !models_root.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in fs::read_dir(models_root)
        .with_context(|| format!("failed to scan models root: {}", models_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "bin").unwrap_or(false) {
            out.push(path);
        }
    }

    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(name: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("model_store_test_{}_{}", name, ts))
    }

    #[test]
    fn default_store_has_expected_schema() {
        let path = unique_temp_dir("default").join("store.json");
        let store = load_or_default_store(&path).expect("default store should load");
        assert_eq!(store.schema_version, STORE_SCHEMA_VERSION);
        assert_eq!(store.store_version, 1);
    }

    #[test]
    fn model_health_checks_required_files() {
        let dir = unique_temp_dir("health");
        std::fs::create_dir_all(&dir).expect("create test dir");
        std::fs::write(dir.join("model.bin"), b"ok").expect("write model");

        let ok = evaluate_model_health(&dir, &[String::from("model.bin")]);
        assert_eq!(ok, "ok");

        let missing = evaluate_model_health(&dir, &[String::from("missing.bin")]);
        assert_eq!(missing, "missing_files");

        let _ = std::fs::remove_dir_all(&dir);
    }


    #[test]
    fn embedded_catalog_whisper_models_have_download_metadata() {
        let catalog = load_embedded_catalog().expect("embedded catalog should load");
        for model in catalog.models.into_iter().filter(|m| m.family == "whisper_ggml") {
            assert!(
                model.download_filename.is_some(),
                "missing download_filename for {}",
                model.id
            );
            assert!(
                model.download_url.is_some(),
                "missing download_url for {}",
                model.id
            );
            assert!(
                !model.required_files.is_empty(),
                "missing required_files for {}",
                model.id
            );
        }
    }    #[test]
    fn discover_ggml_bin_files() {
        let dir = unique_temp_dir("discover");
        std::fs::create_dir_all(&dir).expect("create discover dir");
        std::fs::write(dir.join("ggml-base.bin"), b"x").expect("write bin");
        std::fs::write(dir.join("notes.txt"), b"x").expect("write txt");

        let found = discover_local_ggml_models(&dir).expect("discover should work");
        assert_eq!(found.len(), 1);
        assert!(found[0].ends_with("ggml-base.bin"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

