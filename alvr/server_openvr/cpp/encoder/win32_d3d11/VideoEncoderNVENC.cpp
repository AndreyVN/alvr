#include "VideoEncoderNVENC.h"
#include "NvCodecUtils.h"
#include "encoder/NvencConfig.h"

#include "alvr_server/Logger.h"
#include "alvr_server/Settings.h"
#include "alvr_server/Utils.h"

VideoEncoderNVENC::VideoEncoderNVENC(std::shared_ptr<CD3DRender> pD3DRender, int width, int height)
    : m_pD3DRender(pD3DRender)
    , m_codec(Settings::Instance().m_codec)
    , m_refreshRate(Settings::Instance().m_refreshRate)
    , m_renderWidth(width)
    , m_renderHeight(height)
    , m_bitrateInMBits(30) { }

VideoEncoderNVENC::~VideoEncoderNVENC() { }

void VideoEncoderNVENC::Initialize() {
    //
    // Initialize Encoder
    //

    NV_ENC_BUFFER_FORMAT format
        = Settings::Instance().m_enableHdr ? NV_ENC_BUFFER_FORMAT_NV12 : NV_ENC_BUFFER_FORMAT_ABGR;

    if (Settings::Instance().m_use10bitEncoder) {
        format = Settings::Instance().m_enableHdr ? NV_ENC_BUFFER_FORMAT_YUV420_10BIT
                                                  : NV_ENC_BUFFER_FORMAT_ABGR10;
    }

    Debug(
        "Initializing CNvEncoder. Width=%d Height=%d Format=%d\n",
        m_renderWidth,
        m_renderHeight,
        format
    );

    try {
        m_NvNecoder = std::make_shared<NvEncoderD3D11>(
            m_pD3DRender->GetDevice(), m_renderWidth, m_renderHeight, format, 0
        );
    } catch (NVENCException e) {
        throw MakeException(
            "NvEnc NvEncoderD3D11 failed. Code=%d %hs\n", e.getErrorCode(), e.what()
        );
    }

    NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
    NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
    initializeParams.encodeConfig = &encodeConfig;

    FillEncodeConfig(
        initializeParams,
        m_refreshRate,
        m_renderWidth,
        m_renderHeight,
        m_bitrateInMBits * 1'000'000L
    );
    try {
        m_NvNecoder->CreateEncoder(&initializeParams);
    } catch (NVENCException e) {
        if (e.getErrorCode() == NV_ENC_ERR_INVALID_PARAM) {
            throw MakeException(
                "This GPU does not support H.265 encoding. (NvEncoderCuda NV_ENC_ERR_INVALID_PARAM)"
            );
        }
        throw MakeException("NvEnc CreateEncoder failed. Code=%d %hs", e.getErrorCode(), e.what());
    }

    Debug("CNvEncoder is successfully initialized.\n");
}

void VideoEncoderNVENC::Shutdown() {
    std::vector<std::vector<uint8_t>> vPacket;
    if (m_NvNecoder)
        m_NvNecoder->EndEncode(vPacket);

    for (std::vector<uint8_t>& packet : vPacket) {
        if (fpOut) {
            fpOut.write(reinterpret_cast<char*>(packet.data()), packet.size());
        }
    }
    if (m_NvNecoder) {
        m_NvNecoder->DestroyEncoder();
        m_NvNecoder.reset();
    }

    Debug("CNvEncoder::Shutdown\n");

    if (fpOut) {
        fpOut.close();
    }
}

void VideoEncoderNVENC::Transmit(
    ID3D11Texture2D* pTexture, uint64_t presentationTime, uint64_t targetTimestampNs, bool insertIDR
) {
    auto params = GetDynamicEncoderParams();
    if (params.updated) {
        m_bitrateInMBits = params.bitrate_bps / 1'000'000;
        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        FillEncodeConfig(
            initializeParams,
            params.framerate,
            m_renderWidth,
            m_renderHeight,
            m_bitrateInMBits * 1'000'000L
        );
        NV_ENC_RECONFIGURE_PARAMS reconfigureParams = { NV_ENC_RECONFIGURE_PARAMS_VER };
        reconfigureParams.reInitEncodeParams = initializeParams;
        m_NvNecoder->Reconfigure(&reconfigureParams);
    }

    std::vector<std::vector<uint8_t>> vPacket;

    const NvEncInputFrame* encoderInputFrame = m_NvNecoder->GetNextInputFrame();

    ID3D11Texture2D* pInputTexture
        = reinterpret_cast<ID3D11Texture2D*>(encoderInputFrame->inputPtr);
    m_pD3DRender->GetContext()->CopyResource(pInputTexture, pTexture);

    NV_ENC_PIC_PARAMS picParams = {};
    if (insertIDR) {
        Debug("Inserting IDR frame.\n");
        picParams.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR;
    }
    m_NvNecoder->EncodeFrame(vPacket, &picParams);

    for (std::vector<uint8_t>& packet : vPacket) {
        uint8_t* buf = packet.data();
        int len = (int)packet.size();

        // NVENC's AV1 encoding includes a bunch of IVF wrapping,
        // so we need to strip it down to just the OBUs
        if (m_codec == ALVR_CODEC_AV1) {
            const uint8_t ivf_magic[4] = { 0x44, 0x4B, 0x49, 0x46 };
            if (len >= 4 && !memcmp(buf, ivf_magic, 4)) {
                buf += 32;
                len -= 32;
            }
            if (len <= 12) {
                continue;
            }
            buf += 12; // skip past the IVF packet size header thing
            len -= 12;
        }

        if (len <= 0) {
            continue;
        }

        if (fpOut) {
            fpOut.write(reinterpret_cast<char*>(buf), len);
        }

        ParseFrameNals(m_codec, buf, len, targetTimestampNs, insertIDR);
    }
}

