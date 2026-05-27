# Compositor

Code: `openxr/src/xrt/compositor/`. The compositor is Vulkan-only and split into four layers:

```
state_tracker (oxr)           multi/ (one per client)             main/ (the real one)
   │                              ┌────────────────────────────┐    ┌──────────────────────┐
   ▼                              │ comp_multi_system          │    │ comp_compositor      │
client/ (Vulkan/GL/D3D wrapper)  ─►  forwards layers to ───────┴───►│ comp_renderer        │
                                                                    │ comp_target/window/sc│
                                                                    └──────────────────────┘
util/  (shared helpers used by main/null/multi/mock)
```

## `compositor/client/` — per-graphics-API client compositors

Each OpenXR client app speaks a chosen graphics API. Monado provides a thin compositor wrapper that:

1. Implements the `xrt_compositor` interface in terms of GAPI-native swapchain images.
2. Allocates / imports textures using the GAPI.
3. Translates them into native handles (Vulkan VkImage, Windows HANDLE, fd, GL texture) that the *native* compositor below can consume.

Files (paired `.c/.cpp` per API + a `_glue.c` providing the public `xrt_gfx_*_provider_create*` function):

| API | Files | Provider entry |
| --- | --- | --- |
| Vulkan | `comp_vk_client.{c,h}` + `comp_vk_glue.c` | `xrt_gfx_vk_provider_create` |
| OpenGL (GLX) | `comp_gl_client.{c,h}` + `comp_gl_glue.c` + `comp_gl_xlib_client.{c,h}` + `comp_gl_xlib_glue.c` | `xrt_gfx_xlib_provider_create` |
| OpenGL (WGL) | `comp_gl_win32_client.{c,h}` + `comp_gl_win32_glue.c` | `xrt_gfx_win32_provider_create` |
| OpenGL (EGL) | `comp_egl_client.{c,h}` + `comp_egl_client_glue` | `xrt_gfx_egl_provider_create` |
| OpenGL ES | `comp_gles_glue.c` | (Android entry) |
| D3D11 | `comp_d3d11_client.{cpp,h}` + `comp_d3d11_glue.c` | `xrt_gfx_d3d11_provider_create` |
| D3D12 | `comp_d3d12_client.{cpp,h}` + `comp_d3d12_glue.c` | `xrt_gfx_d3d12_provider_create` |
| Shared | `comp_d3d_common.{cpp,hpp}`, `comp_gl_memobj_swapchain.{c,h}`, `comp_gl_eglimage_swapchain.{c,h}` | — |

The state tracker picks one based on which graphics-binding struct the app passed to `XrSessionCreateInfo::next` and calls the matching `xrt_gfx_*_provider_create*` (see `state_trackers/oxr/oxr_session_gfx_*.c`).

## `compositor/main/` — the real compositor

Central type: `struct comp_compositor` (`comp_compositor.h:90`).

```c
struct comp_compositor {
    struct comp_base base;             // extends util/comp_base (which gives the xrt_compositor_native vtable)

    struct comp_settings settings;     // env-var / config-driven settings
    struct xrt_device *xdev;           // the HMD

    struct render_shaders shaders;     // SPIR-V loaded at init
    struct render_resources nr;        // VkPipelines, descriptor sets, samplers, etc.

    const struct comp_target_factory *target_factory;
    struct comp_target *target;        // owns the actual display

    struct comp_renderer *r;           // per-frame command building

    int64_t frame_interval_ns;
    struct { struct comp_frame waited; struct comp_frame rendering; } frame;

    struct chl_scratch scratch;        // scratch render targets
    ...
};
```

Notable files:
* `comp_compositor.{c,h}` — orchestration, `comp_main_create_system_compositor()` entry.
* `comp_renderer.{c,h}` — per-frame work: read submitted layers, dispatch the composite shaders, transition images, submit + present.
* `comp_target.h` + `comp_target_swapchain.{c,h}` — abstraction over "what we present to". Implemented by the windowing backends:
  * `comp_window_xcb.c` — X11/xcb (windowed).
  * `comp_window_wayland.c` — Wayland.
  * `comp_window_direct.{c,h}` + `comp_window_direct_randr.c` / `_nvidia.c` / `_wayland.c` — direct-mode DRM leases.
  * `comp_window_mswin.c` — Win32.
  * `comp_window_vk_display.c` — Vulkan `VK_KHR_display` direct.
  * `comp_window_android.c` — Android Surface.
  * `comp_window_peek.{c,h}` — the small "preview window" so devs can see what's on the HMD.
  * `comp_window_debug_image.c` — write frames to disk for testing.
