# OpenXR-mode integration — next steps

Pick-up doc for future sessions. Pairs with the master plan at [`/openxr-migration.md`](../../openxr-migration.md). When the two disagree, the migration plan is authoritative and this file is wrong.

## Where we are

**Phases 0–2 scaffolding is in place and compiles clean.** OpenVR mode is unchanged and remains the default. To verify the state of the tree at any point:

```sh
cargo check -p alvr_server_openxr -p alvr_session -p alvr_dashboard -p alvr_xtask
```

What landed (this is the "starting state" for future work):

| Area | Files | Status |
| --- | --- | --- |
| New Rust crate | `alvr/server_openxr/{Cargo.toml, build.rs, src/lib.rs, include/alvr_runtime_bridge.h}` | Compiles. All 10 bridge fns return `NotImplemented`. |
| Workspace | `Cargo.toml` (added `alvr_server_openxr`) | Done. |
| Settings schema | `alvr/session/src/settings.rs` (`RuntimeMode` enum + `ExtraConfig.runtime` field) | Default `Steamvr`. Dashboard auto-renders the toggle. |
| Dashboard launcher | `alvr/dashboard/src/steamvr_launcher/mod.rs` | When `runtime == Openxr`, `launch_steamvr` warns and returns instead of starting SteamVR. |
| xtask | `alvr/xtask/src/build_openxr.rs` + `main.rs` | New subcommand `cargo xtask build-openxr-runtime [--release] [--enable-alvr-driver]`. |
| Monado driver | `openxr/src/xrt/drivers/alvr/{alvr_interface.h, alvr_internal.h, alvr_hub.c, alvr_hmd.c, alvr_controller.c, CMakeLists.txt}` | Gated by `XRT_BUILD_DRIVER_ALVR=OFF`. Returns identity poses (placeholder). |
| Monado compositor | `openxr/src/xrt/compositor/alvr/{comp_alvr.h, comp_alvr.c, CMakeLists.txt}` | Gated by `XRT_FEATURE_COMP_ALVR=OFF`. Stub returns `XRT_ERROR_NOT_IMPLEMENTED`. |
| Monado target builder | `openxr/src/xrt/targets/common/target_builder_alvr.c` | Calls `alvr_create_devices`. Not yet inserted into `target_lists.c`. |
| Submodule prep | `.gitmodules` (commented-out block), `docs/monado-notes/SUBMODULE_PIN.md` | Not yet converted. |
| Docs | `docs/monado-notes/*` (moved out of `openxr/`) | Done. |

## Known issues to resolve before Phase 3

These block real progress; fix them in this order.

### 1. The bridge header — RESOLVED (2026-05-20)

Root cause was cbindgen 0.27's syn version not parsing Rust 2024 syntax (`#[unsafe(no_mangle)]`, `unsafe extern "C" fn`); it silently dropped the entire C surface. Fixed by bumping the build-dep to `cbindgen = "0.29"` and adding `enumeration.rename_variants = QualifiedScreamingSnakeCase` to the cbindgen `Config` in `alvr/server_openxr/build.rs` (matches the convention in `alvr/server_core/cbindgen.toml`).

Regeneration stays opt-in via `ALVR_REGENERATE_BRIDGE_HEADER=1 cargo build -p alvr_server_openxr` so unrelated `cargo check` runs don't churn the header on disk.

Driver C consumers also updated to match: `AlvrOxrSide side` (typedef, 1 byte — was `enum AlvrOxrSide`, 4 bytes, ABI mismatch with `#[repr(u8)]`); `ALVR_OXR_RESULT_OK` (was `ALVR_OXR_OK`); `ALVR_OXR_SIDE_LEFT/RIGHT` now actually emitted by cbindgen.

Note: CMakeLists.txt only checks file existence, not content. A future hardening would be to also check the header isn't a bare include-guard shell.

### 2. The Monado-side patches conflict with the submodule plan — PREPPED (2026-05-20)

Path forward decided: **Option A (fork branch)**. Local prep artefacts now live in `docs/monado-notes/`:

- `PHASE2_MANIFEST.md` — 10 Phase 2 files inventoried; upstream baseline pinned to Monado 25.1.0; lists upstream-file edits (parent CMakeLists subdir registrations, `target_lists.c`, top-level options) that must be authored on the fork branch alongside the patch.
- `phase2_alvr.patch` — mailbox-format patch with all 10 additive files. Verified `git am`-applicable to an empty repo. Apply onto `v25.1.0` checkout.
- `convert_to_submodule.sh` — annotated conversion script. Edit `FORK_URL`, then run from the ALVR repo root.
- `SUBMODULE_PIN.md` — full procedure pointing at the above.

