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
pub struct LhmReadings {
    pub cpu: CpuSensors,
    pub gpu: GpuSensors,
    pub storages: Vec<StorageSensors>,
    pub dimms: Vec<DimmSensors>,
}

#[derive(Default, Debug, Clone)]
pub struct DimmSensors {
    pub slot: String,
    pub capacity_gb: Option<f32>,
    pub temp_c: Option<f32>,
}

#[derive(Default, Debug, Clone)]
pub struct StorageSensors {
    pub device: String,
    pub temp_c: Option<f32>,
    pub used_pct: Option<f32>,
    pub life_left_pct: Option<f32>,
    pub total_gb: Option<f32>,
    pub free_gb: Option<f32>,
}

#[derive(Default, Debug, Clone)]
pub struct GpuSensors {
    pub name: Option<String>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
    pub fan_pct: Option<f32>,
    pub fans_rpm: Vec<(String, u32)>,
    pub util_pct: Option<f32>,
    pub encoder_util_pct: Option<f32>,
    pub decoder_util_pct: Option<f32>,
    pub mem_used_mb: Option<u32>,
    pub mem_total_mb: Option<u32>,
    pub clock_graphics_mhz: Option<u32>,
    pub clock_memory_mhz: Option<u32>,
    pub clock_video_mhz: Option<u32>,
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

    pub fn read(&self) -> anyhow::Result<LhmReadings> {
        let root: Node = self.agent.get(&self.url).call()?.body_mut().read_json()?;
        let readings = classify(&root);
        debug!(
            "hwmonitor: LHM pkg_temp={:?} pkg_pwr={:?} cores_pwr={:?} | gpu_temp={:?} gpu_pwr={:?} | storages={} dimms={}",
            readings.cpu.package_temp_c,
            readings.cpu.package_power_w,
            readings.cpu.cores_power_w,
            readings.gpu.temp_c,
            readings.gpu.power_w,
            readings.storages.len(),
            readings.dimms.len(),
        );
        Ok(readings)
    }
}

#[derive(Copy, Clone, Debug)]
enum HardwareKind {
    Cpu,
    Gpu,
    Mainboard,
    Storage,
    MemoryDimm,
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
        } else if id.starts_with("/nvme/") || id.starts_with("/hdd/") || id.starts_with("/storage/")
        {
            Self::Storage
        } else if id.starts_with("/memory/dimm/") {
            Self::MemoryDimm
        } else {
            Self::Unknown
        }
    }
}

#[derive(Default)]
struct Scratch {
    /// Sum of GPU power sub-rails — fallback when no Package/Total/TBP sensor.
    gpu_partial_power: f32,
    gpu_partial_seen: bool,
    /// LHM exposes GPU memory as Used + Free; we derive Total = Used + Free.
    gpu_mem_used_mb: Option<u32>,
    gpu_mem_free_mb: Option<u32>,
    /// Storage entries accumulated in the order LHM emits them. The walker
    /// pushes a new entry every time it enters a `/nvme/*` or `/hdd/*` node
    /// and routes that subtree's leaves into the last entry.
    storages: Vec<StorageSensors>,
    /// Same pattern for `/memory/dimm/*`.
    dimms: Vec<DimmSensors>,
}

/// Walks the LHM tree using `HardwareId` to track which hardware we're inside
/// and `Type` to decide how to interpret each leaf.
fn classify(root: &Node) -> LhmReadings {
    let mut cpu = CpuSensors::default();
    let mut gpu = GpuSensors::default();
    let mut scratch = Scratch::default();

    walk(
        root,
        &mut cpu,
        &mut gpu,
        &mut scratch,
        HardwareKind::Unknown,
    );

    // AMD Ryzen exposes per-core power but no aggregate "Cores" rail;
    // derive cores_power_w from the per-core readings so the dashboard
    // gets a single number comparable to Intel's "CPU Cores" sensor.
    if cpu.cores_power_w.is_none() && !cpu.per_core_power_w.is_empty() {
        cpu.cores_power_w = Some(cpu.per_core_power_w.iter().sum());
    }
    if gpu.power_w.is_none() && scratch.gpu_partial_seen {
        gpu.power_w = Some(scratch.gpu_partial_power);
    }
    if gpu.mem_used_mb.is_none() {
        gpu.mem_used_mb = scratch.gpu_mem_used_mb;
    }
    if gpu.mem_total_mb.is_none() {
        gpu.mem_total_mb = match (scratch.gpu_mem_used_mb, scratch.gpu_mem_free_mb) {
            (Some(u), Some(f)) => Some(u + f),
            _ => None,
        };
    }

    LhmReadings {
        cpu,
        gpu,
        storages: scratch.storages,
        dimms: scratch.dimms,
    }
}

