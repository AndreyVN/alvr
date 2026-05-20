# XRT interfaces (the `xrt_*` contract)

All cross-module communication in Monado goes through abstract C interfaces declared in `openxr/src/xrt/include/xrt/xrt_*.h`. Each interface is a `struct` of function pointers (a vtable), with `static inline` helpers `xrt_foo_method(self, ...)` that just call `self->method(self, ...)`. There is no central registry — implementations create the struct, fill the function pointers, and return a pointer. Most interfaces are reference-counted via an embedded `struct xrt_reference`.

This is the contract layer. Everything else (compositor, drivers, IPC, state trackers) plugs into it.

## The graph

```
                xrt_instance
                  │ creates
                  ▼
         ┌───────────────────────┐
         │  xrt_system           │  ── creates ──►  xrt_session   ──►  xrt_compositor_native  ──►  xrt_swapchain
         │  xrt_system_devices   │ owns  ┌──►  xrt_device[N]      session events
         │  xrt_system_compositor│ owns  │      (head, hands, controllers, eye, face, body)
         │  xrt_space_overseer   │ owns  │      └──► xrt_tracking_origin
         └───────────────────────┘
```

## `xrt_instance` — root of the process

File: `src/xrt/include/xrt/xrt_instance.h`.

Methods:
* `is_system_available(xinst, *out_available)`
* `create_system(xinst, *out_xsys, *out_xsysd, *out_xso, *out_xsysc)` — builds the system, its device set, the space overseer, and (optionally) a system compositor.
* `get_prober(xinst, *out_xp)` — only set on in-process targets.
* `destroy(xinst)`.

Fields: `xrt_instance_info instance_info` (app + platform), `startup_timestamp` (CLOCK_MONOTONIC).

Factory: `xrt_result_t xrt_instance_create(struct xrt_instance_info *ii, struct xrt_instance **out);` — each target provides this. Out-of-process targets implement it as `ipc_instance_create()`; in-process ones implement it as `t_instance_create()` (see `targets/common/target_instance.c:170`).

There must be at most **one** `xrt_instance` per process.

## `xrt_system` / `xrt_system_devices` / `xrt_system_compositor` / `xrt_space_overseer`

File: `src/xrt/include/xrt/xrt_system.h` and `xrt_space.h`.

* `xrt_system` is the "form a session" factory.
  * `create_session(xsys, *xsi, *out_xs, *out_xcn)` — produces one `xrt_session` and optionally one `xrt_compositor_native`.
* `xrt_system_devices` is the device set:
  * `xdevs[XRT_SYSTEM_MAX_DEVICES]` array (32 max) of owning pointers.
  * `static_roles.{head,eyes,face,body,hand_tracking.{unobstructed,conforming}.{left,right}}` — observing aliases.
  * `get_roles(*out_roles)` returns the dynamic input mapping (`left`, `right`, `gamepad`, plus their interaction profiles).
  * `feature_inc(type) / feature_dec(type)` — refcount per `xrt_device_feature_type`. When a feature transitions 0→1 the system "begins" it (e.g. powers a sensor on); on the last `_dec` it "ends" it.
* `xrt_system_compositor` (declared in `xrt_compositor.h` near line 2376) is the *server-side* compositor — the entity that mints `xrt_compositor_native` instances, one per session. Implementations: `compositor/main/comp_compositor.c` (real) and `compositor/multi/comp_multi_system.c` (multi-client wrapper).
* `xrt_space_overseer` (`xrt_space.h`) owns the scene-graph of `xrt_space` objects:
  * semantic spaces (`root`, `view`, `local`, `local_floor`, `stage`, `unbounded`),
  * a per-client `localspace[XRT_MAX_CLIENT_SPACES]` array,
  * device pose / action pose locating, reference-space recentering. Default impl is `auxiliary/util/u_space_overseer.{c,h}`.

## `xrt_device` — the universal device abstraction

File: `src/xrt/include/xrt/xrt_device.h` (1160 lines — the biggest interface).

Key shape:

```c
struct xrt_device {
    enum xrt_device_name name;     // e.g. XRT_DEVICE_VIVE_CONTROLLER, XRT_DEVICE_GENERIC_HMD
    enum xrt_device_type device_type;
    char str[XRT_DEVICE_NAME_LEN], serial[XRT_DEVICE_NAME_LEN];

    struct xrt_tracking_origin *tracking_origin;

    struct xrt_hmd_parts *hmd;     // non-NULL only for HMDs — display+lens info
    struct xrt_device_supported supported; // bitfield: hand tracking, eye tracking, etc.

    struct xrt_input  *inputs;     uint32_t input_count;
    struct xrt_output *outputs;    uint32_t output_count;
    struct xrt_binding_profile *binding_profiles; uint32_t binding_profile_count;

    // Vtable:
    void (*destroy)(self);
    xrt_result_t (*update_inputs)(self);
    xrt_result_t (*get_tracked_pose)(self, name, at_timestamp_ns, *out_relation);
    xrt_result_t (*get_hand_tracking)(self, name, at_timestamp_ns, *out_value, *out_timestamp_ns);
    xrt_result_t (*get_face_tracking)(self, type, at_timestamp_ns, *out);
    xrt_result_t (*get_body_skeleton)(...);
    xrt_result_t (*get_body_joints)(...);
    xrt_result_t (*set_output)(self, name, *value);   // haptics, LEDs
    xrt_result_t (*get_view_poses)(...);              // for HMDs
    xrt_result_t (*compute_distortion)(...);
    xrt_result_t (*get_visibility_mask)(...);
    xrt_result_t (*begin_feature)(self, feature_type);
    xrt_result_t (*end_feature)(self, feature_type);
    ...
};
```

