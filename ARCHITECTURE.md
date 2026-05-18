# ARCHITECTURE.md

Project map for ALVR. Read this before refactoring or introducing new data structures — the module boundaries, packet contracts, and named threads documented here are load-bearing.

ALVR is two cooperating processes:

- A **PC streamer** that runs as a SteamVR driver: ingests submitted frames + tracking from SteamVR, encodes video, and ships everything to the headset over Wi-Fi.
- A **headset client** (Android OpenXR app, plus a desktop mock and a C-ABI library) that decodes video, presents it through OpenXR, and ships tracking/input/audio back.

The two halves share a single Rust workspace (`Cargo.toml`, edition 2024, MSRV 1.92, ~23 crates under `alvr/`).

## Module overview

### Shared crates (used by both sides)

| Crate | What it is |
| --- | --- |
| `alvr_common` | Re-exports (`anyhow`, `glam`, `log`, `parking_lot`, `semver`, `settings_schema`), shared primitives (`Pose`, `Fov`, `DeviceMotion`, device-ID constants), `ConnectionState`/`LifecycleState` enums, the logging frontend, and a C ABI for integrators. Every other crate depends on it. |
| `alvr_session` | The settings schema, derived via `settings-schema-rs` (pinned git rev). `SessionConfig` / `Settings` is the single source of truth for everything user-configurable; field changes propagate to the dashboard UI and to `OpenvrConfig`. |
| `alvr_packets` | Every wire type exchanged between streamer and client: control packets (`ClientControlPacket`, `ServerControlPacket`), `TrackingData`, `VideoPacketHeader`, `StreamConfigPacket`, `ClientStatistics`, `Haptics`, `BatteryInfo`, `ClientTelemetry`, etc. Also the numeric stream IDs (`TRACKING`, `VIDEO`, `AUDIO`, `HAPTICS`, `STATISTICS`). |
| `alvr_sockets` | Control socket (TCP, port 9943) and stream socket (UDP/TCP with throttling). Constants: `CONTROL_PORT`, `KEEPALIVE_INTERVAL`, `KEEPALIVE_TIMEOUT`, mDNS service type `_alvr._tcp.local.`. |
| `alvr_events` | Structured event types the streamer emits for the dashboard / web UI. |
| `alvr_audio` | Cross-platform audio capture/playback (`oboe-rs` on Android, OS APIs elsewhere). |
| `alvr_graphics` | Shared wgpu pipeline: color correction, foveated rendering, lobby room rendering. |
| `alvr_filesystem` | Path layout. Centralizes `target_dir`, `workspace_dir`, `crate_dir`, `deps_dir`, `streamer_build_dir`, etc. Don't compute paths inline. |
| `alvr_system_info` | OS / platform / headset detection. |
| `alvr_adb` | ADB client used by the launcher and by the wired-streaming path. |
| `alvr_gui_common` | Shared egui widgets between the dashboard and the launcher. |
| `alvr_hwmonitor` | Host hardware telemetry sampler (PC-side only). Background thread that queries `sysinfo`, the LibreHardwareMonitor JSON web server, `nvidia-smi`, and Win32 WMI for adapter counters, and exposes the result as a `Snapshot` (`CpuSample`/`GpuSample`/`MemorySample`/`StorageSample`/`NetSample`/`DimmSample`). Used by the dashboard's `HWMonitor` tab and by `alvr_server_core::hwmonitor_exporter`. |

### Streamer (PC)

| Crate | Type | What it is |
| --- | --- | --- |
| `alvr_server_core` | lib | The platform-agnostic streamer brain. Owns the tokio runtime, the connection state machine (`connection.rs`), tracking/input/haptics pipelines, `BitrateManager`, `StatisticsManager`, the embedded web server backing the dashboard, and a C ABI (`c_api.rs`) for non-OpenVR hosts (Monado). Initializes a global `FILESYSTEM_LAYOUT: OnceLock` and `SESSION_MANAGER: LazyLock<RwLock<...>>` — call `initialize_environment` before any read. |
| `alvr_server_openvr` | cdylib | The file SteamVR actually loads. C++ shim in `cpp/` bridges OpenVR's `vrserver` ABI to Rust via `bindgen` + `cc`. The `gpl` feature enables FFmpeg on Windows (always on on Linux). |
| `alvr_server_io` | lib | `session.json` IO and `ServerSessionManager`. |
| `alvr_dashboard` | bin | Standalone `eframe`/egui GUI. Compiles both natively and to `wasm32` (`data_sources` vs `data_sources_wasm`). |
| `alvr_launcher` | bin | Separate `eframe` app that downloads/installs releases and drives ADB-based client install. |
| `alvr_vulkan_layer` | lib | Linux Vulkan layer that intercepts `vrcompositor` GPU work. |
| `alvr_vrcompositor_wrapper` | bin | Linux shim wrapping SteamVR's `vrcompositor-launcher` to inject the Vulkan layer. |

