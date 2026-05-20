#pragma once

// Runtime-agnostic encoder backend interface.
//
// One concrete implementation per (platform, input-texture-kind) pair:
//   - D3d11EncoderBackend (cpp/encoder/win32_d3d11/) — Windows OpenVR path
//     consumes D3D11 shared textures handed in by SteamVR via
//     IVRDriverDirectModeComponent.
//   - VkEncoderBackend (cpp/encoder/win32_vk/, future Slice 3) — Windows
//     OpenXR path consumes Vulkan images submitted by Monado.
//   - The Linux encoder pipeline (cpp/encoder/linux/EncodePipeline) is
//     already runtime-agnostic in shape and will conform to this interface
//     in a later sub-slice.
//
// The lifecycle and stream-control surface lives on this interface; the
// per-frame Submit method differs per implementation (D3D11 textures vs.
// Vulkan images), so callers hold a typed pointer to the concrete class
// rather than going through a virtual dispatch for the hot path.

class IEncoderBackend {
public:
    virtual ~IEncoderBackend() = default;

    // Tear down the backend. Idempotent. Called explicitly from CEncoder's
    // destructor in the current OpenVR path to drain encoder threads
    // before the rest of the driver shuts down.
    virtual void Shutdown() = 0;

    // Phase 3.0 Slice 2.4 will add OnStreamStart / InsertIDR / SetParams
    // here once IDR scheduling and FfiDynamicEncoderParams handling move
    // out of CEncoder. Keeping the interface minimal for 2.2 so that the
    // refactor stays purely extractive.
};
