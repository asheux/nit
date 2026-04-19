use std::time::{Duration, Instant};

use sysinfo::{CpuExt, System, SystemExt};

/// Minimum delay between cached-stats refreshes; keeps UI repaints cheap.
const REFRESH_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug)]
struct GpuInfo {
    usage_percent: Option<f32>,
    mem_total_bytes: Option<u64>,
    name: Option<String>,
}

/// GPU telemetry snapshot normalized for rendering in the status bar.
#[derive(Clone, Debug)]
pub struct GpuSummary {
    pub usage_percent: Option<u8>,
    pub mem_total_gb: Option<f32>,
    pub name: Option<String>,
}

/// Cached CPU/memory/GPU statistics refreshed lazily from `sysinfo` and platform probes.
pub struct SystemStats {
    system: System,
    last_refresh: Instant,
    cpu_percent: f32,
    mem_used_gb: f32,
    mem_total_gb: f32,
    gpu: Option<GpuInfo>,
}

impl SystemStats {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu();
        system.refresh_memory();
        let mut stats = Self {
            system,
            last_refresh: Instant::now(),
            cpu_percent: 0.0,
            mem_used_gb: 0.0,
            mem_total_gb: 0.0,
            gpu: None,
        };
        stats.update_cached();
        stats
    }

    pub fn refresh_if_needed(&mut self) {
        if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.system.refresh_cpu();
            self.system.refresh_memory();
            self.update_cached();
            self.last_refresh = Instant::now();
        }
    }

    pub fn label(&self) -> String {
        let cpu = self.cpu_percent.round().clamp(0.0, 100.0) as u8;
        let gpu_label = gpu_label(self.gpu.as_ref());
        if self.mem_total_gb > 0.0 {
            format!(
                "CPU {cpu:02}% | {gpu_label} | MEM {:.1}/{:.1}G",
                self.mem_used_gb, self.mem_total_gb
            )
        } else {
            format!("CPU {cpu:02}% | {gpu_label}")
        }
    }

    pub fn cpu_percent(&self) -> f32 {
        self.cpu_percent
    }

    pub fn mem_used_gb(&self) -> f32 {
        self.mem_used_gb
    }

    pub fn mem_total_gb(&self) -> f32 {
        self.mem_total_gb
    }

    pub fn gpu_summary(&self) -> GpuSummary {
        let Some(info) = &self.gpu else {
            return GpuSummary {
                usage_percent: None,
                mem_total_gb: None,
                name: None,
            };
        };
        let usage = info
            .usage_percent
            .map(|u| u.round().clamp(0.0, 100.0) as u8);
        let mem_total_gb = info
            .mem_total_bytes
            .map(|b| b as f32 / 1024.0 / 1024.0 / 1024.0);
        GpuSummary {
            usage_percent: usage,
            mem_total_gb,
            name: info.name.clone(),
        }
    }

    fn update_cached(&mut self) {
        let cpus = self.system.cpus();
        self.cpu_percent = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(|c| c.cpu_usage()).sum::<f32>() / cpus.len() as f32
        };
        let total_kib = self.system.total_memory() as f32;
        let used_kib = self.system.used_memory() as f32;
        self.mem_total_gb = total_kib / 1024.0 / 1024.0;
        self.mem_used_gb = used_kib / 1024.0 / 1024.0;
        self.gpu = query_gpu_info();
    }
}

impl Default for SystemStats {
    fn default() -> Self {
        Self::new()
    }
}

fn gpu_label(info: Option<&GpuInfo>) -> String {
    let Some(card) = info else {
        return "GPU N/A".to_string();
    };
    let pct = card
        .usage_percent
        .map(|raw| raw.round().clamp(0.0, 100.0) as u8);
    let gigs = card
        .mem_total_bytes
        .map(|bytes| bytes as f32 / 1024.0 / 1024.0 / 1024.0);
    if let (Some(load), Some(memory)) = (pct, gigs) {
        return format!("GPU {load:02}%/{memory:.1}G");
    }
    if let Some(only_mem) = gigs {
        return format!("GPU --/{only_mem:.1}G");
    }
    if let Some(only_load) = pct {
        return format!("GPU {only_load:02}%");
    }
    if let Some(hardware) = &card.name {
        return format!("GPU {hardware}");
    }
    "GPU N/A".to_string()
}

#[cfg(target_os = "macos")]
fn query_gpu_info() -> Option<GpuInfo> {
    let device = metal::Device::system_default()?;
    let name = device.name().to_string();
    let mem_total = device.recommended_max_working_set_size();
    Some(GpuInfo {
        usage_percent: None,
        mem_total_bytes: if mem_total > 0 { Some(mem_total) } else { None },
        name: Some(name),
    })
}

#[cfg(target_os = "linux")]
fn query_gpu_info() -> Option<GpuInfo> {
    use std::fs;

    let device_root = fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .find(|node| {
            let stem = node.file_name().to_string_lossy().to_string();
            stem.starts_with("card") && !stem.contains('-')
        })?;
    let card_label = device_root.file_name().to_string_lossy().to_string();
    let probe = device_root.path().join("device");
    let driver_id = fs::read_to_string(probe.join("uevent"))
        .ok()
        .and_then(|contents| {
            contents
                .lines()
                .find_map(|line| line.strip_prefix("DRIVER=").map(str::to_string))
        });
    Some(GpuInfo {
        usage_percent: read_u64(probe.join("gpu_busy_percent")).map(|raw| raw as f32),
        mem_total_bytes: read_u64(probe.join("mem_info_vram_total")),
        name: driver_id.or(Some(card_label)),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn query_gpu_info() -> Option<GpuInfo> {
    None
}

#[cfg(target_os = "linux")]
fn read_u64(path: impl AsRef<std::path::Path>) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<u64>().ok()
}
