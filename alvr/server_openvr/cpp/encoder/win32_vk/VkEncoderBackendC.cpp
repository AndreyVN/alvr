#include "VkEncoderBackendC.h"

#include <cstring>

extern "C" {

void* alvr_vk_encoder_create(const NvencConfig* cfg) {
    if (!cfg) {
        return nullptr;
    }
    // Release ownership to the caller; alvr_vk_encoder_destroy reclaims it.
    return VkEncoderBackend::Create(*cfg).release();
}

void alvr_vk_encoder_destroy(void* handle) { delete static_cast<VkEncoderBackend*>(handle); }

void alvr_vk_encoder_on_stream_start(void* handle) {
    if (handle) {
        static_cast<VkEncoderBackend*>(handle)->OnStreamStart();
    }
}

void alvr_vk_encoder_insert_idr(void* handle) {
    if (handle) {
        static_cast<VkEncoderBackend*>(handle)->InsertIDR();
    }
}

int alvr_vk_encoder_get_seq_params(void* handle, uint8_t* out_buf, int cap) {
    if (!handle) {
        return 0;
    }
    std::vector<uint8_t> params = static_cast<VkEncoderBackend*>(handle)->GetSequenceParams();
    int len = static_cast<int>(params.size());
    if (out_buf && cap > 0) {
        int n = len < cap ? len : cap;
        std::memcpy(out_buf, params.data(), static_cast<size_t>(n));
    }
    return len;
}

bool alvr_vk_encoder_submit(
    void* handle, const AlvrVkSubmitDesc* desc, AlvrVkPacketCallback cb, void* ctx
) {
    if (!handle || !desc) {
        return false;
    }
    VkEncoderBackend::SubmitDesc d = { };
    d.imageHandleLeft = desc->image_handle_left;
    d.imageHandleRight = desc->image_handle_right;
    d.imageSizeLeft = desc->image_size_left;
    d.imageSizeRight = desc->image_size_right;
    d.imageFormat = desc->image_format;
    d.imageWidth = desc->image_width;
    d.imageHeight = desc->image_height;
    d.syncSemaphoreHandle = desc->sync_semaphore_handle;
    d.syncSemaphoreValue = desc->sync_semaphore_value;
    d.presentationTimeNs = desc->presentation_time_ns;
    d.targetTimestampNs = desc->target_timestamp_ns;
    return static_cast<VkEncoderBackend*>(handle)->Submit(d, cb, ctx);
}
}
