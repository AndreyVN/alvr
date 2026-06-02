# Phase 7 Slice 2 — Vulkan layer composition (non-projection rasterisation)

Pre-implementation plan for "compositing quad / cylinder / equirect / cube / passthrough layers into the projection image before encoding," called out in [`NEXT_STEPS.md`](NEXT_STEPS.md) §"Phase 7 — stretch". Read that file's Slice 1 entry first — the per-frame layer-type histogram (ABI v3) is already collecting and tells us empirically what kinds of overlays apps submit, which is what motivates this slice now.

> **STATUS 2026-06-02 — ✅ CODE-COMPLETE for the supported layer types; only Slice 2d (verification) is genuinely open.** Slices 2b–2c.1 landed (the squasher reuse). Crucially, the result is broader than the ladder below implies: `comp_alvr_layer_commit` passes the **full, unfiltered** layer list to `compose_via_squasher` → `comp_render_gfx_dispatch` → `comp_render_gfx_layers`, whose layer-type switch already composites `PROJECTION`/`PROJECTION_DEPTH`, `QUAD`, `CYLINDER`, and `EQUIRECT2` into the per-view scratch the encoder reads. So overlay rasterisation for those types is **already happening** — there is no remaining "replace the drop loop with dispatch" work; the `n_quad`/`n_cylinder` loop in `layer_commit` is now telemetry-only. **Not supported** (absent from Monado's gfx squasher switch, so `comp_main` doesn't do them either): `CUBE`, `EQUIRECT1`, `PASSTHROUGH` — add only if the `oxr_layer_types` histogram shows real apps submit them; PASSTHROUGH is a server-side non-goal (resolved on the headset after decode). **Open work = Slice 2d only:** confirm with a real overlay-submitting app that quad/cylinder pixels actually land in the encoded output (the `oxr_overlay_smoke` run verified the state machine + no-crash but predated 2c.1 unblocking the dispatch), and check quad placement for STAGE/LOCAL-space overlays (`compose_via_squasher` feeds the projection view poses as both `world_poses` and `eye_poses` — `comp_alvr.c:1161` — fine for view-locked quads, unverified for space-locked). App/probe-gated. The sub-slice ladder below is preserved as the historical record; check the inline status notes.

## Constraint (carried over from Phase 3.0)

OpenVR mode behaviour stays bit-identical. Slice 2 only touches Monado-side code (`openxr/src/xrt/compositor/alvr/comp_alvr.c`). Nothing in `alvr/server_openvr/` or `alvr_server_core` changes.

## Surprise finding — Monado already ships the squasher

The original NEXT_STEPS sketch (line 155) imagined writing a "textured-quad pipeline" from scratch. That's not the right starting point — Monado's main compositor (`compositor/main/`) already includes a complete layer squasher that handles every XRT layer type we care about, and the squasher is factored out into reusable helpers in `compositor/util/`:

| Helper | What it gives us |
| --- | --- |
| `comp_scratch.{h,c}` | `struct comp_scratch_single_images` — a 4-deep ring of per-view VkImages with `xrt_image_native` handles (Win32 NT / DMABUF fds) and sample/storage views, plus debug-UI integration. |
| `comp_high_level_scratch.{h,c}` | `struct chl_scratch` — wraps the per-view scratch ring with shared render pass + per-image GFX render targets. |
| `comp_render.h` | `struct comp_render_dispatch_data` — input shape feeding both render paths. `comp_render_initial_init` + `comp_render_dispatch_add_squash_view` set up the squash-only case (no distortion target, since the headset side already does distortion). |
| `comp_render_gfx.c` / `comp_render_cs.c` | Two parallel implementations of the squasher — graphics shaders or compute. Either does projection + quad + cylinder + equirect + cube + passthrough. |
| `comp_high_level_render.{h,c}` | `struct chl_frame_state` — per-frame wrapper that wires scratch state + dispatch data + a `render_compute` together. |

Translation: we don't write a quad shader, a cylinder shader, or a layer-blend pipeline. We hand Monado's existing squasher a list of layers + a scratch image per view and read the squashed image back via the same `xrt_image_native` handle the bridge already consumes.

