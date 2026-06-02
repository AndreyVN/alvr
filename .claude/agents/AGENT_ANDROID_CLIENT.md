# AGENT_ANDROID_CLIENT

Scope: **VR headset application.** Everything that runs on a Quest / Pico / Vive Focus / Apple Vision Pro / generic OpenXR headset: the OpenXR session, the MediaCodec video decoder, tracking/input ingestion, the Vulkan compositor and lobby, and the audio paths.

The user spec mentions `alvr/client` and `alvr/vr_compositor`; those names don't exist in this fork. The actual layout is the two crates below plus shared infrastructure from `alvr_graphics` and `alvr_audio`.

## Crates owned by this agent

| Path | Crate | Responsibility |
| --- | --- | --- |
| `alvr/client_core` | `alvr_client_core` (lib, C-ABI exposed) | Platform-agnostic client brain. Connection state machine (`connection.rs`), sockets, statistics, persistent storage, the video decoder lifecycle. The Android-specific MediaCodec wrapper lives at `src/video_decoder/android.rs` and is `#[cfg(target_os = "android")]` only. Audio I/O via `oboe-rs` is also Android-only. |
| `alvr/client_openxr` | `alvr_client_openxr` (cdylib, APK entry point) | The actual VR app. Owns the OpenXR session, swapchains, the lobby room (`lobby.rs`), the streaming overlay (`stream.rs`), passthrough (`passthrough.rs`), and per-vendor extension wrappers (`extra_extensions/`: Meta body tracking, Pico configuration, BD motion tracking, etc.). Calls into `client_core` for everything off-render-thread. |
| `alvr/client_mock` | `alvr_client_mock` | Desktop-only test harness that talks to a streamer without a real headset — useful for protocol/decoder iteration. |
| `alvr/graphics` (shared) | `alvr_graphics` | wgpu-based GPU pipeline (color correction, foveation, lobby rendering). Shared with the streamer. |
| `alvr/audio` (shared) | `alvr_audio` | Cross-platform audio path; the client's mic capture and game audio playback go through here. |

## Coding rules

- The Android client is built via `cargo xtask build-client` (uses `cargo-apk`). The `package.metadata.android` block in `alvr/client_openxr/Cargo.toml` is the single source of truth for permissions, store features (Meta/Pico/Vive/YVR/AndroidXR), `min_sdk_version` (28), and the runtime library directory (`../../deps/android_openxr`). Don't fork manifest values into another file.
- Toolchain requirements: `ANDROID_NDK_HOME` (NDK r26b — pinned by CI) **and** `ANDROID_HOME` pointing at a full SDK with `platforms;android-32` + `build-tools;32.0.0`. `cargo-apk` needs `aapt2` from build-tools and `android.jar` from platforms — having only the NDK fails with "Android SDK is not found". A JDK (17 is fine) must be on `PATH`/`JAVA_HOME` because cargo-apk signs the APK with `keytool`/`jarsigner`. See `install.txt` for the full host setup record.
- `android-activity` is **pinned to `=0.6.0`** in `alvr/client_openxr/Cargo.toml` because 0.6.1 changed `ndk-context` initialization in a way that freezes the app. Don't bump without verifying on hardware.
- Frame-critical paths must avoid allocation on the render thread. `video_decoder/android.rs` already runs a dedicated `dequeue_thread`; new decode/render work should follow that split.
- All logging is `alvr_common::{info, warn, error, debug}`. The Android log backend forwards through the `log` crate to Android logcat; the tag is `ALVR` — see `alvr_client_core::logging_backend`.
- `client_core` and `client_openxr` both expose a C ABI (`c_api.rs`) for non-cargo-apk integrators (Monado, third-party engines). Treat those signatures as product surface.
- Headset telemetry is sourced from `alvr_system_info::android` and emitted by `control_send_thread` (`alvr/client_core/src/connection.rs`) on the battery interval. The HMD `BatteryInfo` is always sent. The controller `BatteryInfo`s (`InputDevice.getBatteryState`, API 29+) and the bundled `ClientTelemetry` payload (battery temperature, `PowerManager` thermal status/headroom, `/proc/meminfo`, `/proc/self/status`, CPU sampler, KGSL Adreno GPU sampler) are gated on `Settings.metrics.extended_headset_telemetry` — when the toggle is off, the client falls back to legacy "HMD battery only" behavior. Every `ClientTelemetry` field is `Option<_>`; a platform that can't read a sensor must omit it, never substitute a sentinel.

## Running, testing, deploying

```
# Type-check (host-side, no NDK required)
cargo check -p alvr_client_core
cargo check -p alvr_client_mock
cargo clippy -p alvr_client_core -- -D warnings

# Type-check the OpenXR app for Android (needs aarch64-linux-android target + NDK r26b)
cargo check -p alvr_client_openxr --target aarch64-linux-android

# Build the APK (uses cargo-apk under the hood). ANDROID_NDK_HOME must point at NDK r26b.
cargo xtask prepare-deps --platform android        # downloads OpenXR loaders into deps/android_openxr
cargo xtask build-client [--release]               # APK lands in ./build/alvr_client_android/
cargo xtask build-client-lib [--all-targets]       # builds the C-ABI client lib for engine integrators

# Desktop client for protocol iteration
cargo run -p alvr_client_mock
```

Headset deploy / run / observe (assumes `adb` in PATH and headset in developer mode):

```
adb devices                                                            # confirm the headset is reachable
adb install -r build/alvr_client_android/alvr_client_openxr.apk        # path matches xtask output; use --release for the release APK
adb shell am start -n alvr.client.dev/android.app.NativeActivity       # package id from alvr/client_openxr/Cargo.toml [package.metadata.android]
adb logcat -s ALVR:*                                                   # tail ALVR logs (alvr_common log tag)
adb logcat -s ALVR:V '*:S'                                             # verbose + silence everything else
adb shell am force-stop alvr.client.dev                                # stop the app
```

For Meta Store builds the package id is rewritten — see `cargo xtask package-client --meta-store` and the comment in `alvr/client_openxr/Cargo.toml`. For Pico Store: `--pico-store`.

## Where to look when something is broken

- **Decoder stalls / black frames**: `adb logcat -s ALVR:* MediaCodec:* OMXClient:*`. Most timing pathologies are visible in the MediaCodec callbacks emitted from `alvr_client_core::video_decoder::android::dequeue_thread`.
- **Tracking jitter / pose lag**: trace the chain from the OpenXR `xrLocateViews` call in `alvr/client_openxr/src/stream.rs` to the `TrackingData` packets emitted on the `TRACKING` stream id. Statistics get reported back on the `STATISTICS` stream — the dashboard "Statistics" tab on the PC side surfaces the round-trip.
- **OpenXR extension fails to enable**: each vendor extension is gated in `alvr/client_openxr/src/extra_extensions/`. Missing-extension errors usually mean the headset OS doesn't expose that extension — not a code bug.
- **Audio drop-outs**: `alvr_audio` + `oboe-rs`; check buffer sizing config in `Settings.audio`.

## Before touching this scope

Read `docs/ARCHITECTURE.md` for the "Tracking Flow" (client → server) and "Video Pipeline" (server → client) sections so you can see which named thread you're modifying on the client side.
