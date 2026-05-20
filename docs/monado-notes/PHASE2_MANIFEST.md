# Phase 2 file manifest — what to put on the fork's `alvr` branch

This document is the precise inventory of ALVR-side additions inside `openxr/` at the time of the `openxr` branch. It is the source of truth for the patch that gets cherry-picked / applied onto the `alvr-org/monado` fork's `alvr` branch when [`SUBMODULE_PIN.md`](SUBMODULE_PIN.md) Option A is executed.

## Upstream baseline

The snapshot in `openxr/` corresponds to **Monado 25.1.0** (released 2025-12-09).

* Version string: `openxr/CMakeLists.txt:7` declares `VERSION 25.1.0`.
* `openxr/doc/CHANGELOG.md` opens with `## Monado 25.1.0 (2025-12-09)`.
* `openxr/doc/changes/{auxiliary,compositor,drivers,...}/` are all empty — there are no post-release unreleased entries.
* Upstream repo and tag: `https://gitlab.freedesktop.org/monado/monado.git`, tag `v25.1.0`.

This is what the fork's `alvr` branch should be based on. After fork creation, do:

```sh
git clone https://gitlab.freedesktop.org/monado/monado.git
cd monado
git checkout -b alvr v25.1.0
git remote rename origin upstream
git remote add origin https://gitlab.freedesktop.org/<your-org>/monado.git
# ...apply the patch from this manifest...
git push -u origin alvr
```

## Phase 2 file list (additive only)

Ten new files. None of them overlap with any path in upstream Monado 25.1.0.

```
openxr/src/xrt/drivers/alvr/CMakeLists.txt
openxr/src/xrt/drivers/alvr/alvr_controller.c
openxr/src/xrt/drivers/alvr/alvr_hmd.c
openxr/src/xrt/drivers/alvr/alvr_hub.c
openxr/src/xrt/drivers/alvr/alvr_interface.h
openxr/src/xrt/drivers/alvr/alvr_internal.h
openxr/src/xrt/compositor/alvr/CMakeLists.txt
openxr/src/xrt/compositor/alvr/comp_alvr.c
openxr/src/xrt/compositor/alvr/comp_alvr.h
openxr/src/xrt/targets/common/target_builder_alvr.c
```

Per-file role:

| File | Role |
| --- | --- |
| `drivers/alvr/CMakeLists.txt` | Defines `drv_alvr` static lib; guards build behind `XRT_BUILD_DRIVER_ALVR`; resolves the `alvr_runtime_bridge.h` header from the ALVR repo (`-DALVR_BRIDGE_HEADER_DIR=...` overridable). |
| `drivers/alvr/alvr_interface.h` | Public surface: `alvr_create_devices(broadcast, out_xsysd, out_xso)`. |
| `drivers/alvr/alvr_internal.h` | Shared declarations: logging macros, `struct alvr_hmd`, `struct alvr_controller`, factory fns. |
| `drivers/alvr/alvr_hub.c` | Driver entry; constructs HMD + L/R controllers via the bridge's `alvr_oxr_init`. |
| `drivers/alvr/alvr_hmd.c` | `xrt_device` impl for the streamed HMD. Reads predicted head pose from the bridge. |
| `drivers/alvr/alvr_controller.c` | `xrt_device` impl for each controller side. Routes haptics back through the bridge. |
| `compositor/alvr/CMakeLists.txt` | Defines `comp_alvr` static lib; guarded by `XRT_FEATURE_COMP_ALVR`. |
| `compositor/alvr/comp_alvr.h` | Fake-compositor public header. |
| `compositor/alvr/comp_alvr.c` | Fake compositor stub. Will replace `comp_main`/`comp_null` when `XRT_FEATURE_COMP_ALVR=ON`. Currently returns `XRT_ERROR_NOT_IMPLEMENTED` from layer-submit. |
| `targets/common/target_builder_alvr.c` | Builder shim that wraps `alvr_create_devices` for Monado's target list. |

## Upstream files that need editing alongside the patch

These are NOT yet edited in the current snapshot. They must be added to the fork's `alvr` branch in the same patch, otherwise the new files never compile (their parent CMake dirs never traverse into `alvr/`):

| File | Required edit | Why |
| --- | --- | --- |
| `openxr/src/xrt/drivers/CMakeLists.txt` | Add `add_subdirectory(alvr)` (or equivalent `option(XRT_BUILD_DRIVER_ALVR ...)` guard + `add_subdirectory`). | Without this, `drivers/alvr/CMakeLists.txt` is never invoked. |
| `openxr/src/xrt/compositor/CMakeLists.txt` | Add `add_subdirectory(alvr)` guarded by `XRT_FEATURE_COMP_ALVR`. | Same as above for the compositor. |
| `openxr/src/xrt/targets/common/target_lists.c` | Insert `t_builder_alvr_create` entry in `target_builder_list[]` (near `qwerty`, `remote`), guarded by `#ifdef XRT_BUILD_DRIVER_ALVR`. Add `#include "../drivers/alvr/alvr_interface.h"`. | Without this, Monado's target instantiation never picks up the ALVR builder even with the gate ON. |
| `openxr/src/xrt/targets/common/target_instance.c` (line ~113) | Wire `XRT_FEATURE_COMP_ALVR` so the fake compositor is selected. | Phase 3.2 work — but the option must at least exist. |
| `openxr/CMakeLists.txt` (top-level) | Add `option(XRT_BUILD_DRIVER_ALVR "..." OFF)` and `option(XRT_FEATURE_COMP_ALVR "..." OFF)` near the other `option_with_deps` lines. | Without this, CMake doesn't recognise the gate variables. |

Phase 2 deliberately left these out because they're inside what was about to become the submodule. Now that we're doing the conversion, they go on the fork branch as part of the same patch series.

## How to verify the file list is current

If anyone edits the `openxr/src/xrt/drivers/alvr/`, `openxr/src/xrt/compositor/alvr/`, or adds further `alvr`-tagged files under `openxr/`, this manifest needs an update. To re-verify:

```sh
# Should produce exactly the 10 files above:
find openxr -name '*alvr*' -o -name '*ALVR*' 2>/dev/null \
  | grep -v ALVR_DOCS \
  | grep -v node_modules

# Should produce zero matches in upstream-named files:
grep -RE 'ALVR|alvr' openxr/src/xrt/ \
  --include='*.c' --include='*.h' --include='*.txt' \
  | grep -vE 'src/xrt/(drivers|compositor)/alvr/|target_builder_alvr.c'
```

If either command's output drifts, update this file before regenerating the patch.
