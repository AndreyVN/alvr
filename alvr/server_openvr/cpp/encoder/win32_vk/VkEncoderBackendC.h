#pragma once

#include "VkEncoderBackend.h"
#include "encoder/NvencConfig.h"

#include <cstdint>

// C-ABI shim over VkEncoderBackend for the Rust OpenXR bridge
// (alvr_server_openxr/src/lib.rs). The opaque handle is a VkEncoderBackend*.
// Every function is null-safe.
//
// NvencConfig (encoder/NvencConfig.h) is already a plain POD, so it crosses the
// ABI directly; the Rust side declares a #[repr(C)] mirror with the identical
// field order/types (repr(C) and the C++ default layout follow the same rules,
// so matching fields => matching bytes). AlvrVkSubmitDesc below is the flat
// mirror of VkEncoderBackend::SubmitDesc (which is nested in a C++ class and so
// not directly nameable from C).

extern "C" {

struct AlvrVkSubmitDesc {
    uint64_t image_handle_left;
    uint64_t image_handle_right;
    uint64_t image_size_left;
    uint64_t image_size_right;
    uint32_t image_format;
    uint32_t image_width;
    uint32_t image_height;
    uint64_t sync_semaphore_handle;
    uint64_t sync_semaphore_value;
    uint32_t sync_semaphore_handle_type;
    uint64_t consumed_semaphore_handle;
    uint64_t presentation_time_ns;
    uint64_t target_timestamp_ns;
};

typedef void (*AlvrVkPacketCallback)(
    void* ctx, const uint8_t* data, int len, bool is_idr, uint64_t target_timestamp_ns
);

// Human-readable reason the most recent alvr_vk_encoder_create returned null
// (valid until the next create call). Never null.
const char* alvr_vk_encoder_last_error();

// Create the encoder from `cfg`. Returns an opaque handle, or null if CUDA/NVENC
// is unavailable or initialization failed.
void* alvr_vk_encoder_create(const NvencConfig* cfg);
void alvr_vk_encoder_destroy(void* handle);
void alvr_vk_encoder_on_stream_start(void* handle);
void alvr_vk_encoder_insert_idr(void* handle);

// Copy up to `cap` bytes of sequence-header (SPS/PPS/VPS) NALs into `out_buf`;
// returns the full length (may exceed `cap` — pass a generous buffer).
int alvr_vk_encoder_get_seq_params(void* handle, uint8_t* out_buf, int cap);

// Encode one frame; invokes `cb` per output NAL. Returns true on success, false
// on a dropped frame (import/encode failure).
bool alvr_vk_encoder_submit(
    void* handle, const AlvrVkSubmitDesc* desc, AlvrVkPacketCallback cb, void* ctx
);
}
