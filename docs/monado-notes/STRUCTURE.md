# Source tree map

Everything important lives under `openxr/src/`. The top of the repo is purely build glue: `CMakeLists.txt`, `vcpkg.json`, `build.gradle` + `gradle*` (Android), `flake.nix` (Nix), `.gitlab-ci.yml`, `cmake/`, and `scripts/` (formatting + IWYU). Monado's own docs are under `openxr/doc/` (Doxygen sources + Markdown — see e.g. `doc/ipc-design.md`, `doc/frame-pacing.md`, `doc/understanding-targets.md`).

## `openxr/src/external/`

Third-party C/C++ code that is statically linked. Of note:

| Folder | What |
| --- | --- |
| `openxr_includes/` | Khronos OpenXR headers (`openxr/openxr.h` etc.) |
| `openvr_includes/` | Valve's OpenVR driver-side headers, only used by `drv_steamvr_lh` and the `steamvr_drv` state tracker. |
| `cjson/`, `stb/`, `valve-file-vdf/`, `tinyceres/`, `flexkalman/`, `nanopb/` | small libs vendored in. |
| `imgui/`, `mermaid/` | dev tooling (debug GUI, doc diagrams). |
| `tracy/`, `renderdoc_api/` | profiling integration. |
| `cardboard/`, `android-jni-wrap/`, `jnipp/` | Android plumbing. |
| `vit_includes/` | "Visual-Inertial Tracker" plug-in interface (external SLAM trackers). |

## `openxr/src/xrt/` — the runtime itself

```
include/        abstract C interfaces (header-only). The contract.
auxiliary/      reusable helpers, NOT cross-process. (math, vk, ogl, util, os, tracking, vive, d3d, ...)
compositor/     the actual frame compositor + per-graphics-API client shims.
drivers/        one folder per supported HMD/controller/peripheral.
ipc/            cross-process glue when service+client split.
state_trackers/ public-API implementations (OpenXR, SteamVR-driver, GUI, prober).
targets/        the leaf "binaries" — small files that wire subsystems into an actual shipped artifact.
tracking/       higher-level optical/SLAM trackers (hand-tracking, mercury HT models).
```

### `include/xrt/` (header-only, the public interfaces)

Each `xrt_*.h` declares one interface struct plus inline forwarding helpers. The full list:

```
xrt_android.h           Android-specific extras (xrt_instance_android, lifecycle events)
xrt_byte_order.h        endian helpers
xrt_compiler.h          XRT_CHECK_RESULT, XRT_PRINTF_FORMAT, alignas helpers
xrt_compositor.h        xrt_compositor / xrt_compositor_native / xrt_swapchain / layer types
xrt_config.h            top-level config (pulls in xrt_config_have/build/os/...)
xrt_defines.h           xrt_pose, xrt_fov, xrt_vec3, xrt_quat, enums (xrt_result_t, xrt_device_name, ...)
xrt_deleters.hpp        C++ smart-pointer deleters for the C interfaces
xrt_device.h / .hpp     xrt_device + xrt_hmd_parts + xrt_view + input/output descriptors
xrt_documentation.h     doxygen groups only
xrt_frame.h             xrt_frame + xrt_frame_node + xrt_frame_sink + xrt_frame_context (camera frame DAG)
xrt_frameserver.h       xrt_fs (frame producer interface)
xrt_future.h / .h       xrt_future async value (used over IPC for async OpenXR functions)
xrt_gfx_*.h             per-graphics-API helpers (vk, gl, gles, egl, d3d11, d3d12, win32, xlib)
xrt_handles.h           graphics handle types (DMABUF fd, HANDLE, ID3D11Texture2D*)
xrt_instance.h          xrt_instance (root of the world). Single per process.
xrt_limits.h            XRT_MAX_VIEWS, XRT_MAX_LAYERS, XRT_MAX_SWAPCHAIN_FORMATS
xrt_openxr_includes.h   the safe wrapper that pulls openxr.h
xrt_plane_detector.h    XR_EXT_plane_detection support
xrt_prober.h            xrt_prober (device discovery)
xrt_results.h           xrt_result_t enumeration
xrt_session.h           xrt_session + xrt_session_event* + xrt_session_event_sink
xrt_settings.h          driver-settings sidecar (loaded from u_config_json)
xrt_space.h             xrt_space + xrt_space_overseer (scene graph)
xrt_system.h            xrt_system / xrt_system_devices / xrt_system_compositor / xrt_system_roles
xrt_tracking.h          xrt_tracking_origin / xrt_tracking_factory / xrt_tracked_*
xrt_visibility_mask.h   hidden-area mesh interface
xrt_vulkan_includes.h   safe wrapper for vulkan.h
xrt_windows.h           safe wrapper for windows.h
```

