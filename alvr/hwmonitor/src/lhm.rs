//! LibreHardwareMonitor web-server sensor reader.
//!
//! Recent LHM versions no longer expose WMI by default — the supported
//! integration path is the built-in HTTP server (Options → Run web server,
//! default `http://localhost:8085`). We fetch `/data.json` once per tick and
//! walk the tree using the `HardwareId` and `Type` fields LHM publishes on
//! every node.
//!
//! When LHM is not running we surface `Error` and the sampler skips this
//! source for that tick (logged once).

use alvr_common::{anyhow, debug};
use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8085/data.json";

#[derive(Deserialize, Debug)]
struct Node {
    #[serde(rename = "Text", default)]
    text: String,
    #[serde(rename = "Value", default)]
    value: String,
    /// Hardware kind identifier (e.g. `/amdcpu/0`, `/gpu-nvidia/0`,
    /// `/motherboard`, `/lpc/nct6797d/0`). Only present on hardware nodes.
    #[serde(rename = "HardwareId", default)]
    hardware_id: String,
    /// Sensor unit identifier (`Temperature`, `Fan`, `Power`, `Control`,
    /// `Load`, `Clock`, `Voltage`, `Data`, `SmallData`, `Factor`,
    /// `Throughput`). Only present on sensor leaves.
    #[serde(rename = "Type", default)]
    sensor_type: String,
    #[serde(rename = "Children", default)]
    children: Vec<Node>,
}

#[derive(Default, Debug, Clone)]
pub struct CpuSensors {
    pub package_temp_c: Option<f32>,
    pub per_core_temp_c: Vec<f32>,
    pub package_power_w: Option<f32>,
    pub cores_power_w: Option<f32>,
    pub per_core_power_w: Vec<f32>,
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
    agent: ureq::Agent,
    url: String,
}

impl LhmSource {
    /// Probes the URL once. Returns an error if LHM isn't reachable.
    pub fn connect(url: &str) -> anyhow::Result<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(2)))
            .build()
            .into();
        let mut probe = agent.get(url).call()?;
        let _ = probe.body_mut().read_to_string();
        Ok(Self {
            agent,
            url: url.to_string(),
        })
    }

    pub fn read(&self) -> anyhow::Result<(CpuSensors, GpuSensors)> {
        let root: Node = self.agent.get(&self.url).call()?.body_mut().read_json()?;
        let (cpu, gpu) = classify(&root);
        debug!(
            "hwmonitor: LHM pkg_temp={:?} pkg_pwr={:?} cores_pwr={:?} cpu_fans={} | gpu_temp={:?} gpu_pwr={:?} gpu_fan%={:?} gpu_fans={}",
            cpu.package_temp_c,
            cpu.package_power_w,
            cpu.cores_power_w,
            cpu.fans_rpm.len(),
            gpu.temp_c,
            gpu.power_w,
            gpu.fan_pct,
            gpu.fans_rpm.len(),
        );
        Ok((cpu, gpu))
    }
}

#[derive(Copy, Clone, Debug)]
enum HardwareKind {
    Cpu,
    Gpu,
    Mainboard,
    Unknown,
}

impl HardwareKind {
    /// Resolves the hardware kind from LHM's `HardwareId` prefix.
    fn of_id(id: &str) -> Self {
        if id.starts_with("/amdcpu/") || id.starts_with("/intelcpu/") {
            Self::Cpu
        } else if id.starts_with("/gpu-") {
            Self::Gpu
        } else if id == "/motherboard" || id.starts_with("/lpc/") {
            Self::Mainboard
        } else {
            Self::Unknown
        }
    }
}

/// Walks the LHM tree using `HardwareId` to track which hardware we're inside
/// and `Type` to decide how to interpret each leaf.
fn classify(root: &Node) -> (CpuSensors, GpuSensors) {
    let mut cpu = CpuSensors::default();
    let mut gpu = GpuSensors::default();
    // Sum GPU power sub-rails when no Package/Total/TBP sensor exists.
    let mut gpu_partial_power = 0.0_f32;
    let mut gpu_partial_seen = false;

    walk(
        root,
        &mut cpu,
        &mut gpu,
        &mut gpu_partial_power,
        &mut gpu_partial_seen,
        HardwareKind::Unknown,
    );

    // AMD Ryzen exposes per-core power but no aggregate "Cores" rail;
    // derive cores_power_w from the per-core readings so the dashboard
    // gets a single number comparable to Intel's "CPU Cores" sensor.
    if cpu.cores_power_w.is_none() && !cpu.per_core_power_w.is_empty() {
        cpu.cores_power_w = Some(cpu.per_core_power_w.iter().sum());
    }
    if gpu.power_w.is_none() && gpu_partial_seen {
        gpu.power_w = Some(gpu_partial_power);
    }

    (cpu, gpu)
}

