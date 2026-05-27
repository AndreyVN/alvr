# Plan: integrate ALVR with the OpenXR (Monado) snapshot, analogous to the OpenVR path

## Framing

Today: `openvr/` is a vendored Valve SDK; `alvr/server_openvr` is a Rust crate that compiles to a SteamVR driver consuming those headers. End state: `openxr/` is a vendored Monado snapshot; a new `alvr/server_openxr` Rust crate plus a new `drivers/alvr/` inside Monado bring up the same PC-side functionality on top of Monado/OpenXR. The headset (`alvr/client_openxr`) does not change — it's already OpenXR. The wire protocol (`alvr_sockets` + `alvr_packets`) does not change.

## Three forks-in-the-road (assumed defaults below; revisit if needed)

| # | Decision | Default | Alternative |
| --- | --- | --- | --- |
| F1 | How to add ALVR into Monado | New `drivers/alvr/` modelled on `drivers/remote/`, with a Monado-side builder | Extend `drivers/remote/` with video — couples our work to upstream |
| F2 | Frame ingress path | "Fake compositor" — implement `xrt_compositor_native` that receives layers and forwards them to ALVR's encoder, skipping the display compositor | Custom `comp_target` — keeps Monado's compositor and intercepts at present time |
| F3 | Build wiring | `openxr/` stays a standalone CMake project; one new `cargo xtask` subcommand drives it. Snapshot is git-pinned in a submodule | Subsume Monado's whole build into `cargo xtask` (high effort, high coupling) |

If F1/F2/F3 change, several phases below shift; F2 in particular changes phase 3 from ~5 days to ~10.

## Architecture diagram (target state)

```
   Headset (Android)                        PC (new path)
   ─────────────────                        ─────────────────────────────────────────────────────
   alvr_client_openxr  ── UDP (alvr_packets) ──►   alvr_server_openxr (Rust crate)
                                                        │  C ABI ("alvr_runtime_bridge.h")
                                                        ▼
   ◄── encoded video + audio + haptics ──    drivers/alvr  +  fake xrt_compositor_native
                                              (inside openxr/)        │
                                                                       ▼
                                               OpenXR loader  ►  libopenxr_monado.so  ◄── PC OpenXR app
```

Compare to today's OpenVR path: same UDP, same encoder pool, but `alvr_server_openxr` + `monado-service` replaces `alvr_server_openvr` + `vrserver`.

---

## Phase 0 — wire `openxr/` into the project (1–2 days)

| Step | What | Files |
| --- | --- | --- |
| 0.1 | Convert `openxr/` from an untracked directory into a pinned git submodule (or vendored snapshot with `UPSTREAM_REV` file). Today `git status` lists `?? openxr/` — that's untenable. | `.gitmodules` or `openxr/UPSTREAM.md` |
| 0.2 | Add CMake option `XRT_BUILD_DRIVER_ALVR=OFF` (default off) to the Monado tree. Behind a `cmake/` patch directory if we want to avoid editing the snapshot. | `openxr/CMakeLists.txt`, `openxr/src/xrt/drivers/CMakeLists.txt` |
| 0.3 | New xtask: `cargo xtask build-openxr-runtime [--release]`. Runs `cmake -B build/openxr -S openxr -DXRT_BUILD_DRIVER_ALVR=ON …` then `cmake --build`. Output: `build/openxr/src/xrt/targets/openxr/libopenxr_monado.{so,dll}` + `monado-service`. | `alvr/xtask/src/main.rs`, `alvr/xtask/src/build.rs` (new module `build_openxr.rs`) |
| 0.4 | One paragraph in `ARCHITECTURE.md` + this fork's `CLAUDE.md` crate map: note that `alvr_server_openxr` is the OpenXR analogue of `alvr_server_openvr`. | `ARCHITECTURE.md`, `CLAUDE.md` |

Exit criterion: `cargo xtask build-openxr-runtime` produces a Monado without the ALVR driver enabled (proves the build wiring works without our code in the way).

---

## Phase 1 — design the ALVR ↔ Monado C bridge (1–3 days, mostly on paper)

Define the C ABI Monado will call. Mirror the shape of `alvr_server_core/src/c_api.rs` (familiar to anyone working in this repo).

| Step | What |
| --- | --- |
| 1.1 | Write `alvr/server_openxr/include/alvr_runtime_bridge.h` (or generate via cbindgen). Surface (minimum): `alvr_oxr_init/shutdown`, `alvr_oxr_get_hmd_info`, `alvr_oxr_get_controller_info(side)`, `alvr_oxr_get_head_pose(at_ns, *out)`, `alvr_oxr_get_controller_state(side, at_ns, *out)`, `alvr_oxr_submit_layers(frame_id, layer_count, *layers, *sync_handle)`, `alvr_oxr_set_haptic(side, *params)`, `alvr_oxr_poll_session_event(*out_event)`. |
| 1.2 | Create the Rust crate `alvr/server_openxr` (cdylib). Implements the ABI on top of the existing `alvr_server_core` + `alvr_sockets` + `alvr_packets`. *Zero* changes to wire protocol expected — only call into existing reception/encoder paths. |
| 1.3 | cbindgen step in `alvr/server_openxr/build.rs` that writes the header into `build/openxr-bridge/`. Monado-side CMake picks it up. |

