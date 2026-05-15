use alvr_common::{info, warn};
use alvr_events::{BitrateDirectives, GraphStatistics};
use flume::{Receiver, RecvTimeoutError, Sender};
use serde_json::{Map, Value, json};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SAMPLE_CHANNEL_CAPACITY: usize = 4096;
const WARN_RATE_LIMIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub enum Sample {
    Frame {
        stats: GraphStatistics,
        video_packets_total: u64,
        video_bytes_total: u64,
    },
    Battery {
        hmd_pct: u32,
        hmd_plugged: bool,
    },
    Bitrate(BitrateDirectives),
}

pub struct ExporterConfig {
    pub url: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub headers: Vec<(String, String)>,
    pub host: String,
}

#[derive(Default)]
struct Acc {
    sum: f64,
    min: f32,
    max: f32,
    n: u32,
}

impl Acc {
    fn push(&mut self, v: f32) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            if v < self.min {
                self.min = v;
            }
            if v > self.max {
                self.max = v;
            }
        }
        self.sum += v as f64;
        self.n += 1;
    }

    fn to_json(&self) -> Option<Value> {
        if self.n == 0 {
            None
        } else {
            Some(json!({
                "min": self.min,
                "max": self.max,
                "avg": (self.sum / self.n as f64) as f32,
                "n": self.n,
            }))
        }
    }
}

#[derive(Default)]
struct Aggregator {
    // Per-frame latency distributions (seconds → flushed as milliseconds).
    total_pipeline: Acc,
    game_time: Acc,
    server_compositor: Acc,
    encoder: Acc,
    network: Acc,
    decoder: Acc,
    decoder_queue: Acc,
    client_compositor: Acc,
    vsync_queue: Acc,
    // FPS distributions
    client_fps: Acc,
    server_fps: Acc,
    // Throughput distributions
    throughput_bps: Acc,
    bitrate_bps: Acc,

    frames: u32,
    dropped_samples: u64,
    failed_posts: u64,

    // Last-value state (carried across windows).
    last_battery_hmd_pct: Option<u32>,
    last_battery_hmd_plugged: bool,
    last_bitrate_directives: BitrateDirectives,

    // Cumulative counters: end-of-window values from latest Frame sample.
    video_packets_total: u64,
    video_bytes_total: u64,
    // Counters at start of window — used to derive per-window rates.
    window_start_packets: u64,
    window_start_bytes: u64,
}

impl Aggregator {
    fn push(&mut self, sample: Sample) {
        match sample {
            Sample::Frame {
                stats,
                video_packets_total,
                video_bytes_total,
            } => {
                self.total_pipeline.push(stats.total_pipeline_latency_s);
                self.game_time.push(stats.game_time_s);
                self.server_compositor.push(stats.server_compositor_s);
                self.encoder.push(stats.encoder_s);
                self.network.push(stats.network_s);
                self.decoder.push(stats.decoder_s);
                self.decoder_queue.push(stats.decoder_queue_s);
                self.client_compositor.push(stats.client_compositor_s);
                self.vsync_queue.push(stats.vsync_queue_s);
                self.client_fps.push(stats.client_fps);
                self.server_fps.push(stats.server_fps);
                self.throughput_bps.push(stats.throughput_bps);
                self.bitrate_bps.push(stats.bitrate_bps);
                self.last_bitrate_directives = stats.bitrate_directives;
                self.video_packets_total = video_packets_total;
                self.video_bytes_total = video_bytes_total;
                self.frames += 1;
            }
            Sample::Battery {
                hmd_pct,
                hmd_plugged,
            } => {
                self.last_battery_hmd_pct = Some(hmd_pct);
                self.last_battery_hmd_plugged = hmd_plugged;
            }
            Sample::Bitrate(directives) => {
                self.last_bitrate_directives = directives;
            }
        }
    }

