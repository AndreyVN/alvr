# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ALVR streams VR games from a PC to a standalone headset over Wi-Fi. The PC side ("streamer") encodes frames from SteamVR and ships them to a headset app ("client"); the client decodes, displays, and ships back tracking/input/audio. Everything is one Rust workspace (`Cargo.toml`) split into ~23 crates under `alvr/`. The OpenVR driver glue and a few low-level GPU/codec pieces are C++ under `alvr/server_openvr/cpp`; everything else is Rust. The `openvr` directory is a git submodule of Valve's SDK — clone with `--recurse-submodules` (or `git submodule update --init`). The Android client is built with `cargo-apk` from `alvr/client_openxr` and links against the C-ABI `alvr_client_core` library.

This fork also ships a host-side telemetry stack on top of upstream ALVR: an `alvr_hwmonitor` crate samples CPU/GPU/RAM/storage/network on the PC; `alvr_server_core::metrics_exporter` aggregates streaming statistics, headset & controller battery, and an Android `ClientTelemetry` payload over each export window; and `alvr_server_core::hwmonitor_exporter` POSTs the hardware snapshot in parallel. The dashboard exposes both via a top-level `HWMonitor` tab and a dedicated `Metrics` settings tab; a reference ingest (ClickHouse + Grafana) lives under `metrics/`.

## Technical context

- **Rust backend** (workspace edition 2024, MSRV 1.92): server core, client core, dashboard, launcher, all shared crates. Tokio is used inside `server_core` for the embedded web server and async sockets, but most hot paths are plain `std::thread` + `parking_lot` locks + `mpsc` channels.
- **C++ on the PC side** (`alvr/server_openvr/cpp`): the SteamVR `vrserver` driver entry point and OpenVR shims, bridged to Rust via `bindgen`/`cc`.
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
```

Important flags: `--gpl` bundles FFmpeg on Windows (always on for Linux); `--no-nvidia` strips NVENC support on Linux `prepare-deps`; `--ci` enables CI tweaks — most importantly it skips the elevated `choco install` step in Windows `prepare-deps`, so use it when the host already has zip/unzip/llvm/vulkan-sdk/pkgconfiglite/cmake on PATH (no UAC prompt that way); `--keep-config` preserves `session.json` between rebuilds. Artifacts land in `./build/` (not `./target/`); native deps live in `./deps/` and are managed by `prepare-deps` — never modify them by hand.

For the exact list of host-side tools, versions, and env vars that make these commands work on Windows, see `install.txt` at the repo root (toolchain audit from one fully-working setup). It covers VS Build Tools components (incl. the ATL workload that Intel VPL needs), Chocolatey + its packages, the Android NDK r26b + full SDK layout (`ANDROID_HOME` with `platforms;android-32` + `build-tools;32.0.0` — `cargo-apk` requires both, not just the NDK), and the gotchas hit while bootstrapping.

## Core verification & run commands

These are the day-to-day "did I break it" commands. Note the crate-name mapping: this fork has no single `alvr_server` crate — the SteamVR driver split into `alvr_server_core` (platform-agnostic streamer brain) and `alvr_server_openvr` (the cdylib SteamVR loads).

```
cargo check -p alvr_server_core              # type-check the streamer brain
cargo check -p alvr_server_openvr            # type-check the OpenVR driver cdylib (needs prepare-deps first)
cargo check -p alvr_dashboard                # type-check the dashboard GUI
cargo test  --workspace                      # run the (small) test suite — CI only requires -p alvr_session
cargo run   -p alvr_dashboard                # launch the dashboard standalone (use `cargo xtask run-streamer` for the full streamer)
cargo xtask clippy --ci                      # the CI lint gate
cargo xtask check-format                     # rustfmt + clang-format
```

For full-system runs always prefer `cargo xtask run-streamer` over `cargo run` — it stages the build into `./build/` where SteamVR expects to find the driver.

## Code rules (enforce, do not relax)

1. **Standard workflows are mandatory.** All builds, packaging, formatting, and lint go through `cargo xtask`. Do not invent ad-hoc build scripts, shell helpers, or `cargo build` invocations that bypass the staging logic in `alvr/xtask/src/build.rs`. If you need a new workflow, add an xtask subcommand.
2. **Error handling uses `alvr_common::log`.** Logging is funneled through the `log` crate re-exported from `alvr_common` (`use alvr_common::{info, warn, error, debug};`). Do not pull in another logging crate, do not `println!` from library code, and do not `eprintln!` for errors. Errors bubble as `anyhow::Result` (also re-exported via `alvr_common::anyhow`) — `panic!`/`unwrap`/`expect` are discouraged and any unavoidable `unwrap` or raw indexing needs a `// # Safety` comment.
3. **`deps/` is owned by `cargo xtask prepare-deps`.** Never edit, patch, or commit anything under `deps/`. If a dependency needs a patch, add it under `alvr/xtask/patches/` and apply it from the xtask code path so the change is reproducible.
4. **Before refactoring or creating new data structures, always read ARCHITECTURE.md to maintain system alignment.** That file is the project map — module boundaries, the streamer↔client data flows, and the named runtime threads. A "small" change to a packet struct, a session field, or a thread ownership boundary frequently has cross-crate consequences (e.g. `alvr_packets` wire compat, `alvr_session` schema, C-ABI in `*_core/src/c_api.rs`). Read it first; update it when you change those boundaries.
5. **Wire and config compatibility is load-bearing.** Changes to `alvr_packets` (wire types) or `alvr_session` (settings schema) cross the streamer/client version boundary. Add a migration in `alvr_session` rather than silently changing field semantics.
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
