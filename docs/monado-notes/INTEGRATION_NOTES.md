# Integration notes: Monado from an ALVR perspective

This page is the bridge between the rest of these docs and the rest of this repo. It exists to answer: *if I'm working in ALVR and I have this `openxr/` tree sitting next to it, what's it good for and how do the two stacks line up?*

It's opinionated and ALVR-specific. The other docs in this folder are neutral.

## How ALVR and Monado are shaped, side by side

| | ALVR (this fork) | Monado (`openxr/`) |
| --- | --- | --- |
| Headset side | Android OpenXR client (`alvr/client_openxr`) — an *app* using the headset's runtime to render frames | A whole *runtime* (state tracker + compositor + drivers) |
| PC side | A SteamVR driver (`alvr/server_openvr`) that hands frames to SteamVR for encoding | `monado-service` (out-of-proc) or in-proc runtime, drives the display via its own Vulkan compositor |
| Wire protocol | Custom UDP (frames, audio, tracking, haptics) — `alvr_sockets` + `alvr_packets` | AF_UNIX / named-pipe IPC between client app and service on the *same machine*. No remoting protocol of its own. |
| API surface to apps | None on PC (apps see SteamVR); OpenXR on the headset | OpenXR on both sides |
| Encoder/decoder | NVENC/AMF/Intel VPL (`alvr_server_openvr/cpp`) → MediaCodec on the headset | None — pixels are presented locally to a real display |

Net: Monado is "OpenXR runtime, all local"; ALVR is "stream a remote headset to a local PC that runs SteamVR". They overlap at the abstract-device layer and at the OpenXR client layer, and nowhere else.

## What's directly useful from this tree

These are reusable without restructuring ALVR:

1. **`include/xrt/*.h`** — the C contracts. If we ever want to expose a `xrt_device`-shaped surface to plug into Monado, the headers we'd target are listed in [XRT_INTERFACES.md](XRT_INTERFACES.md). Stable enough to import.
2. **`drivers/remote/`** — the closest analogue to what ALVR does. It listens on a socket, accepts pose+input packets, and presents one HMD + two controllers to the rest of Monado. If we ever build an "ALVR → Monado" path (instead of "ALVR → SteamVR"), `r_create_devices(port, view_count, broadcast, &xsysd, &xso)` is the entry point and `struct r_remote_data` is the (replaceable) wire format. Limits: head pose + buttons only, no video streaming.
3. **`auxiliary/math/m_*`** — well-tested IMU fusion (`m_imu_3dof`), pose history (`m_relation_history`), 1€ filter (`m_filter_one_euro`), prediction (`m_predict`). Higher quality than what's in ALVR right now for the headset client.
4. **`auxiliary/util/u_pacing_*`** — frame pacing algorithm. Independent of OpenXR; could inform `alvr_server_core::metrics_exporter` pacing/jitter modelling.
5. **`auxiliary/vive/`** — pure parsing for SteamVR-style configs (controller bindings, calibration, pose offsets). Useful as a reference if we need to emit SteamVR-compatible payloads from ALVR.
6. **`compositor/client/comp_d3d11_client.cpp`, `comp_d3d12_client.cpp`** — how a runtime imports D3D11/12 textures into Vulkan via shared NT handles. If ALVR ever wants to share encoded surfaces zero-copy between the SteamVR driver and a Vulkan encoder, the pattern is here.

## What's *not* useful and why

* **The compositor (`compositor/main/**`)** — ALVR doesn't render to a local display. We'd be carrying the entire `comp_window_*` + `comp_target_swapchain` + Vulkan render code for nothing.
* **`ipc/`** — Monado's IPC is local-only (AF_UNIX, named pipes). It is not a network protocol and would be the wrong shape to wedge ALVR's UDP into.
* **`state_trackers/oxr/`** — this is "implement OpenXR API as a runtime." ALVR's PC side is a SteamVR driver; it doesn't expose OpenXR. The headset side is an OpenXR *app*, not a runtime.
* **Most `drivers/*`** — every driver here assumes locally-attached hardware. Only `remote/` and `simulated/` are non-local.

## Two integration paths worth knowing exist

These are *possible* shapes if we ever want them; nothing is planned.

### A. ALVR-as-Monado-feeder (replace SteamVR with Monado on the PC)

