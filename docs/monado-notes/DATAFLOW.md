# Data and control flow

This page traces what happens at runtime, end-to-end. The interface contract is in [XRT_INTERFACES.md](XRT_INTERFACES.md); here we follow the calls.

## 1. Startup

```
App  ─►  OpenXR loader  ─►  libopenxr_monado{.dll,.so}
                                │
                                ▼
                       xrNegotiateLoaderRuntimeInterface
                           (state_trackers/oxr/oxr_api_negotiate.c)
                                │
                                ▼
                       xrCreateInstance ──► oxr_xrCreateInstance ──► oxr_instance_create
                                │                                    (oxr_instance.c)
                                ▼
                       xrt_instance_create(&ii, &xinst)
                              │
       ┌──────────────────────┴──────────────────────┐
       │                                             │
       │  In-process target:                         │  Out-of-process target:
       │  targets/openxr/target.c (else branch)      │  targets/openxr/target.c:24
       │  ─► targets/common/target_instance.c:170    │  ─► ipc/client/ipc_client_instance.c
       │      xrt_prober_create_with_lists(...)      │      open AF_UNIX socket to monado-service
       │      builders/auto_probers/entry_lists      │      receive xrt_system + shmem region
       │                                             │
       ▼                                             ▼
   xrt_instance with create_system/get_prober populated
```

The state tracker then calls `xrt_instance_create_system(&xsys, &xsysd, &xso, &xsysc)` which triggers:
* In-process: prober probes hardware, the first builder that says "yes I can" assembles devices, then `comp_main_create_system_compositor(head, NULL, NULL, &xsysc)` builds the real compositor (see `targets/common/target_instance.c:125`).
* Out-of-process: client just receives handles; the real probing happened in `monado-service` at *its* startup.

## 2. Session creation

```
xrCreateSession ─► oxr_session_create
                     │  picks the graphics binding (XR_KHR_vulkan_enable, _opengl_enable, _d3d11_enable...)
                     │  calls oxr_session_populate_<gapi> in oxr_session_gfx_<gapi>.c
                     ▼
            xrt_system_create_session(xsys, &xsi, &xs, &xcn)
                     │  -> server-side: ipc_handle_session_create (ipc_server_handler.c)
                     │     -> xrt_system_create_session on the multi-system-compositor
                     ▼
            Per-API client compositor wraps `xcn`:
               xrt_gfx_vk_provider_create  / xrt_gfx_gl_provider_create
               xrt_gfx_d3d11_provider_create / xrt_gfx_d3d12_provider_create
               (compositor/client/comp_*_glue.c)
```

After this point the OpenXR app sees an `XrSession`, the state tracker holds a `oxr_session` whose `compositor` field is the **client compositor** (e.g. `comp_vk_client`), and the client compositor talks to either the in-process native compositor or the IPC proxy `ipc_client_compositor`.

## 3. Frame lifecycle (the hot path)

OpenXR exposes `xrWaitFrame`/`xrBeginFrame`/`xrEndFrame`. Monado pages this to:

```
   Client app                                Service / compositor
   ──────────                                ────────────────────
1. xrWaitFrame
     oxr_session_frame_wait
       comp->wait_frame(comp, &frame_id, &predicted_display_time_ns, ...)
         → IPC: compositor_predict_frame + compositor_wait_woke
                       │
                       ▼
                  u_pacing_compositor.predict
                       (auxiliary/util/u_pacing_compositor.c)
                       → returns next wake-up + desired present time
                       → app pacer (u_pacing_app) tracks app's CPU/GPU times

2. xrBeginFrame
     comp->begin_frame(comp, frame_id) → IPC: compositor_begin_frame
        marks U_TIMING_POINT_BEGIN on the pacer.

   ── App now renders into its swapchain images ──

3. xrEndFrame  (the big one)
     oxr_session_frame_end (state_trackers/oxr/oxr_session_frame_end.c)
       For each XrCompositionLayer*:
         translate XR layer → xrt_layer_data
         comp->layer_<type>(comp, ..., &xrt_layer_data, swapchains)
       comp->layer_commit(comp, frame_id, sync_handle)
          → IPC: compositor_layer_sync
              (slot_id picked by the client; ipc_layer_slot is in shmem)

   Server                                       Compositor render thread
   ──────                                       ───────────────────────
   ipc_server_handler.c                         comp_renderer (compositor/main/comp_renderer.c)
   pulls ipc_layer_slot[slot_id] out of shmem   composes via comp_render_cs / comp_render_gfx
   passes layers to comp_multi_compositor       (compositor/util/comp_render_*.c)
   which forwards to comp_compositor            present via comp_target_swapchain
                                                  (KHR_swapchain / direct mode / Wayland / xcb / mswin)
```

