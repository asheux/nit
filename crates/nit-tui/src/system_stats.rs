use std::time::{Duration, Instant};

use sysinfo::{CpuExt, System, SystemExt};

#[derive(Clone, Debug)]
struct GpuInfo {
    usage_percent: Option<f32>,
    mem_total_bytes: Option<u64>,
    name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct GpuSummary {
    pub usage_percent: Option<u8>,
    pub mem_total_gb: Option<f32>,
    pub name: Option<String>,
}

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
        if self.last_refresh.elapsed() >= Duration::from_millis(500) {
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
                "CPU {:02}% | {} | MEM {:.1}/{:.1}G",
                cpu, gpu_label, self.mem_used_gb, self.mem_total_gb
            )
        } else {
            format!("CPU {:02}% | {}", cpu, gpu_label)
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

fn gpu_label(info: Option<&GpuInfo>) -> String {
    match info {
        Some(info) => {
            let usage = info
                .usage_percent
                .map(|u| u.round().clamp(0.0, 100.0) as u8);
            let total_gb = info
                .mem_total_bytes
                .map(|b| b as f32 / 1024.0 / 1024.0 / 1024.0);
            if let (Some(usage), Some(total)) = (usage, total_gb) {
                return format!("GPU {:02}%/{:.1}G", usage, total);
            }
            if let Some(total) = total_gb {
                return format!("GPU --/{:.1}G", total);
            }
            if let Some(usage) = usage {
                return format!("GPU {:02}%", usage);
            }
            if let Some(name) = &info.name {
                return format!("GPU {}", name);
            }
            "GPU N/A".to_string()
        }
        None => "GPU N/A".to_string(),
    }
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
    use std::path::Path;

    let drm = Path::new("/sys/class/drm");
    let entries = fs::read_dir(drm).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let device_path = entry.path().join("device");
        let usage = read_u64(device_path.join("gpu_busy_percent")).map(|v| v as f32);
        let mem_total = read_u64(device_path.join("mem_info_vram_total"));
        let driver = fs::read_to_string(device_path.join("uevent"))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("DRIVER="))
                    .map(|l| l.trim_start_matches("DRIVER=").to_string())
            });
        return Some(GpuInfo {
            usage_percent: usage,
            mem_total_bytes: mem_total,
            name: driver.or(Some(name)),
        });
    }
    None
}

#[cfg(target_os = "windows")]
fn query_gpu_info() -> Option<GpuInfo> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn query_gpu_info() -> Option<GpuInfo> {
    None
}

#[cfg(target_os = "linux")]
fn read_u64(path: impl AsRef<std::path::Path>) -> Option<u64> {
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse::<u64>().ok()
}