### Client (headset)

| Crate | Type | What it is |
| --- | --- | --- |
| `alvr_client_core` | lib (C-ABI) | Platform-agnostic client brain: connection state machine, sockets, statistics, persistent storage, video decoder lifecycle. The Android-specific MediaCodec wrapper lives at `src/video_decoder/android.rs` and is `#[cfg(target_os = "android")]`. |
| `alvr_client_openxr` | cdylib (APK) | The OpenXR app shipped to headsets. Owns the OpenXR session, swapchains, the lobby room, streaming overlay, passthrough, per-vendor extension wrappers (`extra_extensions/` for Meta / Pico / BD / etc.). Built via `cargo-apk` from `cargo xtask build-client`. |
| `alvr_client_mock` | bin | Desktop test harness that talks to a streamer without a real headset. |

### Tooling

| Crate | What it is |
| --- | --- |
| `alvr_xtask` | The build system. Every workflow (`prepare-deps`, `build-streamer`, `build-client`, `package-*`, `format`, `clippy`, `bump`, `check-msrv`, `check-licenses`, `clean`, `kill-oculus`) goes through here. Source of truth — CI calls the same subcommands. |

## Core data flows

The two diagrams below are the loops everything else is in service of. The thread/function names are real and grep-able.

### Tracking flow (Client → Server → SteamVR driver)

Headset-side input rides on the `TRACKING` stream from `alvr_packets`, plus control on the TCP control socket.

1. **`Client OpenXR session`** (`alvr/client_openxr/src/stream.rs`): each frame, the OpenXR runtime is queried for view poses, controller poses, hand-tracking joints, eye/face tracking (when enabled per vendor extension), and button/axis state.
2. **Per-vendor extension fan-in** (`alvr/client_openxr/src/extra_extensions/*` and `interaction/*`): vendor-specific data (Meta body tracking, Pico face tracking, BD motion tracking, etc.) is normalized into the shared `TrackingData` shape from `alvr_packets`.
3. **`alvr_client_core` connection** (`alvr/client_core/src/connection.rs`): packs the `TrackingData` into a stream packet and pushes it on the `TRACKING` stream via `alvr_sockets`. Button events go on the control socket as `ClientControlPacket::Buttons`. Statistics ride on a separate `STATISTICS` stream.
4. **UDP stream socket** (`alvr_sockets::StreamSocketBuilder`): default protocol is UDP with throttling; TCP is also selectable via `Settings.connection.stream_protocol`. The control socket is always TCP on `CONTROL_PORT = 9943`. Discovery is mDNS over `_alvr._tcp.local.`.
5. **Server reception** (`alvr/server_core/src/connection.rs`, `tracking_receive_thread`): unpacks `TrackingData`, feeds it to `TrackingManager` (`alvr/server_core/src/tracking.rs`), which applies smoothing, body-tracking source selection, and hand-gesture interpretation (`alvr/server_core/src/hand_gestures.rs`). Button events go through `ButtonMappingManager` (`alvr/server_core/src/input_mapping.rs`).
6. **`ServerCoreEvent` → SteamVR**: poses and button states are emitted as `ServerCoreEvent` variants (`Battery`, `SetOpenvrProperty`, ...) and consumed by the C++ side in `alvr/server_openvr/cpp/`, which calls into the OpenVR driver host (`vrserver`). SteamVR sees ALVR as a standard tracked-device driver.

### Video pipeline (SteamVR → Server encoder → UDP → Client MediaCodec → OpenXR compositor)

This is the latency-critical path. The streamer never queues more than one frame; the bitrate manager closes the loop using client-reported statistics.

