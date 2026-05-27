#pragma once

#include "alvr_server/nvEncodeAPI.h"

#include <cstdint>

// Runtime-agnostic NVENC initialize-params builder.
//
// The OpenVR (D3D11) and OpenXR (Vulkan-input) NVENC backends need identical
// NV_ENC_INITIALIZE_PARAMS / NV_ENC_CONFIG construction: same codec, preset,
// rate-control and VUI logic. That logic used to live inside
// VideoEncoderNVENC::FillEncodeConfig, reading the OpenVR-side
// Settings::Instance() singleton directly. That singleton does not exist in the
// alvr_server_openxr build, so the shared logic takes a plain NvencConfig POD
// instead: the D3D11 side fills it from Settings, the Vulkan side fills it from
// values passed across the runtime bridge.
//
// Self-contained on purpose — depends only on nvEncodeAPI.h, never on
// alvr_server/bindings.h or Logger.h (OpenVR-side glue absent from the
// server_openxr cc::Build).

class NvEncoder;

// Mirrors the ALVR enum integer values so this header needn't pull in
// ALVR-common/packet_types.h (which includes bindings.h). VideoEncoderNVENC.cpp
// static_asserts each against its ALVR/NVENC source enum, so a drift in either
// place is a compile error rather than a silent mis-encode.
enum NvencCodecKind {
    NVENC_CODEC_H264 = 0,
    NVENC_CODEC_HEVC = 1,
    NVENC_CODEC_AV1 = 2,
};
enum NvencRateControl {
    NVENC_RC_CBR = 0,
    NVENC_RC_VBR = 1,
};
enum NvencEntropyCoding {
    NVENC_ENTROPY_CABAC = 0,
    NVENC_ENTROPY_CAVLC = 1,
};
enum NvencAqMode {
    NVENC_AQ_SPATIAL = 1,
    NVENC_AQ_TEMPORAL = 2,
};

// Field names and types mirror the OpenVR Settings members they originate from
// (see Settings.h) so the D3D11-side copy stays mechanical.
struct NvencConfig {
    int codec; // NvencCodecKind
    int refreshRate;
    int renderWidth;
    int renderHeight;
    uint64_t bitrateBps;

    bool enableHdr;
    bool use10bitEncoder;
    uint32_t nvencQualityPreset;
    uint32_t nvencTuningPreset;
    int64_t nvencRefreshRate;
    bool nvencEnableWeightedPrediction;
    int64_t nvencMaxNumRefFrames;
    int64_t nvencGopLength;
    uint32_t entropyCoding; // NvencEntropyCoding
    bool nvencEnableIntraRefresh;
    int64_t nvencIntraRefreshPeriod;
    int64_t nvencIntraRefreshCount;
    bool fillerData;
    uint32_t rateControlMode; // NvencRateControl
    int64_t nvencPFrameStrategy;
    uint32_t nvencMultiPass;
    int64_t nvencLowDelayKeyFrameScale;
    uint32_t nvencAdaptiveQuantizationMode; // NvencAqMode
    int64_t nvencRateControlMode;
    int64_t nvencRcBufferSize;
    int64_t nvencRcInitialDelay;
    int64_t nvencRcMaxBitrate;
    int64_t nvencRcAverageBitrate;
};

// Populate `initializeParams` (and its referenced NV_ENC_CONFIG) from `cfg`,
// using `encoder` for CreateDefaultEncoderParams. Behaviour is identical to the
// former VideoEncoderNVENC::FillEncodeConfig.
void FillNvencConfig(
    const NvencConfig& cfg, NvEncoder* encoder, NV_ENC_INITIALIZE_PARAMS& initializeParams
);
