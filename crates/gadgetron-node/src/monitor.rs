use gadgetron_core::node::{GpuInfo, NodeResources};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

pub struct ResourceMonitor {
    sys: System,
}

impl ResourceMonitor {
    pub fn new() -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::everything()
                .with_memory(MemoryRefreshKind::everything())
                .with_cpu(CpuRefreshKind::everything()),
        );
        Self { sys }
    }

    /// Collect current system resource metrics.
    pub fn collect(&mut self) -> NodeResources {
        self.sys.refresh_specifics(
            RefreshKind::everything()
                .with_memory(MemoryRefreshKind::everything())
                .with_cpu(CpuRefreshKind::everything()),
        );

        let cpu_usage = self.sys.global_cpu_usage();
        let memory_total = self.sys.total_memory();
        let memory_used = self.sys.used_memory();

        let gpus = self.collect_gpu_info();

        NodeResources {
            gpus,
            cpu_usage_pct: cpu_usage,
            memory_total_bytes: memory_total,
            memory_used_bytes: memory_used,
        }
    }

    fn collect_gpu_info(&self) -> Vec<GpuInfo> {
        // NVML GPU monitoring is available when the "nvml" feature is enabled
        #[cfg(feature = "nvml")]
        {
            if let Ok(nvml) = nvml_wrapper::Nvml::init() {
                if let Ok(count) = nvml.device_count() {
                    let mut gpus = Vec::new();
                    for i in 0..count {
                        if let Ok(device) = nvml.device_by_index(i) {
                            let name = device.name().unwrap_or_default();
                            let mem_info = device.memory_info().unwrap_or(
                                nvml_wrapper::struct_wrappers::device::MemoryInfo {
                                    total: 0,
                                    free: 0,
                                    used: 0,
                                },
                            );
                            let util = device
                                .utilization_rates()
                                .map(|u| u.gpu as f32)
                                .unwrap_or(0.0);
                            let temp = device
                                .temperature(
                                    nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu,
                                )
                                .unwrap_or(0);
                            let power = device
                                .power_usage()
                                .map(|p| (p as f32) / 1000.0) // mW to W
                                .unwrap_or(0.0);
                            let power_limit = device
                                .power_management_limit()
                                .map(|p| (p as f32) / 1000.0)
                                .unwrap_or(0.0);

                            gpus.push(GpuInfo {
                                index: i,
                                name,
                                vram_total_mb: mem_info.total / (1024 * 1024),
                                vram_used_mb: mem_info.used / (1024 * 1024),
                                utilization_pct: util,
                                temperature_c: temp,
                                power_draw_w: power,
                                power_limit_w: power_limit,
                            });
                        }
                    }
                    return gpus;
                }
            }
        }

        Vec::new() // No GPU detected or NVML not available
    }
}

impl Default for ResourceMonitor {
    fn default() -> Self {
        Self::new()
    }
}
