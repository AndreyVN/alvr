# Architecture overview

## What Monado is

Monado is a complete OpenXR runtime. It implements `XR_KHR_*` and the OpenXR core API end-to-end: from the OpenXR loader entrypoint (`xrNegotiateLoaderRuntimeInterface`) down through a Vulkan compositor that drives the headset's display. It is **not** an SDK like `openvr/` — there is no thin client library; it is the whole stack.

## The three pieces every Monado runtime has

Regardless of build configuration, Monado is conceptually three things glued together:

1. **State tracker** (OpenXR API impl). One file per `xr*` family: `src/xrt/state_trackers/oxr/oxr_api_*.c`. Converts OpenXR handles + structs into calls on `xrt_*` interfaces.
2. **Devices** (`xrt_device`). One or more hardware drivers from `src/xrt/drivers/**`, selected and assembled by a *target builder* (`src/xrt/targets/common/target_builder_*.c`). Devices expose tracked poses, inputs, outputs (haptics), and HMD optics.
3. **Compositor**. Owns the display + Vulkan resources, paces frames, composites OpenXR layers, and presents. Lives in `src/xrt/compositor/main/` for the real one and `src/xrt/compositor/null/` for the headless one.

These three are stitched together via the abstract interfaces in `src/xrt/include/xrt/` (see [XRT_INTERFACES.md](XRT_INTERFACES.md)).

## Process topologies

The same source tree builds three substantially different deployments. The topology is a **build-time** decision driven by `XRT_FEATURE_IPC_CLIENT` / `XRT_FEATURE_SERVICE` / `XRT_FEATURE_OPENXR` cmake options.

### 1. Out-of-process (default on Linux desktop)

```
+-------------------- App process --------------------+      +------------- monado-service --------------+
| App  ->  OpenXR loader  ->  libopenxr_monado.so    |      | target_instance.c  -> xrt_instance         |
|                       (state_trackers/oxr +         |  ┌──>|   xrt_system (devices, space overseer)    |
|                        ipc_client + client-side     |  │   |   xrt_system_compositor (main compositor) |
|                        compositor shim)            ──┘   |   drivers/** (xrt_device impls)            |
+----------- IPC: AF_UNIX + shmem + fd-passing ------+      +-------------------------------------------+
```

* Implemented by: `targets/openxr/target.c` (when `XRT_FEATURE_IPC_CLIENT` is set, it forwards `xrt_instance_create` to `ipc_instance_create` — see `targets/openxr/target.c:24`) and `targets/service/main.c` for the server.
* IPC protocol generated from `ipc/shared/proto/*.json` by `ipc/shared/proto.py` (see [IPC.md](IPC.md)).

### 2. In-process (Windows desktop default, Android, `monado-sdl-test`)

```
+----------------------- Single process -----------------------+
| App -> OpenXR loader -> libopenxr_monado.dll                 |
|   state_trackers/oxr  --(directly)-->  target_instance.c     |
|   -> xrt_instance -> xrt_system -> drivers + compositor      |
+--------------------------------------------------------------+
```

* Implemented by linking `targets/common/target_instance.c` directly into the loader-facing library instead of going through `ipc_instance_create`. See `targets/common/target_instance.c:170` for `xrt_instance_create()` — it builds the prober + builders inline.

### 3. SteamVR driver topology

```
+--------- vrserver (SteamVR) ---------+
| Loads:  driver_monado.{dll,so}       |  -> ovrd_hmd_driver_impl(...)
|   state_trackers/steamvr_drv/**      |     (exposes vr::IServerTrackedDeviceProvider et al.)
|   -> xrt_instance via target_instance|  -> drivers/** + (no Monado compositor; SteamVR composites)
+--------------------------------------+
```

* Entry: `targets/steamvr_drv/main.c:25` exports `HmdDriverFactory` calling `ovrd_hmd_driver_impl` (defined in `state_trackers/steamvr_drv/**`).
* Note: this is **Monado-as-SteamVR-driver**, the opposite direction from the `drv_steamvr_lh` *driver*, which is "*read SteamVR Lighthouse devices into Monado*". Don't confuse the two — they are different chunks of code in different directories.

## Where the OpenXR loader fits

Monado does not contain the OpenXR loader. The loader (from Khronos) is what an OpenXR app links against; it reads `active_runtime.json` to find which `lib*runtime*.so/.dll` to load. Monado ships `targets/openxr/target.c` as that runtime, which exports `xrNegotiateLoaderRuntimeInterface` (`state_trackers/oxr/oxr_api_negotiate.c:46`). The negotiation handshake hands the loader a function pointer to `xrCreateInstance`, and from there `oxr_*` runs the show.

## Threading model in one diagram

```
   client app thread                     monado-service                     compositor
   ─────────────────                     ──────────────                     ──────────
   xrWaitFrame  ─ ipc ──────────►  predict next frame  ─────────────►  pacing helper
   xrBeginFrame ─ ipc ──────────►  begin frame slot
       (app renders)
   xrEndFrame   ─ ipc + shmem ──►  collect layers       ─────────────►  per-client thread submits
                                                                        to comp_renderer (Vulkan)
                                                                        which queues onto comp_target
                                                                        (the swapchain that owns the display)

   driver IMU thread (in service) ─► xrt_device push_tracked_pose ─► tracking origin / space overseer
   ipc_server_per_client_thread.c handles one socket per client; ipc_server_mainloop_{linux,windows,android}.c accepts new ones
```

Key files:
* Pacing: `auxiliary/util/u_pacing.h`, `u_pacing_compositor*.c`, `u_pacing_app.c`. See `doc/frame-pacing.md` in upstream Monado for the timing diagrams.
* Per-client thread: `ipc/server/ipc_server_per_client_thread.c`.
* Mainloop (accept new clients): `ipc/server/ipc_server_mainloop_{linux,windows,android}.{c,cpp}`.

## Vulkan everywhere

The compositor is Vulkan-only. Client OpenXR apps can present in D3D11/D3D12/OpenGL/Vulkan because `compositor/client/comp_*_client.{c,cpp}` translates each into Vulkan-importable native handles, then the actual swapchains are Vulkan images on the server side. This is the same pattern as SteamVR's "shared D3D textures imported into Vulkan", but baked into the compositor's `client/` layer. See [COMPOSITOR.md](COMPOSITOR.md).