* `xrt_hmd_parts` (declared mid-`xrt_device.h`) holds display dimensions, per-view info (`xrt_view` × N), refresh rate, distortion mesh.
* `xrt_input` / `xrt_output` are the runtime input/haptic table, keyed by OpenXR-style path enums.
* Helper layer: `auxiliary/util/u_device.{c,h}` provides `u_device_allocate(...)` and reasonable defaults so drivers can override only what they implement.

## `xrt_compositor` and `xrt_compositor_native`

File: `src/xrt/include/xrt/xrt_compositor.h` (2740 lines — covers layers, swapchains, semaphores, and the compositor itself).

* `xrt_compositor_info` — formats supported, max texture size.
* `xrt_compositor`'s vtable mirrors the OpenXR session contract:
  * `create_swapchain`, `import_swapchain`, `create_passthrough`, `create_passthrough_layer`,
  * `begin_session(*xbsi)`, `end_session`,
  * `wait_frame`, `begin_frame`, `discard_frame`,
  * `layer_begin`, `layer_projection*`, `layer_quad`, `layer_cylinder`, `layer_equirect*`, `layer_cube`, `layer_passthrough`, `layer_commit`,
  * `poll_events`, `set_thread_hint`,
  * `destroy`.
* `xrt_compositor_native` extends `xrt_compositor` with native-handle import (e.g. `xrt_compositor_native_create_swapchain_from_native_image`).
* Client-side per-API wrappers in `compositor/client/comp_*_client.{c,cpp}` adapt this interface to Vulkan/GL/D3D11/D3D12. The state tracker only talks to the wrapper.

`xrt_swapchain` is a separate reference-counted interface in the same header — `acquire_image`, `wait_image`, `release_image`, `destroy`, plus `image_count`.

## `xrt_session` and `xrt_session_event*`

File: `src/xrt/include/xrt/xrt_session.h` (318 lines).

`xrt_session` is the "OpenXR session minus the rendering surface"; rendering is on the parallel `xrt_compositor`. It has:
* `xrt_session_event` (a tagged union — see lines 199+). One event type per OpenXR `XrEventData*` that the runtime cares about:
  * `STATE_CHANGE`, `OVERLAY_CHANGE`, `LOSS_PENDING`, `LOST`, `DISPLAY_REFRESH_RATE_CHANGE`, `REFERENCE_SPACE_CHANGE_PENDING`, `PERFORMANCE_CHANGE`, `PASSTHRU_STATE_CHANGE`, `VISIBILITY_MASK_CHANGE`, `USER_PRESENCE_CHANGE`.
* `xrt_session_event_sink` — sink interface; the device set publishes events into it.
* `poll_events(*out_event)` on the session retrieves them in FIFO order.

## `xrt_tracking_origin`

`xrt_tracking.h`. Each tracked device hangs off an origin; the origin has a `type` (`XRT_TRACKING_TYPE_LIGHTHOUSE`, `_RGB`, `_EXTERNAL_SLAM`, `_OTHER`, `_NONE`, `_ATTACHABLE`) and an `initial_offset` pose. The space overseer uses origins to root the graph.

## `xrt_prober`

`xrt_prober.h`. Device discovery: VID/PID enumeration on USB/HID/Bluetooth, plus `xrt_auto_prober` for "always try" probers, plus *builders* (`xrt_builder` declared in `xrt_prober.h`). Builders combine multiple probed pieces into a coherent system (e.g. lighthouse builder pairs HMD + base stations + controllers). See `targets/common/target_builder_*.c` for concrete builders.

## `xrt_future`

`xrt_future.h` + `xrt_future_value.h`. Async result type used over IPC where OpenXR exposes async functions (e.g. anchors, plane detection). Stored server-side, polled client-side via `xrt_future_poll`.

## Refcount idiom

Several types (`xrt_swapchain`, `xrt_space`, `xrt_compositor_semaphore`, ...) use:

```c
struct xrt_reference reference;
void (*destroy)(struct foo *);
```

Helper inline (template repeated in each header): `xrt_foo_reference(*dst, src)` does the inc/dec and calls `destroy` when the count hits zero. There is no global GC; every owner does its own reference dance.

## Naming convention to recognise

* `xrt_*` — public interface.
* `u_*` — internal utility, header in `auxiliary/util/`.
* `m_*` — math (`auxiliary/math/`).
* `vk_*` — Vulkan helper (`auxiliary/vk/`).
* `os_*` — OS helper (`auxiliary/os/`).
* `comp_*` — compositor types and helpers.
* `ipc_*` — IPC.
* `oxr_*` — OpenXR state tracker.
* `t_*` — target / target-builder.

If you see a struct prefixed `xrt_`, it's part of the contract. Treat it as a versioned ABI boundary.
