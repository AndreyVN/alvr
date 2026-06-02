# Phase 3.0 — encoder refactor scoping

> **STATUS 2026-06-02 — ✅ ESSENTIALLY COMPLETE.** The refactor (Option C, adapter pattern) shipped: Slice 1 (Linux relocation), Slice 2.1–2.3 (Windows `IEncoderBackend` extraction + `D3d11EncoderBackend` + IDR-scheduler hoist), and Slice 3.1–3.3 (`VkEncoderBackend` + the CUDA-interop NVENC `Submit` body wired to `alvr_oxr_submit_layers`, **landed 2026-05-27**, proven e2e on RTX 3090 + Quest 3). Both runtimes share `cpp/encoder/`. **Remaining tails are optional cleanup, not blockers:** sub-slice 2.4 (Linux `EncodePipeline` → `IEncoderBackend`); `FfiDynamicEncoderParams` unification; Linux `FrameRender` ↔ `protocol.h` decoupling. **Verification debt:** the W5 OpenVR byte-diff A/B harness was never written (Slice-2 "bit-identical" exit gate is trusted-on-review, not proven) and the Slice-1 Linux `--gpl` byte-diff is Linux-host-gated. Slice/decision detail below is preserved as the historical record; where a slice says "DEFERRED" check the inline status notes — Slice 3.3 in particular landed despite its heading.

Pre-implementation plan for the encoder refactor blocker called out in [`NEXT_STEPS.md`](NEXT_STEPS.md) §"Phase 3 — 3.0". Read [`/openxr-migration.md`](../../openxr-migration.md) §Phase 3 first.

## Constraint (from CLAUDE.md rule §"How to not break the existing OpenVR mode")

> Encoder refactor in `alvr/server_openvr/cpp/` (Phase 3.0) must be **purely extractive** — extract a runtime-agnostic interface without changing OpenVR-facing behaviour.

Translation: `cargo xtask build-streamer && SteamVR ALVR session` must produce **bit-identical** video stream (same encoder selection, same NAL contents, same timing, same configs round-trip) before vs after the refactor.

## Current state of `alvr/server_openvr/cpp/`

```
alvr_server/          ← OpenVR driver glue + bindings.h + shared helpers (~25 files, mostly tracking/pose/IDR)
platform/win32/       ← Windows encoder + D3D11 frame composition
platform/linux/       ← Linux encoder + Vulkan frame composition
platform/macos/       ← (stub)
shared/               ← amf SDK + threadtools + backward-cpp
```

### Windows encoder path

| File | Lines | Role |
| --- | --- | --- |
| `OvrDirectModeComponent.{h,cpp}` | 75 + 347 | Implements `vr::IVRDriverDirectModeComponent`. Receives D3D11 shared-texture handles from SteamVR, manages the swap-texture-set lifecycle, calls `CEncoder::CopyToStaging`. **Pure SteamVR coupling — cannot move.** |
| `CEncoder.{h,cpp}` | 80 + 162 | Worker thread (`CThread`-derived). Owns `FrameRender` (D3D11) + a `VideoEncoder`. Backend selection: AMF → NVENC → VPL → SW (gpl-only). |
| `FrameRender.{h,cpp}` | 172 + 894 | D3D11 frame composition: RGB→YUV, foveated rendering (FFR), color correction, layer compositing. |
| `VideoEncoder.{h,cpp}` | 19 + 1 | Abstract base: `Initialize() / Shutdown() / Transmit(ID3D11Texture2D*, presentationTime, targetTimestamp, insertIDR)`. |
| `VideoEncoderNVENC.{h,cpp}` | 45 + 418 | NVENC backend via `NvEncoderD3D11`. |
| `VideoEncoderAMF.{h,cpp}` | 116 + 829 | AMD AMF backend. |
| `VideoEncoderVPL.{h,cpp}` | 64 + 463 | Intel oneVPL (formerly MSDK) backend. |
| `VideoEncoderSW.{h,cpp}` | 60 + 306 | FFmpeg software fallback (gpl-only). |
| `NvEncoder.{h,cpp}` + `NvEncoderD3D11.{h,cpp}` | 448+1058 + 55+152 | NVENC SDK helpers. |

**Coupling pattern:** all four `VideoEncoder` backends take `ID3D11Texture2D*` as their per-frame input. The texture comes from `FrameRender`, which itself runs entirely in D3D11.

