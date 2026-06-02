# Session summary — 2026-05-22

Operations / verification wrap. No new code surface added to the streamer or the bridge; instead this session built up a real-GPU verification environment for the OpenXR-mode work that landed in the 2026-05-21 session, wired Monado's existing CTest into CI, and surfaced a long-standing hosting blocker that had silently kept every previous `openxr.yml` run red.

Pairs with [`NEXT_STEPS.md`](../NEXT_STEPS.md) (per-phase status) and the prior [`SESSION_SUMMARY_2026-05-21.md`](SESSION_SUMMARY_2026-05-21.md).

## What landed

### Real-GPU verification environment — host `192.168.10.101`

- Brought a Windows host with an RTX 3090 + Intel UHD 770 online for OpenXR-mode testing via PowerShell remoting (no SSH; `Invoke-Command -ComputerName 192.168.10.101 -ScriptBlock { ... }`).
- Discovered + fixed the **Catch2 `[.][needgpu]` gotcha**: `tests_comp_client_vulkan.exe` (the only test deployed at session start) was reporting "No tests ran" / `EXIT_CODE=2`. Root cause: the sole `TEST_CASE` is tagged `[.][needgpu]`, and Catch2 hides `[.]`-tagged cases by default. Patched the on-host `run_client_test.bat` to pass `[needgpu]` explicitly; test then ran the documented `client_compositor` case and reported **All tests passed (2 assertions in 1 test case)** with the expected `vk_create_image_from_native size mismatch` non-fatal noise (the mock compositor doesn't really back the image; the test only verifies the swapchain-create hook fires).
- Deployed the remaining **24 Monado test binaries** from the local `build/openxr-debug/tests/Debug/` tree to `C:\alvr\test-openxr\` (the test exes share the same Vulkan/cjson/pthreads DLLs as `monado-service.exe`, so they resolve cleanly from that folder).
- Wrote `C:\alvr\test-openxr\run_all_tests.bat` — single command runs all 25 binaries: 5 GPU/D3D with `[needgpu]`, 20 unit tests with the default filter + `--allow-running-no-tests`. Captures combined output to `all_tests.out`, exits 0 only if every binary succeeded.
- **First green run: 25/25 binaries, 45 test cases, 3759 assertions.** Composition: `tests_pacing` (1061 assertions / 2 cases), `tests_lowpass_float` (585/3), `tests_lowpass_integer` (390/3), `tests_input_transform` (341/2), `tests_aux_d3d_d3d11` (296/3), `tests_aux_d3d_d3d12` (238/2), `tests_rational` (250/4), `tests_history_buf` (176/4), `tests_id_ringbuffer` (92/1), `tests_generic_callbacks` (61/2), `tests_quatexpmap` (52/1), `tests_relation_chain` (47/1), `tests_quat_swing_twist` (40/2), `tests_worker` (36/1), `tests_json` (22/1), `tests_uv_to_tangent` (20/1), `tests_deque` (18/1), `tests_pose` (11/2), `tests_quat_change_of_basis` (6/1), `tests_vector` (5/1), `tests_cxx_wrappers` (3/1), `tests_vec3_angle` (3/1); plus the five GPU/D3D `*_comp_client_*` and `*_aux_d3d_*` binaries at 2 assertions / 1 case each.

### Monado test suite wired into CI

- alvr `77264c00`: `.github/workflows/openxr.yml` gains a `Run Monado test suite` step that invokes `ctest -C Debug --output-on-failure` from `build/openxr-debug/` after the existing build step. Monado already registered every test binary via `add_test(NAME ... COMMAND ${testname} --success --allow-running-no-tests)` in `openxr/tests/CMakeLists.txt`, so a single `ctest` invocation covers all 25 binaries and the GPU-only `[needgpu]` cases pass cleanly on a GPU-less Actions runner (they report "No tests ran" → exit 0 instead of failing). Real-GPU coverage stays on the 192.168.10.101 host. Local pre-push validation: 25/25 in 2.02s, 100% pass.

### Hosting blocker surfaced — `AndreyVN/monado` 404 + PAT-token patch

- After pushing `77264c00`, polled the workflow run via the GitHub REST API and found it had failed at **the very first step (`actions/checkout@v6`)** — and so had the two prior runs on `781a3368` (docs-only) and `582d4dfc` (CLAUDE.md refresh). Every step after checkout was skipped. So the workflow has been red since `0d1347e2` (the original CI-add commit).
- Root cause: `.gitmodules` declares `openxr → https://github.com/AndreyVN/monado.git`, but that URL **404s anonymously**. The local in-tree submodule and the standalone `D:/projects/monado-alvr-fork/` clone both have it as `origin`, but the fork has never been pushed to a published GitHub remote. Probed alternatives (`github.com/alvr-org/monado`, `gitlab.freedesktop.org/monado/monado`) — only upstream Monado resolves, and it doesn't have the `alvr` branch.
- alvr `1efed4c9`: `.github/workflows/openxr.yml` checkout step now passes `token: ${{ secrets.MONADO_PAT }}` to `actions/checkout@v6`. Documented inline that the secret needs `Contents: read` on `AndreyVN/monado` (fine-grained PAT) or classic `repo` scope.

**Still on the maintainer:** two manual GitHub-UI actions are required before the workflow can clear checkout:
1. Push the local fork (`D:/projects/monado-alvr-fork/`) to `github.com/AndreyVN/monado` — the repo needs to exist publicly, or exist privately and be readable by the PAT.
2. Add a `MONADO_PAT` Actions secret on `AndreyVN/alvr` containing the PAT.

Until both land, `openxr.yml` runs will keep showing red — but now with a `bad credentials` / `permission denied` failure instead of a 404, so the difference will be visible.

### Memory entries added

Three new persistent memory records on the dev workstation:

- `reference_remote_test_host.md` — full layout of `C:\alvr\test\` (SteamVR-mode dashboard install) and `C:\alvr\test-openxr\` (Monado + bridge + 25 test binaries) on host 101, the PowerShell remoting recipe, the Catch2 `[.][needgpu]` gotcha, the PowerShell 5.1 `2>&1`-on-native-exes stderr-wrapping pitfall, and the full per-binary test inventory (cases + assertions).
- `project_monado_fork_hosting.md` — the AndreyVN/monado 404 blocker, the chain of failing runs since `0d1347e2`, the PAT-token plumbing in `1efed4c9`, and what still needs to happen on the maintainer's end.
- `MEMORY.md` updated index → 5 entries total (2 added).

## Verification status (delta from yesterday)

| Gate | Yesterday | Today |
| --- | --- | --- |
| A (Monado-side compile) | ✅ local | ✅ unchanged |
| B (bridge ABI + builder + compositor selection) | ✅ local | ✅ unchanged |
| **B'** (full CTest suite locally) | n/a | ✅ **25/25 in 2.02s** |
| **B''** (full CTest suite on real GPU — host 101) | n/a | ✅ **25/25, 3759 assertions** |
| **CI** (Actions workflow green) | ⏸ never run (hosting blocker, undiagnosed) | ⏸ **diagnosed + patched on our side, waiting on maintainer for fork push + secret** |
| C–G (real client + headset) | ⏸ hardware-blocked | ⏸ unchanged |

---

# Afternoon session — dashboard polish + Phase 7 scoping trilogy

Same date, separate session, all of it pivoted to non-submodule work after re-confirming the hosting blocker was still active. CI run on `2acf1713` (this morning's commit) still failed at `actions/checkout@v6` — every subsequent step skipped, including the new Monado CTest. The maintainer-side TODO (push the fork to `github.com/AndreyVN/monado`, create `MONADO_PAT` Actions secret) remains the unblocker.

## What landed (afternoon)

### Dashboard surfaces the OpenXR-mode aggregates

- alvr `41ee8929`: `EventType::OxrFrameSummary { pacing, layer_types }` wired end-to-end. The streamer was already pushing `oxr_pacing` / `oxr_layer_types` sections to the metrics exporter (Phase 5 / Phase 7 Slice 1 work), but those JSON sections only travelled to the remote ingest — the dashboard had no view. Now `StatisticsManager` grows a `Mutex<OxrLocalAcc>` leaf (so the existing `&self` contract on `report_oxr_pacing` / `report_oxr_layer_types` stays intact — the live driver call sites still take a read lock), drains it on the existing 500 ms cadence alongside `StatisticsSummary`, and emits the new variant only when at least one axis saw a sample in the window. Dashboard Statistics tab renders a compact "OpenXR mode (Monado bridge)" block under the existing overview, kept fully hidden until the first event lands — so SteamVR-mode dashboards stay visually byte-identical.
- 6 new unit tests in `statistics::tests` (drain-empty / min-avg-max / negative-duration clamp / layer-totals / drain-resets / independent-axes). `cargo test -p alvr_server_core --lib` is now **14/14 green**.
- Verification ceiling: the visual render is hardware-gated on a connected OpenXR-mode client (Gates C–G). The aggregator math is asserted by tests; the event-emission boundary is asserted by inspection.
- Format note: the repo has wide pre-existing rustfmt drift across files this session didn't touch (`xtask/src/main.rs`, `client_core/src/connection.rs`, `hwmonitor/src/*`, etc.). `cargo xtask check-format` fails on those — not on anything new. Touched files are clean. Don't blanket-format the repo without a separate dedicated commit.

### Phase 7 scoping doc trilogy

The three non-submodule pivots from the recommended-next-session menu (per-view foveation / hand-tracking passthrough / dashboard polish) all landed as either code or design docs:

- alvr `f86a13c2`: `docs/monado-notes/HAND_TRACKING_PASSTHROUGH.md` — pipe `TrackingData.hand_skeletons` through the OpenXR-mode bridge so PC apps see them via `XR_EXT_hand_tracking`. **Headline: zero `alvr_packets` break for the basic case** — the wire and the `server_core` API already carry everything. Whole gap is OpenXR-mode-side. Bridge ABI v4 sketch: `alvr_oxr_get_hand_skeleton(side, at_timestamp_ns, *out_joints, *out_is_tracked)` mirroring `alvr_oxr_get_controller_state`. Session schema unchanged (existing `controllers.hand_skeleton` Switch gates both modes). `NEXT_STEPS.md` Phase 7 hand-tracking item cross-links the doc.
- alvr `fb9f4c68`: `docs/monado-notes/PER_VIEW_FOVEATION.md` — sister doc. Per-view (eye-driven) foveation requires per-frame params travelling alongside the frame. **Wire-compat win: same as hand-tracking — additive `RealTimeConfig.per_view_foveation: Option<[FoveationView; 2]>` for the v0 ~10 Hz case, zero `alvr_packets` break.** Bridge ABI v5 sketch with two entries: `alvr_oxr_get_foveation` (encoder read) + `alvr_oxr_set_foveation` (drain-thread write). Bridge bumps 3 → 5 (4 reserved for hand-tracking-passthrough; the two pivots are independent and may land in either order). Landing order spelt out — steps 1–3 + 5 (bridge stubs, session schema, server_core glue, eye-tracking math) are non-blocked even today; step 4 is the wire-compat coordination point; step 6 is hardware-gated on NVENC regardless.

Both docs share an identical anatomy (today's state table → wire-compat decision → bridge ABI sketch → session changes → out-of-scope → wire-compat checklist → verification ceiling → recommended landing order). The shape is reusable for the next stretch-feature scoping (eye-tracking surface? Body tracking?).

### Memory updates

- `openxr-mode-integration.md` description + commit ladder refreshed: tip moves alvr `2acf1713 → 41ee8929`, dashboard polish noted as the headline-non-submodule slice this session.
- `MEMORY.md` index entry refreshed to match.

## Verification status (delta from this morning)

| Gate | This morning | Now |
| --- | --- | --- |
| A (Monado-side compile) | ✅ unchanged | ✅ unchanged |
| B (bridge ABI + builder + compositor selection) | ✅ unchanged | ✅ unchanged |
| B' (full CTest suite locally) | ✅ 25/25 in 2.02s | ✅ unchanged |
| B'' (full CTest suite on real GPU — host 101) | ✅ 25/25, ~3755 assertions | ✅ unchanged |
| **Rust workspace tests** | 16 (was the lib count after morning ops) | **14 in alvr_server_core::lib alone (was 8); 6 new for `OxrLocalAcc`** |
| **Dashboard render of `OxrFrameSummary`** | n/a | ⏸ hardware-gated (needs a connected OpenXR-mode client; same ceiling as Gates C–G) |
| CI (Actions workflow green) | ⏸ blocked at checkout (hosting) | ⏸ unchanged — same maintainer-side TODO |
| C–G (real client + headset) | ⏸ hardware-blocked | ⏸ unchanged |

## Final tips (refreshed)

| | Tip |
| --- | --- |
| alvr `origin/openxr` | `fb9f4c68` |
| openxr submodule (= fork `origin/alvr`) | `e4d281eb2` (still unchanged) |
| Bridge ABI version | `3` (unchanged in code; v4 reserved for hand-tracking passthrough, v5 for per-view foveation, per the scoping docs) |
| Test count | 14 (alvr_server_core lib) + 8 (alvr_launcher) + 25 (Monado CTest, host 101) |

## Recommended next-session start (refreshed)

1. **Re-check the hosting blocker first.** Single curl: `curl -s -o /dev/null -w "%{http_code}\n" https://github.com/AndreyVN/monado`. 200 means the maintainer pushed the fork; 404 means we're still blocked. If pushed, also confirm the `MONADO_PAT` Actions secret exists on `AndreyVN/alvr` (the workflow ran `1efed4c9` already plumbs the token).
2. **If blocker cleared**, kick off Phase 7 Slice 2 (Vulkan quad rasterisation) — it lives in `openxr/src/xrt/compositor/alvr/comp_alvr.c`. The diagnostic-only Slice 1 (layer-type histogram) is already showing what kinds of overlays apps submit; build quad support first if `quad_total` dominates the histogram on a real run.
3. **If blocker still stands**, all the Phase 7 stretch slices are now scoped with concrete next steps. Pick one and execute:
   - Hand-tracking passthrough — `HAND_TRACKING_PASSTHROUGH.md`. Steps 1 (bridge ABI v4 stub) and 3 (wire to `ServerCoreContext::get_hand_skeleton`) are alvr-only and non-blocked even today; the Monado-side `xrt_device` (step 2) is submodule-touching but only follows after step 1 lands.
   - Per-view foveation — `PER_VIEW_FOVEATION.md`. Steps 1, 2, 3, 5 of the landing order are all non-blocked. Step 1 (bridge ABI v5 stubs) is the natural starting commit.
   - Dashboard follow-up polish on the just-shipped `OxrFrameSummary` (tighten with a runtime-mode badge + "last seen Xs ago" stale indicator + serde round-trip test in `alvr_events`).
4. **Phase 3.3 (NVENC body of `alvr_oxr_submit_layers`)** stays hardware-blocked. No change.

## Commit ladder (afternoon, chronological)

```
alvr  41ee8929   feat(dashboard): surface OpenXR-mode pacing + layer-type aggregates on Statistics tab
alvr  f86a13c2   docs(openxr): scope hand-tracking passthrough for OpenXR mode
alvr  fb9f4c68   docs(openxr): scope per-view foveation for OpenXR mode
```

## Files added (afternoon)

```
docs/monado-notes/HAND_TRACKING_PASSTHROUGH.md   123 lines — Phase 7 stretch scoping (hands)
docs/monado-notes/PER_VIEW_FOVEATION.md          146 lines — Phase 7 stretch scoping (foveation)
```

## Files modified (afternoon)

```
alvr/events/src/lib.rs                                    +50 (OxrPacingSummary / OxrLayerTypesSummary / OxrFrameSummary variant)
alvr/server_core/src/statistics.rs                       +210 (OxrLocalAcc + drain glue + 6 tests)
alvr/dashboard/src/dashboard/components/statistics.rs    +80  (draw_oxr_section + last_oxr_* fields)
alvr/dashboard/src/dashboard/mod.rs                      +6   (event handler)
docs/monado-notes/NEXT_STEPS.md                          +2   (cross-links to the two scoping docs)
```

## Remote-host changes (host 192.168.10.101)

None this afternoon — the dashboard slice was code-only, no test harness changes; the scoping docs touched no binaries. State from this morning still holds:

```
C:\alvr\test-openxr\run_client_test.bat   patched to pass [needgpu] tag (morning)
C:\alvr\test-openxr\run_all_tests.bat     new — runs all 25 binaries (morning)
C:\alvr\test-openxr\tests_*.exe           24 binaries deployed (morning)
C:\alvr\test-openxr\all_tests.out         last full-suite output (morning, 25/25 green)
```
