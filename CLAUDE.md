# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ALVR streams VR games from a PC to a standalone headset over Wi-Fi. The PC side ("streamer") encodes frames from SteamVR and ships them to a headset app ("client"); the client decodes, displays, and ships back tracking/input/audio. Everything is one Rust workspace (`Cargo.toml`) split into ~24 crates under `alvr/`. The OpenVR driver glue and a few low-level GPU/codec pieces are C++ under `alvr/server_openvr/cpp`; everything else is Rust. Two directories are git submodules: `openvr/` (Valve's SDK) and `openxr/` (an ALVR fork of Monado at `github.com/AndreyVN/monado@alvr`, branched at `v25.1.0` — see `docs/monado-notes/SUBMODULE_PIN.md`). Clone with `--recurse-submodules` or run `git submodule update --init --recursive` after clone. The Android client is built with `cargo-apk` from `alvr/client_openxr` and links against the C-ABI `alvr_client_core` library.

**PC runtime mode** (`Settings.extra.runtime`, default `Steamvr`): the streamer can run as a SteamVR driver (`alvr_server_openvr` cdylib loaded by `vrserver`) or as a Monado-based OpenXR runtime (`alvr_server_openxr` cdylib loaded by the in-fork Monado driver at `openxr/src/xrt/drivers/alvr/`). The OpenXR mode is preview-only and gated behind `cargo xtask build-openxr-runtime --enable-alvr-driver`; the SteamVR mode is the default and always builds. The two runtimes share `alvr_server_core` (state machine, sockets, embedded web server, statistics). See `docs/monado-notes/NEXT_STEPS.md` and `docs/openxr-migration.md` for the migration plan.

This fork also ships a host-side telemetry stack on top of upstream ALVR: an `alvr_hwmonitor` crate samples CPU/GPU/RAM/storage/network on the PC; `alvr_server_core::metrics_exporter` aggregates streaming statistics, headset & controller battery, and an Android `ClientTelemetry` payload over each export window; and `alvr_server_core::hwmonitor_exporter` POSTs the hardware snapshot in parallel. When OpenXR mode is active the exporter additionally emits `oxr_pacing` (per-frame compositor timing) and `oxr_layer_types` (histogram of dropped non-projection layers) sections, both forwarded from `comp_alvr` via the bridge ABI; these are omitted from OpenVR-mode snapshots so the existing wire shape stays stable. The dashboard exposes everything via a top-level `HWMonitor` tab and a dedicated `Metrics` settings tab; a reference ingest (ClickHouse + Grafana) lives under `metrics/`.

## Technical context

