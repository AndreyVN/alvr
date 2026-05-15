use crate::{CpuSample, MemorySample};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, System};

const VRSERVER_NAMES: &[&str] = &["vrserver.exe", "vrserver"];

pub struct SysinfoSource {
    sys: System,
}

impl SysinfoSource {
    pub fn new() -> Self {
        let mut sys = System::new();
        // Prime CPU counters; first reading is always zero.
        sys.refresh_cpu_specifics(CpuRefreshKind::nothing().with_cpu_usage().with_frequency());
        Self { sys }
    }

    pub fn refresh(&mut self) {
        self.sys.refresh_cpu_specifics(
            CpuRefreshKind::nothing().with_cpu_usage().with_frequency(),
        );
        self.sys
            .refresh_memory_specifics(MemoryRefreshKind::everything());
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory(),
        );
    }

    pub fn cpu(&self) -> CpuSample {
        let cpus = self.sys.cpus();
        let per_core_pct: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
        let total_pct = self.sys.global_cpu_usage();
        let freq_mhz = cpus.first().map(|c| c.frequency() as u32).unwrap_or(0);

        CpuSample {
            total_pct,
            per_core_pct,
            freq_mhz,
            vrserver_pct: self.find_vrserver_cpu_pct(),
            package_temp_c: None,
            per_core_temp_c: Vec::new(),
            package_power_w: None,
            cores_power_w: None,
            per_core_power_w: Vec::new(),
            fans_rpm: Vec::new(),
        }
    }

    pub fn memory(&self) -> MemorySample {
        const MB: u64 = 1024 * 1024;
        let total = self.sys.total_memory();
        let used = self.sys.used_memory();
        let available = self.sys.available_memory();
        let used_pct = if total > 0 {
            (used as f64 / total as f64 * 100.0) as f32
        } else {
            0.0
        };

        MemorySample {
            total_mb: total / MB,
            used_mb: used / MB,
            available_mb: available / MB,
            used_pct,
            swap_total_mb: self.sys.total_swap() / MB,
            swap_used_mb: self.sys.used_swap() / MB,
            vrserver_working_set_mb: self.find_vrserver_working_set_mb(),
        }
    }

    fn find_vrserver(&self) -> Option<&sysinfo::Process> {
        self.sys.processes().values().find(|p| {
            let n = p.name().to_string_lossy();
            VRSERVER_NAMES.iter().any(|target| n.eq_ignore_ascii_case(target))
        })
    }

    fn find_vrserver_cpu_pct(&self) -> Option<f32> {
        self.find_vrserver().map(|p| p.cpu_usage())
    }

    fn find_vrserver_working_set_mb(&self) -> Option<u64> {
        self.find_vrserver().map(|p| p.memory() / (1024 * 1024))
    }
}
