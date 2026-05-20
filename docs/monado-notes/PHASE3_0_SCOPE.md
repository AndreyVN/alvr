# Phase 3.0 — encoder refactor scoping

Pre-implementation plan for the encoder refactor blocker called out in [`NEXT_STEPS.md`](NEXT_STEPS.md) §"Phase 3 — 3.0 (blocker)". Read [`/openxr-migration.md`](../../openxr-migration.md) §Phase 3 first.

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

### Slice 1 — Linux extraction (2 days)

The Linux side is already runtime-agnostic; the work is just to relocate so it's reachable from both runtimes.

1. Move `platform/linux/EncodePipeline*`, `FrameRender.{h,cpp}` (linux), `Renderer.{h,cpp}`, `ffmpeg_helper.{h,cpp}`, `FormatConverter.{h,cpp}` to `alvr/server_openvr/cpp/encoder/linux/`.
2. Keep `platform/linux/CEncoder.{h,cpp}` (the OpenVR Unix-socket receiver) in place.
3. Update `alvr_server_openvr/build.rs` includes; verify `cargo xtask build-streamer --gpl` on Linux still produces a working driver.
4. Surface `EncodePipeline::Create` to the OpenXR side via `alvr_server_openxr/build.rs`.

**Exit:** Linux-only `cargo check -p alvr_server_openxr` succeeds AND the existing `cargo xtask build-streamer --gpl` on Linux produces bit-identical NALs.

### Slice 2 — Windows interface extraction (3–4 days)

The hard one. Pure refactor — no new behavior.

1. Define `IEncoderBackend` interface in `cpp/encoder/EncoderBackend.h`.
2. Define `D3d11EncoderBackend` typed adapter wrapping existing `VideoEncoder*`. Move `VideoEncoder*` files to `cpp/encoder/win32_d3d11/`.
3. Hoist backend selection (the AMF/NVENC/VPL/SW try-fallthrough in `CEncoder::Initialize`) to `cpp/encoder/EncoderSelector.cpp`. Keep ordering identical.
4. Hoist IDR scheduling (`alvr_server/IDRScheduler.{h,cpp}`) and dynamic-params plumbing into the shared layer.
5. `CEncoder.cpp` and `OvrDirectModeComponent.cpp` continue to use `D3d11EncoderBackend` via the new interface.
6. **Verification step is the big one.** Run a controlled A/B against `master`:
   - Same headset, same SteamVR app, same Settings.video.
   - Capture 60s of encoded bitstream both ways via `dump_video_to_file`-style probe.
   - Diff byte-for-byte. **Identical** is the bar.

**Exit:** Side-by-side bitstream comparison shows zero diff. CI green. `cargo xtask build-streamer` produces a driver that loads cleanly into SteamVR.

### Slice 3 — Windows OpenXR backend (NEW, 3 days, optional within Phase 3.0)

This is the deliverable that Phase 3.1 actually needs.

1. Add `VkEncoderBackend` family under `cpp/encoder/win32_vk/`. Initially supports NVENC only (it has the cleanest Vulkan-input story via `nvenc_vulkan_swapchain` interop in newer NVENC SDK).
2. Implement `VkEncoderBackend_NVENC::Submit(VkImage, sync_handle, ...)`.
3. Wire `alvr_oxr_submit_layers` in `alvr_server_openxr/src/lib.rs` to instantiate the right backend.
4. AMF + VPL Vulkan input deferred to Phase 7 (stretch).

**Exit:** OpenXR mode on Windows + NVIDIA GPU streams a frame end-to-end. AMF/Intel/SW users get a friendly "not yet supported" error in OpenXR mode.

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
3. `alvr/server_openvr/cpp/platform/linux/EncodePipeline.h` — already-correct shape. Reference for the new `IEncoderBackend`.
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
