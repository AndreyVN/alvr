//! nvidia-smi streaming subprocess.
//!
//! Spawns `nvidia-smi` once with `-l 1` and parses each CSV line into a fresh
//! `GpuSample`. The latest sample is held under a mutex; readers get a cheap
//! clone. When the binary is missing we never spawn and `latest()` returns
//! `None`.

use crate::GpuSample;
use alvr_common::{anyhow, debug, warn};
use parking_lot::Mutex;
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

const QUERY_FIELDS: &str = concat!(
    "name,",
    "utilization.gpu,",
    "utilization.encoder,",
    "utilization.decoder,",
    "memory.used,",
    "memory.total,",
    "temperature.gpu,",
    "power.draw,",
    "power.limit,",
    "clocks.gr,",
    "clocks.mem,",
    "clocks.video,",
    "clocks_throttle_reasons.active,",
    "pstate,",
    "fan.speed",
);

pub struct NvidiaSmiSource {
    latest: Arc<Mutex<Option<GpuSample>>>,
    child: Option<Child>,
    _reader: Option<JoinHandle<()>>,
}

impl NvidiaSmiSource {
    pub fn spawn() -> anyhow::Result<Self> {
        let mut child = Command::new("nvidia-smi")
            .args([
                "--query-gpu",
                QUERY_FIELDS,
                "--format=csv,noheader,nounits",
                "-l",
                "1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("nvidia-smi spawned without stdout"))?;

        let latest = Arc::new(Mutex::new(None));
        let reader = spawn_reader(stdout, Arc::clone(&latest));

        Ok(Self {
            latest,
            child: Some(child),
            _reader: Some(reader),
        })
    }

    pub fn latest(&self) -> Option<GpuSample> {
        self.latest.lock().clone()
    }
}

impl Drop for NvidiaSmiSource {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_reader(stdout: ChildStdout, latest: Arc<Mutex<Option<GpuSample>>>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("hwmonitor_nvsmi".into())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if let Some(sample) = parse_line(&line) {
                            *latest.lock() = Some(sample);
                        } else {
                            debug!("hwmonitor: nvidia-smi line did not parse: {line}");
                        }
                    }
                    Err(e) => {
                        warn!("hwmonitor: nvidia-smi stdout read failed: {e}");
                        break;
                    }
                }
            }
        })
        .expect("spawn hwmonitor_nvsmi reader")
}

fn parse_line(line: &str) -> Option<GpuSample> {
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() < 15 {
        return None;
    }
    Some(GpuSample {
        name: parse_string(parts[0]),
        util_pct: parse_f32(parts[1]),
        encoder_util_pct: parse_f32(parts[2]),
        decoder_util_pct: parse_f32(parts[3]),
        mem_used_mb: parse_u32(parts[4]),
        mem_total_mb: parse_u32(parts[5]),
        temp_c: parse_f32(parts[6]),
        power_w: parse_f32(parts[7]),
        power_limit_w: parse_f32(parts[8]),
        clock_graphics_mhz: parse_u32(parts[9]),
        clock_memory_mhz: parse_u32(parts[10]),
        clock_video_mhz: parse_u32(parts[11]),
        throttle_reasons: parse_throttle_mask(parts[12]),
        pstate: parse_string(parts[13]),
        fan_pct: parse_f32(parts[14]),
        ..Default::default()
    })
}

fn parse_string(s: &str) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("[N/A]") || s.eq_ignore_ascii_case("[Not Supported]")
    {
        None
    } else {
        Some(s.to_string())
    }
}

fn parse_f32(s: &str) -> Option<f32> {
    parse_string(s).and_then(|v| v.parse().ok())
}

fn parse_u32(s: &str) -> Option<u32> {
    parse_string(s).and_then(|v| v.parse().ok())
}

fn parse_throttle_mask(s: &str) -> Option<String> {
    let raw = parse_string(s)?;
    let value = u64::from_str_radix(raw.trim_start_matches("0x"), 16).ok()?;
    if value == 0 {
        return Some("None".to_string());
    }
    const REASONS: &[(u64, &str)] = &[
        (0x0001, "GpuIdle"),
        (0x0002, "ApplicationsClocksSetting"),
        (0x0004, "SwPowerCap"),
        (0x0008, "HwSlowdown"),
        (0x0010, "SyncBoost"),
        (0x0020, "SwThermalSlowdown"),
        (0x0040, "HwThermalSlowdown"),
        (0x0080, "HwPowerBrakeSlowdown"),
        (0x0100, "DisplayClockSetting"),
    ];
    let mut names = Vec::new();
    for (bit, name) in REASONS {
        if value & bit != 0 {
            names.push(*name);
        }
    }
    if names.is_empty() {
        Some(format!("0x{value:X}"))
    } else {
        Some(names.join(","))
    }
}
