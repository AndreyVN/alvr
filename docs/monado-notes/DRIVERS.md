# Drivers

Code: `openxr/src/xrt/drivers/`. Every supported hardware device is a driver. A driver provides one of three things (often more than one):

1. **Devices** — implementations of `xrt_device`. Surfaced via either:
   * An `xrt_auto_prober` (always tries to enumerate; for non-USB devices like SLAM, simulated, OpenHMD, RealSense).
   * A USB VID/PID entry in `target_entry_list[]` matched by `xrt_prober` enumeration.
   * A *builder* (`xrt_builder`, declared in `xrt_prober.h`) that picks several probed bits and assembles a full system — used when device discovery is non-trivial (Vive, WMR, Rift S, lighthouse).
2. **Tracking origins** (`xrt_tracking_origin`) — where the device's coordinate system is rooted.
3. **Frame sources** (`xrt_fs`) — only for camera-based drivers feeding tracking / HT / SLAM.

Every driver exposes a small interface header `*_interface.h` consumed by `targets/common/target_lists.c` and the matching `target_builder_*.c`.

## Build wiring

`targets/common/target_lists.c` is the gateway. It declares three arrays:

```c
xrt_builder_create_func_t target_builder_list[]   = { t_builder_qwerty_create, ... };
struct xrt_prober_entry    target_entry_list[]    = { {VID, PID, found_func, name, id}, ... };
xrt_auto_prober_create_func_t target_auto_list[]  = { psvr_create_auto_prober, ... };
```

Each entry is `#ifdef XRT_BUILD_DRIVER_*`-gated so cmake can toggle drivers individually. The probe order is significant — builders run before auto-probers, and within builders the more specific ones run before the catch-all `legacy` builder.

## Driver catalogue

| Folder | Build flag | Notes |
| --- | --- | --- |
| `android/` | `XRT_BUILD_DRIVER_ANDROID` | Android sensors via Sensor Manager / Cardboard. |
| `arduino/` | `XRT_BUILD_DRIVER_ARDUINO` | DIY 3-DoF Arduino IMU. |
| `blubur_s1/` | `XRT_BUILD_DRIVER_BLUBUR_S1` | Blubur S1 HMD. |
| `daydream/` | `XRT_BUILD_DRIVER_DAYDREAM` | Google Daydream controller. |
| `depthai/` | `XRT_BUILD_DRIVER_DEPTHAI` | Luxonis DepthAI cameras — frame source only, feeds `ht/` / SLAM. |
| `euroc/` | `XRT_BUILD_DRIVER_EUROC` | EuRoC dataset replay; not real hardware. |
| `hdk/` | `XRT_BUILD_DRIVER_HDK` | OSVR HDK. |
| `ht/` | `XRT_BUILD_DRIVER_HANDTRACKING` | Computer-vision hand tracker (Mercury models). |
| `ht_ctrl_emu/` | — | Wraps a `ht/` instance into emulated controllers. |
| `hydra/` | `XRT_BUILD_DRIVER_HYDRA` | Razer Hydra. |
| `illixr/` | `XRT_BUILD_DRIVER_ILLIXR` | ILLIXR research integration. |
| `multi_wrapper/` | — | Combine multiple devices into a single logical device. |
| `north_star/` | — | Leap Motion North Star. |
| `ohmd/` | `XRT_BUILD_DRIVER_OHMD` | OpenHMD wrapper — many old HMDs. |
| `opengloves/` | — | OpenGloves haptic gloves. |
| `psmv/` | `XRT_BUILD_DRIVER_PSMV` | PS Move (Bluetooth). |
| `pssense/` | `XRT_BUILD_DRIVER_PSSENSE` | PSVR2 Sense controllers. |
| `psvr/` | `XRT_BUILD_DRIVER_PSVR` | PSVR1. |
| `qwerty/` | `T_BUILDER_QWERTY` | Keyboard/mouse-driven debug HMD + controllers. |
| `realsense/` | `XRT_BUILD_DRIVER_REALSENSE` | Intel RealSense — tracking. |
| `remote/` | `T_BUILDER_REMOTE` | UDP/TCP-fed pose + input source. **Most relevant to ALVR-style integration.** |
| `rift/` | `XRT_BUILD_DRIVER_RIFT` | Oculus DK2. |
| `rift_s/` | `XRT_BUILD_DRIVER_RIFT_S` | Oculus Rift S — full builder. |
| `rokid/` | `XRT_BUILD_DRIVER_ROKID` | Rokid Air/Max glasses. |
| `sample/` | — | Hello-world template. |
| `simula/` | — | SimulaVR HMD. |
| `simulated/` | `T_BUILDER_SIMULATED` / `XRT_BUILD_DRIVER_SIMULATED` | Pure-software wobble/rotate/stationary HMD + simple controllers. |
| `solarxr/` | — | SolarXR body trackers. |
| `steamvr_lh/` | — | **Bring an installed SteamVR Lighthouse driver into Monado as `xrt_device`s.** Opposite direction from "monado as a SteamVR driver". |
| `survive/` | — | libsurvive Lighthouse without SteamVR. |
| `twrap/` | — | Tracking-wrapper helpers. |
| `ultraleap_v2/` `ultraleap_v5/` | `XRT_BUILD_DRIVER_ULV2` / `ULV5` | Ultraleap hand tracking. |
| `v4l2/` | — | V4L2 camera frame source. |
| `vf/` | — | Video-file frame source (for testing CV pipelines). |
| `vive/` | — | Native USB driver for Vive / Valve Index / Vive Cosmos. |
| `vp2/` | — | (newer Vive prefix). |
| `wmr/` | `XRT_BUILD_DRIVER_WMR` | Windows Mixed Reality (HP G2, Samsung Odyssey+, etc.). |
| `xreal_air/` | `XRT_BUILD_DRIVER_XREAL_AIR` | Xreal (formerly nReal) Air glasses. |

