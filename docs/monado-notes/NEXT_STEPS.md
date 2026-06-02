# OpenXR-mode integration — next steps

Pick-up doc for future sessions. Pairs with the master plan at [`docs/openxr-migration.md`](../openxr-migration.md). When the two disagree, the migration plan is authoritative and this file is wrong.

> **Trimmed 2026-06-02.** Phases 0–6 + hand-tracking + per-view foveation are shipped and merged; the blow-by-blow that used to fill this file is collapsed into the [Shipped ledger](#shipped-ledger). Full detail lives in git history (read this file at commit `ad0421ce` for the pre-collapse version), the `PHASE*_SCOPE.md` docs, `archive/SESSION_SUMMARY_*.md`, and the `[[openxr-mode-integration]]` / `[[project-openxr-remaining-work]]` memories.

## Where we are

OpenXR mode is **merged to master and works end-to-end on RTX 3090 + Quest 3**: `comp_alvr` squashes + FFR-compresses the per-view layers, the Vulkan-input CUDA-interop NVENC encoder streams them, and the client decodes and displays. Video, `XR_EXT_hand_tracking`, and per-view (eye-tracked) foveation are all headset-verified. Real third-party OpenXR apps render (hello_xr cubes; AK/UE5 after the visibility-mask fix). `openxr/` is a real git submodule on the `alvr` fork branch (`github.com/AndreyVN/monado`); bridge ABI is at **v8**. OpenVR (SteamVR) mode is unchanged and remains the default.

The last weeks-long blocker — **AK/UE5 black-on-Monado — was the visibility mask** (the ALVR HMD let Monado synthesize `XR_KHR_visibility_mask` from a placeholder symmetric FOV; UE5 re-projected its resolve mesh off-frame → black). Fixed by returning an empty mask (openxr `70c28dcf1`, master `fe8e7a3b`). Full writeup in [[openxr-ak-4096-limit]].

## What's left (the actual next steps)

All genuinely-remaining work is **hardware- or Linux-host-gated** — nothing is locally actionable on the AMD Windows dev host. Reconcile against the code before scoping any of it (this doc has historically over-stated remaining work).

- **Phase 7 overlay residuals** — quad composite is verified (2026-06-02); `CYLINDER`/`EQUIRECT2` ride the same proven `comp_render_gfx_dispatch` but aren't independently exercised, and small/edge-overlay positional accuracy under the `world_poses==eye_poses` simplification (`comp_alvr.c:1161`) is unpinned. Confirm opportunistically in a real headset session with an overlay app. `CUBE`/`EQUIRECT1`/`PASSTHROUGH` stay unsupported — add only if the `oxr_layer_types` histogram shows real apps submit them (PASSTHROUGH is a server-side non-goal).
- **Encoder backends** — only Vulkan-input **NVENC** ships for OpenXR mode. AMF + VPL Vulkan-input bodies (Slice 3.4) are RTX-host / hardware-gated.
- **Linux-host-gated encoder tails** — conform Linux `EncodePipeline` to `IEncoderBackend` (`PHASE3_0_SCOPE.md` sub-slice 2.4), `FrameRender`↔`protocol.h` decoupling, the Slice-1 `--gpl` functional build.
- **Optional / perf** — Slice 2e (GFX→CS squasher dispatch swap, only if measured); replace `compose_via_squasher`'s `vkQueueWaitIdle` CPU stall with a semaphore handoff; Gate H (per-eye foveation behaviour) on an eye-tracked headset.
- **Small ergonomics** — propagate encoder-unavailable out of `alvr_oxr_submit_layers` as non-OK; populate `comp_scratch_single_images::native_images[idx].size` for the FFR-off path. (Two items that used to live here have **landed**: auto-build the cdylib in `5dc8d270`, and the `--monado-source` build flag — see the stale-cdylib lesson below.)

Verify the tree at any point:

```sh
cargo check -p alvr_server_openxr -p alvr_session -p alvr_dashboard -p alvr_xtask
```

## Durable lessons + dead-ends (don't re-chase)

- **Visibility-mask debugging rule:** an app that renders on SteamVR but is black on Monado with valid poses + a running render loop ⇒ suspect `XR_KHR_visibility_mask` built from a placeholder FOV *before* chasing the swapchain/import layers.
- **AK-black confirmed dead-ends** (all eliminated; listed so they aren't re-chased): the 4096-px D3D12→Vulkan import limit; shared-stereo-atlas vs per-eye swapchain shape; `D3D12_RESOURCE` / NT-vs-DXGI handle import; squasher sampling; `multi_compositor` fence/sem sync; the source-tile-dump "atlas empty" reading; D3D11→Vulkan IPC import; head-pose-NONE; depth+stencil format (`D32_FLOAT_S8X24`); the 156 `VIEW_ID` PSO failures (benign — identical count on SteamVR, which renders); OpenXR API version; multiview / arraySize=2; client TrackingData-feed.
- **"Static link success doesn't prove dynamic behaviour":** `XRT_FEATURE_COMP_ALVR` built + linked, but a missing `xrt_config_build.h.cmake_in` entry left the `#ifdef` false, so `comp_alvr` was silently unreachable. Grep the boot log for the specific factory-fn name, not just the builder/driver name.
- **Stale cdylib trap (now mitigated):** a stale `target/{debug,release}/alvr_server_openxr.dll` once masqueraded as a baseline-present bug across a whole bisect, because `build-openxr-runtime` deployed it without rebuilding the Rust crate. **Fixed in `5dc8d270`** — `build_bridge_cdylib` now runs `cargo build -p alvr_server_openxr` *before* the CMake build (gated on `--enable-alvr-driver`, the only mode that links/uses the cdylib), so the xtask path can't ship a stale DLL. Still rebuild explicitly if you ever hand-deploy a cdylib outside the xtask.
- **No client/server clock sync** in OpenXR mode — pull the "latest sample," never do exact-timestamp lookup (bit both hand-tracking and head pose).
- **Docs over-state remaining work** — slices marked DEFERRED/pending in scope docs frequently landed later under a different commit boundary. Reconcile against `comp_alvr.c` / the bridge / the code before scoping anything "remaining."

## Shipped ledger

Phases 0–6 + hand-tracking + per-view foveation, all merged to master. One line each; commit ladders and rationale live in the scope docs and git history.

| Area | Status | Detail in |
| --- | --- | --- |
| Phases 0–2 — scaffolding (crate, settings, xtask, Monado driver/compositor stubs) | ✅ | git history; landed table at `ad0421ce` |
| Bridge header (cbindgen 0.29, opt-in regen) | ✅ | F4 below |
| `openxr/` → real submodule on the `alvr` fork branch | ✅ | `SUBMODULE_PIN.md`, `PHASE2_MANIFEST.md` |
| Phase 3.0 — encoder refactor (Option-C `IEncoderBackend` adapter; D3d11 + Vk backends) | ✅ | `PHASE3_0_SCOPE.md` |
| Phase 3.1 — all 10 bridge fns wired to `alvr_server_core` (incl. CUDA-interop NVENC `submit_layers`) | ✅ | `server_openxr/src/lib.rs` |
| Phase 3.2/3.3 — `comp_alvr` real compositor + projection pack + pacing markers | ✅ | `comp_alvr.c` |
| Phase 3.4 — end-to-end video (IDR-on-`RequestIDR` fix `e2eb150e`) | ✅ verified RTX3090+Quest3 | — |
| Phase 4 — runtime registration (`register`/`unregister-openxr-runtime`, mutual exclusion) | ✅ | `SMOKE_TESTS.md` |
| Phase 5 — telemetry (`oxr_pacing` ABI v2, `oxr_layer_types` ABI v3) | ✅ | — |
| Phase 6 — launcher selector, CLAUDE.md / ARCHITECTURE.md updates, CI (`openxr.yml` green) | ✅ | — |
| Phase 7 Slice 1 — layer-type histogram telemetry (ABI v3) | ✅ | — |
| Phase 7 Slice 2 / 2c.1 — overlay squasher reuse (quad/cylinder/equirect2); quad composite verified | ✅ | `PHASE7_SLICE2_SCOPE.md` |
| Hand-tracking (`XR_EXT_hand_tracking`, ABI v4) | ✅ verified 100% joint validity | `HAND_TRACKING_PASSTHROUGH.md` |
| Per-view eye-tracked foveation (ABI v5; wire `FoveationView` on `RealTimeConfig` + `VideoPacketHeader`; client de-foveation consumer; per-eye encoder) | ✅ code; Gate H behaviour unverified | `PER_VIEW_FOVEATION.md` |
| Late-join state replay + ConnectionLost/STOPPING debounce + encoder-resolution lockstep | ✅ | — |

**CI auth note** (kept — bites on token rotation): `openxr/` is a **private** submodule (`AndreyVN/monado`); `openxr.yml` checkout needs the `MONADO_PAT` fine-grained PAT (Contents: Read-only on `monado`). A sudden checkout-only CI failure = expired PAT → re-issue and update the `MONADO_PAT` secret. Anything `prepare-deps` choco-installs must be re-exported to `$GITHUB_ENV`/`$GITHUB_PATH` in CI.

## Open decisions still on the table

| | Decision | Currently |
| --- | --- | --- |
| F1 | Driver shape — in-tree vs out-of-tree overlay | in-tree (`openxr/src/xrt/drivers/alvr/`) |
| F2 | Frame ingress — fake compositor vs custom `comp_target` | fake compositor (`comp_alvr.c`) |
| F3 | Build wiring — standalone CMake vs xtask | standalone CMake invoked by xtask (locked) |
| F4 | Bridge header — cbindgen vs hand-maintained | cbindgen 0.29 + `Config` in `build.rs`, opt-in regen via `ALVR_REGENERATE_BRIDGE_HEADER=1` |
| ~~F5~~ | ~~Submodule architecture~~ | ✅ resolved — Option A (fork branch); `openxr/` is a live submodule |

## Files a future session should read first

1. `docs/openxr-migration.md` — master plan with phase breakdown and risk list
2. `docs/monado-notes/NEXT_STEPS.md` — **this file**
3. `docs/monado-notes/XRT_INTERFACES.md` / `INTEGRATION_NOTES.md` — the Monado contracts + the ALVR↔Monado mapping
4. `alvr/server_openxr/src/lib.rs` — the bridge implementation (all 10 fns live)
5. `openxr/src/xrt/{drivers,compositor}/alvr/` — the Monado-side driver + `comp_alvr` compositor
6. `alvr/server_openvr/src/lib.rs` — the OpenVR reference impl that `server_openxr` mirrors

## How to not break the existing OpenVR mode

Three rules that keep `master` shippable:

1. **Default `RuntimeMode` stays `Steamvr`.** Never change `RuntimeModeDefault::variant` in `settings.rs`.
2. **`XRT_BUILD_DRIVER_ALVR` and `XRT_FEATURE_COMP_ALVR` stay OFF by default** in `openxr/`'s CMake; both flip only via `cargo xtask build-openxr-runtime --enable-alvr-driver`.
3. **Touch `alvr/server_openvr/cpp/` only via the runtime-agnostic `IEncoderBackend`** — don't change the OpenVR-facing wrapper.

If those hold, `cargo xtask build-streamer` keeps producing a working SteamVR driver regardless of OpenXR-mode state.