fn walk(
    node: &Node,
    cpu: &mut CpuSensors,
    gpu: &mut GpuSensors,
    gpu_partial_power: &mut f32,
    gpu_partial_seen: &mut bool,
    kind: HardwareKind,
) {
    // Only override the inherited kind when this node is a recognised
    // hardware root (carries a HardwareId we know).
    let kind = match HardwareKind::of_id(&node.hardware_id) {
        HardwareKind::Unknown => kind,
        other => other,
    };
    for child in &node.children {
        walk(child, cpu, gpu, gpu_partial_power, gpu_partial_seen, kind);
    }
    if !node.sensor_type.is_empty() && !node.value.is_empty() {
        apply_leaf(node, kind, cpu, gpu, gpu_partial_power, gpu_partial_seen);
    }
}

fn apply_leaf(
    node: &Node,
    kind: HardwareKind,
    cpu: &mut CpuSensors,
    gpu: &mut GpuSensors,
    gpu_partial_power: &mut f32,
    gpu_partial_seen: &mut bool,
) {
    let Some(value) = parse_value(&node.value) else {
        return;
    };
    let name_l = node.text.to_lowercase();

    match node.sensor_type.as_str() {
        "Temperature" => match kind {
            HardwareKind::Cpu | HardwareKind::Mainboard => {
                apply_cpu_temperature(&name_l, value, cpu);
            }
            HardwareKind::Gpu => {
                let prefer = name_l.contains("core") || name_l.contains("hot");
                if gpu.temp_c.is_none() || prefer {
                    gpu.temp_c = Some(value);
                }
            }
            HardwareKind::Unknown => {}
        },
        "Fan" => match kind {
            HardwareKind::Gpu => gpu.fans_rpm.push((node.text.clone(), value as u32)),
            HardwareKind::Cpu | HardwareKind::Mainboard => {
                // CPU + chassis fans live on the Super I/O chip; disambiguate
                // by name so we don't surface random chassis fans as CPU.
                if name_l.contains("cpu") || name_l.contains("pump") {
                    cpu.fans_rpm.push((node.text.clone(), value as u32));
                }
            }
            HardwareKind::Unknown => {}
        },
        "Power" => match kind {
            HardwareKind::Cpu => {
                if name_l.contains("package") {
                    cpu.package_power_w = Some(value);
                } else if name_l == "cpu cores" || name_l == "cores" || name_l.contains("ia cores")
                {
                    cpu.cores_power_w = Some(value);
                } else if name_l.starts_with("core #")
                    || name_l.starts_with("cpu core #")
                    || (name_l.starts_with("core ") && name_l.contains("(smu)"))
                {
                    // Per-core power readings (Ryzen `Core #N (SMU)` and
                    // some Intel CPUs that expose per-core RAPL).
                    cpu.per_core_power_w.push(value);
                }
            }
            HardwareKind::Gpu => {
                let is_total = name_l.contains("package")
                    || name_l.contains("total")
                    || name_l.contains("tbp");
                if is_total {
                    gpu.power_w = Some(value);
                } else if gpu.power_w.is_none() {
                    // No board-total sensor on this GPU (typical for iGPUs):
                    // accumulate sub-rail powers as a fallback.
                    *gpu_partial_power += value;
                    *gpu_partial_seen = true;
                }
            }
            _ => {}
        },
        "Control" if matches!(kind, HardwareKind::Gpu) => {
            // GPU fan duty cycle (0–100). LHM exposes this as a Control leaf.
            if name_l.contains("fan") && gpu.fan_pct.is_none() {
                gpu.fan_pct = Some(value);
            }
        }
        _ => {}
    }
}

fn apply_cpu_temperature(name_l: &str, value: f32, cpu: &mut CpuSensors) {
    // Intel: "CPU Package", "CPU Core #1..#N", "CPU Core Max/Average"
    // AMD:   "Core (Tctl/Tdie)" overall, "CCD #1 (Tdie).." per-chiplet
    let is_per_core_intel = name_l.contains("core #");
    let is_intel_package = name_l.contains("package");
    let is_amd_chiplet = name_l.contains("ccd");
    let is_amd_overall =
        (name_l.contains("tctl") || name_l.contains("tdie")) && !is_amd_chiplet;
    let is_aggregate =
        name_l.contains("max") || name_l.contains("average") || name_l.contains("avg");

    if is_per_core_intel {
        cpu.per_core_temp_c.push(value);
    } else if is_intel_package || is_amd_overall {
        cpu.package_temp_c = Some(value);
    } else if is_amd_chiplet && cpu.package_temp_c.is_none() {
        cpu.package_temp_c = Some(value);
    } else if !is_aggregate && cpu.package_temp_c.is_none() && name_l.contains("cpu") {
        cpu.package_temp_c = Some(value);
    }
}

/// Pulls the first whitespace-delimited token and parses it as f32.
/// LHM values are formatted like "45.5 °C", "800 RPM", "9,8 W", "50 %"
/// — note the comma decimal separator on non-English Windows locales.
fn parse_value(s: &str) -> Option<f32> {
    let token = s.split_whitespace().next()?;
    token.replace(',', ".").parse().ok()
}
