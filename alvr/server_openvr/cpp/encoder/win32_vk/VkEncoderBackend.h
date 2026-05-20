#pragma once

#include "encoder/EncoderBackend.h"

#include <cstdint>
#include <memory>

// Windows OpenXR encoder backend. Conforms to IEncoderBackend (shared
// lifecycle + IDR semantics with the D3D11 backend) but takes its
// per-frame input as opaque external-memory Vulkan handles, matching what
// Monado's compositor ships through alvr_oxr_submit_layers.
//
// **Skeleton stage (Phase 3.0 Slice 3.1).** All methods are stubs. The
// real implementation needs:
//
//   1. Vulkan SDK (already present at C:\VulkanSDK\1.4.341.0\) — wire
//      via alvr_filesystem-style deps lookup in alvr_server_openxr's
//      build.rs once we add cc::Build there.
//   2. NVENC SDK 12.1+ Vulkan-input support (nvEncRegisterResource with
//      NV_ENC_INPUT_RESOURCE_TYPE_VULKAN_IMAGE_HANDLE). Currently the
//      Windows D3D11 backend uses NvEncoderD3D11 which is a separate
//      surface — the new Vulkan path will not reuse NvEncoderD3D11.
//   3. External-memory import: VK_KHR_external_memory_win32 to turn the
//      uint64_t handle from Monado back into a usable VkImage in our
//      Vulkan device. Sync via VK_KHR_external_semaphore_win32 against
//      the caller-provided semaphore + value.
//   4. Cross-runtime verification: a build of Monado + alvr_server_openxr
//      + alvr_oxr_submit_layers actually pushing a frame through to a
//      headset.
//
// None of items 1–4 can be done from a Windows-only refactoring host
// without the additional setup; that's why this file is a contract
// definition rather than a working implementation. Slice 3.2+ pick this
// up when the verification environment is available.

class VkEncoderBackend : public IEncoderBackend {
public:
    // Per-frame submit payload. Mirrors the relevant fields of
    // AlvrOxrLayer (from alvr/server_openxr/include/alvr_runtime_bridge.h),
    // with timing metadata added. Vulkan types live behind opaque uint64_t
    // to keep this header free of <vulkan/vulkan.h> until the real impl
    // lands; the .cpp will reinterpret_cast back to VkImage / VkFormat /
    // HANDLE as needed.
    struct SubmitDesc {
        // Native image handles from Monado, left + right view. Win32
        // OPAQUE_WIN32 HANDLE cast to uint64_t (or DMABUF fd on Linux —
        // but the Linux Vk backend uses EncodePipeline, not this class).
        uint64_t imageHandleLeft;
        uint64_t imageHandleRight;
        // Underlying VkFormat of the imported image, as the uint32_t enum
        // value (e.g. VK_FORMAT_R8G8B8A8_UNORM = 37).
        uint32_t imageFormat;
        uint32_t imageWidth;
        uint32_t imageHeight;
        // External timeline semaphore the caller signals at submit time.
        // The backend waits on (semaphore, value) before reading the
        // image. 0 means "no GPU sync, use a fence" — only valid in
        // single-process integration tests.
        uint64_t syncSemaphoreHandle;
        uint64_t syncSemaphoreValue;
        // Timing metadata, matches Win32-OpenVR's CEncoder convention.
        uint64_t presentationTimeNs;
        uint64_t targetTimestampNs;
    };

    // Backend selection happens here once we have multiple Vk-input
    // encoders (Slice 3 ships NVENC-only; AMF/VPL Vulkan-input deferred
    // to Phase 7). For now this just throws "not implemented".
    static std::unique_ptr<VkEncoderBackend>
    Create(uint32_t encoderWidth, uint32_t encoderHeight);

    // IEncoderBackend
    void Shutdown() override;
    void OnStreamStart() override;
    void InsertIDR() override;

    // Vulkan-typed hot-path. Diverges from D3d11EncoderBackend::Transmit
    // by taking an external-memory image rather than an ID3D11Texture2D*.
    // Returns false on submit failure (e.g. handle import failure); the
    // caller logs and drops the frame.
    bool Submit(const SubmitDesc& desc);

private:
    VkEncoderBackend();
};
