use serde::Serialize;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::System;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Windows,
    Macos,
    Linux,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostArch {
    X86_64,
    Arm64,
    X86,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostInfo {
    pub os: HostOs,
    pub arch: HostArch,
    pub os_version: Option<String>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    pub vendor: Option<String>,
    pub model: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub max_clock_mhz: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    pub total_mb: u64,
    pub available_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: GpuVendor,
    pub is_integrated: bool,
    pub vram_mb: u64,
    pub cuda_compatible: bool,
    pub metal_compatible: bool,
    pub directml_compatible: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Accelerators {
    pub cuda: bool,
    pub metal: bool,
    pub directml: bool,
    pub rocm: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HardwareProfile {
    pub schema_version: String,
    pub generated_at_unix_s: u64,
    pub source_app: String,
    pub host: HostInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpus: Vec<GpuInfo>,
    pub accelerators: Accelerators,
    pub tier: Tier,
    pub confidence: f32,
    pub notes: Vec<String>,
}

pub fn default_audio_models_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let base = env::var("LOCALAPPDATA").unwrap_or_else(|_| String::from("."));
        return PathBuf::from(base).join("AudioModels");
    }

    #[cfg(target_os = "macos")]
    {
        let home = env::var("HOME").unwrap_or_else(|_| String::from("."));
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("AudioModels");
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg_data_home) = env::var("XDG_DATA_HOME") {
            return PathBuf::from(xdg_data_home).join("audio-models");
        }
        let home = env::var("HOME").unwrap_or_else(|_| String::from("."));
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("audio-models");
    }

    #[allow(unreachable_code)]
    PathBuf::from("./audio-models")
}