Exit criterion: header compiles into a tiny `monado-cli`-style probe that calls `alvr_oxr_init`/`alvr_oxr_shutdown` against a stub Rust impl returning synthetic poses.

---

## Phase 2 — Monado driver `drivers/alvr/` (3–5 days)

Inside `openxr/src/xrt/drivers/alvr/`:

| File | Role |
| --- | --- |
| `alvr_interface.h` | Public entry `alvr_create_devices(*broadcast_sink, **out_xsysd, **out_xso)` |
| `alvr_hmd.c` | `xrt_device` for HMD. Fills `xrt_hmd_parts.{display, views, view_count}` from `alvr_oxr_get_hmd_info`. Implements `get_tracked_pose`, `get_view_poses`, `update_inputs`. |
| `alvr_controller.c` | `xrt_device` for left/right controllers. `update_inputs` reads `alvr_oxr_get_controller_state`. `set_output` calls `alvr_oxr_set_haptic`. |
| `alvr_hub.c` | Driver-internal worker thread that polls the bridge for events and pushes them into a `xrt_session_event_sink`. |
| `targets/common/target_builder_alvr.c` | Monado builder; pairs HMD + 2 controllers into one system. Added to `target_builder_list[]` between `remote` and `simulated`. |
| `CMakeLists.txt` | Gated by `XRT_BUILD_DRIVER_ALVR`. Links to `alvr_runtime_bridge` (the cdylib). |