- **Rust backend** (workspace edition 2024, MSRV 1.92): server core, client core, dashboard, launcher, all shared crates. Tokio is used inside `server_core` for the embedded web server and async sockets, but most hot paths are plain `std::thread` + `parking_lot` locks + `mpsc` channels.
- **C++ on the PC side** (`alvr/server_openvr/cpp`): the SteamVR `vrserver` driver entry point and OpenVR shims, bridged to Rust via `bindgen`/`cc`. Encoder backends live under `cpp/encoder/` — Linux `EncodePipeline*` under `encoder/linux/`, Windows D3D11 backends under `encoder/win32_d3d11/`, and a Vulkan-input skeleton for OpenXR mode under `encoder/win32_vk/` that `alvr_server_openxr` compiles via its own `cc::Build`.
- **OpenXR mode bridge** (`alvr/server_openxr`): a cdylib that exports a small C ABI (`include/alvr_runtime_bridge.h`, regenerated via cbindgen) consumed by the Monado-side ALVR driver inside `openxr/`. Mirrors what `alvr_server_openvr` does for SteamVR, but for Monado. **ABI is versioned** (`ALVR_OXR_BRIDGE_ABI_VERSION`, currently v10); Monado checks it at driver init and refuses to load on mismatch. Surface includes init/shutdown, head + per-controller pose, view params, haptics, session events (`alvr_oxr_poll_session_event` — bridge emits `StateChange` / `ConnectionLost` and `alvr_hub.c` dispatches them as `xrt_session_event_state_change` to OpenXR clients), pacing telemetry (`alvr_oxr_report_pacing`, v2), layer-type telemetry (`alvr_oxr_report_layer_types`, v3 — diagnostic histogram of dropped non-projection layers), hand-tracking (`alvr_oxr_get_hand_skeleton`, v4 — 26-joint `XR_EXT_hand_tracking` source feeding the per-side `alvr_hand` xrt_device), per-view foveation (`alvr_oxr_get_foveation` / `alvr_oxr_set_foveation`, v5 — gaze-driven `AlvrOxrFoveationView[2]` cache the encoder reads on the hot path; producer wired via `ServerCoreEvent::PerViewFoveation` from `PerViewFoveationEmitter` in `tracking_loop`), and `alvr_oxr_submit_layers` (Vulkan-input CUDA-interop NVENC body landed v6, plus `image_sizes[2]` + `display_time_ns`; v7 `alvr_oxr_get_view_resolution` so the HMD/compositor advertise at the negotiated streaming resolution; v8 `alvr_oxr_get_foveation_vars` driving the server-side FFR compress pass in `comp_alvr`; v9–v10 the squash→encoder timeline-semaphore handoff — `alvr_oxr_submit_layers` gains the forward + reverse "consumed" semaphore handles + a CUDA handle-type so the encoder waits/signals GPU-side, and the NVENC `EncodeFrame` now runs on a worker thread off Monado's compositor thread, headset-verified on RTX 3090 + Quest 3). The same per-view params also travel server→client as `alvr_packets::FoveationView` (see rule 5) and are consumed by a dormant `FFE_RUNTIME` de-foveation pipeline in `alvr_graphics::stream` — a second pipeline that derives the warp from a per-view `center_shift` uniform, used only when a frame carries per-view foveation (i.e. once the hardware-gated NVENC body actually produces a per-view-foveated image; the static path is byte-identical otherwise). The launcher (`alvr/launcher/`) surfaces `RuntimeMode` per-installation: per-row badge plus a ComboBox in the Edit popup that writes back to `session.json` via `serde_json::Value` manipulation (Windows-only). Out-of-tree Monado iteration: pass `cargo xtask build-openxr-runtime --monado-source <path>` (or set `ALVR_MONADO_SOURCE_DIR=<path>`) to build from a fork clone instead of the in-tree submodule (avoids a submodule bump for each iteration cycle). The flag takes precedence over the env var; both fall back to the `openxr/` submodule.
- **Android client** (`alvr/client_openxr` cdylib): an OpenXR app packaged via `cargo-apk` and `cargo xtask build-client`. Decoding uses Android `MediaCodec` (called from `alvr_client_core::video_decoder::android`); rendering uses Vulkan via OpenXR swapchains and `alvr_graphics` (wgpu).
- **Build system**: every workflow goes through `cargo xtask` (alias in `.cargo/config.toml` → `cargo run -p alvr_xtask --`). There is no Makefile, npm script, or shell wrapper. CI calls the same xtask subcommands; they're the source of truth. See `alvr/xtask/src/main.rs`.

## Build system: cargo xtask

```
cargo xtask prepare-deps --platform <windows|linux|macos|android>   # one-time: populate ./deps with FFmpeg/OpenXR loader/etc.
cargo xtask build-streamer [--release] [--gpl] [--profiling] [--keep-config]
cargo xtask build-client [--release]                                # Android APK via cargo-apk
cargo xtask build-client-lib [--release] [--all-targets]            # C-ABI client lib for third-party engines
cargo xtask run-streamer [--no-rebuild]                             # build then launch the dashboard
cargo xtask package-streamer / package-launcher / package-client    # distribution profile + archives
cargo xtask format / check-format
cargo xtask clippy [--ci]                                           # curated restriction + pedantic lint set
cargo xtask clean                                                   # nukes ./build, ./deps, and ./target
cargo xtask bump --version <X.Y.Z> [--nightly]
cargo xtask kill-oculus                                             # Windows only: kills OVR* processes before debugging
cargo xtask build-openxr-runtime [--release] [--enable-alvr-driver] # PC OpenXR mode: drives CMake on openxr/. --enable-alvr-driver
                                                                    # also turns on XRT_FEATURE_COMP_ALVR.
cargo xtask register-openxr-runtime / unregister-openxr-runtime     # Activate/deactivate the built ALVR-Monado runtime per-user.
                                                                    # Windows: HKCU registry. Linux: XDG file. Refuses to clobber
                                                                    # a different vendor's manifest on unregister.
```

Important flags: `--gpl` bundles FFmpeg on Windows (always on for Linux); `--no-nvidia` strips NVENC support on Linux `prepare-deps`; `--ci` enables CI tweaks — most importantly it skips the elevated `choco install` step in Windows `prepare-deps`, so use it when the host already has zip/unzip/llvm/vulkan-sdk/pkgconfiglite/cmake on PATH (no UAC prompt that way); `--keep-config` preserves `session.json` between rebuilds. Artifacts land in `./build/` (not `./target/`); native deps live in `./deps/` and are managed by `prepare-deps` — never modify them by hand.

For the exact list of host-side tools, versions, and env vars that make these commands work on Windows, see `install.txt` at the repo root (toolchain audit from one fully-working setup). It covers VS Build Tools components (incl. the ATL workload that Intel VPL needs), Chocolatey + its packages, the Android NDK r26b + full SDK layout (`ANDROID_HOME` with `platforms;android-32` + `build-tools;32.0.0` — `cargo-apk` requires both, not just the NDK), and the gotchas hit while bootstrapping.

## Core verification & run commands

These are the day-to-day "did I break it" commands. Note the crate-name mapping: this fork has no single `alvr_server` crate — the SteamVR driver split into `alvr_server_core` (platform-agnostic streamer brain) and `alvr_server_openvr` (the cdylib SteamVR loads). The OpenXR-mode counterpart is `alvr_server_openxr`.

```
cargo check -p alvr_server_core              # type-check the streamer brain
cargo check -p alvr_server_openvr            # type-check the OpenVR driver cdylib (needs prepare-deps first)
cargo check -p alvr_server_openxr            # type-check the OpenXR-mode bridge cdylib (Monado-loaded; Vulkan SDK ideal)
cargo check -p alvr_dashboard                # type-check the dashboard GUI
cargo test  --workspace                      # run the (small) test suite — CI only requires -p alvr_session
cargo test  -p alvr_launcher                 # 8 round-trip tests for session.json runtime-mode read/write (Windows-only)
cargo test  -p alvr_server_core metrics_exporter  # 8 aggregator tests covering OxrPacing/OxrLayerTypes/Battery paths
cargo run   -p alvr_dashboard                # launch the dashboard standalone (use `cargo xtask run-streamer` for the full streamer)
cargo xtask clippy --ci                      # the CI lint gate
cargo xtask check-format                     # rustfmt + clang-format
```

For full-system runs always prefer `cargo xtask run-streamer` over `cargo run` — it stages the build into `./build/` where SteamVR expects to find the driver.

## Code rules (enforce, do not relax)

1. **Standard workflows are mandatory.** All builds, packaging, formatting, and lint go through `cargo xtask`. Do not invent ad-hoc build scripts, shell helpers, or `cargo build` invocations that bypass the staging logic in `alvr/xtask/src/build.rs`. If you need a new workflow, add an xtask subcommand.
2. **Error handling uses `alvr_common::log`.** Logging is funneled through the `log` crate re-exported from `alvr_common` (`use alvr_common::{info, warn, error, debug};`). Do not pull in another logging crate, do not `println!` from library code, and do not `eprintln!` for errors. Errors bubble as `anyhow::Result` (also re-exported via `alvr_common::anyhow`) — `panic!`/`unwrap`/`expect` are discouraged and any unavoidable `unwrap` or raw indexing needs a `// # Safety` comment.
3. **`deps/` is owned by `cargo xtask prepare-deps`.** Never edit, patch, or commit anything under `deps/`. If a dependency needs a patch, add it under `alvr/xtask/patches/` and apply it from the xtask code path so the change is reproducible.
4. **Before refactoring or creating new data structures, always read `docs/ARCHITECTURE.md` to maintain system alignment.** That file is the project map — module boundaries, the streamer↔client data flows, and the named runtime threads. A "small" change to a packet struct, a session field, or a thread ownership boundary frequently has cross-crate consequences (e.g. `alvr_packets` wire compat, `alvr_session` schema, C-ABI in `*_core/src/c_api.rs`). Read it first; update it when you change those boundaries.
5. **Wire and config compatibility is load-bearing.** Changes to `alvr_packets` (wire types) or `alvr_session` (settings schema) cross the streamer/client version boundary. Add a migration in `alvr_session` rather than silently changing field semantics. Per-view (eye-tracked) foveation adds the wire type `alvr_packets::FoveationView { center_size[2], center_shift[2], edge_ratio[2] }`, carried server→client on two channels: `RealTimeConfig.per_view_foveation: Option<[FoveationView; 2]>` (low-rate baseline, 1 Hz diff-on-change) and `VideoPacketHeader.per_view_foveation: Option<[FoveationView; 2]>` (per-frame; a deliberate lockstep wire break — `protocol_id` gates only the major version, so client and server must be built from the same revision). Both default `None`; only populated when the `per_view_eye_tracked` switch is enabled.
6. **C-ABI surfaces are product surface, not internal.** `c_api.rs` in `alvr_server_core`, `alvr_client_core`, `alvr_client_openxr`, and `alvr_common` is consumed by external integrators; `cbindgen` regenerates headers at build time. Don't change signatures without considering downstream consumers.

## Toolchain pins

- Rust edition 2024, **MSRV 1.92** (enforced by `cargo xtask check-msrv`).
- Windows: forces `-C target-feature=+crt-static` (see `.cargo/config.toml`).
- Android: NDK r26b, `aarch64-linux-android`, min SDK 28 / target SDK 32, requires `cargo-apk`.
- `settings-schema` is pinned to a specific git rev — bumping it can ripple through every settings field.

## Specialized agents

When a task is clearly scoped to one subsystem, prefer the matching agent in `.claude/agents/`:

- `AGENT_CORE_NET.md` — protocol, serialization, sockets, settings schema (`alvr_common`, `alvr_sockets`, `alvr_session`, `alvr_packets`, `alvr_events`).
- `AGENT_PC_SERVER.md` — streamer/SteamVR driver, NVENC/AMF/VPL encoders, dashboard, launcher, Vulkan layer.
- `AGENT_ANDROID_CLIENT.md` — headset app: OpenXR session, MediaCodec decode, tracking input, Vulkan compositor.

## Conventions (from CONTRIBUTING.md)

- Naming: respect Rust conventions; prefer `maybe_` prefix for `Option`/`Result` locals (never on fields/params); shadowing encouraged; suffix paths with `_dir`/`_path`/`_fname` when both kinds appear in the same scope.
- File ordering: private imports → public imports → ffi imports → constants → structs (`Default` then custom impl then `Drop` in one module) → private fns → public fns.
- Use `.get()` over `[]`. Encode invalid states out of existence with enums rather than parallel booleans. Extract a constant for any "arbitrary" literal and prefer `Duration`/`Path` over raw types.
- Comments only when meaning isn't obvious from the names. Don't restate language behavior.