pub fn detect_hardware_profile(source_app: &str) -> HardwareProfile {
    let host = detect_host();
    let cpu = detect_cpu();
    let memory = detect_memory();
    let gpus = detect_gpus();

    let accelerators = Accelerators {
        cuda: gpus.iter().any(|g| g.cuda_compatible),
        metal: gpus.iter().any(|g| g.metal_compatible),
        directml: gpus.iter().any(|g| g.directml_compatible),
        rocm: false,
    };

    let (tier, confidence, mut notes) = score_tier(&memory, &gpus);
    if memory.total_mb == 0 {
        notes.push(String::from("warning: total RAM detection failed"));
    }
    if cpu.max_clock_mhz.is_none() {
        notes.push(String::from("note: cpu max clock unavailable (fallback path used)"));
    }
    if gpus.is_empty() {
        notes.push(String::from("note: no GPU detected by current probes"));
    }
    notes.push(format!(
        "default_audio_models_dir={}",
        default_audio_models_dir().display()
    ));

    HardwareProfile {
        schema_version: String::from("hardware_profile.v1"),
        generated_at_unix_s: now_unix(),
        source_app: source_app.to_string(),
        host,
        cpu,
        memory,
        gpus,
        accelerators,
        tier,
        confidence,
        notes,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn detect_host() -> HostInfo {
    let os = match env::consts::OS {
        "windows" => HostOs::Windows,
        "macos" => HostOs::Macos,
        "linux" => HostOs::Linux,
        _ => HostOs::Unknown,
    };

    let arch = match env::consts::ARCH {
        "x86_64" => HostArch::X86_64,
        "aarch64" => HostArch::Arm64,
        "x86" => HostArch::X86,
        _ => HostArch::Unknown,
    };

    HostInfo {
        os,
        arch,
        os_version: detect_os_version(),
        device_name: env::var("COMPUTERNAME")
            .ok()
            .or_else(|| env::var("HOSTNAME").ok()),
    }
}

fn detect_cpu() -> CpuInfo {
    let sys = System::new_all();
    let logical = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let physical = sys
        .physical_core_count()
        .map(|v| v as u32)
        .unwrap_or(logical);

    let model = detect_cpu_model().unwrap_or_else(|| String::from("Unknown CPU"));
    let max_clock_mhz = detect_cpu_mhz();

    CpuInfo {
        vendor: detect_cpu_vendor(&model),
        model,
        physical_cores: physical,
        logical_cores: logical,
        max_clock_mhz,
    }
}

fn detect_memory() -> MemoryInfo {
    MemoryInfo {
        total_mb: detect_total_ram_mb().unwrap_or(0),
        available_mb: detect_available_ram_mb(),
    }
}

fn detect_gpus() -> Vec<GpuInfo> {
    #[cfg(target_os = "windows")]
    {
        return detect_gpus_windows();
    }

    #[cfg(target_os = "macos")]
    {
        return detect_gpus_macos();
    }

    #[cfg(target_os = "linux")]
    {
        return detect_gpus_linux();
    }

    #[allow(unreachable_code)]
    Vec::new()
}

fn score_tier(memory: &MemoryInfo, gpus: &[GpuInfo]) -> (Tier, f32, Vec<String>) {
    let mut notes = Vec::new();
    let ram = memory.total_mb;
    let top_vram = gpus.iter().map(|g| g.vram_mb).max().unwrap_or(0);
    let has_cuda = gpus.iter().any(|g| g.cuda_compatible);

    if has_cuda && (top_vram >= 8192 || ram >= 16384) {
        notes.push(String::from("high tier: cuda + strong memory"));
        return (Tier::High, 0.90, notes);
    }

    if ram >= 8192 || top_vram >= 2048 {
        notes.push(String::from("medium tier: sufficient RAM or modest VRAM"));
        return (Tier::Medium, 0.75, notes);
    }

    if ram > 0 {
        notes.push(String::from("low tier: limited local resources"));
        return (Tier::Low, 0.65, notes);
    }

    notes.push(String::from("unknown tier: insufficient probe data"));
    (Tier::Unknown, 0.30, notes)
}

fn detect_os_version() -> Option<String> {
    System::long_os_version().or_else(|| run_and_trim(os_version_command()))
}

fn normalize_memory_to_mb(raw: u64) -> u64 {
    if raw > 4 * 1024 * 1024 * 1024 {
        raw / (1024 * 1024)
    } else {
        raw / 1024
    }
}

fn detect_cpu_model() -> Option<String> {
    let sys = System::new_all();
    let brand = sys.global_cpu_info().brand().trim().to_string();
    if !brand.is_empty() {
        return Some(brand);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("reg"),
            String::from("query"),
            String::from(r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            String::from("/v"),
            String::from("ProcessorNameString"),
        ]) {
            for line in raw.lines() {
                if line.contains("ProcessorNameString") {
                    if let Some(value) = line.split("REG_SZ").nth(1) {
                        let v = value.trim();
                        if !v.is_empty() {
                            return Some(v.to_string());
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("grep -m1 'model name' /proc/cpuinfo | cut -d ':' -f2"),
        ]) {
            return Some(raw.trim().to_string());
        }
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("grep -m1 'Hardware' /proc/cpuinfo | cut -d ':' -f2"),
        ]) {
            return Some(raw.trim().to_string());
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sysctl"),
            String::from("-n"),
            String::from("machdep.cpu.brand_string"),
        ]) {
            let v = raw.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }

    None
}

fn detect_cpu_mhz() -> Option<u32> {
    let sys = System::new_all();
    let freq = sys.global_cpu_info().frequency();
    if freq > 0 {
        return Some(freq as u32);
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("reg"),
            String::from("query"),
            String::from(r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            String::from("/v"),
            String::from("~MHz"),
        ]) {
            for line in raw.lines() {
                if line.contains("~MHz") {
                    if let Some(value) = line.split("REG_DWORD").nth(1) {
                        let v = value.trim().trim_start_matches("0x");
                        if let Ok(mhz) = u32::from_str_radix(v, 16) {
                            return Some(mhz);
                        }
                    }
                }
            }
        }

        if let Some(raw) = run_and_trim(windows_powershell_command(
            "(Get-CimInstance Win32_Processor | Select-Object -First 1).MaxClockSpeed",
        )) {
            return raw.parse::<u32>().ok();
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sysctl"),
            String::from("-n"),
            String::from("hw.cpufrequency_max"),
        ]) {
            if let Ok(hz) = raw.parse::<u64>() {
                return Some((hz / 1_000_000) as u32);
            }
        }
        if let Some(raw) = run_and_trim(vec![
            String::from("sysctl"),
            String::from("-n"),
            String::from("hw.cpufrequency"),
        ]) {
            if let Ok(hz) = raw.parse::<u64>() {
                return Some((hz / 1_000_000) as u32);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("grep -m1 'cpu MHz' /proc/cpuinfo | awk -F ':' '{print $2}'"),
        ]) {
            if let Ok(v) = raw.trim().parse::<f32>() {
                return Some(v as u32);
            }
        }
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("lscpu | grep -i 'CPU max MHz' | awk -F ':' '{print $2}'"),
        ]) {
            if let Ok(v) = raw.trim().parse::<f32>() {
                return Some(v as u32);
            }
        }
    }

    None
}