Action bindings: reuse the Touch / Index / Vive Wand profile JSON in `openxr/src/xrt/auxiliary/bindings/` (whichever ALVR's protocol most closely matches — needs one mapping decision).

Exit criterion: `monado-cli --probe` with `XRT_BUILD_DRIVER_ALVR=ON` and the bridge stub returns "HMD + 2 controllers found".

---

## Phase 3 — frame ingress (the hard part, 5–10 days)

**With F2 = "fake compositor" (recommended):**

| Step | What | Files |
| --- | --- | --- |
| 3.1 | Add `compositor/alvr/comp_alvr.c` — a new `xrt_compositor_native` that extends `comp_base`. Swapchain creation goes through the existing `compositor/util/comp_swapchain.c` (gets us Vulkan images that other GPU APIs can import via shared handles). | `openxr/src/xrt/compositor/alvr/**` |
| 3.2 | Override `layer_commit`: package the per-layer `xrt_layer_data` + image handles (Win32 NT handle / DMABUF fd) into a struct and call `alvr_oxr_submit_layers`. | same |
| 3.3 | Add `XRT_FEATURE_COMP_ALVR` cmake option. In `targets/common/target_instance.c:113`, if `XRT_BUILD_DRIVER_ALVR` is active, select this compositor instead of `comp_main` / `comp_null`. | `openxr/src/xrt/targets/common/target_instance.c` |
| 3.4 | Refactor encoder in `alvr/server_openvr/cpp` so the NVENC/AMF/VPL paths take a runtime-agnostic `Encoder` interface (input: VkImage handle + sync object; output: encoded packet). Currently they're stitched into the SteamVR DirectMode component. **This refactor must land before 3.5 — flag as a blocker.** | `alvr/server_openvr/cpp/**`, possibly a new `alvr/encoder/` shared crate |
| 3.5 | `alvr_server_openxr::handle_submit_layers` walks the layer list: projection layer (left/right view) → into the encoder; quad/cylinder/etc. → either rasterise into the projection layer first (Phase 7) or drop with a warning. | `alvr/server_openxr/src/layers.rs` |
| 3.6 | Frame pacing: feed `u_pc_mark_point(POINT_SUBMIT_END)` from `comp_alvr.c` so Monado's compositor pacer learns ALVR's actual present cadence. | `openxr/src/xrt/compositor/alvr/comp_alvr.c` |

Exit criterion: `hello_xr` running against the new runtime shows up on the headset.

---

## Phase 4 — runtime registration & coexistence (2–3 days)

| Step | What |
| --- | --- |
| 4.1 | Generate `build/openxr/active_runtime_alvr.json` pointing at the built `libopenxr_monado`. Launcher writes it to the right per-user OpenXR config path on Windows / Linux. |
| 4.2 | Mutual exclusion with the SteamVR path: if `vrserver.exe` (Windows) / `vrserver` (Linux) is alive, refuse to start the OpenXR path with a clear error. Both want the same headset stream. |
| 4.3 | Smoke tests: hello_xr, an OpenXR sample like `XrSamples/Compositor`, and one real game that the OpenVR path also supports. |

---

## Phase 5 — dashboard, config, telemetry (2–3 days)

| Step | What | Files |
| --- | --- | --- |
| 5.1 | New dashboard tab "OpenXR runtime" alongside the existing SteamVR settings. | `alvr/dashboard/src/dashboard/components/**` |
| 5.2 | `alvr_session` schema: add an `enum Runtime { SteamVR, OpenXR }` field plus subfields (runtime install path, log level, frame-pacing knobs). Migration per CLAUDE.md rule 5. | `alvr/session/src/settings.rs` |
| 5.3 | Telemetry: bridge the Monado-side `u_pc_*` markers (already produced in phase 3.6) up through `alvr_runtime_bridge` and into `alvr_server_core::metrics_exporter` so existing dashboards keep working. | `alvr/server_openxr/src/metrics.rs`, `alvr/server_core/src/metrics_exporter.rs` |
| 5.4 | LHM/hardware exporter — unchanged, it's process-local. |

---

## Phase 6 — coexistence + cleanup (2–3 days)

| Step | What |
| --- | --- |
| 6.1 | Launcher: surface the new path as an option, default unchanged (SteamVR). |
| 6.2 | CI: add a Windows job that builds with `XRT_BUILD_DRIVER_ALVR=ON` and runs the unit tests. Add to `cargo xtask check-msrv` if any new MSRV implications. |
| 6.3 | Update docs: top-level `README.md` "Runtimes" section, `CONTRIBUTING.md`, `CLAUDE.md` crate map, and `openxr/ALVR_DOCS/INTEGRATION_NOTES.md` (cross-reference the actual implementation). |
| 6.4 | Update `ARCHITECTURE.md` with the OpenXR-runtime data flow diagram. |

---

## Phase 7 — optional stretch (1–2 weeks)

* Quad / cylinder / equirect / cube / passthrough layer support (full layer composition before encoding).
* Hand-tracking input passthrough (uses XR_EXT_hand_tracking on the headset, fills `xrt_device.get_hand_tracking` on PC).
* `XrSessionStateChanged` events plumbed end-to-end (FOCUSED, VISIBLE, READY).
* Per-view foveation hint from ALVR client → Monado distortion shader.

---

## File map (where new code lands)

```
New Rust crates:
  alvr/server_openxr/                       cdylib, exports alvr_runtime_bridge.h
  alvr/server_openxr/build.rs               cbindgen step

New code inside openxr/ (snapshot):
  openxr/src/xrt/drivers/alvr/              the Monado driver
  openxr/src/xrt/compositor/alvr/           the "fake" xrt_compositor_native
  openxr/src/xrt/targets/common/target_builder_alvr.c
  openxr/cmake/                             cmake patch dir for the XRT_BUILD_DRIVER_ALVR option

Touched (small):
  Cargo.toml                                add server_openxr to workspace
  alvr/xtask/src/{main.rs, build.rs, build_openxr.rs (new)}
  alvr/launcher/src/**                      runtime toggle
  alvr/dashboard/src/dashboard/components/** OpenXR settings panel
  alvr/session/src/settings.rs              Runtime enum + migration
  alvr/server_core/src/lib.rs               expose hooks for the bridge (likely minor)
  alvr/server_openvr/cpp/**                 encoder refactor (Phase 3.4 — biggest internal change)
```

---

## Risks & dependencies (read this part)

1. **Encoder refactor is on the critical path.** Phase 3.4 must land first, or the OpenXR path will end up duplicating the encoder code. Suggest splitting the encoder into its own crate before phase 3 starts.
2. **Wire format may need additions.** If we want hand tracking, per-view fov, or passthrough we need new `alvr_packets` fields, which is a wire-compat event (CLAUDE.md rule 5).
3. **Upstream Monado ABI drift.** If `xrt_compositor`/`xrt_device` change between snapshots, our driver breaks. Snapshot-pin is the mitigation; treat updates as deliberate.
4. **Coexistence on Windows.** Both paths grab the headset stream; we need launcher-side mutual exclusion.
5. **Display ownership.** Monado's compositor wants a display by default. The fake compositor (F2 = A) avoids this; the alternative (custom `comp_target`) needs us to fake a Vulkan KHR_display device so Monado will start.

---

## Suggested order to actually execute

If you want a "smallest interesting milestone" first:

1. Phase 0 (build wiring) — visible, low risk.
2. Phase 1 (bridge header) — pure design, no commits to Monado.
3. Phase 2 with a stub bridge — `monado-cli` sees ALVR devices, no video yet.
4. Phase 3.4 (encoder refactor in `alvr/server_openvr/cpp`) — pause Monado-side work here.
5. Phase 3.1–3.6 — first end-to-end video frame to the headset.
6. Then phases 4–6 to make it usable, in any order.

Total: ~3–5 weeks of focused work for phases 0–6 under F2 = "fake compositor"; +1–2 weeks if F2 = "custom comp_target".
