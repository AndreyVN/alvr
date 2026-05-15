//! LibreHardwareMonitor web-server sensor reader.
//!
//! Recent LHM versions no longer expose WMI by default — the supported
//! integration path is the built-in HTTP server (Options → Run web server,
//! default `http://localhost:8085`). We fetch `/data.json` once per tick and
//! walk the hardware → group → sensor tree.
//!
//! When LHM is not running we surface `Error` and the sampler skips this
//! source for that tick (logged once).

use alvr_common::{anyhow, debug};
use serde::Deserialize;
use std::time::Duration;

pub const DEFAULT_URL: &str = "http://127.0.0.1:8085/data.json";

#[derive(Deserialize, Debug)]
struct Node {
    #[serde(rename = "Text")]
    #[serde(default)]
    text: String,
    #[serde(rename = "Value")]
    #[serde(default)]
    value: String,
    #[serde(rename = "ImageURL")]
    #[serde(default)]
    image_url: String,
    #[serde(rename = "Children")]
    #[serde(default)]
    children: Vec<Node>,
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
        // Drain to release the connection.
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
            "hwmonitor: LHM HTTP returned cpu_fans={} gpu_fans={} package_temp_c={:?}",
            cpu.fans_rpm.len(),
            gpu.fans_rpm.len(),
            cpu.package_temp_c,
        );
        Ok((cpu, gpu))
    }
}

/// Walks the root → computer → hardware → group → sensor tree.
fn classify(root: &Node) -> (CpuSensors, GpuSensors) {
    let mut cpu = CpuSensors::default();
    let mut gpu = GpuSensors::default();

    fn walk(node: &Node, cpu: &mut CpuSensors, gpu: &mut GpuSensors, kind: HardwareKind) {
        // Re-classify at hardware-level nodes (those with an ImageURL).
        let kind = if !node.image_url.is_empty() {
            HardwareKind::of(&node.image_url, &node.text)
        } else {
            kind
        };
        for child in &node.children {
            walk(child, cpu, gpu, kind);
        }
        // Leaves carry the Value; nodes with children may have empty Value.
        if !node.value.is_empty() {
            apply_leaf(node, kind, cpu, gpu);
        }
    }

    walk(root, &mut cpu, &mut gpu, HardwareKind::Unknown);
    (cpu, gpu)
}

fn apply_leaf(node: &Node, kind: HardwareKind, cpu: &mut CpuSensors, gpu: &mut GpuSensors) {
    let Some(value) = parse_value(&node.value) else {
        return;
    };
    let unit = unit_hint(&node.value);
    let name_l = node.text.to_lowercase();

    match unit {
        Unit::Celsius => match kind {
            HardwareKind::Cpu | HardwareKind::Mainboard => apply_cpu_temperature(&name_l, value, cpu),
            HardwareKind::Gpu => {
                let prefer = name_l.contains("core") || name_l.contains("hot");
                if gpu.temp_c.is_none() || prefer {
                    gpu.temp_c = Some(value);
                }
            }
            HardwareKind::Unknown => {}
        },
        Unit::Rpm => match kind {
            HardwareKind::Gpu => gpu.fans_rpm.push((node.text.clone(), value as u32)),
            // CPU + chassis fans live under the mainboard / superio chip in LHM.
            HardwareKind::Cpu | HardwareKind::Mainboard => {
                if name_l.contains("cpu") || name_l.contains("pump") {
                    cpu.fans_rpm.push((node.text.clone(), value as u32));
                }
            }
            HardwareKind::Unknown => {}
        },
        Unit::Watts => match kind {
            HardwareKind::Cpu => {
                if name_l.contains("package") {
                    cpu.package_power_w = Some(value);
                } else if name_l == "cpu cores" || name_l == "cores" || name_l.contains("ia cores")
                {
                    cpu.cores_power_w = Some(value);
                }
            }
            HardwareKind::Gpu => {
                let is_total = name_l.contains("package")
                    || name_l.contains("total")
                    || name_l.contains("tbp");
                if gpu.power_w.is_none() || is_total {
                    gpu.power_w = Some(value);
                }
            }
            _ => {}
        },
        Unit::Percent => {
            // GPU fan duty cycle: "GPU Fan" leaf with "%" unit under a GPU
            // node lives in the "Controls" group.
            if matches!(kind, HardwareKind::Gpu) && name_l.contains("fan") && gpu.fan_pct.is_none()
            {
                gpu.fan_pct = Some(value);
            }
        }
        Unit::Other => {}
    }
}

fn apply_cpu_temperature(name_l: &str, value: f32, cpu: &mut CpuSensors) {
    // Intel:  "CPU Package", "CPU Core #1..#N", "CPU Core Max/Average"
    // AMD:    "Core (Tctl/Tdie)" overall, "CCD #1 (Tdie)..#N" per-chiplet
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

#[derive(Copy, Clone, Debug)]
enum HardwareKind {
    Cpu,
    Gpu,
    Mainboard,
    Unknown,
}

impl HardwareKind {
    fn of(image_url: &str, text: &str) -> Self {
        let url_l = image_url.to_lowercase();
        let text_l = text.to_lowercase();
        if url_l.contains("cpu") || text_l.contains(" cpu") || text_l.starts_with("amd ryzen")
            || text_l.starts_with("intel core") || text_l.starts_with("intel xeon")
            || text_l.contains("threadripper") || text_l.contains("epyc")
        {
            Self::Cpu
        } else if url_l.contains("gpu")
            || text_l.contains("geforce")
            || text_l.contains("radeon")
            || text_l.contains("nvidia")
            || text_l.contains("intel arc")
        {
            Self::Gpu
        } else if url_l.contains("mainboard") || url_l.contains("superio") || url_l.contains("lpc")
        {
            Self::Mainboard
        } else {
            Self::Unknown
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum Unit {
    Celsius,
    Rpm,
    Watts,
    Percent,
    Other,
}

fn unit_hint(value: &str) -> Unit {
    if value.contains('°') || value.contains(" C") {
        Unit::Celsius
    } else if value.contains("RPM") {
        Unit::Rpm
    } else if value.ends_with(" W") || value.contains(" W ") {
        Unit::Watts
    } else if value.contains('%') {
        Unit::Percent
    } else {
        Unit::Other
    }
}

/// Pulls the first whitespace-delimited token and parses it as f32.
/// LHM values are formatted like "45.5 °C", "800 RPM", "95.0 W", "50 %".
fn parse_value(s: &str) -> Option<f32> {
    let token = s.split_whitespace().next()?;
    token.replace(',', ".").parse().ok()
}
