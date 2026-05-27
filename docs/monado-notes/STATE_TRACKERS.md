# State trackers

Code: `openxr/src/xrt/state_trackers/`. A "state tracker" is the layer that translates an external API into `xrt_*` calls. There are four in tree, but only one — `oxr` — is the main product.

## `oxr/` — the OpenXR state tracker

This is what implements the OpenXR API. Every `xrCreateInstance`, `xrCreateSession`, `xrLocateSpace`, `xrEndFrame`, etc. eventually lands in a function here.

### Layout

```
oxr_api_*.c          One per OpenXR function family. The xr* entrypoints live here.
                      action, body_tracking, debug, face_tracking2_fb, face_tracking_android,
                      face_tracking_htc, future, instance, negotiate (loader handshake),
                      passthrough, session, space, swapchain, system, xdev.
oxr_session_gfx_*.c  One per graphics binding: vk, gles_android, gl_xlib, gl_win32, egl, d3d11, d3d12.
                      Picks the matching xrt_gfx_*_provider_create at session creation.
oxr_swapchain_*.c    Per-GAPI swapchain: gl, vk, d3d11, d3d12 + oxr_swapchain.c shared.
oxr_objects.h        All oxr_* handle types: oxr_instance, oxr_session, oxr_space, oxr_swapchain,
                      oxr_action_set, oxr_action, oxr_subaction_paths, oxr_action_attachment,
                      oxr_handle_base, oxr_logger, ...
oxr_handle_base.c    Base "OpenXR-style refcounted handle" mechanics.
oxr_event.c          OpenXR event queue (per-instance).
oxr_input.c          XR_KHR_simple_controller, action sets, binding tables, sync.
oxr_input_transform.{c,h}  Action-pose transforms; tests in `openxr/tests/tests_input_transform.cpp`.
oxr_path.c           XrPath atom interning.
oxr_subaction.h      Subaction path constants (left/right/gamepad/...).
oxr_chain.h          XrStructureType::next chain walker (oxr_input_get_next_xr_chain etc.).
oxr_conversions.h    OpenXR ↔ xrt enum mappings.
oxr_d3d{,11,12}.cpp  D3D-specific helpers.
oxr_dpad.c           Dpad emulation from thumbstick.
oxr_frame_sync.{c,h} Client-side xrWaitFrame/Begin/End ordering.
oxr_messenger.c      XR_EXT_debug_utils.
oxr_pretty_print.{c,h} Logging helpers.
oxr_session_frame_end.c   The big layer-submission switch (one branch per XrCompositionLayer*).
oxr_space.c          XrSpace impl, locating, recentering.
oxr_logger.{c,h}     Per-instance logger (level, color, sink).
oxr_extension_support.h   The list of supported XR_* extensions (auto-included from the bindings).
oxr_two_call.h       The xrEnumerateFoo(0, &cap, NULL)/(cap, &cap, arr) pattern helper.
oxr_verify.c, oxr_api_verify.h  Argument-validation macros used by oxr_api_*.c.
oxr_xdev.c           Helpers wrapping xrt_device lookups.
oxr_xret.h           xrt_result_t → XrResult mapping.
oxr_bindings/        Generated binding tables.
```

### Loader handshake

`oxr_api_negotiate.c:46` exports `xrNegotiateLoaderRuntimeInterface(XrNegotiateLoaderInfo*, XrNegotiateRuntimeRequest*)`. The OpenXR loader calls this immediately after loading the runtime library. We hand back:
* API version we support,
* `xrGetInstanceProcAddr` pointer (which routes to all the other `oxr_xr*` functions).

After negotiation the loader uses `xrGetInstanceProcAddr` for everything — there are no other symbol imports from the runtime.

### Instance creation flow

