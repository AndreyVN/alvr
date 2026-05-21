# Session summary — 2026-05-22

Operations / verification wrap. No new code surface added to the streamer or the bridge; instead this session built up a real-GPU verification environment for the OpenXR-mode work that landed in the 2026-05-21 session, wired Monado's existing CTest into CI, and surfaced a long-standing hosting blocker that had silently kept every previous `openxr.yml` run red.

Pairs with [`NEXT_STEPS.md`](NEXT_STEPS.md) (per-phase status) and the prior [`SESSION_SUMMARY_2026-05-21.md`](SESSION_SUMMARY_2026-05-21.md).

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

## Final tips

| | Tip |
| --- | --- |
| alvr `origin/openxr` | `1efed4c9` |
| openxr submodule (= fork `origin/alvr`) | `e4d281eb2` (unchanged from yesterday) |
| Bridge ABI version | `3` (unchanged) |
| Test count | 16 (Rust workspace) + 25 (Monado CTest, host 101) |
| New artifacts on 101 | `C:\alvr\test-openxr\run_all_tests.bat`, `all_tests.out`, 24 newly-deployed `tests_*.exe` |

## Recommended next-session start

1. **Check the hosting blocker.** If `github.com/AndreyVN/monado` now resolves and the `MONADO_PAT` secret exists on `AndreyVN/alvr`, push any small change (or rerun the latest `openxr.yml` run via the Actions UI) and confirm the workflow finally reaches the `Run Monado test suite` step → should report 25/25 pass.
2. **If the blocker still stands**, either start on the maintainer side first, OR pivot to non-submodule-touching work in the alvr repo (no shortage of options: per-view foveation bridge-ABI design, dashboard polish for the new `oxr_pacing`/`oxr_layer_types` JSON sections, alvr_packets wire-compat scoping for hand-tracking passthrough).
3. **Phase 7 Slice 2 (Vulkan quad rasterisation)** is the next coding chunk on the OpenXR critical path, but it lives in `openxr/src/xrt/compositor/alvr/comp_alvr.c` — the submodule. Don't start it until the hosting blocker is resolved or you've explicitly decided to iterate via `ALVR_MONADO_SOURCE_DIR` on the local fork clone and accept that commits won't be pushable until the fork remote exists.

## Commit ladder (chronological)

```
alvr  77264c00   ci(openxr): run Monado CTest suite after build
alvr  1efed4c9   ci(openxr): pass MONADO_PAT to checkout for the private submodule
```

## Remote-host changes (host 192.168.10.101 — not in git)

```
C:\alvr\test-openxr\run_client_test.bat   patched to pass [needgpu] tag
C:\alvr\test-openxr\run_all_tests.bat     new — runs all 25 binaries
C:\alvr\test-openxr\tests_*.exe           24 new binaries deployed (vulkan one was already there)
C:\alvr\test-openxr\all_tests.out         generated — full output of the last run_all_tests.bat run
```
