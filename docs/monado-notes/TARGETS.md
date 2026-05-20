# Targets (the shipped artifacts)

Code: `openxr/src/xrt/targets/`. Every entry here is a thin "main file" that links the right combination of subsystems into one shipped binary or library. The upstream doc is `openxr/doc/understanding-targets.md`.

Targets are how Monado *deploys* — they pick the topology (in-proc vs out-of-proc, see [ARCHITECTURE.md](ARCHITECTURE.md)), the active drivers (via `targets/common/target_lists.c`), and the platform glue.

## Targets table

| Target dir | Produces | Topology | What it links | When |
| --- | --- | --- | --- | --- |
| `openxr/` | `libopenxr_monado.{so,dll}` | Both | `oxr` + per-topology backend | Loaded by the OpenXR loader. Default OpenXR runtime artifact. |
| `openxr_android/` | `libopenxr_monado.so` (Android) | In-proc | `oxr` + targets/common + Android driver | OpenXR app on Android. |
| `service/` | `monado-service` | Out-of-proc server | `ipc/server` + `targets/common` + all drivers + compositor | Background server process on Linux desktop. |
| `service-lib/` | `libmonado-service.{so,dll}` | Out-of-proc server, embedded | Same as `service/` but as a library | When embedding the server inside another app. |
| `steamvr_drv/` | `driver_monado/bin/.../driver_monado.{so,dll}` + `driver.vrdrivermanifest` | SteamVR driver | `state_trackers/steamvr_drv` + drivers/** + `targets/common` (no compositor — SteamVR composes) | Drop into `Steam/steamapps/common/SteamVR/drivers/`; SteamVR loads it like any vrdriver. |
| `libmonado/` | `libmonado.{so,dll}` + headers + bindings | Library | Service inspection / control API | Tools and external integrations that want to query a running `monado-service`. Headers: `targets/libmonado/monado.h`. Example consumers in the same dir (`example.c`, `example.py`, `example.lua`). |
| `cli/` | `monado-cli` | Tool | `targets/common` + minimal IO | Probe hardware, dump configs, run device-specific commands. |
| `gui/` | `monado-gui` | Tool | `state_trackers/gui` + `targets/common` | Standalone debug GUI window (not embedded inside the runtime). |
| `ctl/` | `monado-ctl` | Tool | `libmonado` | Send control commands to a running `monado-service`. |
| `sdl_test/` | `monado-sdl-test` | Test app | SDL2 + the whole stack | Self-contained sample that opens a window, presents through the compositor, useful when debugging the renderer without a real headset. |
| `common/` | (helper static lib) | — | `target_instance.c`, `target_instance_no_comp.c`, `target_builder_*.c`, `target_lists.c` | Pulled in by every executable / library target except the pure proxy in `openxr/`. |
| `android_common/` | (helper static lib, Android) | — | Android-specific glue | Pulled in by Android targets. |

## How the OpenXR target picks topology

`targets/openxr/target.c` (47 lines total) is the entire entry of `libopenxr_monado`:

```c
#ifdef XRT_FEATURE_IPC_CLIENT
    // Out-of-process: forward to ipc_instance_create, which opens a socket to monado-service.
    xrt_result_t xrt_instance_create(struct xrt_instance_info *ii, struct xrt_instance **out_xinst) {
        return ipc_instance_create(ii, out_xinst);
    }
#else
    // In-process: xrt_instance_create is provided by targets/common/target_instance.c
    // and the prober + builders + compositor are linked in directly.
#endif
```

Everything else — OpenXR loader negotiation, session, layers, swapchains — is the same in both topologies because it goes through the `xrt_*` interfaces.

## How `target_lists.c` controls what's compiled in

`targets/common/target_lists.c` is regenerated implicitly by cmake choosing which `XRT_BUILD_DRIVER_*` and `T_BUILDER_*` flags to define. Each `#ifdef` block adds (or doesn't) a builder/auto-prober/USB-entry to the lists. To strip a driver out of every target binary, turn off its cmake flag; to add one, add the `*_interface.h` include + the appropriate entry.

The three lists feeding `target_lists`:
* `target_builder_list[]` — explicit builders (run first; can override hardware).
* `target_entry_list[]` — USB VID/PID matches; `target_entry_lists[]` is the (`NULL`-terminated) list-of-lists in case targets want to add their own.
* `target_auto_list[]` — auto-probers, run last.

The order matters: qwerty → remote → simulated → real hardware → legacy fallbacks. See `target_lists.c:114-164`.

## `targets/service/main.c` — the server

`service/main.c` is ~60 lines. All it does is:
1. On Windows, raise priority/privileges via `u_win_try_privilege_or_priority_from_args`.
2. `u_trace_marker_init()` + `u_metrics_init()`.
3. Build an `ipc_server_main_info` with debug-GUI title + an `exit_on_disconnect` flag.
4. Call `ipc_server_main(argc, argv, &ismi)` which is the real entry point in `ipc/server/ipc_server_process.c`.

That's the entire server bring-up; everything interesting is inside `ipc_server_main`.

## `targets/steamvr_drv/main.c` — driver_monado

30 lines. Defines `HmdDriverFactory` as the SteamVR-required DLL export, which delegates to `ovrd_hmd_driver_impl(pInterfaceName, pReturnCode)` (implemented in `state_trackers/steamvr_drv/`). Companion files in the same target dir handle install: `driver.vrdrivermanifest`, `steamvr.vrsettings`, plus `copy_plugin.py` + `copy_assets.py` build helpers.

## `targets/libmonado/` — external control API

C ABI exposed to tools that want to talk to a running `monado-service`. Headers:
* `monado.h` — the C interface (clients, devices, properties).
* `libmonado.def` — Windows symbol exports.

Sample consumers shipped in the same folder:
* `example.c` — minimal C usage.
* `example.py` + `monado.py` — Python ctypes bindings.
* `example.lua` — Lua FFI bindings.

This is the API `monado-ctl` and external diagnostic GUIs use.

## Build outputs in practice

After `cmake --build .`, expect (Linux example):

```
build/src/xrt/targets/openxr/libopenxr_monado.so          ← OpenXR runtime
build/src/xrt/targets/service/monado-service              ← server binary
build/src/xrt/targets/libmonado/libmonado.so              ← control API
build/src/xrt/targets/cli/monado-cli                      ← CLI tool
build/src/xrt/targets/gui/monado-gui                      ← GUI tool
build/src/xrt/targets/ctl/monado-ctl                      ← controller for the service
build/src/xrt/targets/sdl_test/monado-sdl-test            ← sample app
build/src/xrt/targets/openxr/openxr_monado-dev.json       ← runtime manifest with absolute path
```

The dev manifest is what `XR_RUNTIME_JSON=...` should be set to during development. Production installs replace it with `/etc/xdg/openxr/1/active_runtime.json` (or `~/.config/openxr/1/active_runtime.json`).

On Windows the same layout but `.dll` / `.exe`, with an additional `driver_monado.dll` if SteamVR support is built.
