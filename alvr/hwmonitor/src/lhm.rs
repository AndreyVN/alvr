//! LibreHardwareMonitor WMI sensor reader.
//!
//! Requires LibreHardwareMonitor to be running (as Administrator). When the
//! `root\LibreHardwareMonitor` namespace is missing we surface
//! `Error::NotAvailable` and the sampler skips this source for that tick.

use alvr_common::{anyhow, debug};
use serde::Deserialize;
use wmi::{COMLibrary, WMIConnection};

const NAMESPACE: &str = "root\\LibreHardwareMonitor";

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct Sensor {
    name: String,
    parent: String,
    #[serde(rename = "SensorType")]
    sensor_type: String,
    value: f32,
}

#[derive(Default, Debug, Clone)]
pub struct CpuSensors {
    pub package_temp_c: Option<f32>,
    pub per_core_temp_c: Vec<f32>,
    pub package_power_w: Option<f32>,
    pub cores_power_w: Option<f32>,
    pub fans_rpm: Vec<(String, u32)>,
}

#[derive(Default, Debug, Clone)]
pub struct GpuSensors {
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub fan_pct: Option<f32>,
    pub fans_rpm: Vec<(String, u32)>,
}

pub struct LhmSource {
    conn: WMIConnection,
}

impl LhmSource {
    /// `com` must be initialized on the current thread before calling.
    pub fn connect(com: COMLibrary) -> anyhow::Result<Self> {
        let conn = WMIConnection::with_namespace_path(NAMESPACE, com)?;
        Ok(Self { conn })
    }

    pub fn read(&self) -> anyhow::Result<(CpuSensors, GpuSensors)> {
        let sensors: Vec<Sensor> = self
            .conn
            .raw_query("SELECT Name, Parent, SensorType, Value FROM Sensor")?;
        debug!("hwmonitor: LHM returned {} sensors", sensors.len());
        Ok(classify(sensors))
    }
}

fn classify(sensors: Vec<Sensor>) -> (CpuSensors, GpuSensors) {
    let mut cpu = CpuSensors::default();
    let mut gpu = GpuSensors::default();

    for s in sensors {
        let kind = HardwareKind::of(&s.parent);
        let name_l = s.name.to_lowercase();

        match s.sensor_type.as_str() {
            "Temperature" => match kind {
                HardwareKind::Cpu => {
                    if name_l.contains("package") || name_l.contains("ccd") {
                        cpu.package_temp_c = Some(s.value);
                    } else if name_l.starts_with("core") || name_l.contains("core #") {
                        cpu.per_core_temp_c.push(s.value);
                    } else if cpu.package_temp_c.is_none() && name_l.contains("cpu") {
                        cpu.package_temp_c = Some(s.value);
                    }
                }
                HardwareKind::Gpu => {
                    if gpu.temp_c.is_none() || name_l.contains("core") || name_l.contains("hot") {
                        gpu.temp_c = Some(s.value);
                    }
                }
                HardwareKind::Other => {}
            },
            "Fan" => match kind {
                HardwareKind::Gpu => {
                    gpu.fans_rpm.push((s.name, s.value as u32));
                }
                _ => {
                    // Most fans (CPU_FAN, CPU_OPT, pumps, chassis) live on the
                    // Super I/O chip under /lpc/*. Disambiguate by name.
                    if name_l.contains("cpu") || name_l.contains("pump") {
                        cpu.fans_rpm.push((s.name, s.value as u32));
                    }
                }
            },
            "Control" if matches!(kind, HardwareKind::Gpu) => {
                // GPU fan duty cycle %. LHM reports this as a Control sensor.
                if name_l.contains("fan") && gpu.fan_pct.is_none() {
                    gpu.fan_pct = Some(s.value);
                }
            }
            "Power" => match kind {
                HardwareKind::Cpu => {
                    // LHM exposes several CPU power sub-domains. We capture the
                    // two most useful: whole-package and cores-only.
                    if name_l.contains("package") {
                        cpu.package_power_w = Some(s.value);
                    } else if name_l == "cpu cores"
                        || name_l == "cores"
                        || name_l.contains("ia cores")
                    {
                        cpu.cores_power_w = Some(s.value);
                    }
                }
                HardwareKind::Gpu => {
                    // Prefer board-total readings ("Package" / "Total" / "TBP")
                    // over rail-specific ones; first match wins otherwise.
                    let is_total = name_l.contains("package")
                        || name_l.contains("total")
                        || name_l.contains("tbp");
                    if gpu.power_w.is_none() || is_total {
                        gpu.power_w = Some(s.value);
                    }
                }
                HardwareKind::Other => {}
            },
            _ => {}
        }
    }

    (cpu, gpu)
}

#[derive(Copy, Clone)]
enum HardwareKind {
    Cpu,
    Gpu,
    Other,
}

impl HardwareKind {
    fn of(parent: &str) -> Self {
        if parent.starts_with("/intelcpu/") || parent.starts_with("/amdcpu/") {
            Self::Cpu
        } else if parent.starts_with("/gpu-") {
            Self::Gpu
        } else {
            Self::Other
        }
    }
}
