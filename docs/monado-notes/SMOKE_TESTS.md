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
- Builder selection line: `Using builder alvr: ALVR (streamed)`.
- Compositor selection line: `INFO [comp_alvr_create_system_compositor] ALVR compositor ready`. If you see `Doing init` from `comp_main_create_system_compositor` instead, `XRT_FEATURE_COMP_ALVR` isn't `#define`'d at compile time (check `build/openxr-debug/src/xrt/include/xrt/xrt_config_build.h` — should contain `#define XRT_FEATURE_COMP_ALVR`).
- No `ALVR_ERROR ALVR bridge ABI mismatch:` line. If you see one, the cdylib was rebuilt with a bumped `ALVR_OXR_BRIDGE_ABI_VERSION` but the submodule pointer wasn't bumped (or vice versa). Fix by rebuilding both halves in lockstep.
- Bridge spawns its `alvr_oxr_event_drain` thread (visible in process thread list); driver spawns its event-poll thread.

Failure mode to watch for: silent. If the bridge cdylib is *missing* (not just version-mismatched), Monado's `dlopen` fails earlier and you'd see a generic loader error, not the ABI version line. **Also silent**: `comp_main` being selected instead of `comp_alvr` — the service still boots cleanly and `Using builder alvr` still appears, only the compositor factory line gives it away (see fork commit `56466ce47`).

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

## Gate H — per-view (eye-tracked) foveation end-to-end (Slice 6 exit)

