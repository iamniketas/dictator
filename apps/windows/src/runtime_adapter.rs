//! Adaptive runtime policy engine for Dictator.
//!
//! Produces deterministic backend/device/model decisions and a fallback chain
//! based on hardware profile + installed models + catalog models.

use hardware_profiler::{HardwareProfile, Tier};

use crate::config::{RuntimePreference, WhisperBackend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePreference {
    Cuda,
    Metal,
    DirectMl,
    Cpu,
}

impl DevicePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            DevicePreference::Cuda => "cuda",
            DevicePreference::Metal => "metal",
            DevicePreference::DirectMl => "directml",
            DevicePreference::Cpu => "cpu",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelProfile {
    Quality,
    Balanced,
    Fast,
}

impl ModelProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelProfile::Quality => "quality",
            ModelProfile::Balanced => "balanced",
            ModelProfile::Fast => "fast",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePolicy {
    pub backend: WhisperBackend,
    pub device: DevicePreference,
    pub model_profile: ModelProfile,
    pub preferred_model: String,
    pub preferred_model_score: f32,
    pub ranked_models: Vec<(String, f32)>,
    pub fallback_models: Vec<String>,
    pub needs_model_download: bool,
    pub enable_server_fallback: bool,
    pub enable_cloud_fallback: bool,
    pub reasons: Vec<String>,
}

impl RuntimePolicy {
    pub fn summary_line(&self) -> String {
        format!(
            "backend={} device={} profile={} model={} score={:.1} fallbacks=[{}] download_needed={} server_fallback={} cloud_fallback={}",
            backend_to_str(&self.backend),
            self.device.as_str(),
            self.model_profile.as_str(),
            self.preferred_model,
            self.preferred_model_score,
            self.fallback_models.join(", "),
            self.needs_model_download,
            self.enable_server_fallback,
            self.enable_cloud_fallback,
        )
    }
}

pub fn plan_runtime_policy(
    profile: &HardwareProfile,
    configured_backend: WhisperBackend,
    configured_model_ref: Option<&str>,
    runtime_preference: RuntimePreference,
    installed_model_filenames: &[String],
    catalog_model_filenames: &[String],
    catalog_remote_model_refs: &[String],
) -> RuntimePolicy {
    let mut device = if profile.accelerators.cuda {
        DevicePreference::Cuda
    } else if profile.accelerators.metal {
        DevicePreference::Metal
    } else if profile.accelerators.directml {
        DevicePreference::DirectMl
    } else {
        DevicePreference::Cpu
    };

    let (mut model_profile, mut candidates, tier_reason) = match profile.tier {
        Tier::High => (
            ModelProfile::Quality,
            vec![
                "ggml-large-v3.bin",
                "ggml-large-v3-turbo.bin",
                "ggml-medium.bin",
                "ggml-base.bin",
            ],
            "tier=high -> prioritize quality",
        ),
        Tier::Medium => (
            ModelProfile::Balanced,
            vec![
                "ggml-medium.bin",
                "ggml-small.bin",
                "ggml-base.bin",
                "ggml-tiny.bin",
            ],
            "tier=medium -> prioritize balance",
        ),
        Tier::Low | Tier::Unknown => (
            ModelProfile::Fast,
            vec!["ggml-base.bin", "ggml-small.bin", "ggml-tiny.bin"],
            "tier=low/unknown -> prioritize speed and stability",
        ),
    };

    match runtime_preference {
        RuntimePreference::Auto => {}
        RuntimePreference::ForceGpu => {
            if !matches!(device, DevicePreference::Cpu) {
                model_profile = match profile.tier {
                    Tier::High => ModelProfile::Quality,
                    Tier::Medium => ModelProfile::Balanced,
                    Tier::Low | Tier::Unknown => ModelProfile::Balanced,
                };
                candidates = vec![
                    "ggml-large-v3.bin",
                    "ggml-large-v3-turbo.bin",
                    "ggml-medium.bin",
                    "ggml-base.bin",
                    "ggml-small.bin",
                ];
            }
        }
        RuntimePreference::ForceCpu => {
            device = DevicePreference::Cpu;
            model_profile = ModelProfile::Fast;
            candidates = vec!["ggml-base.bin", "ggml-small.bin", "ggml-tiny.bin"];
        }
    }

    let configured_remote_ref = configured_model_ref.and_then(normalize_remote_model_ref);

    if configured_remote_ref.is_none()
        && let Some(configured) = configured_model_ref
            .and_then(|m| configured_embedded_filename(m, catalog_model_filenames))
    {
        candidates.retain(|c| !c.eq_ignore_ascii_case(configured));
        candidates.insert(0, configured);
    }

    let ranked_models = rank_candidate_models(
        &candidates,
        profile,
        &runtime_preference,
        installed_model_filenames,
        catalog_model_filenames,
    );
    let mut preferred_model = ranked_models
        .first()
        .map(|(name, _)| name.clone())
        .unwrap_or_else(|| select_preferred_model(&candidates, installed_model_filenames, catalog_model_filenames));
    let mut preferred_model_score = ranked_models
        .first()
        .map(|(_, score)| *score)
        .unwrap_or(0.0);
    let mut backend = WhisperBackend::Embedded;

    let mut fallback_models = Vec::new();
    for candidate in candidates {
        if candidate.eq_ignore_ascii_case(&preferred_model) {
            continue;
        }
        if catalog_model_filenames
            .iter()
            .any(|m| m.eq_ignore_ascii_case(candidate))
        {
            fallback_models.push(candidate.to_string());
        }
    }

    let mut preferred_installed = installed_model_filenames
        .iter()
        .any(|m| m.eq_ignore_ascii_case(&preferred_model));

    if let Some(remote_ref) = configured_remote_ref {
        preferred_model = remote_ref.to_string();
        preferred_model_score = 9.5;
        preferred_installed = true;
        backend = WhisperBackend::Server;
    }

    let mut reasons = vec![
        format!("runtime preference={}", runtime_preference.as_str()),
        tier_reason.to_string(),
        format!("detected device preference={}", device.as_str()),
        format!("configured backend={}", backend_to_str(&configured_backend)),
        "local-first policy: embedded inference preferred".to_string(),
    ];

    if let Some(remote_ref) = configured_remote_ref {
        reasons.push(format!(
            "configured remote model ref detected: '{}' -> using server backend",
            remote_ref
        ));
        if catalog_remote_model_refs
            .iter()
            .any(|m| m.eq_ignore_ascii_case(remote_ref))
        {
            reasons.push("remote model ref found in catalog".to_string());
        } else {
            reasons.push("remote model ref not found in catalog, attempting anyway".to_string());
        }
    }

    if matches!(runtime_preference, RuntimePreference::ForceGpu)
        && matches!(device, DevicePreference::Cpu)
    {
        reasons.push(
            "force_gpu requested but no compatible accelerator detected; using cpu".to_string(),
        );
    }

    if let Some(configured) = configured_model_ref {
        if configured.eq_ignore_ascii_case(&preferred_model) {
            reasons.push("configured model is compatible with current policy".to_string());
        } else {
            reasons.push(format!(
                "configured model '{}' unavailable or suboptimal; selected '{}'",
                configured, preferred_model
            ));
        }
    }

    if !preferred_installed {
        reasons.push("preferred model is not installed; download recommended".to_string());
    }
    reasons.push(format!(
        "ranked model suitability: {}",
        ranked_models
            .iter()
            .take(4)
            .map(|(m, s)| format!("{}:{:.1}", m, s))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let enable_server_fallback = matches!(profile.tier, Tier::Low | Tier::Unknown)
        || !preferred_installed
        || configured_remote_ref.is_some();
    let enable_cloud_fallback =
        matches!(profile.tier, Tier::Low | Tier::Unknown) && !preferred_installed;

    RuntimePolicy {
        backend,
        device,
        model_profile,
        preferred_model,
        preferred_model_score,
        ranked_models,
        fallback_models,
        needs_model_download: !preferred_installed,
        enable_server_fallback,
        enable_cloud_fallback,
        reasons,
    }
}

fn normalize_remote_model_ref(model_ref: &str) -> Option<&str> {
    let trimmed = model_ref.trim();
    if trimmed.is_empty() {
        return None;
    }
    let is_hf_like = trimmed.contains('/')
        && !trimmed.contains('\\')
        && !trimmed.starts_with("./")
        && !trimmed.starts_with("../")
        && !trimmed.contains(':');
    if is_hf_like {
        return Some(trimmed);
    }
    None
}

fn configured_embedded_filename<'a>(
    model_ref: &'a str,
    catalog_model_filenames: &[String],
) -> Option<&'a str> {
    let filename = std::path::Path::new(model_ref)
        .file_name()
        .and_then(|n| n.to_str())?;
    if catalog_model_filenames
        .iter()
        .any(|m| m.eq_ignore_ascii_case(filename))
    {
        return Some(filename);
    }
    None
}

fn select_preferred_model(candidates: &[&str], installed: &[String], catalog: &[String]) -> String {
    for candidate in candidates {
        if installed.iter().any(|m| m.eq_ignore_ascii_case(candidate)) {
            return (*candidate).to_string();
        }
    }

    for candidate in candidates {
        if catalog.iter().any(|m| m.eq_ignore_ascii_case(candidate)) {
            return (*candidate).to_string();
        }
    }

    if let Some(first_catalog) = catalog.first() {
        return first_catalog.clone();
    }

    candidates
        .first()
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| "ggml-base.bin".to_string())
}

