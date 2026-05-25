#pragma once

#include "encoder/EncoderBackend.h"
#include "encoder/NvencConfig.h"

#include <cstdint>
#include <memory>
#include <vector>

// Windows OpenXR-mode NVENC encoder backend. Conforms to IEncoderBackend
// (shared lifecycle + IDR semantics with the D3D11 backend) but takes its
// per-frame input as external-memory Vulkan image handles submitted by
// Monado's comp_alvr through alvr_oxr_submit_layers.
//
// Bridge to NVENC is the CUDA driver API: NVENC has no native Vulkan-image
// input, and Monado exports VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32 (see
// vk_image_allocator.c), which can't be reopened as a D3D11 texture either. So
// each per-view image is imported via cuImportExternalMemory, mapped to a CUDA
// array, and copied (cuMemcpy2D) into the left|right half of one combined ABGR
// NVENC input frame. The encoder reuses the device-agnostic base NvEncoder with
// NV_ENC_DEVICE_TYPE_CUDA and the shared FillNvencConfig.
//
// All CUDA/NVENC state lives behind a pimpl (struct Impl, defined in the .cpp
// only when ALVR_OXR_HAVE_CUDA is set) so this header — and the rest of the
// crate — compiles on hosts without the CUDA Toolkit. There, Create() returns
// nullptr ("NVENC unavailable").

// Human-readable reason the most recent VkEncoderBackend::Create() returned
// null (diagnostic for hardware bring-up; valid until the next Create()). Never
// null. Exposed across the bridge so the Rust side can log it.
const char* VkEncoderBackendLastError();

// A sample of the most recently imported scratch image (read back from the GPU):
// dimensions, the centre-row first pixel, and min/max/non-zero byte stats. Tells
// whether the encoder is fed real pixels or zeros. Never null.
const char* VkEncoderBackendSubmitDiag();

class VkEncoderBackend : public IEncoderBackend {
public:
    // Per-frame submit payload. Mirrors the relevant fields of AlvrOxrLayer
    // (alvr/server_openxr/include/alvr_runtime_bridge.h) plus timing metadata.
    // Vulkan handles stay opaque uint64_t; the .cpp reinterprets them as Win32
    // HANDLEs for cuImportExternalMemory.
    struct SubmitDesc {
        // Native image handles from Monado, left + right view. Win32
        // OPAQUE_WIN32 HANDLE cast to uint64_t.
        uint64_t imageHandleLeft;
        uint64_t imageHandleRight;
        // Size in bytes of each view's backing device allocation — required by
        // cuImportExternalMemory. Supplied by comp_alvr across the bridge.
        uint64_t imageSizeLeft;
        uint64_t imageSizeRight;
        // Underlying VkFormat of the imported image, as the uint32_t enum value
        // (e.g. VK_FORMAT_R8G8B8A8_UNORM = 37).
        uint32_t imageFormat;
        // Per-view dimensions. The encoded frame packs both views side by side,
        // so the NVENC session is (2 * imageWidth) x imageHeight.
        uint32_t imageWidth;
        uint32_t imageHeight;
        // External timeline semaphore the caller signals at submit time. 0 =
        // "no GPU sync". Currently unused: comp_alvr does a vkQueueWaitIdle
        // before calling the bridge (Slice 2c.1), so the image is already
        // GPU-complete. A future slice replaces that CPU stall with a real
        // semaphore wait here.
        uint64_t syncSemaphoreHandle;
        uint64_t syncSemaphoreValue;
        // Timing metadata, matches Win32-OpenVR's CEncoder convention.
        uint64_t presentationTimeNs;
        uint64_t targetTimestampNs;
    };

    // Invoked once per encoded NAL/packet for the submitted frame. C-style
    // function pointer + ctx so the OpenXR bridge (Rust) can pass a callback
    // across the C ABI without C++-exception/ownership concerns.
    using PacketCallback
        = void (*)(void* ctx, const uint8_t* data, int len, bool isIdr, uint64_t targetTimestampNs);

    // Stand up the CUDA context + NVENC session for `cfg` (codec, the combined
    // encode dimensions in renderWidth/renderHeight, bitrate, tuning). Returns
    // nullptr if CUDA/NVENC is unavailable or initialization fails — the caller
    // reports "OpenXR mode cannot stream on this GPU".
    static std::unique_ptr<VkEncoderBackend> Create(const NvencConfig& cfg);

    // IEncoderBackend
    void Shutdown() override;
    void OnStreamStart() override;
    void InsertIDR() override;

    // Sequence header (SPS/PPS/VPS) NALs for the current session, for
    // alvr_server_core::set_video_config_nals. Call after Create.
    std::vector<uint8_t> GetSequenceParams();

    // Encode one frame from the submitted layers. Imports + composites both
    // views, runs NVENC, and invokes `onPacket` for each output NAL. Returns
    // false on import/encode failure; the caller logs and drops the frame.
    bool Submit(const SubmitDesc& desc, PacketCallback onPacket, void* ctx);

    ~VkEncoderBackend() override;

private:
    VkEncoderBackend();

    struct Impl;
    std::unique_ptr<Impl> m_impl;
};
