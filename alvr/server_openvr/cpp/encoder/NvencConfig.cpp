#include "encoder/NvencConfig.h"

#include "NvEncoder.h"

void FillNvencConfig(
    const NvencConfig& cfg, NvEncoder* encoder, NV_ENC_INITIALIZE_PARAMS& initializeParams
) {
    auto& encodeConfig = *initializeParams.encodeConfig;

    GUID encoderGUID;
    switch (cfg.codec) {
    case NVENC_CODEC_H264:
        encoderGUID = NV_ENC_CODEC_H264_GUID;
        break;
    case NVENC_CODEC_HEVC:
        encoderGUID = NV_ENC_CODEC_HEVC_GUID;
        break;
    case NVENC_CODEC_AV1:
        encoderGUID = NV_ENC_CODEC_AV1_GUID;
        break;
    }

    GUID qualityPreset;
    // See recommended NVENC settings for low-latency encoding.
    // https://docs.nvidia.com/video-technologies/video-codec-sdk/nvenc-video-encoder-api-prog-guide/#recommended-nvenc-settings
    switch (cfg.nvencQualityPreset) {
    case 7:
        qualityPreset = NV_ENC_PRESET_P7_GUID;
        break;
    case 6:
        qualityPreset = NV_ENC_PRESET_P6_GUID;
        break;
    case 5:
        qualityPreset = NV_ENC_PRESET_P5_GUID;
        break;
    case 4:
        qualityPreset = NV_ENC_PRESET_P4_GUID;
        break;
    case 3:
        qualityPreset = NV_ENC_PRESET_P3_GUID;
        break;
    case 2:
        qualityPreset = NV_ENC_PRESET_P2_GUID;
        break;
    case 1:
    default:
        qualityPreset = NV_ENC_PRESET_P1_GUID;
        break;
    }

    NV_ENC_TUNING_INFO tuningPreset = static_cast<NV_ENC_TUNING_INFO>(cfg.nvencTuningPreset);

    encoder->CreateDefaultEncoderParams(
        &initializeParams, encoderGUID, qualityPreset, tuningPreset
    );

    initializeParams.encodeWidth = initializeParams.darWidth = cfg.renderWidth;
    initializeParams.encodeHeight = initializeParams.darHeight = cfg.renderHeight;
    initializeParams.frameRateNum = cfg.refreshRate;
    initializeParams.frameRateDen = 1;

    if (cfg.nvencRefreshRate != -1) {
        initializeParams.frameRateNum = cfg.nvencRefreshRate;
    }

    initializeParams.enableWeightedPrediction = cfg.nvencEnableWeightedPrediction;

    // 16 is recommended when using reference frame invalidation. But it has caused bad visual
    // quality. Now, use 0 (use default).
    uint32_t maxNumRefFrames = 0;
    uint32_t gopLength = NVENC_INFINITE_GOPLENGTH;

    if (cfg.nvencMaxNumRefFrames != -1) {
        maxNumRefFrames = cfg.nvencMaxNumRefFrames;
    }
    if (cfg.nvencGopLength != -1) {
        gopLength = cfg.nvencGopLength;
    }

    switch (cfg.codec) {
    case NVENC_CODEC_H264: {
        auto& config = encodeConfig.encodeCodecConfig.h264Config;
        config.repeatSPSPPS = 1;
        config.enableIntraRefresh = cfg.nvencEnableIntraRefresh;

        if (cfg.nvencIntraRefreshPeriod != -1) {
            config.intraRefreshPeriod = cfg.nvencIntraRefreshPeriod;
        }
        if (cfg.nvencIntraRefreshCount != -1) {
            config.intraRefreshCnt = cfg.nvencIntraRefreshCount;
        }

        switch (cfg.entropyCoding) {
        case NVENC_ENTROPY_CABAC:
            config.entropyCodingMode = NV_ENC_H264_ENTROPY_CODING_MODE_CABAC;
            break;
        case NVENC_ENTROPY_CAVLC:
            config.entropyCodingMode = NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC;
            break;
        }

        config.maxNumRefFrames = maxNumRefFrames;
        config.idrPeriod = gopLength;

        if (cfg.fillerData) {
            config.enableFillerDataInsertion = cfg.rateControlMode == NVENC_RC_CBR;
        }

        config.h264VUIParameters.videoSignalTypePresentFlag = 1;
        config.h264VUIParameters.videoFormat = NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        config.h264VUIParameters.videoFullRangeFlag = 1;
        config.h264VUIParameters.colourDescriptionPresentFlag = 1;
        if (cfg.enableHdr) {
            config.h264VUIParameters.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT2020;
            config.h264VUIParameters.transferCharacteristics
                = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.h264VUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT2020_NCL;
        } else {
            config.h264VUIParameters.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
            config.h264VUIParameters.transferCharacteristics
                = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.h264VUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        }
    } break;
    case NVENC_CODEC_HEVC: {
        auto& config = encodeConfig.encodeCodecConfig.hevcConfig;
        config.repeatSPSPPS = 1;
        config.enableIntraRefresh = cfg.nvencEnableIntraRefresh;

        if (cfg.nvencIntraRefreshPeriod != -1) {
            config.intraRefreshPeriod = cfg.nvencIntraRefreshPeriod;
        }
        if (cfg.nvencIntraRefreshCount != -1) {
            config.intraRefreshCnt = cfg.nvencIntraRefreshCount;
        }

        config.maxNumRefFramesInDPB = maxNumRefFrames;
        config.idrPeriod = gopLength;

        if (cfg.use10bitEncoder) {
            encodeConfig.encodeCodecConfig.hevcConfig.pixelBitDepthMinus8 = 2;
        }

        if (cfg.fillerData) {
            config.enableFillerDataInsertion = cfg.rateControlMode == NVENC_RC_CBR;
        }

        config.hevcVUIParameters.videoSignalTypePresentFlag = 1;
        config.hevcVUIParameters.videoFormat = NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        config.hevcVUIParameters.videoFullRangeFlag = 1;
        config.hevcVUIParameters.colourDescriptionPresentFlag = 1;
        if (cfg.enableHdr) {
            config.hevcVUIParameters.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT2020;
            config.hevcVUIParameters.transferCharacteristics
                = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.hevcVUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT2020_NCL;
        } else {
            config.hevcVUIParameters.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
            config.hevcVUIParameters.transferCharacteristics
                = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.hevcVUIParameters.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        }
    } break;
    case NVENC_CODEC_AV1: {
        auto& config = encodeConfig.encodeCodecConfig.av1Config;
        config.repeatSeqHdr = 1;
        config.enableIntraRefresh = cfg.nvencEnableIntraRefresh;

        if (cfg.nvencIntraRefreshPeriod != -1) {
            config.intraRefreshPeriod = cfg.nvencIntraRefreshPeriod;
        }
        if (cfg.nvencIntraRefreshCount != -1) {
            config.intraRefreshCnt = cfg.nvencIntraRefreshCount;
        }

        config.maxNumRefFramesInDPB = maxNumRefFrames;
        config.idrPeriod = gopLength;

        if (cfg.use10bitEncoder) {
            config.pixelBitDepthMinus8 = 2;
        }

        if (cfg.fillerData) {
            config.enableBitstreamPadding = cfg.rateControlMode == NVENC_RC_CBR;
        }

        config.chromaFormatIDC = 1; // 4:2:0, 4:4:4 currently not supported
        config.colorRange = 1;
        if (cfg.enableHdr) {
            config.colorPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT2020;
            config.transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.matrixCoefficients = NV_ENC_VUI_MATRIX_COEFFS_BT2020_NCL;
        } else {
            config.colorPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
            config.transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB;
            config.matrixCoefficients = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        }
    } break;
    }

    // Disable automatic IDR insertion by NVENC. We need to manually insert IDR when packet is
    // dropped if don't use reference frame invalidation.
    encodeConfig.gopLength = gopLength;
    encodeConfig.frameIntervalP = 1;

    if (cfg.nvencPFrameStrategy != -1) {
        encodeConfig.frameIntervalP = cfg.nvencPFrameStrategy;
    }

    switch (cfg.rateControlMode) {
    case NVENC_RC_CBR:
        encodeConfig.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        break;
    case NVENC_RC_VBR:
        encodeConfig.rcParams.rateControlMode = NV_ENC_PARAMS_RC_VBR;
        break;
    }
    encodeConfig.rcParams.multiPass = static_cast<NV_ENC_MULTI_PASS>(cfg.nvencMultiPass);
    encodeConfig.rcParams.lowDelayKeyFrameScale = 1;

    if (cfg.nvencLowDelayKeyFrameScale != -1) {
        encodeConfig.rcParams.lowDelayKeyFrameScale = cfg.nvencLowDelayKeyFrameScale;
    }

    uint32_t maxFrameSize = static_cast<uint32_t>(cfg.bitrateBps / cfg.refreshRate);
    encodeConfig.rcParams.vbvBufferSize = maxFrameSize * 1.1;
    encodeConfig.rcParams.vbvInitialDelay = maxFrameSize * 1.1;
    encodeConfig.rcParams.maxBitRate = static_cast<uint32_t>(cfg.bitrateBps);
    encodeConfig.rcParams.averageBitRate = static_cast<uint32_t>(cfg.bitrateBps);
    if (cfg.nvencAdaptiveQuantizationMode == NVENC_AQ_SPATIAL) {
        encodeConfig.rcParams.enableAQ = 1;
    } else if (cfg.nvencAdaptiveQuantizationMode == NVENC_AQ_TEMPORAL) {
        encodeConfig.rcParams.enableTemporalAQ = 1;
    }

    if (cfg.nvencRateControlMode != -1) {
        encodeConfig.rcParams.rateControlMode = (NV_ENC_PARAMS_RC_MODE)cfg.nvencRateControlMode;
    }
    if (cfg.nvencRcBufferSize != -1) {
        encodeConfig.rcParams.vbvBufferSize = cfg.nvencRcBufferSize;
    }
    if (cfg.nvencRcInitialDelay != -1) {
        encodeConfig.rcParams.vbvInitialDelay = cfg.nvencRcInitialDelay;
    }
    if (cfg.nvencRcMaxBitrate != -1) {
        encodeConfig.rcParams.maxBitRate = cfg.nvencRcMaxBitrate;
    }
    if (cfg.nvencRcAverageBitrate != -1) {
        encodeConfig.rcParams.averageBitRate = cfg.nvencRcAverageBitrate;
    }
}
