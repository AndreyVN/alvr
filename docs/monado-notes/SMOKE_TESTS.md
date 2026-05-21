# Smoke tests for the OpenXR (Monado) runtime mode

The end-to-end verification plan that's been deferred through the Phase 3 and 4 slices. Most of these can't be run from the Windows refactoring host this branch was built on — they need either a Linux box with full Monado deps, or a Windows host with Vulkan SDK 1.4+ and NVENC SDK 12.1+ paired with a real headset. Document them here so the path to verification is concrete the moment a host is available.

When a test passes, move it to a "Verified on … with … on YYYY-MM-DD" line at the bottom of its section. Don't delete the procedure — the same gates apply to future bridge ABI bumps.

## Prerequisites (one-time)

1. **alvr repo** cloned with submodules — `git clone --recurse-submodules` or `git submodule update --init --recursive` after a non-recursive clone. The `openxr/` directory must contain Monado source pinned at the fork's `alvr` branch.
2. **Toolchain audit** per `install.txt` (Windows) or the Monado upstream `BUILDING.md` (Linux). The Monado-side deps Linux needs are listed there; the ALVR-side deps Windows needs are listed in `install.txt`.
3. **Vulkan SDK 1.4.x** installed and `$VULKAN_SDK` set. The bridge's `cc::Build` only emits a warning if it's missing, but Phase 3.2's real `comp_alvr.c` body and Slice 3.3's Vulkan-input NVENC encoder both need it linked.
4. **NVENC SDK 12.1+** for Slice 3.3 work specifically. The D3D11 backends already pinned at 12.2 work fine; the Vulkan-input API path needs ≥12.1's `nvEncRegisterResource(NV_ENC_INPUT_RESOURCE_TYPE_VULKAN_IMAGE_HANDLE)`.
5. **Headset** that the OpenVR path already supports — Quest 2/3/Pro, Pico 4, etc. The point of these smoke tests is comparing OpenXR mode against OpenVR mode behavior, so the headset must work on the existing SteamVR path first.

## Gate A — build clean (any host)

`cargo check -p alvr_server_openxr -p alvr_xtask -p alvr_dashboard` clean. `cargo xtask clippy --ci` clean. This is the gate every PR has to clear before consideration; routinely run from CI. **Rust side as of this branch: clean** (verified locally after each slice).

**Monado-side compile gate (`cargo xtask build-openxr-runtime --enable-alvr-driver`)** is the stronger version of Gate A. **Verified 2026-05-21 on the Windows refactoring host** — see the Verification log below. The path:
- **vcpkg path (default)**: `ensure_vcpkg_windows()` in `build_openxr.rs` clones `microsoft/vcpkg` (full clone) into `build/_thirdparty/vcpkg`, bootstraps it, and threads `-DCMAKE_TOOLCHAIN_FILE=...\vcpkg.cmake` into Monado's CMake invocation. Monado's `vcpkg.json` then declares the full dep set (`pthreads`, `wil`, `cjson`, `eigen3`, `glslang`, `vulkan`, plus `libusb`, `hidapi`, `sdl2` from the default features). One-shot dep closure on first run.
- **First-run cost**: vcpkg clone (~1 GB git tree, ~3 min) + bootstrap (~1 min) + per-port build (~15–30 min depending on cores). Subsequent runs reuse the installed ports.
- **Stale partial-clone defense** (`2646cfe1`): early code paths used `--filter=blob:none` which trips vcpkg's `checkout-index` step under Windows DNS-resolver pressure. `ensure_vcpkg_windows()` now refuses to reuse such a clone and prints the `rd /s /q` remediation command.
- **Per-dep fallback** (`ALVR_OPENXR_SKIP_VCPKG=1`): the older `ensure_eigen3_windows()` path (`d78e35ab`) stays available as a last resort. Closes Eigen3 only; subsequent REQUIREDs (`pthreads_windows` / PThreads4W next) would need their own `ensure_*_windows()` helpers mirroring the Eigen3 shape.
- **Optionals** Monado lists but doesn't fatal-error on: bluetooth, OpenHMD, OpenCV, JPEG, realsense2, depthai, ZLIB, LeapV2, LeapSDK, ONNXRuntime. Mostly HID-driver / vision-pipeline deps the ALVR build path doesn't exercise.

