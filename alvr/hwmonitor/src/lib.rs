//! Host hardware telemetry for the ALVR streamer.
//!
//! Spawns a background sampler thread that periodically queries:
//!   * `sysinfo`                  — CPU load, memory, per-process stats
//!   * `LibreHardwareMonitor` WMI — temperatures and fan RPM
//!   * `nvidia-smi` (subprocess)  — NVIDIA GPU utilization / encoder load / power
//!   * `Win32_PerfFormattedData_Tcpip_NetworkInterface` WMI — adapter counters
//!
//! Each source degrades gracefully when its backend is missing; callers always
//! get a `Snapshot` with `None` filled in for unavailable sources.

mod network;
mod nvidia_smi;
mod sampler;
mod sysinfo_source;

#[cfg(windows)]
mod lhm;

pub use sampler::{Hwmonitor, HwmonitorConfig};

use serde::{Deserialize, Serialize};

/// One full hardware telemetry sample. Fields are `Option` so a missing
/// backend produces `null` in the serialized JSON rather than failing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub cpu: Option<CpuSample>,
    pub gpu: Option<GpuSample>,
    pub memory: Option<MemorySample>,
    pub network: Vec<NetSample>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CpuSample {
    pub total_pct: f32,
    pub per_core_pct: Vec<f32>,
    pub freq_mhz: u32,
    /// Per-process load for the streamer (`vrserver.exe` on Windows).
    pub vrserver_pct: Option<f32>,
    /// Package / die temperature in degrees Celsius, when available from LHM.
    pub package_temp_c: Option<f32>,
    /// Per-core temperatures, when reported by LHM.
    pub per_core_temp_c: Vec<f32>,
    /// Whole-socket power draw in watts (Intel RAPL `CPU Package` / AMD `Package`).
    pub package_power_w: Option<f32>,
    /// Cores-only power draw in watts (`CPU Cores` / `IA Cores`).
    pub cores_power_w: Option<f32>,
    /// Named fan readings in RPM (e.g. `CPU Fan`, `Pump Fan`).
    pub fans_rpm: Vec<NamedValue<u32>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuSample {
    pub name: Option<String>,
    /// Overall GPU utilization, 0..=100. From `nvidia-smi utilization.gpu`.
    pub util_pct: Option<f32>,
    /// NVENC encoder block utilization, 0..=100.
    pub encoder_util_pct: Option<f32>,
    /// NVDEC decoder block utilization, 0..=100.
    pub decoder_util_pct: Option<f32>,
    pub mem_used_mb: Option<u32>,
    pub mem_total_mb: Option<u32>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub power_limit_w: Option<f32>,
    pub clock_graphics_mhz: Option<u32>,
    pub clock_memory_mhz: Option<u32>,
    /// Video / NVENC clock domain.
    pub clock_video_mhz: Option<u32>,
    /// Comma-separated active throttle reasons from `clocks_throttle_reasons.active`.
    pub throttle_reasons: Option<String>,
    pub pstate: Option<String>,
    /// Reported fan duty cycle, 0..=100.
    pub fan_pct: Option<f32>,
    /// Per-fan readings in RPM (from LHM, since `nvidia-smi` only exposes %).
    pub fans_rpm: Vec<NamedValue<u32>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MemorySample {
    pub total_mb: u64,
    pub used_mb: u64,
    pub available_mb: u64,
    pub used_pct: f32,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
    /// Working set of the streamer process in MB, when found.
    pub vrserver_working_set_mb: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetSample {
    pub adapter: String,
    pub bytes_sent_per_sec: u64,
    pub bytes_recv_per_sec: u64,
    pub packets_sent_per_sec: u64,
    pub packets_recv_per_sec: u64,
    pub outbound_errors: u64,
    pub outbound_discarded: u64,
    pub current_bandwidth_bps: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NamedValue<T> {
    pub name: String,
    pub value: T,
}