1. **SteamVR submits a frame** to the ALVR OpenVR driver (`alvr/server_openvr/cpp/`). The driver hands the GPU texture to the Rust encoder layer.
2. **Encoder** (`alvr_server_openvr`, vendor-specific paths in C++): NVENC on NVIDIA, AMF on AMD, VPL on Intel, FFmpeg software/H.264 on Linux (and on Windows when built with `--gpl`). Codec selection (H.264 / HEVC / AV1) and profile come from `Settings.video.encoder_config` and the active `CodecType`.
3. **Encoded NAL chunks** are passed back to Rust as a `VideoPacket { header: VideoPacketHeader, payload }` (see `alvr/server_core/src/connection.rs`).
4. **`video_send_thread`** (`alvr/server_core/src/connection.rs`): consumes `VideoPacket`s from the `mpsc` channel and pushes them onto the `VIDEO` stream via `alvr_sockets::StreamSender`. The same module's `tracking_receive_thread`, `statistics_thread`, `real_time_update_thread`, `keepalive_thread`, `control_receive_thread`, `stream_receive_thread`, and `lifecycle_check_thread` cover the rest of the per-connection threadset.
5. **UDP transport**: the `VIDEO` stream is fragmented to MTU-sized chunks by `alvr_sockets`; reassembly with sequence numbers happens on the client.
6. **Client receive** (`alvr/client_core/src/connection.rs`, `video_receive_thread`): reassembles NAL units from `VIDEO` and feeds them to `alvr_client_core::video_decoder`.
7. **MediaCodec decode** (`alvr/client_core/src/video_decoder/android.rs`, `dequeue_thread`): the input queue is fed from the receive thread; a dedicated `dequeue_thread` pops decoded frames as `OutputBuffer`s and signals the render thread. Hardware-surface output goes directly into an OpenXR swapchain image without a host-side copy.
8. **OpenXR compositor** (`alvr/client_openxr/src/stream.rs` + `alvr_graphics`): submits the decoded image to the OpenXR swapchain. Foveation, color correction, and lobby compositing are all wgpu compute/render passes from `alvr_graphics`. Reprojection / late-latching uses the latest pose available from the OpenXR runtime at submission time.
9. **Statistics feedback**: the client reports decode/present timing back on the `STATISTICS` stream as `ClientStatistics`. `alvr_server_core::statistics::StatisticsManager` consumes these and drives `BitrateManager`, closing the rate-control loop.

The audio path mirrors the video path on its own stream (`AUDIO`), and haptics flow server → client on `HAPTICS`.

### Telemetry & metrics export

A separate, optional pipeline collects out-of-band health signals (battery, headset thermals, host hardware) and ships them to an external TSDB. None of this is on the latency-critical path.

1. **Client `control_send_thread`** (`alvr/client_core/src/connection.rs`): every battery interval samples Android sensors via `alvr_system_info::android` — HMD battery (`BatteryManager`), controller batteries (`InputDevice.getBatteryState`, API 29+), battery temperature, `PowerManager` thermal status/headroom, `/proc/meminfo`, `/proc/self/status`, `/proc/stat`-derived CPU load, and Adreno KGSL GPU counters. The HMD `BatteryInfo` is always sent. The controller `BatteryInfo`s and the `ClientTelemetry` payload are sent only when `metrics.extended_headset_telemetry` is true; otherwise the client falls back to the legacy "HMD battery only" behavior.
2. **Wire**: `ClientControlPacket::Battery(BatteryInfo { device_id, gauge_value, is_plugged })` for each device (HMD plus, optionally, both controllers) and a single `ClientControlPacket::Telemetry(ClientTelemetry)` per interval. Both ride the control socket.
3. **Server `control_receive_thread`** demuxes them. `Battery` produces a `ServerCoreEvent::Battery` and a `metrics_exporter::Sample::Battery { slot, pct, plugged }` (where `slot` is `Hmd` / `ControllerLeft` / `ControllerRight`, derived from `device_id`). `Telemetry` becomes `metrics_exporter::Sample::ClientTelemetry`. The OpenVR driver side surfaces controller battery via `SetBattery` → `Prop_DeviceBatteryPercentage_Float`; the `DeviceProvidesBatteryStatusBool` advertisement on controllers is gated on `metrics.extended_headset_telemetry` (see `alvr/server_openvr/src/props.rs`).
4. **`metrics_exporter`** (PC, `alvr/server_core/src/metrics_exporter.rs`): a bounded `flume` channel feeds the `metrics_exporter` thread, which folds `Frame` samples into min/max/avg accumulators per latency/FPS/throughput dimension, carries the latest battery/telemetry/bitrate-directive values across windows, and POSTs a single JSON snapshot per `interval_ms` to `metrics.metrics_export.url`. Producers use non-blocking `try_push`; the channel drops on overflow rather than backpressuring the hot path.
5. **`hwmonitor_exporter`** (PC, `alvr/server_core/src/hwmonitor_exporter.rs`): in parallel, an `alvr_hwmonitor::Hwmonitor` sampler runs on its own thread (sources: `sysinfo`, the LibreHardwareMonitor JSON web server, `nvidia-smi`, Win32 WMI for network counters). On the same interval, `hwmonitor_exporter` POSTs a per-resource JSON payload to `metrics.metrics_export.hw_url`. The two endpoints can be served by the same ingest service or different ones — they're independently switchable. See `metrics/` at the repo root for the ClickHouse schema (`metrics/clickhouse_schema.sql`) and Grafana provisioning (`metrics/setup_grafana.py`) used by the reference setup.
6. **Dashboard `HWMonitor` tab** (`alvr/dashboard/src/dashboard/components/hwmonitor.rs`, native-only): polls the same `alvr_hwmonitor` snapshot locally inside the dashboard process for an at-a-glance live view of host hardware, independent of whether metrics export is enabled.