## Gate B — bridge ABI version contract

Pre-flight check that the bridge and driver halves agree on the contract.

```sh
cargo xtask build-openxr-runtime --enable-alvr-driver --release
ALVR_LOG=info ./build/openxr-release/openxr_monado-service  # or the host's Monado service binary
```

Expected:
- Monado log line: `ALVR_INFO ALVR driver scaffolding ready (3 devices)` (HMD + L + R controllers).
- No `ALVR_ERROR ALVR bridge ABI mismatch:` line. If you see one, the cdylib was rebuilt with a bumped `ALVR_OXR_BRIDGE_ABI_VERSION` but the submodule pointer wasn't bumped (or vice versa). Fix by rebuilding both halves in lockstep.
- Bridge spawns its `alvr_oxr_event_drain` thread (visible in process thread list); driver spawns its event-poll thread.

Failure mode to watch for: silent. If the bridge cdylib is *missing* (not just version-mismatched), Monado's `dlopen` fails earlier and you'd see a generic loader error, not the ABI version line.

## Gate C — hello_xr through Monado-as-ALVR (Phase 3.4 exit)

The canonical "is OpenXR plumbed at all?" test.

```sh
cargo xtask build-openxr-runtime --enable-alvr-driver --release
cargo xtask register-openxr-runtime --release   # writes HKCU or XDG
# ALVR client running on the headset; client connected to the streamer over Wi-Fi.
hello_xr -g Vulkan2                              # from the OpenXR-SDK-Source samples
```

Expected:
- `hello_xr` window enumerates view configs from Monado, requests 2 stereo views.
- Driver log: `Session state change: data[0]=1` (client connected event surfaced through the event-poll thread).
- View poses on the headset track real motion (head + controllers), not identity.
- Trigger / grip / thumbstick / A-B-X-Y all surface in `hello_xr`'s input dump.
- A haptic pulse fires on the headset side when the sample triggers it.
- ❌ Video stream itself is **not** expected to work until Phase 3.2 + Slice 3.3 land — `alvr_oxr_submit_layers` is still a stub, so `hello_xr`'s rendered cube doesn't reach the headset display.