fn rank_candidate_models(
    candidates: &[&str],
    profile: &HardwareProfile,
    runtime_preference: &RuntimePreference,
    installed: &[String],
    catalog: &[String],
) -> Vec<(String, f32)> {
    let mut ranked: Vec<(String, f32)> = candidates
        .iter()
        .filter(|candidate| {
            catalog.iter().any(|m| m.eq_ignore_ascii_case(candidate))
                || installed.iter().any(|m| m.eq_ignore_ascii_case(candidate))
        })
        .map(|candidate| {
            let score = compute_model_suitability(candidate, profile, runtime_preference, installed);
            ((*candidate).to_string(), score)
        })
        .collect();

    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
}

fn compute_model_suitability(
    model: &str,
    profile: &HardwareProfile,
    runtime_preference: &RuntimePreference,
    installed: &[String],
) -> f32 {
    let (speed, quality, vram_req_gb) = if model.eq_ignore_ascii_case("ggml-large-v3.bin") {
        (3.0, 10.0, 8.0)
    } else if model.eq_ignore_ascii_case("ggml-large-v3-turbo.bin") {
        (5.0, 9.0, 6.0)
    } else if model.eq_ignore_ascii_case("ggml-medium.bin") {
        (4.0, 9.0, 5.0)
    } else if model.eq_ignore_ascii_case("ggml-small.bin") {
        (6.0, 8.0, 3.0)
    } else if model.eq_ignore_ascii_case("ggml-base.bin") {
        (8.0, 7.0, 2.0)
    } else {
        (10.0, 5.0, 1.0)
    };

    let (speed_w, quality_w) = match profile.tier {
        Tier::High => (0.35, 0.65),
        Tier::Medium => (0.5, 0.5),
        Tier::Low | Tier::Unknown => (0.7, 0.3),
    };

    let mut score: f32 = speed * speed_w + quality * quality_w;

    let has_cuda = profile.accelerators.cuda;
    let ram_gb = profile.memory.total_mb as f32 / 1024.0;

    if matches!(runtime_preference, RuntimePreference::ForceCpu) && vram_req_gb > 4.0 {
        score -= 1.5;
    }
    if !has_cuda && vram_req_gb > 4.0 {
        score -= 2.0;
    }
    if ram_gb < 8.0 && vram_req_gb > 3.0 {
        score -= 1.0;
    }
    if installed.iter().any(|m| m.eq_ignore_ascii_case(model)) {
        score += 0.4;
    }

    score.clamp(0.0, 10.0)
}

