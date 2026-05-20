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

    // IDR (instantaneous decode refresh) scheduling. OnStreamStart is
    // called once per client connection; InsertIDR is called whenever a
    // keyframe is requested out-of-band (e.g. a packet-loss recovery
    // signal from the client). The backend owns its IDRScheduler instance
    // and consults it once per submitted frame; callers do not pass an
    // explicit IDR flag through the hot path.
    virtual void OnStreamStart() = 0;
    virtual void InsertIDR() = 0;

    // FfiDynamicEncoderParams handling is currently per-backend on
    // Windows (each VideoEncoder* polls GetDynamicEncoderParams() in its
    // Transmit) and explicit on Linux (EncodePipeline::SetParams). A
    // future sub-slice may unify these behind a SetParams method on this
    // interface; deferred to keep 2.3 narrowly scoped to IDR scheduling.
};