What's still on the maintainer:
1. Create `alvr-org/monado` fork (or equivalent) on GitLab.
2. Build the `alvr` branch from `v25.1.0`, apply `phase2_alvr.patch`, hand-author the upstream-file edits in `PHASE2_MANIFEST.md`, push.
3. Edit `FORK_URL` in `convert_to_submodule.sh` and run it.

Until step 3 is run, do not run `git submodule add openxr` by hand — it will erase the Phase 2 files.

### 3. ~~`target_builder_alvr.c` is not yet wired into Monado's target list~~ — RESOLVED in fork commit `c2ee5dffc` (the same one that converted `openxr/` to a real submodule). `target_lists.c` includes `alvr_interface.h` under `XRT_BUILD_DRIVER_ALVR` and slots `t_builder_alvr_create` at the top of `target_builder_list[]`. `target_instance.c` selects `comp_alvr` over `comp_main`/`comp_null` when built in (fork commit landing 2026-05-21 alongside this doc update).

## Phase 3 — the next real chunk (5–10 days)

Order matters because of the encoder refactor blocker.

### 3.0 (blocker) — Refactor encoder out of `alvr/server_openvr/cpp/`

Today the NVENC/AMF/VPL paths in `alvr/server_openvr/cpp/` are stitched into SteamVR's `IVRDriverDirectModeComponent` interface. Phase 3 needs them runtime-agnostic.

Suggested shape:
- Extract a `class Encoder` (or similar) whose constructor takes "input: shared VkImage handle + sync object" and emits encoded packets.
- Both `alvr_server_openvr` and `alvr_server_openxr` instantiate it. The SteamVR-specific glue stays in `server_openvr/cpp` as a wrapper.
- Consider extracting to a new crate `alvr/encoder/` with a C++ core + a thin Rust ABI.

This refactor must land before 3.5 or both paths duplicate code.

### 3.1 — Wire `alvr_server_core` into the bridge stubs — LANDED 2026-05-20 (except `submit_layers`)

| Stub | Status | Notes |
| --- | --- | --- |
| `alvr_oxr_init` | ✅ 3.1.1 | Takes `root_dir`; constructs `ServerCoreContext`, starts connection, spawns event-drain thread (3.1.2). |
| `alvr_oxr_shutdown` | ✅ 3.1.1 | Flips SHUTDOWN_FLAG, drops context, joins drain thread. |
| `alvr_oxr_get_hmd_info` / `_get_controller_info` | ✅ 3.1.3 | Returns stable bridge-side serials (`ALVR_HMD`, `ALVR_Controller_{Left,Right}`). HeadsetEmulationMode spoofing left as a follow-up if a downstream tool needs it. |
| `alvr_oxr_get_head_pose` | ✅ 3.1.2 | `context.get_device_motion(HEAD_ID, target)`; explicit `motion.predict()` deferred (Monado already passes a future predicted timestamp). |
| `alvr_oxr_get_controller_state` | ✅ 3.1.3 + 3.1.4 | Pose + velocities via `get_device_motion`; trigger / squeeze / thumbstick / 16-bit buttons bitfield from `CONTROLLER_BUTTON_CACHE` populated by the drain thread on `ServerCoreEvent::Buttons`. |
| `alvr_oxr_set_haptic` | ✅ 3.1.3 | Forwards to `context.send_haptics`. |
| `alvr_oxr_submit_layers` | ⏸ pending | **Only stub left.** Awaits Slice 3.2/3.3 (Vulkan-input NVENC encoder); needs Vulkan SDK + NVENC 12.1+ + Monado verification environment. |
| `alvr_oxr_poll_session_event` | ✅ 3.1.2 | Drains a `SESSION_EVENTS_RX` queue the drain thread populates from `ClientConnected` / `ClientDisconnected` / `ShutdownPending`. |
| `alvr_oxr_get_view_params` (new in 3.1.5) | ✅ 3.1.5 | Per-eye pose + FOV from the `LOCAL_VIEW_PARAMS` cache the drain thread maintains. Drives Monado's `xrLocateViews` equivalent. |