### Linux encoder path

| File | Lines | Role |
| --- | --- | --- |
| `CEncoder.{h,cpp}` | 36 + 317 | OpenVR-specific Unix-socket receiver. Connects to the `alvr_vulkan_layer` (Linux only), receives DMABUF handles from intercepted `vrcompositor` work, and pushes frames into `EncodePipeline`. **OpenVR coupling lives here.** |
| `EncodePipeline.{h,cpp}` | 56 + 88 | Already runtime-agnostic. Factory `Create(Renderer*, VkContext&, VkFrame&, ...)`. Per-frame call: `PushFrame(uint64_t targetTimestampNs, bool idr)` + `GetEncoded(FramePacket&)`. |
| `EncodePipelineNvEnc.{h,cpp}` | + 231 | Vulkan→NVENC (via FFmpeg's `h264_nvenc`/`hevc_nvenc`/`av1_nvenc` with `vulkan` hwaccel). |
| `EncodePipelineVAAPI.{h,cpp}` | + 424 | Vulkan→VAAPI (AMD/Intel on Linux). |
| `EncodePipelineSW.{h,cpp}` | + 144 | FFmpeg software fallback. |
| `Renderer.{h,cpp}` | + 1198 | Vulkan renderer/blitter (foveation, format conversion). |
| `FrameRender.{h,cpp}` | + 195 | Vulkan frame composition. |

**Coupling pattern:** the encoder backends already take Vulkan input. The OpenVR-specific code is `CEncoder.cpp` (the socket receiver). The OpenXR-side adapter on Linux can just instantiate `EncodePipeline` directly — no per-backend changes needed.

## The asymmetry that drives this refactor

* **Linux**: encoder backends are already runtime-agnostic (Vulkan input). The runtime coupling is contained in `CEncoder.cpp` (Linux), which is the Unix-socket bridge to the Vulkan layer.
* **Windows**: encoder backends are D3D11-coupled. Monado submits Vulkan textures, not D3D11. **This is the actual hard part of Phase 3.0.**

## Three options for the Windows path

### Option A — Two separate encoder paths (skip unification)

Keep `VideoEncoder*` D3D11-based for OpenVR. Write a parallel `VkVideoEncoder*` hierarchy for OpenXR mode. Both runtimes implement their own backend selection.

* **Pros:** zero risk to OpenVR mode. Fast to ship. NVENC and AMF have direct Vulkan-input APIs we can use for the OpenXR path.
* **Cons:** duplicated backend selection logic. Bug fixes need both paths. Doesn't actually meet "extract a runtime-agnostic interface" — there is no shared interface, just code separation.
* **Estimate:** 3–4 days for OpenXR-side encoders alone.

### Option B — Pivot all encoders to Vulkan input

Rewrite the Win32 encoder hierarchy to take `VkImage` instead of `ID3D11Texture2D*`. OvrDirectModeComponent does D3D11→Vulkan interop (via `VK_KHR_external_memory_win32`) before handing the image to the encoder. OpenXR mode skips the interop and feeds VkImage directly.

* **Pros:** truly runtime-agnostic; one encoder hierarchy serves both. Aligns with the Linux design.
* **Cons:** every Win32 encoder backend changes input format. AMF and NVENC support Vulkan input natively but the existing code uses `NvEncoderD3D11`/AMF's DirectX path, not their Vulkan paths — that's a real port. VPL (Intel oneVPL) prefers D3D11 surfaces; a Vulkan→D3D11 export shim would be needed. Highest risk of regressing OpenVR mode.
* **Estimate:** 8–12 days. Probably needs `--gpl` software-fallback verification as the last step.

### Option C — Adapter pattern (recommended)

Define a polymorphic interface `IEncoderBackend` in a new file, with two implementation families:
* `D3d11EncoderBackend` — wraps the existing `VideoEncoder*` hierarchy with a typed adapter taking `ID3D11Texture2D*`.
* `VkEncoderBackend` — new family for OpenXR mode, taking `VkImage` + a sync semaphore handle.

Both families implement:
```cpp
class IEncoderBackend {
public:
    virtual ~IEncoderBackend() = default;
    virtual void Initialize(EncoderConfig cfg) = 0;
    virtual void Shutdown() = 0;
    virtual void SetParams(FfiDynamicEncoderParams params) = 0;
    virtual void OnStreamStart() = 0;
    virtual void InsertIDR() = 0;
    // Frame submission shapes diverge — solved by NOT putting Submit on the
    // interface and instead using a typed factory:
    //   std::unique_ptr<D3d11EncoderBackend> CreateD3d11Encoder(...);
    //   std::unique_ptr<VkEncoderBackend>    CreateVkEncoder(...);
};
```

A typed adapter at instantiation time means callers know which kind they have and call the appropriate `Submit*` method on it. The shared piece is the lifecycle + dynamic-params + IDR-scheduler logic, which is small but worth deduplicating.

Backend selection (AMF vs NVENC vs VPL vs SW) is hoisted up so both runtimes share the priority order and the platform fallback rules (one shared function in a new `EncoderSelector.cpp`).

* **Pros:** OpenVR-facing code paths unchanged (D3D11 stays D3D11). The "extraction" is real — IDR scheduling, dynamic params, backend selection are unified. OpenXR mode gets a fresh `VkEncoderBackend` family without forcing changes to the OpenVR backends.
* **Cons:** still some code duplication between the two backend families. The shared interface is thin.
* **Estimate:** 5–7 days.

## Cross-platform shape

Independent of Win32 decision, the C/Rust ABI boundary needs to be unified:

* Today `alvr_server_openvr/src/lib.rs` uses ~50 `bindings::*` functions generated from `bindings.h`. The encoder-relevant ones are `InitializeStreaming`, `DeinitializeStreaming`, `GetDynamicEncoderParams`, `SetVideoConfigNals`, `VideoSend`, `RequestIDR`, `ReportPresent`, `ReportComposed`.
* `alvr_server_openxr/src/lib.rs` currently has stub bridge functions but does not yet wire any of these. Phase 3.1 plumbs them through.
* Suggestion: **do not** create a new `alvr/encoder/` crate as the migration plan tentatively suggests. The encoder is platform-specific C++ that gets built once; the right home is `alvr/server_openvr/cpp/encoder/` (a new sibling to `alvr_server/`, `platform/`, `shared/`) referenced by both `alvr_server_openvr` and `alvr_server_openxr`'s build.rs.

## Recommended scoping → Option C in three slices

### Slice 1 — Linux extraction (LANDED 2026-05-20)

The Linux side is already runtime-agnostic; the work is just to relocate so it's reachable from both runtimes.

Final scope (smaller than originally drafted, because `FrameRender.h` `#include`s `protocol.h` and `protocol.h` is shared with `alvr_vulkan_layer` — both stayed in `platform/linux/`):

1. ✅ Moved to `alvr/server_openvr/cpp/encoder/linux/`: `EncodePipeline.{h,cpp}`, `EncodePipelineNvEnc.{h,cpp}`, `EncodePipelineSW.{h,cpp}`, `EncodePipelineVAAPI.{h,cpp}`, `Renderer.{h,cpp}`, `ffmpeg_helper.{h,cpp}`, `FormatConverter.{h,cpp}` (14 files).
2. ✅ Kept in `platform/linux/`: `CEncoder.{h,cpp}` (OpenVR Unix-socket receiver), `FrameRender.{h,cpp}` (uses `protocol.h`), `protocol.h` (shared with Vulkan layer), `CrashHandler.cpp`, `shader/`.
3. ✅ `alvr_server_openvr/build.rs`: added `encoder` to common-walker exclusions, walks `cpp/encoder/linux` on Linux, and adds it to the include path so sibling-style `#include "X.h"` from `CEncoder.cpp` / `FrameRender.h` still resolves moved headers.
4. ⏭ `alvr_server_openxr/build.rs` not modified — there's nothing to compile yet. Slice 2/Phase 3.1 will add that crate's cc::Build when there's a `VkEncoderBackend` family to compile against.

**Verified from Windows host:** `cargo check -p alvr_server_openvr -p alvr_server_openxr` passes — confirms the Win32 build doesn't try to pick up Linux files (the walker exclusion works).

**STILL TO DO from a Linux host before merge:**
- `cargo xtask build-streamer --gpl` succeeds.
- Side-by-side stream against `master`: bitstream byte-diff is zero across NVENC, VAAPI, and SW backends.

`FrameRender` decoupling from `protocol.h` is left to a later slice (3.2 or 3.5) — it's a real cross-runtime issue but doesn't block Slice 2 or Phase 3.1.

### Slice 2 — Windows interface extraction (3–4 days)

The hard one. Pure refactor — no new behavior. Broken into sub-slices so each is reviewable and compile-testable:

**Sub-slice 2.1** — relocate `VideoEncoder*`, `NvEncoder*`, `NvEncoderD3D11*`, `NvCodecUtils.h` from `cpp/platform/win32/` to `cpp/encoder/win32_d3d11/` (15 files). Update `build.rs` so the new dir is on the source walk + include path on Windows, and `cpp/platform/win32/` stays on the include path so the moved files keep resolving `"shared/d3drender.h"` (a genuinely cross-cutting D3D11 utility — relocating it cleanly is left as a follow-up to keep 2.1 purely extractive). **LANDED 2026-05-20.** Verified: `cargo build -p alvr_server_openvr` cleanly re-links all moved files; `cargo check -p alvr_server_openxr` unchanged.

**Sub-slice 2.2 — LANDED 2026-05-20** — defined empty `IEncoderBackend` interface (`cpp/encoder/EncoderBackend.h`, only `virtual ~IEncoderBackend() = default` + a `Shutdown()` method so far) and a `D3d11EncoderBackend` typed adapter (`cpp/encoder/win32_d3d11/D3d11EncoderBackend.{h,cpp}`) that owns the AMF → NVENC → VPL → SW try-fallthrough selection logic verbatim from the pre-refactor `CEncoder::Initialize`. `CEncoder` now owns a `unique_ptr<D3d11EncoderBackend>` instead of a `shared_ptr<VideoEncoder>`; the worker thread's `Transmit` call goes through the new wrapper. Selection ordering and exception-rethrow shape are byte-faithful, so bitstream output is identical. Verified: full `cargo build -p alvr_server_openvr` rebuild succeeds on Windows.

**Sub-slice 2.3 — LANDED 2026-05-20** — hoisted `IDRScheduler` ownership from `CEncoder` into `D3d11EncoderBackend`. Added `OnStreamStart()` / `InsertIDR()` to `IEncoderBackend`; `D3d11EncoderBackend::Transmit` no longer takes an explicit `insertIDR` parameter (the backend consults its owned scheduler internally right before forwarding to the underlying `VideoEncoder`). `CEncoder::OnStreamStart()` / `InsertIDR()` collapse to single-line pass-throughs. Note: the originally-scoped "backend selection hoist" was already done in Slice 2.2 (smaller-than-planned commit boundaries). `FfiDynamicEncoderParams` handling deliberately not hoisted yet — on Windows each `VideoEncoder*` polls `GetDynamicEncoderParams()` directly in its hot path, so there is no CEncoder-level plumbing to hoist; a unification with Linux's explicit `EncodePipeline::SetParams` is deferred to a later cleanup.

**Sub-slice 2.4 — DEFERRED to Phase 3.1** — originally planned to make Linux's `EncodePipeline` conform to `IEncoderBackend`. Re-evaluated 2026-05-20: not actually required for Slice 3 (`VkEncoderBackend` only needs the current `IEncoderBackend` shape), and forcing it now would (a) require Linux-side compile verification we cannot run from a Windows host, and (b) require merging two structurally different submission patterns (Windows synchronous `Transmit` vs Linux queue-based `PushFrame`/`GetEncoded`). Re-pick this up in Phase 3.1, when `alvr_server_openxr/src/lib.rs` actually wires up the Linux backend and the merge can be informed by real consumers.

**Phase 3.1.2 (LANDED 2026-05-20, related work).** Event-drain thread now spawned in `alvr_oxr_init`; `alvr_oxr_get_head_pose` and `alvr_oxr_poll_session_event` are real. `ClientConnected/Disconnected/ShutdownPending` are translated to `AlvrOxrEvent` variants the drain thread pushes to a `SESSION_EVENTS_RX` queue. `LocalViewParams` cached in `LOCAL_VIEW_PARAMS` ready for future per-eye APIs. Tracking events drained without explicit handling — `get_head_pose` reads directly via `context.get_device_motion`. Tracked as Phase 3.1 work, not Phase 3.0; included in the scope-doc for continuity.

**Phase 3.1.6 (LANDED 2026-05-21, related work).** Battery event wiring on the bridge: added `AlvrOxrEventType::Battery = 2`, `ALVR_OXR_DEVICE_KIND_{HMD,LEFT_CONTROLLER,RIGHT_CONTROLLER}` discriminants, and `ALVR_OXR_BATTERY_GAUGE_SCALE = 10 000`. Drain thread now translates `ServerCoreEvent::Battery` into `AlvrOxrEvent { event_type: Battery, data: [kind, gauge_bp, plugged, 0] }`. Unknown device IDs are dropped silently. RefreshRate intentionally not wired — no upstream `ServerCoreEvent::RefreshRate` exists; the enum value stays reserved.

---

**Verification gate (after each Slice-2 sub-slice).** Run a controlled A/B against `master`:
- Same headset, same SteamVR app, same Settings.video.
- Capture 60s of encoded bitstream both ways via `dump_video_to_file`-style probe.
- Diff byte-for-byte. **Identical** is the bar.

**Exit:** Side-by-side bitstream comparison shows zero diff after Sub-slice 2.3. CI green. `cargo xtask build-streamer` produces a driver that loads cleanly into SteamVR. Sub-slice 2.4 deferred per note above.

### Slice 3 — Windows OpenXR backend (NEW)

Three sub-slices. Originally estimated 3 days total in one shot, but the verifiability profile is very different from Slice 1/2 (refactors) — Slice 3 is net-new code that needs a real Vulkan/NVENC/Monado stack, so the cheap-to-verify "skeleton" was carved out as its own sub-slice.

**Sub-slice 3.1 — LANDED 2026-05-20**: `cpp/encoder/win32_vk/VkEncoderBackend.{h,cpp}` skeleton. Class conforms to `IEncoderBackend` (same `Shutdown` / `OnStreamStart` / `InsertIDR` as the D3D11 backend). `Submit(SubmitDesc)` takes opaque external-memory Vulkan handles + sync semaphore + timing — the `SubmitDesc` field layout mirrors `AlvrOxrLayer` from the bridge header so future wiring is a straight copy. `Create` throws `std::runtime_error`; all per-frame methods stub out. Deliberately self-contained: no `alvr_server/Logger.h` or `Utils.h` includes so this TU can compile in any future `alvr_server_openxr` `cc::Build` without dragging in the OpenVR-side logging-callback glue. Verified: `g++ -std=c++17 -Wall -Wextra` compiles clean. **Not yet integrated into any build** — design landing only.

**Sub-slice 3.2 — LANDED 2026-05-20**: `alvr_server_openxr/build.rs` gains a `cc::Build` step that compiles the `win32_vk/` skeleton on Windows. Vulkan SDK include path picked up from `$VULKAN_SDK` (LunarG installer convention); `vulkan-1.lib` link is deliberately deferred to 3.3 since the skeleton uses opaque uint64_t handles and references no Vulkan symbols. Cross-crate include of `../server_openvr/cpp` resolves the shared `encoder/EncoderBackend.h`. Produces a 178KB `alvr_server_openxr_encoder.lib` bundled into the cdylib. Bridge ABI unchanged.

**Sub-slice 3.3 — ✅ LANDED 2026-05-27** (proven e2e on RTX 3090 + Quest 3; see NEXT_STEPS §3.4). Implemented `VkEncoderBackend::Submit` and wired `alvr_oxr_submit_layers`. **Shipped via CUDA-interop, not the originally-anticipated `nvEncRegisterResource` Vulkan-image path:** the submitted per-view Vulkan images (OPAQUE_WIN32 external memory) are imported into CUDA via `VK_KHR_external_memory_win32`, composited into the combined NVENC input frame, and encoded (H.264/HEVC) — the CUDA driver API (`cuda.h`/`cudaTypedefs.h`) is loaded dynamically from `nvcuda.dll` at runtime (`win32_vk/CudaDriverApi.h` + `VkEncoderBackendC.cpp`), so no `cuda.lib` link and no NVENC-SDK-version bump was needed. The C-ABI between `server_openxr/src/lib.rs` and the static lib lives in the inline `encoder_bridge` module. The three "still left" items the earlier draft listed (NVENC SDK Vulkan entry points, the Rust↔lib C-ABI, and the `AlvrOxrLayer[]` forwarding) are all done — the SDK-upgrade item became moot because the CUDA path sidesteps NVENC's Vulkan-image registration entirely.

**Sub-slice 3.4 (Phase 7 stretch)**: AMF + VPL Vulkan input. NVENC-only is the Slice 3 ship target.

**Exit (Slice 3): ✅ MET 2026-05-27.** OpenXR mode on Windows + NVIDIA streams frames end-to-end (verified RTX 3090 + Quest 3). AMF/Intel/SW in OpenXR mode remain unsupported (Sub-slice 3.4, Phase 7 stretch). 3.2/3.3 were verified on the RTX 3090 test host once the Vulkan + NVENC + Monado stack was in place.

## Risk register

| Risk | Mitigation |
| --- | --- |
| Bitstream regression on OpenVR mode | Slice 2's byte-diff is the gate. Don't merge Slice 2 unless the diff is zero across all four backends. |
| `IDRScheduler` reordering breaking timing | Pull it out behind a feature flag in slice 2 so we can A/B with old code path. |
| AMF SDK / VPL SDK API drift when files move | None expected — we're moving files, not bumping versions. Verify `deps/` paths in `build.rs` after relocation. |
| Cross-platform CMake-equivalent in `cc::Build` | `alvr_server_openvr/build.rs` uses `cc::Build`, not CMake. Path-list changes only; no toolchain risk. |
| Windows Vulkan input requires newer NVENC SDK | We pin NVENC SDK 12.2 already (`alvr_server_openvr/cpp/platform/win32/NvEncoder.h` includes nvEncodeAPI.h v12.2). Vulkan input is supported from 12.1. |
| Linux runtime-agnostic split affecting the Vulkan-layer flow | The Vulkan layer (`alvr_vulkan_layer`) is OpenVR-only and uses a Unix socket. It's not touched by Slice 1. |

## Files a Phase 3 implementer should read first

After this scope doc, in order:

1. `alvr/server_openvr/cpp/platform/win32/CEncoder.cpp` — the backend-selection logic. Anchor for Slice 2.
2. `alvr/server_openvr/cpp/platform/win32/VideoEncoder.h` — the existing interface. The new `D3d11EncoderBackend` wraps this.
3. `alvr/server_openvr/cpp/encoder/linux/EncodePipeline.h` — already-correct shape. Reference for the new `IEncoderBackend`.
4. `alvr/server_openvr/cpp/platform/win32/OvrDirectModeComponent.cpp` — the SteamVR-coupled entry point. Don't change it in Slice 2.
5. `alvr/server_openvr/cpp/alvr_server/bindings.h` — the Rust↔C++ ABI. Encoder-relevant symbols are listed in the "Cross-platform shape" section above.
6. `alvr/server_openvr/build.rs` — the `cc::Build` source-list. Path changes during slice moves.
7. `alvr/server_openxr/src/lib.rs` — the 10 stubs awaiting Phase 3.1 wiring after Slice 2 is done.

## Decisions

| | Question | Decided | Decided when |
| --- | --- | --- | --- |
| W1 | Option A (parallel) vs B (full Vulkan port) vs C (adapter) | **C — adapter pattern.** `IEncoderBackend` interface + typed factories; OpenVR D3D11 path untouched, OpenXR gets a new `VkEncoderBackend` family. | 2026-05-20 |
| W2 | New crate `alvr/encoder/` vs new directory `alvr/server_openvr/cpp/encoder/` | **Directory.** Build system is per-crate `cc::Build`; a new crate adds boilerplate without buying anything. New tree: `cpp/encoder/{EncoderBackend.h, EncoderSelector.cpp, linux/, win32_d3d11/, win32_vk/}`. | 2026-05-20 |

## Open decisions

| | Question | Recommendation |
| --- | --- | --- |
| W3 | Linux move (Slice 1) before or after Windows interface extraction (Slice 2)? | Linux first — lower risk, lower scope, teaches us where the cross-runtime boundary sits before tackling Windows. |
| W4 | Slice 3 in this Phase 3.0 or in Phase 3.1? | If Slice 1+2 stay on budget, do Slice 3 in 3.0 — it unblocks 3.1. Otherwise punt. |
| W5 | Bitstream-diff verification harness — write fresh or reuse existing? | Write fresh, small (~200 lines): two driver processes side-by-side, both dump encoded NALs to file, `cmp` the files. Throwaway after Slice 2. |