```
xrCreateInstance(info, &xrInst)
  └ oxr_instance_create(logger, info, &oxr_inst)
       u_trace_marker_init();
       oxr_path_init(&inst);
       xrt_instance_create(&xinst_info, &xinst);   // ← here we cross into xrt_* land
       xrt_instance_create_system(xinst, &xsys, &xsysd, &xso, &xsysc);
       u_hashset_create(&extensions);   // build extension table from oxr_extension_support.h + chain
       ... fill oxr_instance fields ...
       return XR_SUCCESS;
```

After this the app has an `XrInstance` handle that internally points at an `oxr_instance` which owns the `xrt_instance` + `xrt_system*`.

### Session frame end (the heaviest function)

`oxr_session_frame_end.c` handles `xrEndFrame`. It validates each `XrCompositionLayer*` header, walks its `next` chain for FB/META blending extensions, builds an `xrt_layer_data`, looks up the underlying `xrt_swapchain` for each `XrSwapchain` handle, and calls into the client compositor:

```c
xc->layer_projection(xc, head_xdev, &xrt_layer_data, swapchains);
xc->layer_quad(...);   // or quad / cylinder / equirect1 / equirect2 / cube / passthrough
xc->layer_commit(xc, frame_id, sync_handle);
```

This is the file to read if you want to understand exactly how Monado normalises OpenXR composition.

### Extension surface

`oxr_extension_support.h` is generated from `auxiliary/bindings/oxr_bindings/*.json` by `scripts/generate_oxr_ext_support.py`. Adding a new extension requires:
1. Adding it to the JSON,
2. Implementing the new entry points in a new (or existing) `oxr_api_<feature>.c`,
3. Wiring any `xrt_*` impl needed (often touching `xrt_device` or `xrt_compositor`).

## `steamvr_drv/` — make Monado a SteamVR driver

This is the *opposite direction* from the `drv_steamvr_lh` driver: instead of consuming SteamVR, this state tracker exposes Monado's `xrt_system` to **SteamVR** as a `vr::IServerTrackedDeviceProvider`. SteamVR loads it as `driver_monado`.

* Pairs with target `targets/steamvr_drv/main.c` which provides the `HmdDriverFactory` export.
* Implementation is `state_trackers/steamvr_drv/**` (referenced by `targets/steamvr_drv/CMakeLists.txt`); files like `ovrd_driver.{c,cpp}` define the OpenVR interface bridges (search `ovrd_hmd_driver_impl`).
* Useful when you want Monado's drivers but SteamVR's rendering/compositor.

## `gui/` — debug GUI

Optional dear-imgui based panel exposed when `XRT_FEATURE_CLIENT_DEBUG_GUI` is on. Reads any `u_var`-registered field in real time — devices register live values (`u_var_add_*`) so you can inspect IMU samples, predicted poses, pacing timings, etc. The main entry is `gui_main_init/loop` (see `state_trackers/gui/`). Toggled via env var `XRT_DEBUG_GUI=1`.

## `prober/` — hardware-discovery support

Shared helpers for `xrt_prober` implementations: walks USB / HID buses, matches against the `target_entry_list[]`, runs auto-probers. Used by `targets/common/target_instance.c` when building the in-process `xrt_instance`. The interface contract (`xrt_prober`) is in `include/xrt/xrt_prober.h`; the default impl lives here.

## Where do new APIs live?

| If you want to add… | Touch… |
| --- | --- |
| A new OpenXR extension | `state_trackers/oxr/` (oxr_api_<ext>.c, oxr_extension_support.h, possibly `xrt_compositor.h` / `xrt_device.h`). |
| A new SteamVR feature | `state_trackers/steamvr_drv/`. |
| A new debug visualisation | `state_trackers/gui/` + `u_var_add_*` calls in the relevant subsystem. |
| A new device | `drivers/<name>/` (NOT a state tracker — see [DRIVERS.md](DRIVERS.md)). |
| A new "way to ship the runtime" | `targets/<name>/` (NOT a state tracker — see [TARGETS.md](TARGETS.md)). |