### `auxiliary/`

Internal-only libraries usable from drivers, compositor, ipc, state trackers. The most heavily-used ones:

```
util/   ~130 files — u_device, u_logging, u_pacing(_app|_compositor), u_system, u_space_overseer,
        u_config_json, u_sink_*, u_var (debug GUI introspection), u_worker (jobs), u_time,
        u_threading, u_handles, u_hashmap, u_template_historybuf (lock-free ring), u_metrics,
        u_trace_marker (Perfetto/Tracy bridging), u_pretty_print, u_misc.
math/   linear-algebra helpers (m_vec3, m_quat, m_relation_history, m_filter_one_euro, m_imu_3dof, m_optics).
vk/     Vulkan helpers (vk_helpers.{c,h}, vk_image_allocator, vk_cmd, vk_sync_objects, vk_surface_info,
        vk_image_readback_to_xf_pool). All compositor + many drivers use these.
ogl/    GL helpers — only for compositor/client/comp_gl_client.
os/     thin OS abstraction (os_time, os_threading, os_hid, os_ble).
d3d/    D3D11/12 helpers, for compositor/client/* and the OpenXR D3D session bindings.
gstreamer/  gst frame producer (debug + recording).
bindings/   generated OpenXR action binding tables (from .json).
vive/   shared code for Valve Index / HTC Vive / Lighthouse stack (vive_config, vive_calibration,
        vive_bindings, vive_builder, vive_poses, vive_tweaks).
android/, android_cardboard/, tracking/ → smaller adjuncts.
```

### `compositor/`

```
main/    The real compositor. comp_compositor.{c,h} is the central type. comp_target/swapchain/window_*
         drive the display. comp_renderer is the per-frame submit. comp_settings is configuration.
client/  Per-graphics-API "client compositor" wrappers — what an OpenXR app talks to before IPC.
         One file pair per API: comp_vk_client / comp_gl_client / comp_egl_client /
         comp_gl_win32_client / comp_gl_xlib_client / comp_d3d11_client.cpp / comp_d3d12_client.cpp.
         The "_glue" .c files provide the xrt_gfx_*_provider_create() entrypoints.
multi/   The multi-client wrapper. comp_multi_system is a xrt_system_compositor that owns one
         comp_multi_compositor per client and arbitrates their layers into the underlying native compositor.
util/    Compositor helpers reusable by main/null/mock. comp_base (base class for native compositors),
         comp_swapchain (Vulkan swapchain allocation), comp_sync (semaphores / fences), comp_render_*
         (the actual GFX + CS render passes that read submitted layers and write to the target).
render/  GLSL shader sources + Vulkan render-pass scaffolding.
shaders/ Pre-built SPIR-V / source for distortion + composite shaders.
mock/    Test double — implements xrt_compositor without doing anything real.
null/    Headless compositor (no display). Used in tests + headless servers.
```

### `drivers/`

Each subdirectory is one peripheral or driver family. Builds are gated by `XRT_BUILD_DRIVER_*` cmake options, and the corresponding interface header is referenced from `targets/common/target_lists.c`. Notable:

```
android/        Android sensor backend
arduino/        Custom Arduino IMU
blubur_s1/      Blubur S1
daydream/       Google Daydream View controller
depthai/        Luxonis DepthAI camera (for tracking)
euroc/          EuRoC dataset replay (SLAM testing)
hdk/            OSVR HDK
ht/             Hand-tracking (computer vision)
ht_ctrl_emu/    Hand-tracking → controller emulation
hydra/          Razer Hydra
illixr/         ILLIXR research integration
multi_wrapper/  Combine multiple devices into one logical device
north_star/     Leap Motion North Star HMD
ohmd/           OpenHMD wrapper
opengloves/     OpenGloves haptic gloves
psmv/           PS Move
pssense/        PSVR2 Sense controllers
psvr/           PS VR HMD (PSVR1)
qwerty/         Keyboard-emulated HMD/controllers — debug tool
realsense/      Intel RealSense (for tracking)
remote/         "Remote" driver: HMD + controllers driven by an external UDP/TCP debugger feeding poses
rift/, rift_s/  Oculus DK2 / Rift S (USB native drivers)
rokid/          Rokid Air/Max smart glasses
sample/         Hello-world sample driver template
simula/         SimulaVR HMD
simulated/      Built-in software HMD (wobble/rotate/stationary) + controllers
solarxr/        SolarXR full-body tracking
steamvr_lh/     **Wrap an installed SteamVR Lighthouse driver and present its devices as xrt_devices**
survive/        libsurvive Lighthouse implementation (no SteamVR install needed)
twrap/          xrTracking wrapper helpers
ultraleap_v2/v5 Ultraleap hand-tracking
v4l2/           Video for Linux frame source
vf/             Video file frame source
vive/           Vive / Valve Index native USB driver
vp2/            (newer Vive prefix?)
wmr/            Windows Mixed Reality HMD + controllers (HP G2, etc.)
xreal_air/      Xreal Air glasses
```

### `ipc/`

```
client/  ipc_client_compositor / ipc_client_session / ipc_client_device / ipc_client_hmd /
         ipc_client_space_overseer / ipc_client_system_devices / ipc_client_system /
         ipc_client_instance / ipc_client_xdev — each is a proxy implementation of one xrt_* interface
         that forwards calls over the message channel.
server/  ipc_server_process (main loop, init), ipc_server_handler (RPC dispatch),
         ipc_server_per_client_thread (one per attached client), ipc_server_mainloop_{linux,windows,android}
         (accept logic), ipc_server.h is the central state.
shared/  ipc_protocol.h (shared structs incl. ipc_shared_memory), ipc_message_channel(_unix|_windows),
         ipc_shmem, ipc_utils. proto.py + proto/*.json + proto.schema.json + ipcproto/* generate
         ipc_*_generated.{h,c} at build time.
android/ Android-only Binder bridging.
```

### `state_trackers/`

```
oxr/        The OpenXR API implementation. oxr_api_*.c maps every OpenXR entrypoint family,
            oxr_objects.h defines the oxr_instance/oxr_session/oxr_space/oxr_swapchain/... handles,
            oxr_session_frame_end.c is the layer-submit big one. oxr_session_gfx_* per GAPI.
steamvr_drv/ Implementation of the SteamVR vr::IServerTrackedDeviceProvider + per-device classes
            (ovrd_hmd_driver_impl) so Monado can be loaded **as** a SteamVR driver. Pairs with
            targets/steamvr_drv.
gui/        Optional in-process debug GUI (dear imgui).
prober/     Hardware-discovery support code used by xrt_prober implementations.
```

### `targets/`

```
android_common/  Android wiring shared between APK targets.
cli/             monado-cli — diagnostic CLI tool.
common/          target_lists.c (which builders/drivers are compiled in), target_instance.c
                 (the in-process xrt_instance), target_instance_no_comp.c, and one
                 target_builder_*.c per builder family.
ctl/             monado-ctl — control protocol for the running service.
gui/             monado-gui — standalone debug GUI.
libmonado/       libmonado — C API exposing service inspection / control to external tools.
openxr/          libopenxr_monado — the runtime DLL/SO the OpenXR loader loads.
openxr_android/  Android-specific OpenXR runtime target.
sdl_test/        monado-sdl-test — minimal Vulkan/SDL app that drives the compositor for tests.
service/         monado-service — the out-of-process server.
service-lib/     Same as `service` but built as a library (for embedding).
steamvr_drv/     driver_monado — the SteamVR driver factory entry point.
```
