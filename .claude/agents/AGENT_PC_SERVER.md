# AGENT_PC_SERVER

Scope: **PC streamer.** Everything that runs on the host: the SteamVR OpenVR driver, the streamer brain, the dashboard, the launcher, and the Linux-only Vulkan layer / vrcompositor wrapper. This is the side that ingests SteamVR submitted frames, encodes them with NVENC/AMF/VPL (or FFmpeg on Linux), and serves the dashboard's data.

## Crates owned by this agent

| Path | Crate | Responsibility |
| --- | --- | --- |
| `alvr/server_core` | `alvr_server_core` (lib) | The platform-agnostic streamer brain. Owns the tokio runtime, the connection state machine (`connection.rs`), the tracking/input/haptics pipelines, `BitrateManager`, `StatisticsManager`, the embedded web server backing the dashboard, and the optional telemetry exporters (`metrics_exporter.rs`, `hwmonitor_exporter.rs`). Exposes a C ABI (`c_api.rs`) so Monado and other hosts can drive it. |
| `alvr/server_openvr` | `alvr_server_openvr` (cdylib) | The actual file SteamVR loads. C++ glue in `cpp/` bridges OpenVR's `vrserver` ABI to Rust; the Rust side translates events to `server_core`. Built via `bindgen` + `cc`. The `gpl` feature enables FFmpeg on Windows (always on on Linux). |
| `alvr/server_io` | `alvr_server_io` | `session.json` IO and `ServerSessionManager`. The single writer for persistent server-side config. |
| `alvr/dashboard` | `alvr_dashboard` | Standalone `eframe`/egui GUI. Compiles both natively and to `wasm32` (`data_sources` vs `data_sources_wasm`). Talks to `server_core`'s embedded web server. Native build adds a `HWMonitor` tab driven by `alvr_hwmonitor`; settings ship a top-level `Metrics` tab covering metrics export and extended headset telemetry. |
| `alvr/hwmonitor` | `alvr_hwmonitor` | Host hardware telemetry sampler. Background thread queries `sysinfo`, the LibreHardwareMonitor JSON web server (preferred over WMI), `nvidia-smi`, and Win32 WMI for adapter counters. Exposes a `Snapshot` consumed by the dashboard's `HWMonitor` tab and by `server_core::hwmonitor_exporter`. PC-side only. |
| `alvr/launcher` | `alvr_launcher` | Separate `eframe` app that downloads/installs ALVR releases and handles ADB-based client install. |
| `alvr/vulkan_layer` | `alvr_vulkan_layer` | Linux Vulkan layer that intercepts SteamVR/`vrcompositor` GPU work. (The user spec calls this `alvr/vulkan-layer`; the actual crate is `alvr_vulkan_layer`.) |
| `alvr/vrcompositor_wrapper` | `alvr_vrcompositor_wrapper` | Linux shim that wraps SteamVR's `vrcompositor-launcher` to inject the Vulkan layer. |

## Coding rules

- All builds go through `cargo xtask`. Don't `cargo build` the streamer directly — it won't be staged into `./build/` where SteamVR loads it from. Use `cargo xtask build-streamer` or `cargo xtask run-streamer`.
- Logging is `alvr_common::{info, warn, error, debug}`. The streamer's logging backend lives in `alvr_server_core::logging_backend`. Don't bypass it — the dashboard pipes those records over the event bus.
- Threads in `server_core::connection` are named in code via comments — when adding a new persistent thread, follow the existing naming and add it to the `connection_threads` join list so shutdown is clean.
- `OpenvrConfig` (`alvr_session`) is the only sanctioned way to push config into the OpenVR driver — see `connection::contruct_openvr_config`. Adding a new property means updating the schema, the constructor, and the C++ side.
- GPU encoder selection (NVENC / AMF / VPL / FFmpeg software) is decided based on `Settings.video.encoder_config` plus runtime capability detection. Changes here cross the Rust/C++ boundary in `server_openvr`.
- `--gpl` only enables FFmpeg on Windows; AMD/Intel paths use OS-vendored APIs (AMF / VPL) and are not gated by it.
- The two exporter threads (`metrics_exporter`, `hwmonitor_exporter`) are spawned by `connection_pipeline` only when `Settings.metrics.metrics_export` is `Switch::Enabled`; `hwmonitor_exporter` is additionally gated on a non-empty `hw_url`. Producers (`statistics_thread`, `control_receive_thread`) must use the non-blocking `try_push` helper — never block the hot path waiting on the export channel. The wire shape of the POSTed JSON is paired with `metrics/clickhouse_schema.sql`; changes must land in lockstep.

## Running and testing

```
# Type/lint the streamer
cargo check  -p alvr_server_core
cargo check  -p alvr_server_openvr           # requires `cargo xtask prepare-deps --platform <host>` first
cargo check  -p alvr_dashboard
cargo check  -p alvr_launcher
cargo check  -p alvr_hwmonitor
cargo clippy -p alvr_server_core -- -D warnings
cargo clippy -p alvr_server_openvr -- -D warnings

# Tests (this scope has no dedicated unit tests; rely on the workspace gate)
cargo test --workspace

# Build + run end-to-end
cargo xtask prepare-deps --platform windows   # or `linux` / `macos`
cargo xtask build-streamer [--release] [--gpl]
cargo xtask run-streamer                       # builds (unless --no-rebuild) and launches the dashboard
cargo run -p alvr_dashboard                   # dashboard standalone — no driver, useful for UI work
cargo run -p alvr_launcher

# Linux extras
cargo check -p alvr_vulkan_layer
cargo check -p alvr_vrcompositor_wrapper
```

## Where to look when something is broken

PC log directories:

- **Windows**: `%APPDATA%\\ALVR-Launcher\\<install>\\session_log.txt` and `%TEMP%\\vrserver.txt` (SteamVR's own log of the driver). The dashboard surfaces streamer logs live in the "Logs" tab.
- **Linux**: `~/.config/alvr/session_log.txt` and `~/.steam/steam/logs/vrserver.txt`. With the Vulkan layer, also check `~/.local/share/Steam/logs/` for shader compile errors.
- Driver load failures show up in `vrserver.txt` *before* `session_log.txt` exists — if you see "driver not found" check that `cargo xtask build-streamer` populated `build/alvr_streamer_<platform>/`.

When the dashboard can't reach the streamer, check that port `9943/tcp` (control) is reachable and that `SESSION_MANAGER` initialized (`initialize_environment` runs before any read of the session). When SteamVR loads the driver but no frames arrive, suspect the encoder picker — toggle codec in the dashboard and watch `vrserver.txt`.

## Before touching this scope

Read `ARCHITECTURE.md` for the "Video Pipeline" section — it names the per-stream threads (`video_send_thread`, `tracking_receive_thread`, `statistics_thread`, ...) so you can locate them in `alvr/server_core/src/connection.rs`.