Reference impl: `alvr/server_openvr/src/lib.rs`. Notable deviations from OpenVR mode: no `SetTracking`/`SetButton` FFI push (Monado pulls on demand); no `event_loop` thread runs on the OpenVR side — instead a dedicated `alvr_oxr_event_drain` thread.

### 3.1.x — follow-ups outstanding

- ✅ Battery `AlvrOxrEventType` variant + drain handler. `ServerCoreEvent::Battery` now emits `AlvrOxrEvent::Battery` with `data[4]` = `[device_kind, gauge_bp, plugged, 0]` where `device_kind` is one of `ALVR_OXR_DEVICE_KIND_{HMD,LEFT_CONTROLLER,RIGHT_CONTROLLER}` (other tracking IDs are dropped) and `gauge_bp ∈ 0..=ALVR_OXR_BATTERY_GAUGE_SCALE` (10 000). Landed 2026-05-21.
- RefreshRate variant intentionally **not** wired: no `ServerCoreEvent::RefreshRate` exists upstream (rate is negotiated once at connection time). `AlvrOxrEventType::RefreshRateChange = 5` stays reserved for the day one lands.
- Monado-side `alvr_hmd.c` wiring of `alvr_oxr_get_view_params` (today the driver returns identity views). Lives on the fork's `alvr` branch per Option A.
- Monado-side handler for the new `Battery` event (consume `data[0..3]` and surface to the headset / dashboard if a use case appears).

### 3.2 — Fake compositor in `comp_alvr.c`

- ✅ **3.2.1 (LANDED 2026-05-21)** — fork commit `74635c623`; alvr-side bump `3d093476`. `comp_alvr.c` is no longer a 33-line stub: it now mirrors `compositor/null/null_compositor.c` line-for-line. A new internal header `comp_alvr_internal.h` holds `struct comp_alvr_compositor` (extends `comp_base`), logging macros, and the down-cast helper. The .c body brings up the Vulkan bundle via `comp_vulkan_init_bundle` (same instance/device extension lists as `null`, plus the platform `external_memory_*` / `external_semaphore_*` extensions 3.2.2 needs), `comp_swapchain_shared_init`, a fake pacer seeded from `xdev->hmd->screens[0].nominal_frame_interval_ns` (72 Hz today), and `system_compositor_info` built from `xdev->hmd->view_count` + blend modes. The compositor wraps via `comp_multi_create_system_compositor`. `layer_commit` is a no-op stub that runs the pacing markers (`U_TIMING_POINT_BEGIN` / `SUBMIT_BEGIN` / `SUBMIT_END`) and `comp_swapchain_shared_garbage_collect`.
- ✅ **3.2.2 (LANDED 2026-05-21)** — fork commit `93f91eb92`; alvr-side bump `f2f65d89`. `layer_commit` walks `comp_layer_accum.layers`, filters to `XRT_LAYER_PROJECTION` / `XRT_LAYER_PROJECTION_DEPTH`, packs each into an `AlvrOxrLayer` (native image handle via `xrt_swapchain_native.images[image_index].handle`, width/height from `comp_swapchain.vkic.info`, pose/FOV from view 0), and calls `alvr_oxr_submit_layers`. Non-projection layer types are dropped with one per-frame DEBUG log. CMakeLists now mirrors `drivers/alvr/CMakeLists.txt` around the bridge: `ALVR_BRIDGE_HEADER_DIR` include + `alvr_runtime_bridge.h` existence check + `alvr_server_openxr` cdylib link + `ALVR_SERVER_OPENXR_LIB_DIR` search hint.
- ✅ **3.2.3 (LANDED 2026-05-21)** — fork commit `d4705186d`; alvr-side bump `babf095d`. `layer_commit` converts the incoming `xrt_graphics_sync_handle_t` to the bridge's `u64 sync_handle`, gated on `xrt_graphics_sync_handle_is_valid` so the platform-specific invalid sentinel doesn't leak as a real handle. `u_graphics_sync_unref` happens unconditionally after the bridge call (multi-client compositor handles sync semantics — same pattern as `null_compositor`).
- ✅ `XRT_FEATURE_COMP_ALVR` is wired into `target_instance.c`: when built in, `comp_alvr_create_system_compositor` runs ahead of `comp_main`/`comp_null` (env `XRT_COMPOSITOR_NULL=1` still wins for headless smoke tests). Fork commit 2026-05-21, alvr-side bump.