void VideoEncoderNVENC::FillEncodeConfig(
    NV_ENC_INITIALIZE_PARAMS& initializeParams,
    int refreshRate,
    int renderWidth,
    int renderHeight,
    uint64_t bitrate_bps
) {
    // The NV_ENC config logic is shared with the OpenXR-mode Vulkan-input NVENC
    // backend (FillNvencConfig in encoder/NvencConfig.h). Build the
    // runtime-agnostic POD from the OpenVR Settings singleton + members and hand
    // it over. NvencConfig mirrors a few ALVR enum values so the shared header
    // needn't include bindings.h; pin them here, where both the ALVR/NVENC
    // source enums and the mirrors are in scope.
    static_assert((int)NVENC_CODEC_H264 == (int)ALVR_CODEC_H264, "codec enum drift");
    static_assert((int)NVENC_CODEC_HEVC == (int)ALVR_CODEC_HEVC, "codec enum drift");
    static_assert((int)NVENC_CODEC_AV1 == (int)ALVR_CODEC_AV1, "codec enum drift");
    static_assert((int)NVENC_RC_CBR == (int)ALVR_CBR, "rate-control enum drift");
    static_assert((int)NVENC_RC_VBR == (int)ALVR_VBR, "rate-control enum drift");
    static_assert((int)NVENC_ENTROPY_CABAC == (int)ALVR_CABAC, "entropy enum drift");
    static_assert((int)NVENC_ENTROPY_CAVLC == (int)ALVR_CAVLC, "entropy enum drift");
    static_assert((int)NVENC_AQ_SPATIAL == (int)SpatialAQ, "AQ enum drift");
    static_assert((int)NVENC_AQ_TEMPORAL == (int)TemporalAQ, "AQ enum drift");

    auto& s = Settings::Instance();
    NvencConfig cfg = {};
    cfg.codec = m_codec;
    cfg.refreshRate = refreshRate;
    cfg.renderWidth = renderWidth;
    cfg.renderHeight = renderHeight;
    cfg.bitrateBps = bitrate_bps;
    cfg.enableHdr = s.m_enableHdr;
    cfg.use10bitEncoder = s.m_use10bitEncoder;
    cfg.nvencQualityPreset = s.m_nvencQualityPreset;
    cfg.nvencTuningPreset = s.m_nvencTuningPreset;
    cfg.nvencRefreshRate = s.m_nvencRefreshRate;
    cfg.nvencEnableWeightedPrediction = s.m_nvencEnableWeightedPrediction;
    cfg.nvencMaxNumRefFrames = s.m_nvencMaxNumRefFrames;
    cfg.nvencGopLength = s.m_nvencGopLength;
    cfg.entropyCoding = s.m_entropyCoding;
    cfg.nvencEnableIntraRefresh = s.m_nvencEnableIntraRefresh;
    cfg.nvencIntraRefreshPeriod = s.m_nvencIntraRefreshPeriod;
    cfg.nvencIntraRefreshCount = s.m_nvencIntraRefreshCount;
    cfg.fillerData = s.m_fillerData;
    cfg.rateControlMode = s.m_rateControlMode;
    cfg.nvencPFrameStrategy = s.m_nvencPFrameStrategy;
    cfg.nvencMultiPass = s.m_nvencMultiPass;
    cfg.nvencLowDelayKeyFrameScale = s.m_nvencLowDelayKeyFrameScale;
    cfg.nvencAdaptiveQuantizationMode = s.m_nvencAdaptiveQuantizationMode;
    cfg.nvencRateControlMode = s.m_nvencRateControlMode;
    cfg.nvencRcBufferSize = s.m_nvencRcBufferSize;
    cfg.nvencRcInitialDelay = s.m_nvencRcInitialDelay;
    cfg.nvencRcMaxBitrate = s.m_nvencRcMaxBitrate;
    cfg.nvencRcAverageBitrate = s.m_nvencRcAverageBitrate;

    FillNvencConfig(cfg, m_NvNecoder.get(), initializeParams);
}
