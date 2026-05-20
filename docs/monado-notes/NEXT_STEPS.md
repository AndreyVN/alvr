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

### 1. The bridge header is sparse

`alvr/server_openxr/include/alvr_runtime_bridge.h` was overwritten by cbindgen 0.27 during the first `cargo check` and ended up containing only `#include` lines + the header guard — no types, no function declarations. I made the regeneration opt-in (`ALVR_REGENERATE_BRIDGE_HEADER=1`) so it stops happening on every build, but the current header is unusable.

**Either**:
- (A) Fix the cbindgen config in `alvr/server_openxr/build.rs` so `ALVR_REGENERATE_BRIDGE_HEADER=1 cargo build -p alvr_server_openxr` produces a populated header. Likely needs `parse.parse_deps = false`, `parse.include = ["alvr_server_openxr"]`, and possibly a `cbindgen.toml` file at the crate root. Verify by checking that `alvr_oxr_init`, `AlvrOxrPose`, `AlvrOxrLayer` etc. appear in the output.
- (B) Restore the hand-written header from the original Write in this session (it's in the conversation transcript) and tag the cbindgen step as documentation-only. Less ergonomic but unblocks Phase 3.

CMakeLists.txt in `openxr/src/xrt/drivers/alvr/` already errors clearly if the header is missing or empty, so the failure mode is loud.

### 2. The Monado-side patches conflict with the submodule plan

`openxr/src/xrt/drivers/alvr/`, `openxr/src/xrt/compositor/alvr/`, and `openxr/src/xrt/targets/common/target_builder_alvr.c` all live **inside** what's about to become a clean upstream submodule. `docs/monado-notes/SUBMODULE_PIN.md` lays out the three resolutions (fork branch / patch overlay / upstream PR).

Recommendation: **Option A (fork branch)**. Push the snapshot's contents to a `alvr-org/monado` fork on GitLab, add the Phase 2 files there on an `alvr` branch, then add as submodule pointing to that branch. After that, future Phase 3+ work happens in the fork repo, not in this one's `openxr/` directory.

Until this is resolved, do not run `git submodule add openxr` — it will erase the Phase 2 files.

### 3. `target_builder_alvr.c` is not yet wired into Monado's target list

`openxr/src/xrt/targets/common/target_lists.c` (lines ~114–164) lists every builder. I added the builder file but did not edit `target_lists.c` to insert `t_builder_alvr_create` into `target_builder_list[]`. Add an entry near the top (after `qwerty`, `remote`) guarded by `#ifdef XRT_BUILD_DRIVER_ALVR`, plus the `#include "../drivers/alvr/alvr_interface.h"` pulled in by `XRT_BUILD_DRIVER_ALVR`. This was deliberately deferred because it's inside the soon-to-be-submodule and the resolution above affects how the change is delivered.

## Phase 3 — the next real chunk (5–10 days)

Order matters because of the encoder refactor blocker.

### 3.0 (blocker) — Refactor encoder out of `alvr/server_openvr/cpp/`

Today the NVENC/AMF/VPL paths in `alvr/server_openvr/cpp/` are stitched into SteamVR's `IVRDriverDirectModeComponent` interface. Phase 3 needs them runtime-agnostic.

Suggested shape:
- Extract a `class Encoder` (or similar) whose constructor takes "input: shared VkImage handle + sync object" and emits encoded packets.
- Both `alvr_server_openvr` and `alvr_server_openxr` instantiate it. The SteamVR-specific glue stays in `server_openvr/cpp` as a wrapper.
- Consider extracting to a new crate `alvr/encoder/` with a C++ core + a thin Rust ABI.

This refactor must land before 3.5 or both paths duplicate code.

### 3.1 — Wire `alvr_server_core` into the bridge stubs

`alvr/server_openxr/src/lib.rs` currently has 10 stub `extern "C"` functions. Implement them:

| Stub | Wires to |
| --- | --- |
| `alvr_oxr_init` | Construct `alvr_server_core::ServerCoreContext`; store in `OnceLock`. Start connection. |
| `alvr_oxr_shutdown` | Drop the context. |
| `alvr_oxr_get_hmd_info` / `_get_controller_info` | Pull serial from the connected client's negotiation; for now hard-code matching what `alvr_packets` carries. |
| `alvr_oxr_get_head_pose` | Read from `LOCAL_VIEW_PARAMS` + the head pose queue (mirror what `server_openvr/src/lib.rs` does at lines 38–80). |
| `alvr_oxr_get_controller_state` | Read from the same tracking event stream. |
| `alvr_oxr_set_haptic` | Send `Haptics` packet via `ServerCoreContext`. |
| `alvr_oxr_submit_layers` | Feed into the refactored encoder from 3.0. |
| `alvr_oxr_poll_session_event` | Drain `ServerCoreEvent` queue; translate `ClientConnected`/`ClientDisconnected`/refresh-rate events. |

Reference impl: `alvr/server_openvr/src/lib.rs` — same patterns apply.

### 3.2 — Fake compositor in `comp_alvr.c`

`openxr/src/xrt/compositor/alvr/comp_alvr.c` is currently a stub. Implement for real:

- Extend `comp_base` (see `openxr/src/xrt/compositor/util/comp_base.{c,h}` for the inheritance pattern; `compositor/null/` and `compositor/main/comp_compositor.c` are the two reference impls).
- Allocate swapchains via `compositor/util/comp_swapchain.c` (gets you Vulkan images that can be exported as Win32 NT handles / DMABUF fds).
- Override `layer_commit`: walk the submitted layer list, package per-layer data + native image handles into `AlvrOxrLayer[]`, call `alvr_oxr_submit_layers`.
- For Phase 3 ship projection layer only. Other layer types log a warning and are skipped.
- Wire `XRT_FEATURE_COMP_ALVR` in `openxr/src/xrt/targets/common/target_instance.c` (line ~113) so that when `XRT_BUILD_DRIVER_ALVR` is active the fake compositor is selected instead of `comp_main` / `comp_null`.

### 3.3 — Frame pacing markers

In `comp_alvr.c`, call `u_pc_mark_point(POINT_SUBMIT_END, ...)` at the right point so Monado's compositor pacer learns ALVR's actual present cadence. See `auxiliary/util/u_pacing.h` for the interface and `compositor/main/comp_compositor.c` for the existing usage.

### 3.4 — End-to-end smoke

Exit criterion for Phase 3: `hello_xr` running against `libopenxr_monado` with `XRT_BUILD_DRIVER_ALVR=ON XRT_FEATURE_COMP_ALVR=ON` shows up on the headset and is interactive.

## Phase 4 — runtime registration (2–3 days)

1. Generate `build/openxr/active_runtime_alvr.json` pointing at the built `libopenxr_monado`. Add a launcher action that writes it to the per-user OpenXR config path:
   - Windows: `%LOCALAPPDATA%\openxr\1\active_runtime.json`
   - Linux: `$XDG_CONFIG_HOME/openxr/1/active_runtime.json` (or `~/.config/openxr/1/active_runtime.json`)
2. Mutual exclusion: in `alvr/dashboard/src/steamvr_launcher/mod.rs` (the warn-out already in place), reject starting OpenXR mode if `vrserver` is alive, and vice versa for the SteamVR path. Both want the same headset stream.
3. Smoke tests beyond `hello_xr` — one OpenXR SDK sample and one production game that the OpenVR path also supports.

## Phase 5 — telemetry / dashboard polish (2–3 days)

- Already wired: the `RuntimeMode` selector auto-renders via `SettingsSchema`. No additional dashboard work strictly needed.
- Telemetry: bridge Monado-side `u_pc_*` markers (from 3.3) up through `alvr_runtime_bridge` and into `alvr_server_core::metrics_exporter`. Existing dashboards keep working unchanged.
- Configurable runtime install path. Right now `alvr/xtask/src/build_openxr.rs` hard-codes `openxr/` as the Monado source dir. If we make it configurable, surface it as a setting.

## Phase 6 — coexistence + cleanup (2–3 days)

- Launcher: surface the runtime selector in `alvr/launcher/` if needed. (Today only the dashboard exposes it.)
- CI: add a Windows job that builds with `XRT_BUILD_DRIVER_ALVR=ON XRT_FEATURE_COMP_ALVR=ON` and runs unit tests.
- Update `CLAUDE.md` with `alvr_server_openxr` in the crate map and a note about the new runtime mode.
- Update `ARCHITECTURE.md` with the OpenXR-runtime data flow diagram.

## Phase 7 — stretch (1–2 weeks, optional)

* Quad / cylinder / equirect / cube / passthrough layer support — rasterise into a single projection image before encoding.
* Hand-tracking input passthrough (`XR_EXT_hand_tracking`) — wire-compat event; requires `alvr_packets` change → client also rebuilds.
* `XrSessionStateChanged` events plumbed end-to-end (FOCUSED, VISIBLE, READY).
* Per-view foveation hint to Monado's distortion shader.

## Open decisions still on the table

| | Decision | Currently | Revisit when |
| --- | --- | --- | --- |
| F1 | Driver shape — in-tree (`openxr/src/xrt/drivers/alvr/`) vs out-of-tree overlay | in-tree | submodule conversion |
| F2 | Frame ingress path — fake compositor vs custom `comp_target` | fake compositor (`comp_alvr.c`) | Phase 3.2 |
| F3 | Build wiring — standalone CMake vs subsumed into xtask | standalone CMake invoked by xtask | never (locked) |
| F4 | Bridge header — cbindgen vs hand-maintained | TBD (currently broken; see "Known issues") | first thing in next session |
| F5 | Submodule architecture — fork branch / patch overlay / upstream PR | TBD | before any Phase 3 commits |

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
