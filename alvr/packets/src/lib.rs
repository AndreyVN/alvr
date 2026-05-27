use alvr_common::{
    BodySkeleton, ConnectionState, DeviceMotion, LogSeverity, Pose, ViewParams,
    anyhow::Result,
    glam::{Quat, UVec2, Vec2},
    semver::Version,
};
use alvr_session::{
    ClientsidePostProcessingConfig, CodecType, PassthroughMode, PerformanceLevel, SessionConfig,
    Settings,
};
use serde::{Deserialize, Serialize};
use serde_json as json;
use std::{
    collections::HashSet,
    fmt::{self, Debug},
    net::IpAddr,
    time::Duration,
};

pub const TRACKING: u16 = 0;
pub const HAPTICS: u16 = 1;
pub const AUDIO: u16 = 2;
pub const VIDEO: u16 = 3;
pub const STATISTICS: u16 = 4;

#[derive(Serialize, Deserialize, Clone)]
pub struct VideoStreamingCapabilitiesExt {
    // Nothing for now
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VideoStreamingCapabilities {
    pub default_view_resolution: UVec2,
    pub max_view_resolution: UVec2,
    pub refresh_rates: Vec<f32>,
    pub microphone_sample_rate: u32,
    pub foveated_encoding: bool,
    pub encoder_high_profile: bool,
    pub encoder_10_bits: bool,
    pub encoder_av1: bool,
    pub prefer_10bit: bool,
    pub preferred_encoding_gamma: f32,
    pub prefer_hdr: bool,
    pub ext_str: String,
}

impl VideoStreamingCapabilities {
    pub fn with_ext(self, ext: VideoStreamingCapabilitiesExt) -> Self {
        Self {
            ext_str: json::to_string(&ext).unwrap(),
            ..self
        }
    }

    pub fn ext(&self) -> Result<VideoStreamingCapabilitiesExt> {
        let _ext_json = json::from_str::<json::Value>(&self.ext_str)?;

        // decode values here

        Ok(VideoStreamingCapabilitiesExt {})
    }
}

#[derive(Serialize, Deserialize)]
pub struct ConnectionAcceptedInfo {
    pub client_protocol_id: u64,
    pub platform_string: String,
    pub server_ip: IpAddr,
    pub streaming_capabilities: Option<VideoStreamingCapabilities>,
}

#[derive(Serialize, Deserialize)]
pub enum ClientConnectionResult {
    ConnectionAccepted(Box<ConnectionAcceptedInfo>),
    ClientStandby,
}

#[derive(Serialize, Deserialize)]
pub struct NegotiatedStreamingConfigExt {
    // Nothing for now
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NegotiatedStreamingConfig {
    pub view_resolution: UVec2,
    pub refresh_rate_hint: f32,
    pub game_audio_sample_rate: u32,
    pub enable_foveated_encoding: bool,
    pub encoding_gamma: f32,
    pub enable_hdr: bool,
    pub wired: bool,
    pub ext_str: String,
}

impl NegotiatedStreamingConfig {
    pub fn with_ext(self, ext: NegotiatedStreamingConfigExt) -> Self {
        Self {
            ext_str: json::to_string(&ext).unwrap(),
            ..self
        }
    }

    pub fn ext(&self) -> Result<NegotiatedStreamingConfigExt> {
        let _ext_json = json::from_str::<json::Value>(&self.ext_str)?;

        // decode values here

        Ok(NegotiatedStreamingConfigExt {})
    }
}

#[derive(Serialize, Deserialize)]
pub struct StreamConfigPacket {
    pub session: String, // JSON session that allows for extrapolation
    pub negotiated: NegotiatedStreamingConfig,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StreamConfig {
    pub server_version: Version,
    pub settings: Settings,
    pub negotiated_config: NegotiatedStreamingConfig,
}

impl StreamConfigPacket {
    pub fn new(session: &SessionConfig, negotiated: NegotiatedStreamingConfig) -> Result<Self> {
        Ok(Self {
            session: json::to_string(session)?,
            negotiated,
        })
    }