The native scratch image is created with the platform's external-memory bit (Win32 `VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32_BIT` / Linux DMABUF), so the existing bridge → encoder handoff in `alvr_oxr_submit_layers` doesn't change shape. Only the *source* of `AlvrOxrLayer.image_handles[v]` shifts from "first projection swapchain's native handle" to "this frame's scratch image's native handle."

## Decision (architecture)

**Use Monado's `chl_frame_state` + `chl_scratch`. Do NOT roll a custom rasteriser.**

| | Question | Decision | Why |
| --- | --- | --- | --- |
| L1 | Roll custom quad shader vs. reuse Monado squasher | Reuse | Custom = 3–5 days + shader maintenance, reuse = ~1–2 days, ships every layer type at once, no shader risk. |
| L2 | GFX vs CS path | **GFX** (`comp_render_gfx`) | `comp_main` defaults to CS for performance, but GFX has fewer device-extension prereqs and the squash-only case (no distortion target) is the simpler GFX shape. We can switch to CS in a later slice if measurements justify it. |
| L3 | Scratch image format | Match swapchain default (`VK_FORMAT_R8G8B8A8_SRGB` or whatever `comp_vulkan_formats_check` reports for the rendering color format) | The encoder will downconvert to NV12 anyway; matching the swapchain saves the squasher a colorspace conversion. |
| L4 | Where to place scratch lifetime | `struct comp_alvr_compositor` field, init in `compositor_init_vulkan`, destroy in `comp_alvr_destroy` | Same lifetime as the Vulkan bundle — scratch images can't outlive the device. |
| L5 | When to skip squashing (fast path) | When there are zero non-projection layers AND single projection layer AND fits the bridge ABI shape today | Preserves the existing zero-overhead path; squasher only runs when there's actual composition to do. |

## Sub-slice ladder

Same atomic-slice pattern as Phase 3.0. Each sub-slice ends with a clean compile + boot-clean check (Gates A + B from `SMOKE_TESTS.md`) and an explicit verification ceiling.

### Slice 2a — scope doc (this file) — ✅ LANDED

Establishes the design. ~150 lines. No code, one commit.

### Slice 2b — scratch lifecycle — ✅ LANDED 2026-05-23

Add a `chl_scratch` field to `struct comp_alvr_compositor`. Call `chl_scratch_init` / `chl_scratch_ensure_for_view` after `compositor_init_vulkan` succeeds, and `chl_scratch_fini` from `comp_alvr_destroy` before the device teardown. No render dispatch yet; the existing pack/drop logic in `comp_alvr_layer_commit` is unchanged.

**Verification:** Gate A (`cargo xtask build-openxr-runtime --enable-alvr-driver` clean) + Gate B (`monado-service.exe` still emits `ALVR compositor ready` and the existing device list). Memory consumption on boot grows by ~4 × view × scratch-size; that's the only observable.

### Slice 2c — squasher dispatch in layer_commit — ✅ LANDED 2026-05-23 (+ 2c.1 dummy-target fix 2026-05-25)

Replaced the drop-and-histogram loop with: `compose_via_squasher` builds the dispatch from the full `cla->layers` list, runs `chl_frame_state_init` → `comp_render_gfx_dispatch` → submit, and packs the *scratch* image's native handle into `AlvrOxrLayer.image_handles[v]`. Histogram counts stay (telemetry-only now). **2c.1 (2026-05-25):** the dispatch hard-asserts on `target.initialized`, so a throwaway 1×1 dummy distortion target satisfies it (the squash output the encoder reads is the per-view scratch, not that target). **Net effect** (see top banner): because the dispatch gets the unfiltered layer list and `comp_render_gfx_layers` switches on type, QUAD/CYLINDER/EQUIRECT2 are composited too — not just projection. The skip-squash fast path was later dropped (alvr `c5f6c383`, 2026-05-28): the squasher runs on projection-only frames too, because `pack_projection_layer` read back `handle=0` and only the squasher overwrite produces valid native handles.