* `comp_settings.{c,h}` — env vars: `XRT_COMPOSITOR_*`, log levels, ATW on/off.
* `comp_mirror_to_debug_gui.{c,h}` — mirror compositor output into the u_var debug GUI.

## `compositor/multi/` — multi-client wrapper

Files: `comp_multi_compositor.c`, `comp_multi_system.c`, `comp_multi_interface.h`, `comp_multi_private.h`.

The default `xrt_system_compositor` exposed by `monado-service` is `comp_multi_system`, not the bare `comp_compositor`. It:
* Holds a single underlying native compositor (the `main` one).
* Tracks attached sessions in a `comp_multi_compositor[N]` array.
* Each `comp_multi_compositor` is itself an `xrt_compositor_native`, so clients can't tell the difference.
* On each frame it picks the focused-session's primary layers + any overlay sessions' layers and forwards a single layer list to the underlying compositor.

Without this wrapper the runtime would be single-app like SteamVR's chaperone.

## `compositor/util/` — shared compositor helpers

* `comp_base.{c,h}` — implements `xrt_compositor_native` boilerplate so concrete compositors (main, null, mock) only fill in the GPU-specific bits.
* `comp_swapchain.{c,h}` — Vulkan swapchain *allocation* (not display swapchain — these are the OpenXR per-layer image arrays).
* `comp_sync.{c,h}` — fences and `comp_semaphore` wrappers (`xrt_compositor_semaphore`).
* `comp_layer_accum.{c,h}` — accumulates submitted layers within a frame so the renderer can iterate them.
* `comp_high_level_render.{c,h}` + `comp_high_level_scratch.{c,h}` — higher-level render orchestration / scratch image pool.
* `comp_render.h` + `comp_render_cs.c` (compute-shader path) + `comp_render_gfx.c` (graphics-pipeline path) — the actual layer composition.
* `comp_render_helpers.h` — small inline helpers.
* `comp_scratch.{c,h}` — image pool for temporary intermediates.
* `comp_vulkan.{c,h}` — Vulkan bring-up: device selection, queue, extensions, validation.

## `compositor/render/` and `compositor/shaders/`

* `render/render_interface.h` declares `render_shaders` (one entry per shader) and `render_resources`. The actual implementation files (`render_buffer.c`, `render_compute.c`, `render_distortion.c`, `render_resources.c`, `render_gfx_*.c`) live here.
* `shaders/` — GLSL sources (compiled to SPIR-V at build time) + precompiled `.spv` blobs (depending on cmake config).

## `compositor/null/` and `compositor/mock/`

* `null/` — fully implements `xrt_compositor_native` but never displays; useful for unit tests and headless servers (use `XRT_COMPOSITOR_NULL=1` or build with `XRT_MODULE_COMPOSITOR_NULL=ON`). `target_instance.c:113` chooses null if `XRT_COMPOSITOR_NULL` is set.
* `mock/` — test-double for unit tests; doesn't touch Vulkan at all.

## Frame pacing in one bullet list

* Pacing helper: `auxiliary/util/u_pacing_compositor.c` (real) + `u_pacing_compositor_fake.c` (deterministic, for tests).
* Inputs: GPU present timing (VK_KHR_present_timing / VK_GOOGLE_display_timing if available; otherwise fixed-rate from `frame_interval_ns`), CPU markers from `u_pc_mark_point(...)` in the compositor.
* Outputs: when the client should wake up, when GPU should scan out, predicted display time.
* The app's own pacing (CPU + GPU times to clamp `xrWaitFrame` predictions) is `u_pacing_app.c`.
* See upstream `openxr/doc/frame-pacing.md` for the canonical timing diagrams.
