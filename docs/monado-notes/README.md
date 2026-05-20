# ALVR-side reference docs for `openxr/` (Monado)

The directory `openxr/` in this repo is a **snapshot of [Monado](https://gitlab.freedesktop.org/monado/monado)**, the open-source OpenXR runtime by Collabora — not the Khronos OpenXR SDK and not Valve's OpenVR SDK. It is the analogue, on the OpenXR side, of the `openvr/` submodule on the OpenVR side.

> These notes live at `docs/monado-notes/` (outside `openxr/`) so that they survive when `openxr/` becomes a git submodule. See [`SUBMODULE_PIN.md`](SUBMODULE_PIN.md) for the migration plan.

This folder collects ALVR-side notes about how Monado is organised, what its core C interfaces look like, and how data flows through it, so that future sessions don't have to re-derive the map every time.

These docs intentionally **do not** duplicate Monado's own developer docs (`openxr/doc/`, doxygen, the upstream wiki). They are a fast on-ramp from the perspective of "I'm working on ALVR and want to know what's in this tree and how I'd interact with it."

## Files

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — process model, the three runtime topologies (in-proc, out-of-proc, SteamVR driver), and how Monado relates to the OpenXR loader.
- [`STRUCTURE.md`](STRUCTURE.md) — annotated map of `src/xrt/**` and `src/external/**`.
- [`XRT_INTERFACES.md`](XRT_INTERFACES.md) — the C abstract interfaces in `include/xrt/` (`xrt_instance`, `xrt_system`, `xrt_system_devices`, `xrt_device`, `xrt_compositor`/`xrt_compositor_native`, `xrt_session`, `xrt_space`/`xrt_space_overseer`, `xrt_tracking`).
- [`DATAFLOW.md`](DATAFLOW.md) — frame lifecycle (`xrWaitFrame` → submit → present), input/tracking flow, session-event flow.
- [`COMPOSITOR.md`](COMPOSITOR.md) — `compositor/{client,main,multi,util,render}` and how a client compositor proxies through IPC to the main compositor.
- [`IPC.md`](IPC.md) — `ipc/{client,server,shared}`, the auto-generated RPC, and the shared-memory layout.
- [`DRIVERS.md`](DRIVERS.md) — driver model (`xrt_device` factories), the driver list, and the drivers most relevant to an ALVR-style integration (`remote`, `steamvr_lh`, `simulated`, `wmr`, `vive`, `rift_s`).
- [`STATE_TRACKERS.md`](STATE_TRACKERS.md) — the OpenXR state tracker (`oxr`), the SteamVR driver state tracker (`steamvr_drv`), GUI, prober.
- [`TARGETS.md`](TARGETS.md) — every binary or library this tree produces (`libopenxr_monado`, `monado-service`, `libmonado`, `driver_monado` for SteamVR, `monado-cli`, `monado-gui`, `monado-ctl`, `sdl_test`) and which subsystems each links in.
- [`INTEGRATION_NOTES.md`](INTEGRATION_NOTES.md) — practical notes for using or borrowing from Monado in the context of ALVR: what's reusable, what's already an out-of-tree integration path, and where the two architectures align or diverge.

## Source of truth

When these notes and the code disagree, **the code wins**. File paths cited here are absolute paths inside `openxr/src/xrt/**` and were correct at the time the snapshot was taken (see `openxr/.gitlab-ci.yml` for the upstream commit if a git submodule is later wired up). If you change a `xrt_*` interface, the impl files cited here are the ones to grep first.

## Quick orientation

If you have only 30 seconds: Monado is a Vulkan-based OpenXR runtime composed of (1) a small set of abstract C interfaces under `include/xrt/`, (2) a compositor that knows how to drive a display directly via Vulkan, (3) a set of hardware drivers each implementing `xrt_device`, and (4) an IPC layer that lets the compositor + drivers live in a separate `monado-service` process while OpenXR client apps load a thin `libopenxr_monado` runtime. The OpenXR API itself is implemented inside `state_trackers/oxr/`.