Key timing files:
* `auxiliary/util/u_pacing.h` (interface) + `u_pacing_compositor.c` + `u_pacing_app.c` (implementations).
* `compositor/main/comp_compositor.c` for the orchestration.
* `compositor/main/comp_renderer.c` for the GPU side (the main type is `struct comp_renderer` at line 100 of that file).
* `state_trackers/oxr/oxr_frame_sync.{c,h}` — coordinates `xrWaitFrame`/`xrBeginFrame`/`xrEndFrame` ordering on the client side.

Per-frame layer payload sits in `ipc_layer_slot`:
```c
struct ipc_layer_slot {                       // ipc/shared/ipc_protocol.h:169
    struct xrt_layer_frame_data data;
    uint32_t layer_count;
    struct ipc_layer_entry layers[IPC_MAX_LAYERS];
};

struct ipc_layer_entry {                      // ipc/shared/ipc_protocol.h:145
    uint32_t xdev_id;
    uint32_t swapchain_ids[XRT_MAX_VIEWS * 2];
    struct xrt_layer_data data;               // type, pose, sub-image, blend
};
```
`IPC_MAX_SLOTS == 128` so the client can keep multiple in-flight frames pending. Slots are picked round-robin in `ipc_client_compositor.c`.

## 4. Tracking flow (driver → state tracker)

```
   driver thread (in service process)            consumer
   ─────────────────────────────────              ────────
   USB/Bluetooth/sensor reader  ──► xrt_device.update_inputs / get_tracked_pose
                                       │
                                       ├─► xrt_tracking_origin (frame of reference)
                                       │
   IMU samples ──► m_imu_3dof / m_relation_history ──► fused pose
                                       │
                                       ▼
                       u_space_overseer (auxiliary/util/u_space_overseer.c)
                                       │
                                       ▼  via IPC: ipc_call_space_locate_space ... or direct in-process
                                  oxr_space.c   (state_trackers/oxr/)
                                       │
                                       ▼
                                  XrSpaceLocation -> app
```

`xrt_input` slots on a device are simply written to from the driver's reader thread; `oxr_input.c` polls them on `xrSyncActions`. Haptics goes the other way: app → `oxr_input.c` → `xrt_device.set_output(name=OUTPUT_HAPTIC, value)`.

## 5. Session-event flow

Events flow **service → client → app**:

* A driver / system calls into the `xrt_session_event_sink` for the broadcast group. Multi-session implementations (`u_system_helpers.c`, `u_session.c`) clone the event into a queue per `xrt_session`.
* The state tracker calls `xrt_session.poll_events(&ev)` inside `xrPollEvent`, translates into the right `XrEventData*`, and hands it to the app.

Out-of-process: the IPC `session_poll_events` RPC (`ipc/shared/proto/50-session.json`) shuttles the union across the socket boundary.

## 6. Multi-client arbitration

`compositor/multi/comp_multi_system.c` wraps the *native* (main) compositor with one `comp_multi_compositor` per attached client. Each client's frame state (in-flight slot, layer list, swapchain bindings) is owned by its `comp_multi_compositor`. Multi-system arbitrates:
* primary vs overlay sessions,
* focus and visibility (which session is `XR_SESSION_STATE_FOCUSED`),
* layer ordering (`xrt_session_info.z_order`),
* the wake-up cadence is driven by the *single* underlying compositor's pacer.

Without `multi/`, only one client could ever connect.

## 7. Shutdown order

Destruction is strict and bottom-up; the helpers enforce it:

```
xrt_compositor_destroy (per session)
   └──► swapchain refs drop, semaphore refs drop
xrt_session_destroy
xrt_system_compositor_destroy   ← multi/ wrapper waits for all sessions
xrt_space_overseer_destroy
xrt_system_devices_destroy      ← devices get destroy() in xdevs[] order
xrt_system_destroy
xrt_instance_destroy
```

If you bypass this (e.g. destroy a device while a session still holds a swapchain that refers to its tracking origin), you'll get a use-after-free. The IPC server does this dance per-client in `ipc_server_per_client_thread.c` on disconnect.
