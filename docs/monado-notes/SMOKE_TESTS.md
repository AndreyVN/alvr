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

**Monado-side compile gate (`cargo xtask build-openxr-runtime --enable-alvr-driver`)** is the stronger version of Gate A and is currently blocked on Monado's upstream CMake dep list. Verified 2026-05-21 on the Windows refactoring host:
- Eigen3 ≥3.3 — REQUIRED, **now auto-staged** by `build-openxr-runtime` itself (see `ensure_eigen3_windows()` in `alvr/xtask/src/build_openxr.rs`). First run downloads `eigen-3.4.0.zip`, `cmake --install`'s it to `build/_thirdparty/eigen-install`, and injects the prefix into the Monado cmake invocation via `-DCMAKE_PREFIX_PATH=...`. Subsequent runs short-circuit via marker file. Linux/macOS short-circuit at the platform check (system pkg manager owns Eigen3 there). Closed `d78e35ab`.
- `pthreads_windows` — REQUIRED (CMakeLists.txt:153). Maps to upstream **PThreads4W** (`PThreads4W::PThreads4W_CXXEXC`). No header-only fallback; needs an actual build. Next host gap; configure currently halts here.
- Optionals that emitted `Could NOT find` but did not fatal-error: HIDAPI, bluetooth, OpenHMD, OpenCV, libusb1, JPEG, realsense2, depthai, SDL2, ZLIB, cJSON, LeapV2, LeapSDK, ONNXRuntime, wil. Most are HID-driver or vision-pipeline deps the ALVR build path doesn't exercise; they only become blocking if a specific driver is gated ON.

Once `pthreads_windows` (and any follow-on REQUIREDs) are resolved, the host should be able to run the full Phase 3.2 / 3.3 build verification. Until then, the compositor-side Phase 3.2 code lives at "structurally consistent with `null_compositor` reference + `drv_alvr` wiring, but not compile-verified end-to-end". **Pattern**: an `ensure_<dep>_windows()` helper per REQUIRED dep, mirroring the Eigen3 shape in `build_openxr.rs`.

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

(Append "Verified on … with … on YYYY-MM-DD" lines as gates pass. Empty today — no verification host run yet on this branch.)