**Verification ceiling** for all three sub-slices: `cargo xtask build-openxr-runtime --enable-alvr-driver` on this Windows host fails at Monado's CMake configure for upstream Eigen3 dep (not installed locally, not mentioned in `install.txt`, no vcpkg). The compositor-side code is structurally consistent with the `null_compositor` reference + the existing `drv_alvr` build wiring, so end-to-end compile gate stays open until Eigen3 is installed or a different verification host is used.

### 3.3 — Frame pacing markers — LANDED 2026-05-21

`comp_alvr.c::layer_commit` previously stacked `BEGIN`, `SUBMIT_BEGIN`, and `SUBMIT_END` at the end of the function (same as `null_compositor`, which does no real work). With `alvr_oxr_submit_layers` doing real work in between — and the Vulkan-input NVENC body landing as Slice 3.3 — the pacer was learning nothing about ALVR's submission cost. Fork commit `4cce895cd` re-positions the markers to bookend the bridge call, matching `compositor/main/comp_target_swapchain.c` around its present:

| Marker | Position |
| --- | --- |
| `U_TIMING_POINT_BEGIN` | start of `layer_commit`, after `frame_id` is read |
| `U_TIMING_POINT_SUBMIT_BEGIN` | immediately before the layer pack loop closes / right before `alvr_oxr_submit_layers` |
| `U_TIMING_POINT_SUBMIT_END` | immediately after the bridge call returns |

Markers run unconditionally even on no-layer frames so the per-frame state machine stays consistent. Verification ceiling: `comp_alvr_create_system_compositor` now runs in-process (`Using builder alvr: ALVR (streamed)` + `ALVR compositor ready` in the boot log); the actual pacer learning behaviour is observable once a client connects and frames flow (hardware-gated, Slice 3.3+).

**Latent comp_alvr-not-selected bug fixed in the same session** (fork commit `56466ce47`). `XRT_FEATURE_COMP_ALVR` was set as a CMake option and the comp_alvr target was being built + linked into `target_instance`, but the `#ifdef XRT_FEATURE_COMP_ALVR` guard in `targets/common/target_instance.c` was never true at compile time because the feature wasn't listed in `xrt_config_build.h.cmake_in`. Result: `target_instance` silently fell through to `comp_main_create_system_compositor`, and `comp_alvr_create_system_compositor` was unreachable despite a clean build and clean boot. This is the same class of "static link success doesn't prove dynamic behaviour" pattern that surfaced earlier this session (latent-bug list below). For future Monado-side touches, **always grep the boot log for the specific factory function name, not just for the builder/driver name**.

### 3.4 — End-to-end smoke

Exit criterion for Phase 3: `hello_xr` running against `libopenxr_monado` with `XRT_BUILD_DRIVER_ALVR=ON XRT_FEATURE_COMP_ALVR=ON` shows up on the headset and is interactive.

## Phase 4 — runtime registration (2–3 days)