## Critical threads & classes

Names below are grep-able. They live in `alvr/server_core/src/connection.rs` and `alvr/client_core/src/connection.rs` unless noted.

### Server-side threads (per active client connection)

| Name | File | Role |
| --- | --- | --- |
| `connection_thread` | `alvr/server_core/src/lib.rs` | Top-level lifecycle thread spawned by `ServerCoreContext`. Drives connection setup/teardown and joins all per-connection threads. |
| `video_send_thread` | `connection.rs` | Pulls `VideoPacket`s off the encoder channel and ships them on the `VIDEO` stream. The hot path for latency. |
| `tracking_receive_thread` | `connection.rs` | Receives `TrackingData` on the `TRACKING` stream, hands it to `TrackingManager`. |
| `statistics_thread` | `connection.rs` | Receives `ClientStatistics`; drives `BitrateManager`. |
| `metrics_exporter` | `metrics_exporter.rs` | Optional. Aggregates per-frame stats, HMD + controller battery samples, and the client-reported `ClientTelemetry` into min/max/avg over a configurable window, then POSTs the snapshot to an external HTTP endpoint (Grafana / ClickHouse ingest). Spawned by `connection_pipeline` only when `metrics.metrics_export` is enabled. |
| `hwmonitor_exporter` | `hwmonitor_exporter.rs` | Optional. Owns an `alvr_hwmonitor::Hwmonitor` sampler and POSTs a per-resource JSON payload (cpu / gpu / dram / dimms / storage / network / cpu_cores) to a separate `hw_url` on the same cadence as the streaming-metrics exporter. Spawned alongside `metrics_exporter` whenever `metrics.metrics_export.hw_url` is non-empty. |
| `real_time_update_thread` | `connection.rs` | Periodically (`REAL_TIME_UPDATE_INTERVAL = 1s`) pushes `RealTimeConfig` updates to the client. |
| `keepalive_thread` | `connection.rs` | Sends keepalives at `KEEPALIVE_INTERVAL` over the control socket. |
| `control_receive_thread` | `connection.rs` | Demuxes `ClientControlPacket`s — buttons, statistics, view config changes, disconnect. |
| `stream_receive_thread` | `connection.rs` | Receive loop for stream sockets that aren't tracking/statistics (haptics ack, audio mic, ...). |
| `lifecycle_check_thread` | `connection.rs` | Watches the global `LifecycleState` and tears down the connection on `ShuttingDown`. |

### Server-side singletons

| Name | File | Role |
| --- | --- | --- |
| `SESSION_MANAGER` | `alvr/server_core/src/lib.rs` | `LazyLock<RwLock<ServerSessionManager>>` — the only writer of `session.json`. |
| `FILESYSTEM_LAYOUT` | `alvr/server_core/src/lib.rs` | `OnceLock<afs::Layout>` — set once by `initialize_environment`. Everything that needs a path reads from here. |
| `BitrateManager` | `alvr/server_core/src/bitrate.rs` | Closes the rate-control loop using client statistics → `DynamicEncoderParams` → encoder. |
| `StatisticsManager` | `alvr/server_core/src/statistics.rs` | Aggregates per-frame latency / decode time / loss for the dashboard and the bitrate manager. |
| `TrackingManager` | `alvr/server_core/src/tracking.rs` | Pose smoothing, body-tracking source mixing, hand → controller fallback. |
| `ButtonMappingManager` | `alvr/server_core/src/input_mapping.rs` | Maps client input bindings to OpenVR action sources. |
| `web_server` | `alvr/server_core/src/web_server.rs` | The embedded HTTP+WebSocket server the dashboard talks to. |