    fn flush(&mut self, window: Duration, host: &str) -> Value {
        let window_secs = window.as_secs_f64().max(f64::EPSILON);
        let packets_in_window = self
            .video_packets_total
            .saturating_sub(self.window_start_packets);
        let bytes_in_window = self
            .video_bytes_total
            .saturating_sub(self.window_start_bytes);

        let latency_ms = {
            let mut m = Map::new();
            for (name, acc) in [
                ("total_pipeline", &self.total_pipeline),
                ("game_time", &self.game_time),
                ("server_compositor", &self.server_compositor),
                ("encoder", &self.encoder),
                ("network", &self.network),
                ("decoder", &self.decoder),
                ("decoder_queue", &self.decoder_queue),
                ("client_compositor", &self.client_compositor),
                ("vsync_queue", &self.vsync_queue),
            ] {
                if let Some(stat) = scale_ms(acc) {
                    m.insert(name.into(), stat);
                }
            }
            Value::Object(m)
        };

        let fps = json!({
            "client": self.client_fps.to_json(),
            "server": self.server_fps.to_json(),
        });

        let throughput = json!({
            "throughput_bps": self.throughput_bps.to_json(),
            "bitrate_bps": self.bitrate_bps.to_json(),
            "video_packets_per_sec": packets_in_window as f64 / window_secs,
            "video_mbits_per_sec": (bytes_in_window as f64 * 8.0) / 1_000_000.0 / window_secs,
        });

        let totals = json!({
            "video_packets": self.video_packets_total,
            "video_mbytes": (self.video_bytes_total as f64 / 1_000_000.0) as u64,
        });

        let battery = self.last_battery_hmd_pct.map(|pct| {
            json!({
                "hmd_pct": pct,
                "hmd_plugged": self.last_battery_hmd_plugged,
            })
        });

        let snapshot = json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "host": host,
            "window_ms": window.as_millis() as u64,
            "frames": self.frames,
            "dropped_samples": self.dropped_samples,
            "latency_ms": latency_ms,
            "fps": fps,
            "throughput": throughput,
            "totals": totals,
            "battery": battery,
            "bitrate_directives": self.last_bitrate_directives,
            "exporter": { "failed_posts": self.failed_posts },
        });

        // Reset per-window accumulators; keep last-value state.
        self.total_pipeline = Acc::default();
        self.game_time = Acc::default();
        self.server_compositor = Acc::default();
        self.encoder = Acc::default();
        self.network = Acc::default();
        self.decoder = Acc::default();
        self.decoder_queue = Acc::default();
        self.client_compositor = Acc::default();
        self.vsync_queue = Acc::default();
        self.client_fps = Acc::default();
        self.server_fps = Acc::default();
        self.throughput_bps = Acc::default();
        self.bitrate_bps = Acc::default();
        self.frames = 0;
        self.dropped_samples = 0;
        self.failed_posts = 0;
        self.window_start_packets = self.video_packets_total;
        self.window_start_bytes = self.video_bytes_total;

        snapshot
    }
}

fn scale_ms(acc: &Acc) -> Option<Value> {
    if acc.n == 0 {
        None
    } else {
        Some(json!({
            "min": acc.min * 1000.0,
            "max": acc.max * 1000.0,
            "avg": ((acc.sum / acc.n as f64) * 1000.0) as f32,
            "n": acc.n,
        }))
    }
}

pub fn channel() -> (Sender<Sample>, Receiver<Sample>) {
    flume::bounded(SAMPLE_CHANNEL_CAPACITY)
}

/// Non-blocking push: drops the sample if the channel is full. Returns `true` on success.
pub fn try_push(sender: &Sender<Sample>, sample: Sample) -> bool {
    sender.try_send(sample).is_ok()
}

pub fn spawn_exporter_thread(receiver: Receiver<Sample>, config: ExporterConfig) -> JoinHandle<()> {
    thread::Builder::new()
        .name("metrics_exporter".into())
        .spawn(move || exporter_loop(receiver, config))
        .expect("failed to spawn metrics_exporter thread")
}

fn exporter_loop(receiver: Receiver<Sample>, config: ExporterConfig) {
    info!(
        "metrics_exporter: posting to {} every {} ms",
        config.url,
        config.interval.as_millis()
    );

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.timeout))
        .build()
        .into();

    let mut agg = Aggregator::default();
    let mut window_start = Instant::now();
    let mut next_flush = window_start + config.interval;
    let mut last_warn = Instant::now()
        .checked_sub(WARN_RATE_LIMIT)
        .unwrap_or_else(Instant::now);

    loop {
        // Drain samples until the flush deadline, then post.
        loop {
            let now = Instant::now();
            if now >= next_flush {
                break;
            }
            match receiver.recv_timeout(next_flush - now) {
                Ok(sample) => agg.push(sample),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    info!("metrics_exporter: channel closed, exiting");
                    return;
                }
            }
        }

        // Account for any samples dropped at the producer side. We can't see them directly,
        // but a backlog of len() == capacity implies producers may be try_send-failing.
        let window = Instant::now().saturating_duration_since(window_start);
        let snapshot = agg.flush(window, &config.host);
        window_start = Instant::now();
        next_flush = window_start + config.interval;

        let mut req = agent.post(&config.url);
        for (k, v) in &config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        match req.send_json(&snapshot) {
            Ok(_) => {}
            Err(e) => {
                agg.failed_posts = agg.failed_posts.saturating_add(1);
                if last_warn.elapsed() >= WARN_RATE_LIMIT {
                    warn!("metrics_exporter: POST failed: {e}");
                    last_warn = Instant::now();
                }
            }
        }
    }
}