1. ✅ **4.1 (LANDED 2026-05-21)**: `cargo xtask build-openxr-runtime --enable-alvr-driver` now also turns on `XRT_FEATURE_COMP_ALVR` (the two were artificially separate) and publishes Monado's auto-generated dev manifest to a stable filename `build/openxr-{profile}/active_runtime_alvr.json`. Loader can be pointed at it with `XR_RUNTIME_JSON=<path>` for dev.
2. ✅ **4.2 (LANDED 2026-05-21)**: `cargo xtask register-openxr-runtime` / `unregister-openxr-runtime` cross-platform:
   - **Windows**: writes `HKCU\Software\Khronos\OpenXR\1\ActiveRuntime` via `reg.exe`. (Earlier draft of this doc said `%LOCALAPPDATA%\openxr\1\active_runtime.json` — that's wrong; Windows OpenXR loader uses the registry, not a file at that path.)
   - **Linux / BSD**: writes `$XDG_CONFIG_HOME/openxr/1/active_runtime.json` (fallback `$HOME/.config/openxr/1/...`).
   - macOS deliberately unsupported (no released Monado/ALVR target).
   Unregister refuses to clear when the currently-registered value doesn't match this profile's manifest — won't stomp on a different vendor's runtime.
   The action is **system-modifying for the current user**; the launcher GUI doesn't surface it yet (deliberate — a CLI-only path keeps the side-effect explicit until a UI for it is in place).
3. ✅ **4.3 (LANDED 2026-05-21)**: Mutual exclusion implemented on both sides. SteamVR side already had the warn-out in `dashboard/src/steamvr_launcher/mod.rs::launch_steamvr` (refuses when `RuntimeMode::Openxr`). OpenXR side now mirrors it: `cargo xtask register-openxr-runtime` refuses with `error:` if `vrserver` is alive. Both runtimes claim the same headset connection from the client; letting them race produces a stream that goes nowhere useful.
4. ✅ **4.4 (LANDED 2026-05-21)** — smoke-test plan in [`SMOKE_TESTS.md`](SMOKE_TESTS.md). Concrete gates (build clean / ABI contract / hello_xr / Battery event / OpenVR regression / NVENC streaming / production game) with commands, expected behavior, and a verification log to append to as gates pass. Doc only — actual execution waits on a verification host (Vulkan SDK + NVENC SDK 12.1+ + a real headset).

## Phase 5 — telemetry / dashboard polish (2–3 days)

- Already wired: the `RuntimeMode` selector auto-renders via `SettingsSchema`. No additional dashboard work strictly needed.
- ✅ **Telemetry bridge LANDED 2026-05-21**: bumped bridge ABI to v2 and added `alvr_oxr_report_pacing` (fork `14608d600`, alvr `f4b3cbf4` + `76717ec2`); wired the Rust-side aggregator (alvr `93a4ca62`). Flow:
  `comp_alvr::layer_commit` → `alvr_oxr_report_pacing` → `ServerCoreContext::report_oxr_pacing` → `StatisticsManager::report_oxr_pacing` → `metrics_exporter::try_push(Sample::OxrPacing)`. The aggregator now emits an `oxr_pacing` JSON section with `cpu_us` and `submit_us` distributions whenever a window saw at least one frame; field is omitted otherwise so OpenVR-mode snapshots stay byte-identical. **Verification ceiling**: visible in `metrics_exporter`'s POST output only when a real client connects (StatisticsManager is None pre-connection). Confirmed locally via `cargo check -p alvr_server_core -p alvr_server_openvr -p alvr_server_openxr` + a clean `monado-service.exe` boot at ABI v2.
- Configurable runtime install path. Right now `alvr/xtask/src/build_openxr.rs` hard-codes `openxr/` as the Monado source dir (overridable via `ALVR_MONADO_SOURCE_DIR` env var, but not yet a setting). If we make it a setting, surface it via the same `SettingsSchema` auto-render pattern as `RuntimeMode`.

## Phase 6 — coexistence + cleanup (2–3 days)

- ✅ **Launcher GUI runtime selector LANDED 2026-05-21**. The launcher now reads `session_settings.extra.runtime.variant` for each installation, shows it as a `[SteamVR (OpenVR)]` / `[Monado (OpenXR) — preview]` badge next to the version, and the Edit popup exposes a ComboBox that writes back to `session.json` via `serde_json::Value` manipulation (no schema round-trip — preserves all other fields byte-for-byte). Windows-only (the launcher already gates session-file handling on `cfg!(windows)` per the existing comment). **Verification ceiling**: `cargo check`/`build`/`clippy -p alvr_launcher` all clean, launcher starts without panic on a host with no installations; in-app interaction with a real ALVR installation (read existing variant, switch it, observe re-launched dashboard pick up the change) is not exercised here.
- CI: add a Windows job that builds with `XRT_BUILD_DRIVER_ALVR=ON XRT_FEATURE_COMP_ALVR=ON` and runs unit tests.
- Update `CLAUDE.md` with `alvr_server_openxr` in the crate map and a note about the new runtime mode.
- ✅ **`ARCHITECTURE.md` OpenXR data-flow LANDED 2026-05-21**. Refreshed the "OpenXR runtime mode (preview)" section: item 2 now lists `alvr_oxr_report_pacing` and notes ABI v2 + the on-load mismatch check; item 3 replaces the stale "stub returning XRT_ERROR_NOT_IMPLEMENTED" with the current `layer_commit` behaviour (projection-layer packing + `SUBMIT_BEGIN/END` bookends + pacing-report call), and warns about the cmake_in/`#define` foot-gun; item 5 mentions the launcher selector; new item 6 walks the pacing-telemetry path through to `metrics_exporter::Sample::OxrPacing`. Also extended the "Telemetry & metrics export" section to enumerate the `Sample` variants and added a "New bridge ABI surface (OpenXR mode)" entry to "Where to extend" with the version-bump + atomic-commit recipe.

## Phase 7 — stretch (1–2 weeks, optional)

* Quad / cylinder / equirect / cube / passthrough layer support — rasterise into a single projection image before encoding.
  * ✅ **Slice 1 LANDED 2026-05-21** — diagnostic only. Bridge ABI bumped to v3 with `alvr_oxr_report_layer_types(frame_id, n_quad, n_cylinder, n_equirect, n_cube, n_passthrough)`. `comp_alvr` now reports a per-frame histogram of the non-projection layers it still drops; `metrics_exporter` aggregates into an `oxr_layer_types` JSON section (per-type totals + frame counter). Reveals what kinds of overlays apps actually submit so Slice 2 (Vulkan quad rasterisation) can be prioritised by observed usage. Commit ladder: fork `ae7e32ec2` / alvr `089e9b34` (Rust ABI v3 + aggregator) / alvr `41f7db24` (submodule bump).
  * ⏸ Slice 2: actual Vulkan rasterisation. Allocate a render target the size of the projection image; for each non-projection layer, run a textured-quad pipeline that draws the layer on top in z-order; hand the composited image to the encoder. Start with `XRT_LAYER_QUAD` (most common XR overlay shape), then add `XRT_LAYER_CYLINDER` (cylindrical-to-flat projection — shader-only diff from quad). EQUIRECT / CUBE / PASSTHROUGH are higher-order surfaces; defer until usage justifies them.
* Hand-tracking input passthrough (`XR_EXT_hand_tracking`) — wire-compat event; requires `alvr_packets` change → client also rebuilds.
* `XrSessionStateChanged` events plumbed end-to-end (FOCUSED, VISIBLE, READY).
* Per-view foveation hint to Monado's distortion shader.

## Open decisions still on the table

| | Decision | Currently | Revisit when |
| --- | --- | --- | --- |
| F1 | Driver shape — in-tree (`openxr/src/xrt/drivers/alvr/`) vs out-of-tree overlay | in-tree | submodule conversion |
| F2 | Frame ingress path — fake compositor vs custom `comp_target` | fake compositor (`comp_alvr.c`) | Phase 3.2 |
| F3 | Build wiring — standalone CMake vs subsumed into xtask | standalone CMake invoked by xtask | never (locked) |
| F4 | Bridge header — cbindgen vs hand-maintained | cbindgen 0.29 + `Config` in `build.rs`, opt-in regen via `ALVR_REGENERATE_BRIDGE_HEADER=1` (resolved 2026-05-20) | only revisit if migrating to a `cbindgen.toml` file to match other crates |
| F5 | Submodule architecture — fork branch / patch overlay / upstream PR | Option A (fork branch). Local prep done 2026-05-20 (PHASE2_MANIFEST.md / phase2_alvr.patch / convert_to_submodule.sh). Awaits fork creation. | when maintainer has pushed the fork's alvr branch |

## Files a future session should read first

1. `/openxr-migration.md` — master plan with phase breakdown and risk list
2. `/docs/monado-notes/NEXT_STEPS.md` — **this file**
3. `/docs/monado-notes/SUBMODULE_PIN.md` — exact `git submodule` migration commands
4. `/docs/monado-notes/XRT_INTERFACES.md` — the Monado contracts the new code implements
5. `/docs/monado-notes/INTEGRATION_NOTES.md` — the ALVR ↔ Monado mapping table
6. `alvr/server_openxr/src/lib.rs` — the 10 stubs to fill in
7. `openxr/src/xrt/drivers/alvr/alvr_hub.c` — the driver entry, currently returns identity poses
8. `openxr/src/xrt/compositor/alvr/comp_alvr.c` — the fake compositor stub
9. `alvr/server_openvr/src/lib.rs` — the reference impl to mirror in `server_openxr`

## How to not break the existing OpenVR mode

Three rules that keep `master` shippable while Phase 3 work is in flight:

1. **Default `RuntimeMode` stays `Steamvr`.** Never change `RuntimeModeDefault::variant` in `settings.rs`.
2. **`XRT_BUILD_DRIVER_ALVR` and `XRT_FEATURE_COMP_ALVR` stay OFF by default in `openxr/`'s CMake config.** Both gates must be flipped explicitly via `cargo xtask build-openxr-runtime --enable-alvr-driver`.
3. **Touch `alvr/server_openvr/cpp/` only for the encoder refactor (3.0), and only to extract a runtime-agnostic interface. Do not change the OpenVR-facing wrapper.**

If those three hold, `cargo xtask build-streamer` keeps producing a working SteamVR driver no matter what state Phase 3 is in.