    pub fn to_stream_config(self) -> Result<StreamConfig> {
        let mut session_config = SessionConfig::default();
        session_config.merge_from_json(&json::from_str(&self.session)?)?;
        let settings = session_config.to_settings();

        Ok(StreamConfig {
            server_version: session_config.server_version,
            settings,
            negotiated_config: self.negotiated,
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DecoderInitializationConfig {
    pub codec: CodecType,
    pub config_buffer: Vec<u8>, // e.g. SPS + PPS NALs
    pub ext_str: String,
}

#[derive(Serialize, Deserialize)]
pub enum ServerControlPacket {
    StartStream,
    DecoderConfig(DecoderInitializationConfig),
    Restarting,
    KeepAlive,
    RealTimeConfig(RealTimeConfig),
    Reserved(String),
    ReservedBuffer(Vec<u8>),
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BatteryInfo {
    pub device_id: u64,
    pub gauge_value: f32, // range [0, 1]
    pub is_plugged: bool,
}

/// Client-side resource utilization, sampled on the same cadence as `BatteryInfo`. Every field is
/// `Option` so a platform that can't read a given sensor simply omits it instead of lying with a
/// sentinel.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ClientTelemetry {
    /// Battery sensor temperature in degrees Celsius (Android `BatteryManager.EXTRA_TEMPERATURE`).
    /// On Quest the battery sits near the SoC/display drivers, so this tracks overall HMD warming.
    pub battery_temperature_c: Option<f32>,
    /// `PowerManager.getThermalHeadroom(0)`: 0.0..1.0+, 1.0 means imminent throttling.
    pub thermal_headroom: Option<f32>,
    /// `PowerManager.getCurrentThermalStatus()`: 0=NONE..6=SHUTDOWN.
    pub thermal_status: Option<i32>,
    /// Total RAM in kibibytes (`MemTotal` from /proc/meminfo).
    pub mem_total_kib: Option<u64>,
    /// Available RAM in kibibytes (`MemAvailable` from /proc/meminfo).
    pub mem_available_kib: Option<u64>,
    /// Client process resident set size in kibibytes (`VmRSS` from /proc/self/status).
    pub process_rss_kib: Option<u64>,
    /// System-wide CPU busy fraction in [0, 1] over the last sampling interval.
    pub cpu_total_pct: Option<f32>,
    /// Client-process CPU busy fraction in [0, 1] over the last sampling interval. Can exceed 1.0
    /// on multi-core devices because `utime+stime` aggregates all threads.
    pub cpu_process_pct: Option<f32>,
    /// GPU busy fraction in [0, 1] over the last sampling interval, read from KGSL `gpubusy`.
    /// Adreno only; `None` on other GPUs or when sysfs is restricted.
    pub gpu_busy_pct: Option<f32>,
    /// GPU current frequency in Hz from KGSL devfreq.
    pub gpu_freq_hz: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub enum ButtonValue {
    Binary(bool),
    Scalar(f32),
}

#[derive(Serialize, Deserialize)]
pub struct ButtonEntry {
    pub path_id: u64,
    pub value: ButtonValue,
}

#[derive(Serialize, Deserialize)]
pub enum ClientControlPacket {
    PlayspaceSync(Option<Vec2>),
    RequestIdr,
    KeepAlive,
    StreamReady, // This flag notifies the server the client streaming socket is ready listening
    LocalViewParams([ViewParams; 2]), // In relation to head
    Battery(BatteryInfo),
    Buttons(Vec<ButtonEntry>),
    ActiveInteractionProfile {
        device_id: u64,
        profile_id: u64,
        input_ids: HashSet<u64>,
    },
    Log {
        level: LogSeverity,
        message: String,
    },
    ProximityState(bool),
    Telemetry(ClientTelemetry),
    Reserved(String),
    ReservedBuffer(Vec<u8>),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum FaceExpressions {
    Fb(Vec<f32>), // 70 values
    Bd(Vec<f32>), // 52 values
    Htc {
        eye: Option<Vec<f32>>, // 14 values
        lip: Option<Vec<f32>>, // 37 values
    },
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct FaceData {
    // Can be used for foveated eye tracking
    pub eyes_combined: Option<Quat>,
    // Should be used only for social presence
    pub eyes_social: [Option<Quat>; 2],

    pub face_expressions: Option<FaceExpressions>,
}

#[derive(Serialize, Deserialize)]
pub struct TrackingData {
    pub poll_timestamp: Duration,
    pub device_motions: Vec<(u64, DeviceMotion)>,
    pub hand_skeletons: [Option<[Pose; 26]>; 2],
    pub face: FaceData,
    pub body: Option<BodySkeleton>,
    pub markers: Vec<(String, Pose)>,
}

#[derive(Serialize, Deserialize)]
pub struct VideoPacketHeader {
    pub timestamp: Duration,
    pub global_view_params: [ViewParams; 2],
    pub is_idr: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Haptics {
    pub device_id: u64,
    pub duration: Duration,
    pub frequency: f32,
    pub amplitude: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub enum PathSegment {
    Name(String),
    Index(usize),
}

impl Debug for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSegment::Name(name) => write!(f, "{name}"),
            PathSegment::Index(index) => write!(f, "[{index}]"),
        }
    }
}

impl From<&str> for PathSegment {
    fn from(value: &str) -> Self {
        PathSegment::Name(value.to_owned())
    }
}

impl From<String> for PathSegment {
    fn from(value: String) -> Self {
        PathSegment::Name(value)
    }
}

impl From<usize> for PathSegment {
    fn from(value: usize) -> Self {
        PathSegment::Index(value)
    }
}

// todo: support indices
pub fn parse_path(path: &str) -> Vec<PathSegment> {
    path.split('.').map(|s| s.into()).collect()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ClientConnectionsAction {
    AddIfMissing {
        trusted: bool,
        manual_ips: Vec<IpAddr>,
    },
    SetDisplayName(String),
    Trust,
    SetManualIps(Vec<IpAddr>),
    RemoveEntry,
    UpdateCurrentIp(Option<IpAddr>),
    SetConnectionState(ConnectionState),
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ClientStatistics {
    pub target_timestamp: Duration, // identifies the frame
    pub frame_interval: Duration,
    pub video_decode: Duration,
    pub video_decoder_queue: Duration,
    pub rendering: Duration,
    pub vsync_queue: Duration,
    pub total_pipeline_latency: Duration,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PathValuePair {
    pub path: Vec<PathSegment>,
    pub value: json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum FirewallRulesAction {
    Add,
    Remove,
}

/// Per-view foveation parameters carried over the wire in [`RealTimeConfig`].
/// Per-axis shape mirrors `alvr_session::FoveatedEncodingConfig` and
/// `alvr_server_core::PerViewFoveationView`; lengths are in image-space [0, 1]
/// with the centre at (0.5, 0.5) when shifts are zero. Index 0 = left eye,
/// index 1 = right eye. This is the server→client half of the per-view
/// foveation design: the OpenXR-mode encoder applies these params per eye and
/// the client uses them to invert the warp during reprojection.
#[derive(Serialize, Deserialize, PartialEq, Clone, Copy, Debug)]
pub struct FoveationView {
    pub center_size: [f32; 2],
    pub center_shift: [f32; 2],
    pub edge_ratio: [f32; 2],
}

// Note: server sends a packet to the client at low frequency, binary encoding, without ensuring
// compatibility between different versions, even if within the same major version.
#[derive(Serialize, Deserialize, PartialEq, Clone)]
pub struct RealTimeConfig {
    pub passthrough: Option<PassthroughMode>,
    pub clientside_post_processing: Option<ClientsidePostProcessingConfig>,
    pub cpu_performance_level: Option<PerformanceLevel>,
    pub gpu_performance_level: Option<PerformanceLevel>,
    // Phase 7 per-view foveation. `Some` only when foveated encoding is on AND the eye-tracked
    // per-view switch is enabled; carries the static baseline params (both eyes identical) the
    // client should reproject against. `None` keeps the legacy single-source-of-truth path
    // (foveation params travel once via `OpenvrConfig` at stream init). Because `RealTimeConfig`
    // is built from settings and only re-sent on change, this is a low-rate baseline, not the
    // per-frame eye-tracked centre — that stays server-internal (encoder bridge) for now.
    pub per_view_foveation: Option<[FoveationView; 2]>,
    pub ext_str: String,
}

impl RealTimeConfig {
    pub fn from_settings(settings: &Settings) -> Self {
        let per_view_foveation = settings
            .video
            .foveated_encoding
            .as_option()
            .filter(|config| config.per_view_eye_tracked.enabled())
            .map(|config| {
                let view = FoveationView {
                    center_size: [config.center_size_x, config.center_size_y],
                    center_shift: [config.center_shift_x, config.center_shift_y],
                    edge_ratio: [config.edge_ratio_x, config.edge_ratio_y],
                };
                [view, view]
            });

        Self {
            passthrough: settings.video.passthrough.clone().into_option(),
            clientside_post_processing: settings
                .video
                .clientside_post_processing
                .clone()
                .into_option(),
            cpu_performance_level: settings.headset.performance_level.clone().cpu.into_option(),
            gpu_performance_level: settings.headset.performance_level.clone().gpu.into_option(),
            per_view_foveation,
            ext_str: String::new(), // No extensions for now
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvr_session::SessionConfig;
    use bincode::config;

    fn roundtrip(config: &RealTimeConfig) -> RealTimeConfig {
        let bytes = bincode::serde::encode_to_vec(config, config::standard()).unwrap();
        let (decoded, _) =
            bincode::serde::decode_from_slice::<RealTimeConfig, _>(&bytes, config::standard())
                .unwrap();
        decoded
    }

    /// The per-view foveation field bincode-round-trips in both states. This is
    /// the cross-version coordination point flagged in
    /// `docs/monado-notes/PER_VIEW_FOVEATION.md` (Slice 4): `RealTimeConfig` is
    /// explicitly NOT version-compatible (see the comment on the struct), so the
    /// only contract worth pinning is that the wire encoding is self-consistent.
    #[test]
    fn real_time_config_per_view_foveation_roundtrips() {
        let base = RealTimeConfig::from_settings(&SessionConfig::default().to_settings());

        let none = RealTimeConfig {
            per_view_foveation: None,
            ..base.clone()
        };
        assert!(roundtrip(&none) == none, "None variant must round-trip");

        let view = FoveationView {
            center_size: [0.45, 0.4],
            center_shift: [0.1, -0.2],
            edge_ratio: [4.0, 5.0],
        };
        let some = RealTimeConfig {
            per_view_foveation: Some([view, view]),
            ..base
        };
        assert!(roundtrip(&some) == some, "Some variant must round-trip");
    }

    /// Default settings have `per_view_eye_tracked` Disabled, so the field is
    /// omitted (legacy single-source-of-truth path stays untouched).
    #[test]
    fn from_settings_omits_per_view_foveation_by_default() {
        let config = RealTimeConfig::from_settings(&SessionConfig::default().to_settings());
        assert!(config.per_view_foveation.is_none());
    }
}