fn walk(
    node: &Node,
    cpu: &mut CpuSensors,
    gpu: &mut GpuSensors,
    scratch: &mut Scratch,
    kind: HardwareKind,
) {
    // Only override the inherited kind when this node is a recognised
    // hardware root (carries a HardwareId we know).
    let kind = match HardwareKind::of_id(&node.hardware_id) {
        HardwareKind::Unknown => kind,
        other => {
            match other {
                HardwareKind::Gpu if gpu.name.is_none() && !node.text.is_empty() => {
                    gpu.name = Some(node.text.clone());
                }
                HardwareKind::Storage => {
                    // Open a new bucket for this device; leaves inside this
                    // subtree route to the last (just-pushed) entry.
                    scratch.storages.push(StorageSensors {
                        device: node.text.clone(),
                        ..Default::default()
                    });
                }
                HardwareKind::MemoryDimm => {
                    scratch.dimms.push(DimmSensors {
                        slot: node.text.clone(),
                        ..Default::default()
                    });
                }
                _ => {}
            }
            other
        }
    };
    for child in &node.children {
        walk(child, cpu, gpu, scratch, kind);
    }
    if !node.sensor_type.is_empty() && !node.value.is_empty() {
        apply_leaf(node, kind, cpu, gpu, scratch);
    }
}

fn apply_leaf(
    node: &Node,
    kind: HardwareKind,
    cpu: &mut CpuSensors,
    gpu: &mut GpuSensors,
    scratch: &mut Scratch,
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
            HardwareKind::Storage => {
                // Skip threshold sensors ("Warning Temperature",
                // "Critical Temperature"). Prefer "Composite" when present.
                if name_l.contains("warning") || name_l.contains("critical") {
                    return;
                }
                if let Some(s) = scratch.storages.last_mut() {
                    let prefer_composite = name_l.contains("composite");
                    if s.temp_c.is_none() || prefer_composite {
                        s.temp_c = Some(value);
                    }
                }
            }
            HardwareKind::MemoryDimm => {
                // Skip the static sensor metadata exposed by LHM
                // ("Temperature Sensor Resolution", "Thermal Sensor *Limit").
                if name_l.contains("limit")
                    || name_l.contains("resolution")
                    || name_l.contains("threshold")
                {
                    return;
                }
                if let Some(d) = scratch.dimms.last_mut() {
                    d.temp_c = Some(value);
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
            HardwareKind::Storage | HardwareKind::MemoryDimm | HardwareKind::Unknown => {}
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
                    scratch.gpu_partial_power += value;
                    scratch.gpu_partial_seen = true;
                }
            }
            _ => {}
        },
        "Control"
            if matches!(kind, HardwareKind::Gpu)
                && name_l.contains("fan")
                && gpu.fan_pct.is_none() =>
        {
            // GPU fan duty cycle (0–100). LHM exposes this as a Control leaf.
            gpu.fan_pct = Some(value);
        }
        "Load" if matches!(kind, HardwareKind::Gpu) => {
            // Primary GPU activity. LHM exposes one "GPU Core" Load sensor
            // per GPU, plus per-engine D3D nodes (D3D 3D, D3D Video Codec,
            // D3D Video Decode, D3D Compute, ...).
            if name_l == "gpu core" || name_l == "core" || name_l == "gpu" {
                gpu.util_pct = Some(value);
            } else if name_l.contains("video codec")
                || name_l.contains("video encode")
                || name_l.contains("encoder")
            {
                let v = gpu.encoder_util_pct.unwrap_or(0.0).max(value);
                gpu.encoder_util_pct = Some(v);
            } else if name_l.contains("video decode") || name_l.contains("decoder") {
                let v = gpu.decoder_util_pct.unwrap_or(0.0).max(value);
                gpu.decoder_util_pct = Some(v);
            }
        }
        "Clock" if matches!(kind, HardwareKind::Gpu) => {
            // Pick the headline rails. Skip "(Effective)" variants since
            // they are smoothed versions of the same domain.
            if name_l.contains("effective") {
                return;
            }
            if name_l == "gpu core" || name_l == "core" || name_l.contains("graphics") {
                gpu.clock_graphics_mhz = Some(value as u32);
            } else if name_l.contains("memory") {
                gpu.clock_memory_mhz = Some(value as u32);
            } else if name_l.contains("video") || name_l.contains("encoder") {
                gpu.clock_video_mhz = Some(value as u32);
            }
        }
        "Data" | "SmallData" if matches!(kind, HardwareKind::Gpu) => {
            // LHM publishes VRAM as separate Used / Free sensors.
            if let Some(mb) = parse_data_mb(&node.value) {
                if name_l.contains("used") {
                    scratch.gpu_mem_used_mb = Some(mb);
                } else if name_l.contains("free") {
                    scratch.gpu_mem_free_mb = Some(mb);
                } else if name_l.contains("total") {
                    gpu.mem_total_mb = Some(mb);
                }
            }
        }
        "Load" if matches!(kind, HardwareKind::Storage) => {
            // Skip "Read Activity"/"Write Activity"/"Total Activity" — they
            // are noisy and not useful in a once-per-second dashboard.
            if name_l == "used space"
                && let Some(s) = scratch.storages.last_mut()
            {
                s.used_pct = Some(value);
            }
        }
        "Level" if matches!(kind, HardwareKind::Storage) => {
            if let Some(s) = scratch.storages.last_mut() {
                if name_l == "life" {
                    s.life_left_pct = Some(value);
                } else if name_l == "percentage used" && s.life_left_pct.is_none() {
                    // SMART exposes Percentage Used; derive remaining life.
                    s.life_left_pct = Some((100.0 - value).clamp(0.0, 100.0));
                }
            }
        }
        "Data" | "SmallData" if matches!(kind, HardwareKind::Storage) => {
            if let Some(gb) = parse_data_gb(&node.value)
                && let Some(s) = scratch.storages.last_mut()
            {
                if name_l == "total space" {
                    s.total_gb = Some(gb);
                } else if name_l == "free space" {
                    s.free_gb = Some(gb);
                }
            }
        }
        "Data" | "SmallData" if matches!(kind, HardwareKind::MemoryDimm) => {
            if name_l == "capacity"
                && let Some(gb) = parse_data_gb(&node.value)
                && let Some(d) = scratch.dimms.last_mut()
            {
                d.capacity_gb = Some(gb);
            }
        }
        _ => {}
    }
}

