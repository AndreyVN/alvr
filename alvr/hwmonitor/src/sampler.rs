//! Top-level sampler: owns one background thread that polls every source on a
//! fixed cadence and caches the latest `Snapshot` for cheap reads.

use crate::lhm::{LhmSource, DEFAULT_URL as LHM_DEFAULT_URL};
use crate::nvidia_smi::NvidiaSmiSource;
use crate::sysinfo_source::SysinfoSource;
use crate::{NamedValue, Snapshot};
use alvr_common::{info, warn};
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(windows)]
use crate::network::NetSource;

#[derive(Clone, Debug)]
pub struct HwmonitorConfig {
    pub interval: Duration,
    /// When false, never spawn nvidia-smi even if the binary is present.
    pub enable_nvidia_smi: bool,
    /// When false, never attempt to contact LibreHardwareMonitor.
    pub enable_lhm: bool,
    /// URL of the LHM web server (`Options → Run web server` inside LHM).
    pub lhm_url: String,
    /// When false, network adapter counters are not collected.
    pub enable_network: bool,
}

impl Default for HwmonitorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            enable_nvidia_smi: true,
            enable_lhm: true,
            lhm_url: LHM_DEFAULT_URL.to_string(),
            enable_network: true,
        }
    }
}

pub struct Hwmonitor {
    state: Arc<State>,
    thread: Option<JoinHandle<()>>,
}

struct State {
    latest: Mutex<Snapshot>,
    shutdown: Mutex<bool>,
    cv: Condvar,
}

impl Hwmonitor {
    pub fn spawn(config: HwmonitorConfig) -> Self {
        let state = Arc::new(State {
            latest: Mutex::new(Snapshot::default()),
            shutdown: Mutex::new(false),
            cv: Condvar::new(),
        });
        let thread_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("hwmonitor_main".into())
            .spawn(move || run(thread_state, config))
            .expect("spawn hwmonitor_main");
        Self {
            state,
            thread: Some(thread),
        }
    }

    pub fn latest(&self) -> Snapshot {
        self.state.latest.lock().clone()
    }
}

impl Drop for Hwmonitor {
    fn drop(&mut self) {
        {
            let mut s = self.state.shutdown.lock();
            *s = true;
            self.state.cv.notify_all();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(state: Arc<State>, config: HwmonitorConfig) {
    info!(
        "hwmonitor: starting (interval = {} ms, lhm = {}, nvidia-smi = {}, network = {})",
        config.interval.as_millis(),
        config.enable_lhm,
        config.enable_nvidia_smi,
        config.enable_network,
    );

    let mut sysinfo = SysinfoSource::new();

    let nvidia = if config.enable_nvidia_smi {
        match NvidiaSmiSource::spawn() {
            Ok(s) => {
                info!("hwmonitor: nvidia-smi subprocess started");
                Some(s)
            }
            Err(e) => {
                info!("hwmonitor: nvidia-smi unavailable ({e}); NVIDIA fields omitted");
                None
            }
        }
    } else {
        None
    };

    let lhm = if config.enable_lhm {
        match LhmSource::connect(&config.lhm_url) {
            Ok(s) => {
                info!("hwmonitor: LHM web server connected at {}", config.lhm_url);
                Some(s)
            }
            Err(e) => {
                info!(
                    "hwmonitor: LHM unreachable at {} ({e}); start LibreHardwareMonitor as Administrator and enable Options → Run web server",
                    config.lhm_url
                );
                None
            }
        }
    } else {
        None
    };

    #[cfg(windows)]
    let net = init_net_source(&config);

    let mut warned_lhm = false;
    #[cfg(windows)]
    let mut warned_net = false;

    loop {
        sysinfo.refresh();

        let mut cpu = sysinfo.cpu();
        let mut gpu = nvidia.as_ref().and_then(|n| n.latest());
        let memory = Some(sysinfo.memory());
        #[allow(unused_mut)]
        let mut network = Vec::new();

        if let Some(l) = lhm.as_ref() {
            match l.read() {
                Ok((cpu_sensors, gpu_sensors)) => {
                    cpu.package_temp_c = cpu_sensors.package_temp_c;
                    cpu.per_core_temp_c = cpu_sensors.per_core_temp_c;
                    cpu.package_power_w = cpu_sensors.package_power_w;
                    cpu.cores_power_w = cpu_sensors.cores_power_w;
                    cpu.per_core_power_w = cpu_sensors.per_core_power_w;
                    cpu.fans_rpm = named_values(cpu_sensors.fans_rpm);
                    let g = gpu.get_or_insert_with(Default::default);
                    g.temp_c = g.temp_c.or(gpu_sensors.temp_c);
                    g.power_w = g.power_w.or(gpu_sensors.power_w);
                    g.fan_pct = g.fan_pct.or(gpu_sensors.fan_pct);
                    g.fans_rpm = named_values(gpu_sensors.fans_rpm);
                }
                Err(e) => {
                    if !warned_lhm {
                        warn!("hwmonitor: LHM read failed ({e})");
                        warned_lhm = true;
                    }
                }
            }
        }

        #[cfg(windows)]
        if let Some(n) = net.as_ref() {
            match n.read() {
                Ok(v) => network = v,
                Err(e) => {
                    if !warned_net {
                        warn!("hwmonitor: network counter read failed: {e}");
                        warned_net = true;
                    }
                }
            }
        }

        *state.latest.lock() = Snapshot {
            cpu: Some(cpu),
            gpu,
            memory,
            network,
        };

        if !wait_or_shutdown(&state, config.interval) {
            break;
        }
    }

    info!("hwmonitor: stopped");
}

fn wait_or_shutdown(state: &State, interval: Duration) -> bool {
    let mut guard = state.shutdown.lock();
    if *guard {
        return false;
    }
    state.cv.wait_for(&mut guard, interval);
    !*guard
}

fn named_values(items: Vec<(String, u32)>) -> Vec<NamedValue<u32>> {
    items
        .into_iter()
        .map(|(name, value)| NamedValue { name, value })
        .collect()
}

#[cfg(windows)]
fn init_net_source(config: &HwmonitorConfig) -> Option<NetSource> {
    if !config.enable_network {
        return None;
    }
    use wmi::COMLibrary;
    let com = match COMLibrary::new() {
        Ok(c) => c,
        Err(e) => {
            warn!("hwmonitor: COM init failed ({e}); network counters disabled");
            return None;
        }
    };
    match NetSource::connect(com) {
        Ok(s) => Some(s),
        Err(e) => {
            warn!("hwmonitor: network WMI connect failed: {e}");
            None
        }
    }
}