## Drivers worth knowing in detail

### `remote/` — the ALVR-shaped driver

`r_interface.h`. Spins up an HMD + two controllers + (optionally) hand tracking and listens on a socket. An external "debugger" program (or a network protocol like ALVR's) sends `struct r_remote_data` packets containing:

```c
struct r_remote_data {
    uint64_t header;                       // R_HEADER_VALUE = "mndrmt3\0"
    struct r_head_data head;               // per-view fov+pose, center pose
    struct r_remote_controller_data left, right;  // pose, velocities, buttons, hand_curl[5]
};
```

Entry: `r_create_devices(port, view_count, broadcast, &xsysd, &xso)`. Also exposes a tiny client-side API (`r_remote_connection_init` / `_read_one` / `_write_one`) so a feeder process can be written in C.

This is the closest analogue in Monado to what ALVR does: **the headset and controllers are streamed in over a network, not USB-attached**. If you're integrating ALVR with Monado, this is your starting point. Limitations: TCP/UDP only, no audio, no per-frame video streaming, head pose only — see [INTEGRATION_NOTES.md](INTEGRATION_NOTES.md).

### `simulated/` — the "no hardware" driver

`simulated_interface.h`. Always-on fallback: a wobbling or rotating HMD plus simple controllers. Used by tests and CI, and as a "this should always work" check. Three movements: `WOBBLE`, `ROTATE`, `STATIONARY`. Useful as a reference when writing a new driver because it implements every `xrt_device` callback in the simplest possible way.

### `steamvr_lh/` — wrap SteamVR Lighthouse devices

`steamvr_lh.cpp`. Loads the SteamVR lighthouse driver DLL from the user's Steam install at runtime (`dlopen` / `LoadLibrary`), talks to it through the official `openvr_driver.h` C++ vtable, and exposes each `vr::ITrackedDeviceServerDriver` as an `xrt_device`. This lets Monado present Index/Vive/Lighthouse setups without writing a USB driver from scratch.

Two files of interest:
* `device.{cpp,hpp}` — `Device` and `HmdDevice` classes implementing `xrt_device`.
* `interfaces/` — local copies of the OpenVR driver interface vtables Monado consumes.

### `wmr/` — Windows Mixed Reality

Largest non-Lighthouse driver. Splits into:
* `wmr_hmd.{c,h}` — HMD sensor read + IMU fusion.
* `wmr_controller_base.{c,h}` + per-vendor specialisations (`wmr_controller_hp.c`, `wmr_controller_og.c`).
* `wmr_bt_controller.{c,h}` — Bluetooth controller path.
* `wmr_camera.{c,h}` — onboard cameras for inside-out tracking.
* `wmr_config*` — JSON config blobs unpacked from the headset firmware.

### `vive/` — Vive + Valve Index

* `vive_device.{c,h}` — HMD.
* `vive_controller.{c,h}` — controllers (Vive wand, knuckles).
* `vive_lighthouse.{c,h}` — base station Watchman protocol.
* `vive_protocol.{c,h}` — USB packet parsing.
* `vive_source.{c,h}` — camera source.
Pairs with `auxiliary/vive/` which has shared (driver-agnostic) calibration + binding code that `vive/`, `steamvr_lh/`, and `survive/` all use.

### `qwerty/` — keyboard-driven dev HMD

When you don't have a headset plugged in but want to run an OpenXR app, this driver exposes a software HMD whose pose moves with WASD + mouse. The builder is `t_builder_qwerty_create` and it sits *first* in `target_builder_list[]` so it can override real hardware when explicitly requested via env var.

### `rift_s/`, `psvr/`, `psmv/`

Native USB / Bluetooth implementations for those headsets/controllers. Look at these (rather than `vive/`) as examples when adding a driver for a single device family without builders.

## Anatomy of a driver

The minimum (see `drivers/sample/`):

1. A `*_interface.h` exposing `xrt_device *foo_create(args)` and/or an `xrt_auto_prober` factory.
2. A `xrt_device` allocation, populated via `u_device_allocate(...)` (in `auxiliary/util/u_device.{c,h}`) — that helper fills sensible defaults so the driver only overrides the methods that matter.
3. A reader thread (created with `os_thread_*`) that polls the sensor and writes into the device's `xrt_input` table and IMU history.
4. Registration in `target_lists.c` (auto-prober list, entry list, or a new builder).
5. A cmake gate `XRT_BUILD_DRIVER_FOO` so the driver can be turned off.