```
Android headset                PC (Monado side)
───────────────                ─────────────────────────────────
alvr_client_openxr  ── UDP ──► an ALVR-aware Monado driver
                                 - extension or fork of drivers/remote/
                                 - feeds head pose + per-view fov + controllers + buttons
                                 - and (new) a video stream → exposes it as the HMD display
                              monado-service composites & "presents"
                              by handing the frame back to the ALVR
                              encoder before/instead of a real display
OpenXR app on PC ────► OpenXR loader ────► libopenxr_monado.so ────► monado-service
```

Why it's interesting: the PC side becomes a full OpenXR runtime, not a SteamVR-only one. The headset can use Monado's pose prediction + 1€ filter on the *streamed* poses with no glue. The fragile-on-Windows OpenVR layer gets dropped.

Why it's hard: `drivers/remote/` doesn't carry video. Monado's compositor isn't built to redirect its output frame to a network encoder — `comp_target` assumes a real display. We'd need either a new `comp_target` impl that hands the rendered frame to an NVENC/AMF/VPL encoder, or skip the compositor and intercept layers at the `xrt_compositor_native` level.

### B. Monado-as-Android-runtime in front of ALVR

```
Android device                                      PC
──────────────                                      ──
alvr_client_openxr (the app)  ── OpenXR ──► libopenxr_monado.so (in-proc, on Android)
                                            ↑
                                            └ uses Monado for OpenXR plumbing + IMU fusion + pacing
                                            └ but the "device" it shows the app is the standalone headset
                                              (via android driver or remote driver)
```

This is closer to "use Monado as our OpenXR plumbing" and is more straightforward — Monado on Android already builds (`targets/openxr_android/`). The trade-off is replacing the headset vendor's OpenXR runtime with Monado's, which on quest-class hardware is usually a regression because vendor runtimes have low-level platform access we can't reach.

## Concrete places the trees would interact today

Nothing wires them yet; this is "if we ever connect them, here's where":

| ALVR side | Monado side |
| --- | --- |
| `alvr_session::settings_schema` (settings that affect headset transform) | `xrt_device.tracking_origin.initial_offset` |
| `alvr_packets::Tracking` (per-frame head + controller poses) | `r_remote_data` / `xrt_input` slot updates |
| `alvr_server_core::metrics_exporter` frame timings | `u_pacing_compositor` markers (`u_pc_mark_point`) |
| `alvr_client_openxr::interaction` (action sets) | `oxr_input.c` action sync |
| SteamVR driver in `alvr/server_openvr` | `state_trackers/steamvr_drv/` *or* the `drivers/steamvr_lh/` driver (opposite direction) |

## Practical "first steps" when working in `openxr/`

* If you're hunting for "where does X happen" — start with [DATAFLOW.md](DATAFLOW.md), pick the phase, then jump to the file it cites.
* If you're refactoring something that crosses subsystem boundaries — read [XRT_INTERFACES.md](XRT_INTERFACES.md) first. Almost every cross-boundary change is a change to one of those interfaces.
* If you're adding hardware support — copy `drivers/sample/` and read [DRIVERS.md](DRIVERS.md).
* If you're touching IPC RPCs — edit a JSON in `ipc/shared/proto/`, never the generated `ipc_*_generated.{c,h}`. See [IPC.md](IPC.md).
* If you're building locally — Monado uses CMake (`cmake -B build -S openxr && cmake --build build`). It is **not** wired into ALVR's `cargo xtask` toolchain. Treat `openxr/` as a vendored/external project.

## Don't confuse these

A small glossary because the names collide:

* **OpenXR** vs **OpenVR** — Khronos open standard (this tree implements it) vs. Valve's older proprietary API (the `openvr/` submodule).
* **`drv_steamvr_lh`** vs **`state_trackers/steamvr_drv`** — the *driver* consumes SteamVR; the *state tracker* makes us look like a SteamVR driver. Two opposite directions, two different folders, names a few characters apart.
* **`xrt_compositor`** vs **`xrt_system_compositor`** — the former is per-session (one per client), the latter is the factory that mints them.
* **`xrt_system`** vs **`xrt_system_devices`** — `xrt_system` creates sessions; `xrt_system_devices` is the device set those sessions see. Both come out of one call to `xrt_instance.create_system()`.
* **In `openxr/` the word "client" usually means *OpenXR app process*.** In ALVR the word "client" means *Android headset*. They are not the same client.