/// LHM Data values are formatted like `"1907,0 MB"`, `"2,0 GB"`, etc.
fn parse_data_mb(s: &str) -> Option<u32> {
    let mut tokens = s.split_whitespace();
    let num: f64 = tokens.next()?.replace(',', ".").parse().ok()?;
    let unit = tokens.next().unwrap_or("MB").to_uppercase();
    let mb = match unit.as_str() {
        "B" => num / (1024.0 * 1024.0),
        "KB" => num / 1024.0,
        "MB" => num,
        "GB" => num * 1024.0,
        "TB" => num * 1024.0 * 1024.0,
        _ => num,
    };
    Some(mb.round().max(0.0) as u32)
}

fn parse_data_gb(s: &str) -> Option<f32> {
    let mut tokens = s.split_whitespace();
    let num: f64 = tokens.next()?.replace(',', ".").parse().ok()?;
    let unit = tokens.next().unwrap_or("GB").to_uppercase();
    let gb = match unit.as_str() {
        "B" => num / (1024.0 * 1024.0 * 1024.0),
        "KB" => num / (1024.0 * 1024.0),
        "MB" => num / 1024.0,
        "GB" => num,
        "TB" => num * 1024.0,
        _ => num,
    };
    Some((gb.max(0.0)) as f32)
}

fn apply_cpu_temperature(name_l: &str, value: f32, cpu: &mut CpuSensors) {
    // Intel: "CPU Package", "CPU Core #1..#N", "CPU Core Max/Average"
    // AMD:   "Core (Tctl/Tdie)" overall, "CCD #1 (Tdie).." per-chiplet
    let is_per_core_intel = name_l.contains("core #");
    let is_intel_package = name_l.contains("package");
    let is_amd_chiplet = name_l.contains("ccd");
    let is_amd_overall = (name_l.contains("tctl") || name_l.contains("tdie")) && !is_amd_chiplet;
    let is_aggregate =
        name_l.contains("max") || name_l.contains("average") || name_l.contains("avg");

    // Precedence note: the OR'd branch consumes the value for any of three
    // mutually-distinguishable sources. The first sub-condition (Intel package
    // or AMD overall) is the authoritative one and overwrites whatever's
    // there; the second/third are best-effort fallbacks that only fire if no
    // authoritative reading has landed yet. Clippy's identical-blocks lint
    // doesn't model the `cpu.package_temp_c.is_none()` short-circuit, so
    // collapsing into a single arm reads cleaner anyway.
    if is_per_core_intel {
        cpu.per_core_temp_c.push(value);
    } else if (is_intel_package || is_amd_overall)
        || (is_amd_chiplet && cpu.package_temp_c.is_none())
        || (!is_aggregate && cpu.package_temp_c.is_none() && name_l.contains("cpu"))
    {
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