fn detect_total_ram_mb() -> Option<u64> {
    let sys = System::new_all();
    let total = sys.total_memory();
    if total > 0 {
        let mb = normalize_memory_to_mb(total);
        if mb > 0 {
            return Some(mb);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(raw) = run_and_trim(windows_powershell_command(
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        )) {
            if let Ok(bytes) = raw.parse::<u64>() {
                return Some(bytes / (1024 * 1024));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sysctl"),
            String::from("-n"),
            String::from("hw.memsize"),
        ]) {
            if let Ok(bytes) = raw.parse::<u64>() {
                return Some(bytes / (1024 * 1024));
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("grep -m1 MemTotal /proc/meminfo | awk '{print $2}'"),
        ]) {
            if let Ok(kb) = raw.parse::<u64>() {
                return Some(kb / 1024);
            }
        }
    }

    None
}

fn detect_available_ram_mb() -> Option<u64> {
    let sys = System::new_all();
    let avail = sys.available_memory();
    if avail > 0 {
        let mb = normalize_memory_to_mb(avail);
        if mb > 0 {
            return Some(mb);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("grep -m1 MemAvailable /proc/meminfo | awk '{print $2}'"),
        ]) {
            if let Ok(kb) = raw.parse::<u64>() {
                return Some(kb / 1024);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let (Some(vm_stat_raw), Some(page_size_raw)) = (
            run_and_trim(vec![String::from("vm_stat")]),
            run_and_trim(vec![
                String::from("sysctl"),
                String::from("-n"),
                String::from("hw.pagesize"),
            ]),
        ) {
            if let Ok(page_size) = page_size_raw.trim().parse::<u64>() {
                let free = parse_vm_stat_pages(&vm_stat_raw, "Pages free");
                let inactive = parse_vm_stat_pages(&vm_stat_raw, "Pages inactive");
                let speculative = parse_vm_stat_pages(&vm_stat_raw, "Pages speculative");
                let purgeable = parse_vm_stat_pages(&vm_stat_raw, "Pages purgeable");
                let pages = free + inactive + speculative + purgeable;
                if pages > 0 {
                    return Some((pages * page_size) / (1024 * 1024));
                }
            }
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn detect_gpus_windows() -> Vec<GpuInfo> {
    let mut gpus: Vec<GpuInfo> = Vec::new();

    if let Some(raw) = run_and_trim(vec![
        String::from("nvidia-smi"),
        String::from("--query-gpu=name,memory.total"),
        String::from("--format=csv,noheader,nounits"),
    ]) {
        gpus.extend(parse_nvidia_smi_csv(&raw));
    }

    if gpus.is_empty() {
        if let Some(raw) = run_and_trim(vec![
            String::from("reg"),
            String::from("query"),
            String::from(r"HKLM\SYSTEM\CurrentControlSet\Control\Video"),
            String::from("/s"),
            String::from("/v"),
            String::from("DriverDesc"),
        ]) {
            for line in raw.lines() {
                if line.contains("DriverDesc") {
                    if let Some(value) = line.split("REG_SZ").nth(1) {
                        let name = value.trim();
                        if !name.is_empty() {
                            gpus.push(build_gpu(name, 0));
                        }
                    }
                }
            }
        }
    }

    gpus.sort_by(|a, b| a.name.cmp(&b.name));
    gpus.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    gpus
}

#[cfg(target_os = "macos")]
fn detect_gpus_macos() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    if let Some(json) = run_and_trim(vec![
        String::from("system_profiler"),
        String::from("SPDisplaysDataType"),
        String::from("-json"),
    ]) {
        gpus.extend(parse_macos_system_profiler_json(&json));
    }

    if gpus.is_empty() {
        if let Some(text) = run_and_trim(vec![
            String::from("system_profiler"),
            String::from("SPDisplaysDataType"),
        ]) {
            gpus.extend(parse_macos_system_profiler_text(&text));
        }
    }

    gpus.sort_by(|a, b| a.name.cmp(&b.name));
    gpus.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    gpus
}

#[cfg(target_os = "linux")]
fn detect_gpus_linux() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    if let Some(raw) = run_and_trim(vec![
        String::from("nvidia-smi"),
        String::from("--query-gpu=name,memory.total"),
        String::from("--format=csv,noheader,nounits"),
    ]) {
        gpus.extend(parse_nvidia_smi_csv(&raw));
    }

    if let Some(raw) = run_and_trim(vec![
        String::from("sh"),
        String::from("-c"),
        String::from("lspci | grep -Ei 'vga|3d|display'"),
    ]) {
        gpus.extend(parse_linux_lspci(&raw));
    }

    if gpus.is_empty() {
        if let Some(raw) = run_and_trim(vec![
            String::from("sh"),
            String::from("-c"),
            String::from("lshw -C display 2>/dev/null"),
        ]) {
            gpus.extend(parse_linux_lshw_display(&raw));
        }
    }

    gpus.sort_by(|a, b| a.name.cmp(&b.name));
    gpus.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    gpus
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn parse_nvidia_smi_csv(raw: &str) -> Vec<GpuInfo> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 2 {
            let name = parts[0].trim();
            if name.is_empty() {
                continue;
            }
            let vram_mb = parts[1].trim().parse::<u64>().unwrap_or(0);
            out.push(build_gpu(name, vram_mb));
        }
    }
    out
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_system_profiler_json(raw: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    if let Some(items) = parsed.get("SPDisplaysDataType").and_then(|v| v.as_array()) {
        for item in items {
            let name = item
                .get("sppci_model")
                .or_else(|| item.get("_name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                continue;
            }
            gpus.push(build_gpu(&name, 0));
        }
    }
    gpus
}

#[cfg(any(target_os = "macos", test))]
fn parse_macos_system_profiler_text(raw: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Chipset Model:") {
            let name = rest.trim();
            if !name.is_empty() {
                gpus.push(build_gpu(name, 0));
            }
        }
    }
    gpus
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_lspci(raw: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    for line in raw.lines() {
        let name = line.trim();
        if !name.is_empty() {
            gpus.push(build_gpu(name, 0));
        }
    }
    gpus
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_lshw_display(raw: &str) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    let mut current_product: Option<String> = None;
    let mut current_vendor: Option<String> = None;
    let mut current_vram_mb: u64 = 0;

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("*-display") {
            let _ = rest;
            if let Some(name) = compose_lshw_gpu_name(&current_vendor, &current_product) {
                gpus.push(build_gpu(&name, current_vram_mb));
            }
            current_product = None;
            current_vendor = None;
            current_vram_mb = 0;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("product:") {
            current_product = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("vendor:") {
            current_vendor = Some(rest.trim().to_string());
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("size:") {
            if let Some(vram) = parse_size_to_mb(rest.trim()) {
                current_vram_mb = vram;
            }
        }
    }

    if let Some(name) = compose_lshw_gpu_name(&current_vendor, &current_product) {
        gpus.push(build_gpu(&name, current_vram_mb));
    }

    gpus
}

#[cfg(any(target_os = "linux", test))]
fn compose_lshw_gpu_name(vendor: &Option<String>, product: &Option<String>) -> Option<String> {
    match (vendor, product) {
        (Some(v), Some(p)) if !v.trim().is_empty() && !p.trim().is_empty() => {
            Some(format!("{} {}", v.trim(), p.trim()))
        }
        (None, Some(p)) if !p.trim().is_empty() => Some(p.trim().to_string()),
        (Some(v), None) if !v.trim().is_empty() => Some(v.trim().to_string()),
        _ => None,
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_size_to_mb(raw: &str) -> Option<u64> {
    let lower = raw.trim().to_lowercase();
    let digits: String = lower
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let value = digits.parse::<f64>().ok()?;
    if lower.contains("gib") || lower.contains("gb") {
        return Some((value * 1024.0) as u64);
    }
    if lower.contains("mib") || lower.contains("mb") {
        return Some(value as u64);
    }
    if lower.contains("kib") || lower.contains("kb") {
        return Some((value / 1024.0) as u64);
    }
    None
}

#[cfg(any(target_os = "macos", test))]
fn parse_vm_stat_pages(raw: &str, field: &str) -> u64 {
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(field) {
            let digits: String = trimmed
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect();
            if let Ok(v) = digits.parse::<u64>() {
                return v;
            }
        }
    }
    0
}
fn build_gpu(name: &str, vram_mb: u64) -> GpuInfo {
    let lower = name.to_lowercase();
    let vendor = if lower.contains("nvidia")
        || lower.contains("geforce")
        || lower.contains("rtx")
        || lower.contains("gtx")
    {
        GpuVendor::Nvidia
    } else if lower.contains("amd") || lower.contains("radeon") {
        GpuVendor::Amd
    } else if lower.contains("intel") {
        GpuVendor::Intel
    } else if lower.contains("apple") {
        GpuVendor::Apple
    } else {
        GpuVendor::Unknown
    };

    let cuda_compatible = matches!(vendor, GpuVendor::Nvidia) && is_probably_cuda_capable(&lower);
    let metal_compatible = matches!(vendor, GpuVendor::Apple);
    let directml_compatible =
        matches!(vendor, GpuVendor::Nvidia | GpuVendor::Amd | GpuVendor::Intel);

    GpuInfo {
        name: name.to_string(),
        vendor,
        is_integrated: lower.contains("integrated") || lower.contains("iris"),
        vram_mb,
        cuda_compatible,
        metal_compatible,
        directml_compatible,
    }
}

fn is_probably_cuda_capable(lower_name: &str) -> bool {
    lower_name.contains("rtx")
        || lower_name.contains("gtx")
        || lower_name.contains("tesla")
        || lower_name.contains("quadro")
}

#[cfg(target_os = "windows")]
fn windows_powershell_exe() -> String {
    let candidate = r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe";
    if std::path::Path::new(candidate).exists() {
        String::from(candidate)
    } else {
        String::from("powershell")
    }
}

#[cfg(target_os = "windows")]
fn windows_powershell_command(script: &str) -> Vec<String> {
    vec![
        windows_powershell_exe(),
        String::from("-NoProfile"),
        String::from("-Command"),
        script.to_string(),
    ]
}

fn detect_cpu_vendor(model: &str) -> Option<String> {
    let l = model.to_lowercase();
    if l.contains("intel") {
        return Some(String::from("intel"));
    }
    if l.contains("amd") || l.contains("ryzen") {
        return Some(String::from("amd"));
    }
    if l.contains("apple") || l.contains("m1") || l.contains("m2") || l.contains("m3") {
        return Some(String::from("apple"));
    }
    None
}

#[cfg(target_os = "windows")]
fn os_version_command() -> Vec<String> {
    vec![String::from("cmd"), String::from("/C"), String::from("ver")]
}

#[cfg(target_os = "macos")]
fn os_version_command() -> Vec<String> {
    vec![String::from("sw_vers"), String::from("-productVersion")]
}

#[cfg(target_os = "linux")]
fn os_version_command() -> Vec<String> {
    vec![String::from("sh"), String::from("-c"), String::from("uname -r")]
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn os_version_command() -> Vec<String> {
    vec![]
}

fn run_and_trim(command: Vec<String>) -> Option<String> {
    if command.is_empty() {
        return None;
    }

    let mut cmd = Command::new(&command[0]);
    for arg in command.iter().skip(1) {
        cmd.arg(arg);
    }

    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}







#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_parse_macos_system_profiler_json_is_stable() {
        let raw = include_str!("../tests/fixtures/macos_spdisplays.json");
        let gpus = parse_macos_system_profiler_json(raw);
        assert_eq!(gpus.len(), 2);
        assert!(gpus.iter().any(|g| g.name.contains("Apple M3 Max")));
        assert!(gpus.iter().any(|g| g.name.contains("Radeon Pro 5600M")));
    }

    #[test]
    fn fixture_parse_macos_system_profiler_text_is_stable() {
        let raw = include_str!("../tests/fixtures/macos_spdisplays.txt");
        let gpus = parse_macos_system_profiler_text(raw);
        assert_eq!(gpus.len(), 2);
        assert!(gpus.iter().all(|g| !matches!(g.vendor, GpuVendor::Unknown)));
    }

    #[test]
    fn fixture_parse_linux_lspci_is_stable() {
        let raw = include_str!("../tests/fixtures/linux_lspci_gpu.txt");
        let gpus = parse_linux_lspci(raw);
        assert_eq!(gpus.len(), 2);
        assert!(gpus.iter().any(|g| matches!(g.vendor, GpuVendor::Nvidia)));
        assert!(gpus.iter().any(|g| matches!(g.vendor, GpuVendor::Amd)));
    }

    #[test]
    fn fixture_parse_linux_lshw_is_stable() {
        let raw = include_str!("../tests/fixtures/linux_lshw_display.txt");
        let gpus = parse_linux_lshw_display(raw);
        assert_eq!(gpus.len(), 2);
        assert!(gpus.iter().any(|g| g.vram_mb >= 24 * 1024));
        assert!(gpus.iter().any(|g| g.vram_mb >= 12 * 1024));
    }

    #[test]
    fn fixture_parse_vm_stat_pages_is_stable() {
        let raw = include_str!("../tests/fixtures/macos_vm_stat.txt");
        assert_eq!(parse_vm_stat_pages(raw, "Pages free"), 1024);
        assert_eq!(parse_vm_stat_pages(raw, "Pages inactive"), 64000);
        assert_eq!(parse_vm_stat_pages(raw, "Pages purgeable"), 128);
    }

    #[test]
    fn fixture_score_tier_is_stable() {
        let mem_high = MemoryInfo {
            total_mb: 32768,
            available_mb: Some(16000),
        };
        let gpu_high = vec![build_gpu("NVIDIA GeForce RTX 3090", 24576)];
        let (tier_h, _, _) = score_tier(&mem_high, &gpu_high);
        assert!(matches!(tier_h, Tier::High));

        let mem_mid = MemoryInfo {
            total_mb: 16384,
            available_mb: Some(8000),
        };
        let gpu_mid = vec![build_gpu("Intel Iris Xe", 1024)];
        let (tier_m, _, _) = score_tier(&mem_mid, &gpu_mid);
        assert!(matches!(tier_m, Tier::Medium));

        let mem_low = MemoryInfo {
            total_mb: 4096,
            available_mb: Some(1000),
        };
        let gpu_low = vec![build_gpu("Intel HD Graphics 4000", 0)];
        let (tier_l, _, _) = score_tier(&mem_low, &gpu_low);
        assert!(matches!(tier_l, Tier::Low));
    }
}