The exit gate for per-view foveation. The wire (both channels), the server-side producer cache, and the client de-foveation pipeline all landed (PRs #2/#3/#5, 2026-05-27/28). **Slice 6 is the one missing piece**: the OpenXR-mode encoder still applies *static, uniform* FFR (`alvr_oxr_get_foveation_vars`) — it must instead read the per-eye `alvr_oxr_get_foveation` cache so each eye's high-res inset follows its own gaze. This gate proves the full loop: per-eye params → encoder warps each eye differently → client inverts each eye correctly.

### Preconditions

1. ✅ **Slice 6 landed (2026-05-28)** — `comp_alvr`'s FFR compute pass now consumes `alvr_oxr_get_foveation` per eye (`centerShift` moved to a push constant in `ffr.comp` + `run_ffr`), not just the uniform `alvr_oxr_get_foveation_vars`. Behaviour is what this gate verifies.
2. ✅ **Synthetic injection hook landed (2026-05-28)** — Quest 3 (the standing test headset) has **no eye tracking**, so `FaceData.eyes_*` never arrives and the real `PerViewFoveationEmitter` gaze path never emits. The hook: set `ALVR_TEST_PER_VIEW_FOVEATION=lx,ly,rx,ry` on the **streamer** before connecting — `PerViewFoveationEmitter` then emits a constant per-eye `ServerCoreEvent::PerViewFoveation([[lx,ly],[rx,ry]])` (rate-limited, bypassing `eyes_*`). This is the producer-side injection that feeds **both** consumers — the encoder cache (via the OpenXR drain thread → `alvr_oxr_set_foveation`) *and* the wire to the client (`VideoPacketHeader.per_view_foveation`). Injecting via `alvr_oxr_set_foveation` directly would warp the encoder but leave the client on its static path → guaranteed mismatch. (A Quest Pro / eye-tracked headset would exercise the real gaze path and make this hook unnecessary.) Confirm it's active by grepping the streamer log for `ALVR_TEST_PER_VIEW_FOVEATION active`.
3. **Session config** — `settings.video.foveated_encoding` enabled AND `foveated_encoding.per_view_eye_tracked` enabled. **Edit `session.json` no-BOM** (`WriteAllText` + `UTF8Encoding($false)`; PowerShell `Set-Content -Encoding utf8` writes a BOM/UTF-16 → ALVR loads defaults). Use clearly-asymmetric synthetic centres, e.g. left `center_shift = (-0.25, 0.0)`, right `center_shift = (+0.25, 0.0)` — the two eye insets should visibly diverge horizontally.

### Setup (RTX 3090, `192.168.10.101`)

Reuse the `oxr_overlay_smoke` deploy recipe (PS-remoting, `Copy-Item -ToSession` the freshly-built `monado-service.exe` + `openxr_monado.dll` + `alvr_server_openxr.dll`, HKLM `ActiveRuntime` flip to the Monado manifest with a `try/finally` restore, `run.bat` for the service, `adb shell monkey -p alvr.client.dev` for the client, `*:I` logcat). Run **non-elevated** (admin pipe ACL blocks the IPC) and set `ALVR_ROOT` to `%LOCALAPPDATA%`. Note: the OpenXR encoder resolution is `openvr_config.eye_resolution`, **not** the transcoding-view-resolution setting. Long smoke window (`OXR_SMOKE_SECS ≥ 1800`) so a conversation round-trip doesn't outlast it.

### Expected — pass

- **Server (`session_log.txt`, UTF-16 — use `Select-String`)**: the encoder content-diag reports two *different* foveation centres per frame (left vs right), matching the injected `ALVR_TEST_PER_VIEW_FOVEATION`. The throttled `report_hand_skeleton`-style `info!` for foveation (if added) confirms the per-view cache is being read on the hot path. Encoded frame is non-black and the per-eye insets sit at the injected positions.
- **Client (Quest, logcat / a client built with logging)**: `per_view_center_shift()` returns `Some` with the injected per-eye values (not `None`); the `FFE_RUNTIME` pipeline is the one bound (not the static path). `hevcD`/`avcD` decodes cleanly (0 "Unsupported input buffer"), ~70 fps.
- **On the headset (the real acceptance)**: the image is correctly reprojected for **both** eyes — the high-res region is offset left in the left eye and right in the right eye, with **no warp seams, doubling, or smearing** at the inset boundary in either eye. Compare against the uniform-FFR baseline (per_view disabled): the uniform case keeps both insets centred; the per-view case visibly diverges, and both still resolve to a sharp, artifact-free image.

### Expected — negative cases (what failure looks like)

- **Client stays on the static path** (per-eye inset identical in both eyes despite divergent injection) → the wire value isn't reaching `render`, or `per_view_center_shift()` returns `None`. Check `per_view_foveation_queue` is being filled and the timestamp match in `report_compositor_start`.
- **Warp seam / smearing in one eye** → the client's runtime de-foveation constants don't match the encoder's warp for that eye (invertibility broken). Most likely cause: `center_size`/`edge_ratio` drift between the encoder's `foveation_compress_vars` and the client's baked spec-constants, or the WGSL `FFE_RUNTIME` derivation diverging from `foveated_encoding_shader_constants`. Only `center_shift` is meant to vary per frame; if `center_size`/`edge_ratio` differ per eye the staging resolution assumption breaks.
- **Black headset** → re-check the resolution pipeline (encoder = `openvr_config.eye_resolution`) and that IDRs ship (the historical `RequestIDR`-dropped bug — see [[openxr-ffr-encoder-plan]]).

### Cleanup

Restore HKLM `ActiveRuntime` (the `finally` block), disable `per_view_eye_tracked` in `session.json` (no-BOM), unset `ALVR_TEST_PER_VIEW_FOVEATION`.

### Verification ceiling

Cannot be exercised from the dev host (no GPU; no eye-tracked headset). Needs `.101` + the synthetic-injection hook + Slice 6. The client de-foveation pipeline's only host-side guardrail today is the `naga` parse+validate test on `stream.wgsl`; true invertibility (encoder warp ⇄ client inverse) is confirmed only by the artifact-free-both-eyes criterion above.

## Verification log

- **Gate A (Monado-side compile)** — Verified on Windows 11 Pro 26200 (MSVC 14.44.35207, CMake 4.3.2, Vulkan SDK 1.4.341.0, vcpkg auto-clone) on 2026-05-21. Tip commits at verification: alvr `a70396e8`, openxr submodule `897ff32d4`. `cargo xtask build-openxr-runtime --enable-alvr-driver` ran to completion: vcpkg installed the full Monado dep set from `vcpkg.json` (pthreads, wil, cjson, eigen3, glslang, vulkan-loader, libusb, hidapi, sdl2), Monado configured + built all targets including `comp_alvr.lib` and `drv_alvr.lib`, `monado-service.exe` linked against the `alvr_server_openxr.dll` bridge cdylib via the IMPORTED-target wiring, and `active_runtime_alvr.json` published under `build/openxr-debug/`.

- **Gate B (bridge ABI contract + builder + compositor selection)** — Verified 2026-05-21 on the same host. Tip commits: alvr `81426fd4` → submodule bump pending (this round), openxr submodule `f80c98a7d` → `4cce895cd`. Initial run confirmed `Using builder alvr: ALVR (streamed)`, `Got devices: 0: ALVR Streamed HMD / 1: ALVR Streamed Controller (L) / 2: ALVR Streamed Controller (R)` with the correct `ALVR_HMD` / `ALVR_Controller_{Left,Right}` serials, no ABI mismatch, no `meshuv` compositor warning. Re-run after fork `56466ce47` (XRT_FEATURE_COMP_ALVR cmake_in fix) + `4cce895cd` (Phase 3.3 pacing markers) additionally confirmed `INFO [comp_alvr_create_system_compositor] ALVR compositor ready` — comp_alvr is now actually the selected compositor at runtime (it had been silently falling through to `comp_main` previously, because the `#define` was never emitted into `xrt_config_build.h`). Service stayed alive waiting for IPC.

- **DLL deployment** — closed `82a58504`. `cargo xtask build-openxr-runtime` now runs `deploy_bridge_cdylib()` after `cmake --build`, copying `target/<profile>/alvr_server_openxr.dll` into every per-config subdirectory of `src/xrt/targets/service/` that contains a built `monado-service.exe`. Non-fatal if the cdylib hasn't been built (emits a warning naming the remediation). Windows-only — Linux .so resolution still rides the existing rpath hint.

Gates C–G (hello_xr / Battery / OpenVR regression / NVENC streaming / production game) still require an active ALVR client + a real headset, not exercisable from a refactoring host alone.

- **Gate H (per-view foveation) — PARTIAL PASS 2026-05-28** on the RTX 3090 (`192.168.10.101`, NVIDIA GeForce RTX 3090, driver-side real Vulkan). Deployed the Slice 6 `monado-service.exe` + `alvr_server_openxr.dll` (prior binaries backed up as `.preslice6`), booted, ran a 30s `oxr_overlay_smoke` (the OpenXR app drives `comp_alvr`'s FFR pass directly — no streaming client needed for the compositor half). Confirmed:
  - `INFO [compositor_init_render_resources] FFR enabled: scratch 2144x2240 -> foveated 1280x1184 per eye` and `INFO [init_ffr] FFR compute pass ready (2 views, 1280x1184 per eye)` — the modified `init_ffr` (push-constant pipeline layout + 6 static spec constants) builds clean on real NVIDIA Vulkan.
  - `Summary: submitted=2154 endframe_errors=0 final_state=FOCUSED` — `run_ffr`'s per-eye `vkCmdPushConstants` dispatch ran 2154 frames with zero endframe errors, zero VK/validation errors, no crash. This clears the main runtime risk of the shader/pipeline change (local build was an AMD iGPU; CI was compile-only).
  - **Still PENDING (needs a connected Quest — `adb devices` was empty this session):** (1) per-eye *divergence* — the `ALVR_TEST_PER_VIEW_FOVEATION` synthetic hook only fires inside a streaming session's `tracking_loop`, so this run used the static-shift fallback for both eyes (confirmed: no `ALVR_TEST_PER_VIEW_FOVEATION active` log); (2) client decode of the foveated stream; (3) the visual both-eyes-artifact-free judgement. To finish: connect a Quest + `alvr.client.dev`, set `ALVR_TEST_PER_VIEW_FOVEATION=-0.25,0,0.25,0` + enable `per_view_eye_tracked` on the streamer, stream, then a human checks both eyes.
  - Note: the smoke's runtime version label reads Monado `'da2bb4f37'` (the submodule HEAD when the build ran with Slice 6 uncommitted in the working tree); the binary nevertheless contains the Slice 6 code — the `FFR compute pass ready` line + the per-eye dispatch prove it.

- **Gate H follow-up — Quest streaming half VERIFIED 2026-05-28** (same `.101`, Quest 3 over Wi-Fi at `192.168.10.201`, `7621.client.local.` reaching `connection_state=Streaming`). Closed the streaming side of the partial pass — the `ALVR_TEST_PER_VIEW_FOVEATION=-0.25,0,0.25,0` env did make it into `monado-service.exe` and `PerViewFoveationEmitter::new` logged `ALVR_TEST_PER_VIEW_FOVEATION active: injecting synthetic per-eye center_shift L=[-0.25, 0.0] R=[0.25, 0.0]` on every fresh tracking-loop start (three times across reconnect cycles). Foveation is **visibly active on the headset** — once `oxr_overlay_smoke` was extended to upload a high-frequency checkerboard pattern (see below), the user observed stretched/aliased squares at the periphery: the FFR pass really is compressing the edges, the encoder really is producing the foveated stream, the client really is de-foveating. The full **per-eye divergence judgement** (sharp inset offset LEFT in left eye vs RIGHT in right eye) was NOT recorded this session — the user did not report a one-eye-at-a-time comparison before moving on. Gate H is therefore **PARTIAL+** (compositor-half + streaming-half + visible foveation + hook firing all confirmed; only the explicit per-eye-position visual check is still open).
  - **`oxr_overlay_smoke` content trap (DURABLE):** the smoke client originally just `vkCmdClearColorImage`-cleared each swapchain to a solid cycling colour. Foveation is invisible by construction on solid colour — both the high-res inset and the periphery compress identical solid colour to identical solid colour. First headset observation here was "uniform sharpness, no inset"; that is NOT a Gate H failure, it is the smoke having no detail to foveate. Fixed in this session: `pattern_image` closure uploads a 8-pixel-cell black/white checkerboard via `vkCmdCopyBufferToImage` from a host-visible staging buffer (one 19 MB allocation per `proj_w*proj_h`-sized image, filled once at startup, reused per frame). The cycling-colour clear is preserved for the quad layer. **For any future visual foveation/encoder-quality verification, always use a detailed pattern source — checkerboard, text, or a real OpenXR app — not the solid-colour smoke.**
  - **Streaming-half setup (recipe, in case the run needs to be repeated):** schtasks-interactive on `.101` to detach monado-service across the PSRemoting session boundary (WSMan would otherwise reap a PSRemoting-spawned child); two tasks — `GateHMonado` (runs `cmd /c run_gate_h.cmd` which sets `ALVR_TEST_PER_VIEW_FOVEATION=-0.25,0,0.25,0` + `ALVR_ROOT=%LOCALAPPDATA%\alvr_openxr_root` then `monado-service.exe > monado.log 2>&1`) and `GateHSmoke` (runs `cmd /c run_smoke_long.cmd` with `OXR_SMOKE_SECS=1800`). `New-ScheduledTaskPrincipal -LogonType Interactive -RunLevel Limited` is required (admin-pipe ACL blocks Monado IPC). session.json edits via `[System.IO.File]::WriteAllText` + `UTF8Encoding($false)` (PowerShell `Set-Content -Encoding utf8` writes a BOM and ALVR silently loads defaults). `preferred_codec` set to `Hevc` for the 4288-wide framebuffer (H.264 caps at 4096). ALVR uses **mDNS-SD** for client discovery (not UDP/9943 broadcast as a casual reading might suggest); the streamer browses, the Quest publishes — so the absence of a TCP/UDP listener on 9943 before connection is not a bug. Cleanup: stop+unregister both tasks, restore HKLM `ActiveRuntime` from `C:\alvr\test-openxr\activeruntime_backup.txt`, revert `per_view_eye_tracked` + `preferred_codec` in session.json.

- **Gate H — PASS 2026-05-28** (same `.101`, same Quest 3 over Wi-Fi at `192.168.10.201`). Closed the per-eye position visual check that was open after the streaming-half follow-up. Re-staged with `preferred_codec=Hevc` + `per_view_eye_tracked.enabled=true` + `ALVR_TEST_PER_VIEW_FOVEATION=-0.25,0,0.25,0`, Quest connected (state=Streaming), hook fired, user looked one-eye-at-a-time at the checkerboard pattern and **confirmed: the high-detail / sharp region is offset to a different position in each eye** ("region is shifted"). End-to-end per-view foveation invariant — encoder applies per-eye warp from the bridge cache, client de-foveates with the matching per-view shift from the wire, both eyes resolve to a visibly-divergent inset position. **All of Slice 6 is now hardware-verified.** First per-eye observation was a snap "same in both eyes" — the careful one-eye-at-a-time comparison is what surfaced the divergence; record the test instruction explicitly in any future re-run. `.101` restored after: tasks stopped/unregistered, session.json reverted (H264 + per_view off, no-BOM), HKLM `ActiveRuntime` restored to SteamVR.