**Verification:**
- Gate A clean + Gate B clean (still boots, ALVR compositor ready).
- Manual: `monado-service.exe` boot log shows `[chl_scratch_ensure_for_view] ...` or equivalent on first frame.
- End-to-end: hardware-gated (see 2d).

### Slice 2d — verify quad/cylinder rasterisation — 🔶 THE ONE OPEN ITEM (app/probe-gated)

> The 2026-05-23 `oxr_overlay_smoke` run (below) verified the OpenXR state machine + no-crash, but it **predated 2c.1** (2026-05-25) unblocking the dispatch — so the composite output was never actually confirmed to contain the overlay. This visual/empirical check is the only substantive work left on Slice 2. Per the verification ceiling below it's runnable on this AMD host without NVENC/headset (the composite is a Vulkan output), but needs an overlay-submitting client + a scratch-image readback probe (the kind swept after the AK-black investigation).

Run a known overlay-using OpenXR client (hello_xr `--quad-layer`, the Monado `compositor_demo`, or a real game with HUD overlays) against `monado-service.exe`. Confirm:
- `alvr_oxr_report_layer_types` shows non-zero quad/cylinder counts on the frames where the app submits them.
- The pre-encoder image (sampled via `u_native_images_debug`) contains the composited overlay, not just the bare projection.
- No new validation errors in the boot log.

**Verification ceiling:** requires a real OpenXR client driving overlay-heavy content. Smoke-able on this host (no NVENC, no headset needed for the composite output itself — only for the encoder body which is Slice 3.3 territory). The downstream encoder path stays a separate gate.

### Slice 2e (stretch) — switch GFX → CS

Optional. If the GFX path on stack-allocated `render_gfx` adds measurable per-frame cost vs `comp_main`'s CS path, swap dispatch. Pure call-site change — same `comp_render_dispatch_data`, same scratch image output.

## Risk register

| Risk | Mitigation |
| --- | --- |
| `chl_scratch` requires `render_resources` (the squasher's global pipeline state) — we don't have one in `comp_alvr` today | `comp_main_create_system_compositor` walks the setup; mirror the minimal subset (`render_resources_init` + `render_gfx_render_pass_init`). One-time init in `compositor_init_vulkan`. |
| Scratch image format mismatch with the encoder's expected NV12 input | Encoder side is Slice 3.3 territory; reading from an RGBA scratch image and doing the colour conversion at NVENC submit is the standard pattern. Doesn't affect Slice 2. |
| Squasher's per-view dispatch requires a real `xrt_pose` for the world / eye relations — we feed it the same view-params the projection layer carries today | Use the layer's first view pose for both `world_pose_scanout_*` and `eye_pose` (no reprojection yet — same approximation as the current pack). |
| `comp_render_gfx` enforces a `do_timewarp` flag that we don't have a use for | Pass `do_timewarp = false` — squasher path supports it explicitly. |
| Build break on Linux / missing extension on iGPU host | The squasher already runs on the same Vulkan extensions `comp_alvr` already requires (external-memory + external-semaphore + KHR_dedicated_allocation). No new device-extension prereq. |
| Validation-layer noise from re-binding scratch images across frames | `chl_scratch` was designed for this; the 4-deep ring + `comp_scratch_indices` covers per-frame layout transitions. |

## Files to read before Slice 2b

In order:

1. `openxr/src/xrt/compositor/util/comp_scratch.h` — the per-view scratch ring API.
2. `openxr/src/xrt/compositor/util/comp_high_level_scratch.h` — `chl_scratch` wrapper.
3. `openxr/src/xrt/compositor/util/comp_high_level_render.h` — `chl_frame_state` per-frame wrapper.
4. `openxr/src/xrt/compositor/util/comp_render.h` — `comp_render_dispatch_data` input shape.
5. `openxr/src/xrt/compositor/main/comp_renderer.c` — the canonical example of putting them all together; the call site we're mirroring.
6. `openxr/src/xrt/compositor/alvr/comp_alvr.c` — the file we're modifying.
