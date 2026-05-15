//! Periodic exporter for host hardware telemetry.
//!
//! Owns an `alvr_hwmonitor::Hwmonitor` sampler and POSTs a structured JSON
//! payload to the configured URL every interval. The payload groups telemetry
//! by resource (cpu / gpu / dram / dimms / storage / network / cpu_cores) so
//! the ingestion service can route each section into its own ClickHouse table,
//! keyed by `host`.

use alvr_common::parking_lot::{Condvar, Mutex};
use alvr_common::{info, warn};
use alvr_hwmonitor::{Hwmonitor, HwmonitorConfig, Snapshot};
use serde_json::{Value, json};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const WARN_RATE_LIMIT: Duration = Duration::from_secs(30);

pub struct HwExporterConfig {
    pub url: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub headers: Vec<(String, String)>,
    pub host: String,
}

struct Shutdown {
    flag: Mutex<bool>,
    cv: Condvar,
}

pub struct HwExporterHandle {
    shutdown: Arc<Shutdown>,
    thread: Option<JoinHandle<()>>,
}

impl HwExporterHandle {
    pub fn shutdown(mut self) {
        self.signal_shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn signal_shutdown(&self) {
        let mut guard = self.shutdown.flag.lock();
        *guard = true;
        self.shutdown.cv.notify_all();
    }
}

impl Drop for HwExporterHandle {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.signal_shutdown();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
}

pub fn spawn(config: HwExporterConfig) -> HwExporterHandle {
    let shutdown = Arc::new(Shutdown {
        flag: Mutex::new(false),
        cv: Condvar::new(),
    });
    let thread_shutdown = Arc::clone(&shutdown);
    let thread = thread::Builder::new()
        .name("hwmonitor_exporter".into())
        .spawn(move || run(config, thread_shutdown))
        .expect("failed to spawn hwmonitor_exporter thread");
    HwExporterHandle {
        shutdown,
        thread: Some(thread),
    }
}

fn run(config: HwExporterConfig, shutdown: Arc<Shutdown>) {
    info!(
        "hwmonitor_exporter: posting to {} every {} ms",
        config.url,
        config.interval.as_millis()
    );

    let monitor = Hwmonitor::spawn(HwmonitorConfig {
        interval: config.interval,
        ..HwmonitorConfig::default()
    });

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.timeout))
        .build()
        .into();

    let mut next_post = Instant::now() + config.interval;
    let mut last_warn = Instant::now()
        .checked_sub(WARN_RATE_LIMIT)
        .unwrap_or_else(Instant::now);

    loop {
        if !wait_until(&shutdown, next_post) {
            break;
        }
        next_post = Instant::now() + config.interval;

        let snapshot = monitor.latest();
        let payload = build_payload(&config.host, &snapshot);

        let mut req = agent.post(&config.url);
        for (k, v) in &config.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        if let Err(e) = req.send_json(&payload) {
            if last_warn.elapsed() >= WARN_RATE_LIMIT {
                warn!("hwmonitor_exporter: POST failed: {e}");
                last_warn = Instant::now();
            }
        }
    }

    info!("hwmonitor_exporter: stopped");
}

/// Waits until `deadline` or until shutdown is signalled. Returns `true` if
/// the deadline elapsed normally, `false` if shutdown was requested.
fn wait_until(shutdown: &Shutdown, deadline: Instant) -> bool {
    let mut guard = shutdown.flag.lock();
    while !*guard {
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        shutdown.cv.wait_for(&mut guard, deadline - now);
    }
    false
}

fn build_payload(host: &str, snap: &Snapshot) -> Value {
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let cpu = snap.cpu.as_ref().map(|c| {
        json!({
            "total_pct": c.total_pct,
            "freq_mhz": c.freq_mhz,
            "vrserver_pct": c.vrserver_pct,
            "package_temp_c": c.package_temp_c,
            "package_power_w": c.package_power_w,
            "cores_power_w": c.cores_power_w,
        })
    });

    let cpu_cores: Vec<Value> = snap
        .cpu
        .as_ref()
        .map(|c| {
            (0..c.per_core_pct.len())
                .map(|i| {
                    json!({
                        "index": i as u32,
                        "load_pct": c.per_core_pct.get(i).copied(),
                        "temp_c": c.per_core_temp_c.get(i).copied(),
                        "power_w": c.per_core_power_w.get(i).copied(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let gpu = snap.gpu.as_ref().map(|g| {
        json!({
            "name": g.name,
            "util_pct": g.util_pct,
            "encoder_util_pct": g.encoder_util_pct,
            "decoder_util_pct": g.decoder_util_pct,
            "mem_used_mb": g.mem_used_mb,
            "mem_total_mb": g.mem_total_mb,
            "temp_c": g.temp_c,
            "power_w": g.power_w,
            "power_limit_w": g.power_limit_w,
            "clock_graphics_mhz": g.clock_graphics_mhz,
            "clock_memory_mhz": g.clock_memory_mhz,
            "clock_video_mhz": g.clock_video_mhz,
            "fan_pct": g.fan_pct,
        })
    });

    let dram = snap.memory.as_ref().map(|m| {
        json!({
            "total_mb": m.total_mb,
            "used_mb": m.used_mb,
            "available_mb": m.available_mb,
            "used_pct": m.used_pct,
            "swap_total_mb": m.swap_total_mb,
            "swap_used_mb": m.swap_used_mb,
            "vrserver_working_set_mb": m.vrserver_working_set_mb,
        })
    });

    let dimms: Vec<Value> = snap
        .memory
        .as_ref()
        .map(|m| {
            m.dimms
                .iter()
                .map(|d| {
                    json!({
                        "slot": d.slot,
                        "capacity_gb": d.capacity_gb,
                        "temp_c": d.temp_c,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let storage: Vec<Value> = snap
        .storage
        .iter()
        .map(|s| {
            json!({
                "device": s.device,
                "temp_c": s.temp_c,
                "used_pct": s.used_pct,
                "life_left_pct": s.life_left_pct,
                "total_gb": s.total_gb,
                "free_gb": s.free_gb,
            })
        })
        .collect();

    let network: Vec<Value> = snap
        .network
        .iter()
        .map(|n| {
            json!({
                "adapter": n.adapter,
                "bytes_sent_per_sec": n.bytes_sent_per_sec,
                "bytes_recv_per_sec": n.bytes_recv_per_sec,
                "packets_sent_per_sec": n.packets_sent_per_sec,
                "packets_recv_per_sec": n.packets_recv_per_sec,
                "outbound_errors": n.outbound_errors,
                "outbound_discarded": n.outbound_discarded,
                "current_bandwidth_bps": n.current_bandwidth_bps,
            })
        })
        .collect();

    json!({
        "ts": ts,
        "host": host,
        "cpu": cpu,
        "cpu_cores": cpu_cores,
        "gpu": gpu,
        "dram": dram,
        "dimms": dimms,
        "storage": storage,
        "network": network,
    })
}