fn backend_to_str(backend: &WhisperBackend) -> &'static str {
    match backend {
        WhisperBackend::Embedded => "embedded",
        WhisperBackend::Server => "server",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hardware_profiler::{
        Accelerators, CpuInfo, GpuInfo, GpuVendor, HardwareProfile, HostArch, HostInfo, HostOs,
        MemoryInfo,
    };

    fn profile_with_tier(tier: Tier, cuda: bool) -> HardwareProfile {
        HardwareProfile {
            schema_version: "hardware_profile.v1".to_string(),
            generated_at_unix_s: 0,
            source_app: "dictator-test".to_string(),
            host: HostInfo {
                os: HostOs::Windows,
                arch: HostArch::X86_64,
                os_version: None,
                device_name: None,
            },
            cpu: CpuInfo {
                vendor: Some("Test".to_string()),
                model: "Test CPU".to_string(),
                physical_cores: 8,
                logical_cores: 16,
                max_clock_mhz: Some(4200),
            },
            memory: MemoryInfo {
                total_mb: 16384,
                available_mb: Some(8192),
            },
            gpus: vec![GpuInfo {
                name: "Test GPU".to_string(),
                vendor: if cuda {
                    GpuVendor::Nvidia
                } else {
                    GpuVendor::Intel
                },
                is_integrated: !cuda,
                vram_mb: if cuda { 12288 } else { 0 },
                cuda_compatible: cuda,
                metal_compatible: false,
                directml_compatible: true,
            }],
            accelerators: Accelerators {
                cuda,
                metal: false,
                directml: true,
                rocm: false,
            },
            tier,
            confidence: 0.9,
            notes: vec![],
        }
    }

    #[test]
    fn prefers_installed_model_from_candidates() {
        let profile = profile_with_tier(Tier::High, true);
        let installed = vec!["ggml-medium.bin".to_string()];
        let catalog = vec![
            "ggml-large-v3.bin".to_string(),
            "ggml-medium.bin".to_string(),
        ];

        let policy = plan_runtime_policy(
            &profile,
            WhisperBackend::Embedded,
            None,
            RuntimePreference::Auto,
            &installed,
            &catalog,
            &[],
        );

        assert_eq!(policy.preferred_model, "ggml-medium.bin");
        assert!(!policy.needs_model_download);
    }

    #[test]
    fn keeps_configured_model_first_when_installed() {
        let profile = profile_with_tier(Tier::Medium, false);
        let installed = vec!["ggml-small.bin".to_string()];
        let catalog = vec!["ggml-medium.bin".to_string(), "ggml-small.bin".to_string()];

        let policy = plan_runtime_policy(
            &profile,
            WhisperBackend::Embedded,
            Some("ggml-small.bin"),
            RuntimePreference::Auto,
            &installed,
            &catalog,
            &[],
        );

        assert_eq!(policy.preferred_model, "ggml-small.bin");
    }

    #[test]
    fn force_cpu_sets_cpu_device_and_fast_profile() {
        let profile = profile_with_tier(Tier::High, true);
        let installed = vec!["ggml-base.bin".to_string()];
        let catalog = vec!["ggml-base.bin".to_string(), "ggml-small.bin".to_string()];

        let policy = plan_runtime_policy(
            &profile,
            WhisperBackend::Embedded,
            None,
            RuntimePreference::ForceCpu,
            &installed,
            &catalog,
            &[],
        );

        assert_eq!(policy.device, DevicePreference::Cpu);
        assert_eq!(policy.model_profile, ModelProfile::Fast);
    }

    #[test]
    fn remote_model_ref_switches_policy_to_server_backend() {
        let profile = profile_with_tier(Tier::High, true);
        let installed = vec!["ggml-base.bin".to_string()];
        let catalog = vec!["ggml-base.bin".to_string(), "ggml-small.bin".to_string()];
        let remote = vec!["nvidia/parakeet-tdt-0.6b-v2".to_string()];

        let policy = plan_runtime_policy(
            &profile,
            WhisperBackend::Embedded,
            Some("nvidia/parakeet-tdt-0.6b-v2"),
            RuntimePreference::Auto,
            &installed,
            &catalog,
            &remote,
        );

        assert_eq!(policy.backend, WhisperBackend::Server);
        assert_eq!(policy.preferred_model, "nvidia/parakeet-tdt-0.6b-v2");
        assert!(policy.enable_server_fallback);
    }
}