Cleanup: `cargo xtask unregister-openxr-runtime --release` (or SteamVR mode won't launch — see Phase 4.3 mutual exclusion).

## Gate D — Battery event reaches the driver log

Validates the end-to-end Battery wiring landed across `a2e04690` / `a6c8edd05`.

While Gate C is set up and a client is connected:
- Pull the headset off the charger if plugged in (or plug in if not). Wait for the next telemetry-export cadence (default 1 s).
- Power-cycle a controller, or wait for the periodic battery sample.

Expected (Monado stderr, `ALVR_LOG=info`):
```
ALVR_INFO Battery: kind=1 gauge=8345/10000 plugged=0   # HMD at 83.45%, unplugged
ALVR_INFO Battery: kind=2 gauge=9120/10000 plugged=0   # left controller
ALVR_INFO Battery: kind=3 gauge=8500/10000 plugged=0   # right controller
```

Negative case: kind values other than 1/2/3 mean a tracking device the bridge doesn't expose surfaced in the bridge's queue, which would indicate `encode_battery` is letting `Other` through. (Bridge drops `Other` before emitting; if you see kind=0 there's a regression.)

## Gate E — OpenVR mode regression check

Critical because Phase 3.0's encoder refactor was the original blocker — bit-identical OpenVR-mode output before/after the refactor was the slice-by-slice gate. Re-run the bitstream diff after any encoder-side change:

```sh
# On master (baseline)
git switch master
cargo xtask build-streamer --release
# Capture 60s of encoded bitstream via the existing dump_video_to_file probe.
# Same headset, same SteamVR scene, same Settings.video.

# On openxr branch
git switch openxr
cargo xtask build-streamer --release
# Capture the same 60s.

cmp baseline.h264 openxr.h264   # exit 0 = identical
```

Slice 2 of Phase 3.0 set this as the merge gate. The byte-diff should still hold today (Slices 2.1–2.3 were purely extractive; Slice 3.x added new code paths but doesn't touch the OpenVR encoder selection). If the diff is non-zero, the regression is in Slice 2.x and that's a release-blocking bug.

## Gate F — Slice 3.3 NVENC streaming (when env available)

The actual "video stream end-to-end" smoke test, blocked on Slice 3.3 implementing `VkEncoderBackend::Submit` against NVENC's Vulkan-image input.

When Slice 3.3 lands:
1. Repeat Gate C setup.
2. Confirm `hello_xr` renders its cube on the headset display, not just in the desktop window.
3. Check streaming statistics in the dashboard: video frames received > 0, no decode-side errors on the client.
4. Compare end-to-end latency to OpenVR-mode baseline — expected to be in the same ballpark since the same `alvr_server_core` connection state machine runs both modes.

Then move on to the production-game smoke test below.

## Gate G — Production game smoke

Pick a game both modes support. Beat Saber is the conventional choice (small, deterministic, headset+controllers heavily exercised, no SteamVR Home dependency).

```sh
cargo xtask register-openxr-runtime --release
# Launch the game directly through whichever store (Steam needs to be set to start through OpenXR — typically via the game's launcher flag, or a winetricks shim on Linux).
```

Acceptance:
- Game starts, sees an OpenXR session.
- Stereo render passes through to the headset.
- Tracking + input feel comparable to OpenVR mode (subjective, but no obvious lag, jitter, or dropped controller events).
- Streaming for 10+ minutes without a crash or visible degradation.

After: `cargo xtask unregister-openxr-runtime --release` and `cargo xtask register-openxr-runtime` again only when you mean to.

## Verification log

- **Gate A (Monado-side compile)** — Verified on Windows 11 Pro 26200 (MSVC 14.44.35207, CMake 4.3.2, Vulkan SDK 1.4.341.0, vcpkg auto-clone) on 2026-05-21. Tip commits at verification: alvr `a70396e8`, openxr submodule `897ff32d4`. `cargo xtask build-openxr-runtime --enable-alvr-driver` ran to completion: vcpkg installed the full Monado dep set from `vcpkg.json` (pthreads, wil, cjson, eigen3, glslang, vulkan-loader, libusb, hidapi, sdl2), Monado configured + built all targets including `comp_alvr.lib` and `drv_alvr.lib`, `monado-service.exe` linked against the `alvr_server_openxr.dll` bridge cdylib via the IMPORTED-target wiring, and `active_runtime_alvr.json` published under `build/openxr-debug/`.

- **Gate B (bridge ABI contract + builder selection)** — Verified 2026-05-21 on the same host. Tip commits: alvr `180155bd`, openxr submodule `9f8f97cd8`. `monado-service.exe` (with `alvr_server_openxr.dll` copied next to it for PATH resolution) ran without a client connected; log showed `ALVR_INFO [alvr_create_devices] Creating ALVR system devices`, `ALVR driver scaffolding ready (3 devices)`, builder list contained the `alvr: ALVR (streamed)` entry as the selected builder, and `Got devices: 0: ALVR Streamed HMD / 1: ALVR Streamed Controller (L) / 2: ALVR Streamed Controller (R)` with the correct ALVR_HMD / ALVR_Controller_{Left,Right} serials from the bridge. No `ALVR bridge ABI mismatch` line. Service stayed alive waiting for IPC. **Known open item**: compositor warns `Had to fill in meshuv on xdev 'ALVR Streamed HMD', this should be done in the driver` — alvr_hmd_create needs to populate distortion mesh UVs. Non-fatal; polish item for Phase 3.4 client-side bringup.

- **DLL deployment gap** observed while verifying Gate B: `alvr_server_openxr.dll` must be on monado-service.exe's PATH or copied next to it. The Monado build doesn't auto-deploy the cdylib to the service target dir today. Follow-up xtask `POST_BUILD` step would close this for end users; manual copy is a workable workaround for local verification.

Gates C–G (hello_xr / Battery / OpenVR regression / NVENC streaming / production game) still require an active ALVR client + a real headset, not exercisable from a refactoring host alone.