### Client-side threads

| Name | File | Role |
| --- | --- | --- |
| `connection_thread` | `alvr/client_core/src/lib.rs` | Top-level client lifecycle. Spawns and joins everything below. |
| `video_receive_thread` | `alvr/client_core/src/connection.rs` | Reassembles the `VIDEO` stream and feeds the decoder. |
| `haptics_receive_thread` | `alvr/client_core/src/connection.rs` | Drains the `HAPTICS` stream and forwards to the OpenXR haptic action. |
| `control_send_thread` / `control_receive_thread` | `alvr/client_core/src/connection.rs` | Bidirectional control plane: buttons, view config, keepalive. |
| `stream_receive_thread` | `alvr/client_core/src/connection.rs` | Generic receive loop for any non-video stream the client subscribes to. |
| `dequeue_thread` | `alvr/client_core/src/video_decoder/android.rs` | Pops decoded MediaCodec buffers and signals the render thread. |
| `render thread` | `alvr/client_openxr/src/stream.rs` | The OpenXR app's main loop: queries poses, composites the decoded image plus passthrough/lobby layers, submits frames. Not named in code as a `JoinHandle` — it *is* the main thread of the cdylib. Treat it as the third critical client thread alongside `video_receive_thread` and `dequeue_thread`. |

### Cross-cutting types worth knowing by name

- `ClientConnection` lives logically as the body of `connection::handshake_loop` / `connection::connection_pipeline` in `alvr/server_core/src/connection.rs` — it owns the per-connection `ConnectionContext`, the join handles for the threads above, and the `ProtoControlSocket` / `StreamSocketBuilder` instances from `alvr_sockets`.
- `ServerCoreEvent` (`alvr/server_core/src/lib.rs`) is the variant type the server-side connection emits to the C++ driver glue; new device properties, battery updates, and client connect/disconnect events all flow through here.
- `ClientCoreEvent` (`alvr/client_core/src/lib.rs`) is the corresponding type emitted by the client core to whichever frontend (cdylib OpenXR app, mock, or C-ABI consumer) is driving it.
- `VideoPacket` / `VideoPacketHeader` (`alvr_packets`) is the per-NAL unit shipped on the `VIDEO` stream. Don't change its layout without a wire-compat plan.
- `BatteryInfo` (`alvr_packets`) carries a per-device battery sample over the control socket; `device_id` discriminates HMD vs. left/right controller. `ClientTelemetry` (`alvr_packets`) bundles client-side resource utilization (battery temperature, thermal status/headroom, memory, CPU, GPU). Both are optional, gated by `metrics.extended_headset_telemetry`; their fields are `Option<_>` so a platform that can't read a sensor omits it rather than lying.

## Where to extend

- New **setting**: add the field in `alvr_session`, write a migration, surface it in the dashboard, and (if it affects the OpenVR driver) wire it into `OpenvrConfig` via `connection::contruct_openvr_config`.
- New **wire packet**: add the type in `alvr_packets`, assign a stream ID, add send/receive plumbing in both `server_core::connection` and `client_core::connection`. Bump compatibility expectations and document the migration.
- New **per-vendor extension** on the headset: add a module under `alvr/client_openxr/src/extra_extensions/`, gate it on extension availability, normalize its output into existing `TrackingData` fields (don't grow the wire format unless you have to).
- New **encoder backend**: extend the C++ encoder selection in `alvr/server_openvr/cpp/` and the `CodecType` / encoder config in `alvr_session`. Don't bypass the bitrate-manager feedback loop.
- New **telemetry signal**: if it's client-sourced, add an optional field to `ClientTelemetry` in `alvr_packets` (don't grow `ClientControlPacket` with a new variant unless it can't fit) and surface it in `metrics_exporter::Aggregator::flush`. If it's host-sourced, add it as a sampler in `alvr_hwmonitor` (`Snapshot` field) and expose it through `build_payload` in `hwmonitor_exporter`. Update `metrics/clickhouse_schema.sql` and `metrics/setup_grafana.py` in lockstep so the ingest stays compatible.
