#include "D3d11EncoderBackend.h"

#include "VideoEncoderAMF.h"
#include "VideoEncoderNVENC.h"
#include "VideoEncoderVPL.h"
#include "alvr_server/Logger.h"
#include "alvr_server/Settings.h"
#include "alvr_server/Utils.h"

#ifdef ALVR_GPL
#include "VideoEncoderSW.h"
#endif

D3d11EncoderBackend::D3d11EncoderBackend(std::shared_ptr<VideoEncoder> videoEncoder)
    : m_videoEncoder(std::move(videoEncoder)) { }

std::unique_ptr<D3d11EncoderBackend> D3d11EncoderBackend::Create(
    std::shared_ptr<CD3DRender> d3dRender, uint32_t encoderWidth, uint32_t encoderHeight
) {
    // The selection order and exception-rethrow shape mirror the pre-refactor
    // CEncoder::Initialize byte-for-byte:
    //   GPL build only: if m_force_sw_encoding, try SW first (return on success).
    //   Then in order: AMF -> NVENC -> VPL.
    //   GPL build only: if all hardware backends fail, fall back to SW.
    //   If everything fails, throw with the collected exception messages.
    Exception vplException;
    Exception vceException;
    Exception nvencException;
#ifdef ALVR_GPL
    Exception swException;

    if (Settings::Instance().m_force_sw_encoding) {
        try {
            Debug("Try to use VideoEncoderSW.\n");
            auto encoder
                = std::make_shared<VideoEncoderSW>(d3dRender, encoderWidth, encoderHeight);
            encoder->Initialize();
            return std::unique_ptr<D3d11EncoderBackend>(new D3d11EncoderBackend(std::move(encoder)));
        } catch (Exception e) {
            swException = e;
        }
    }
#endif

    try {
        Debug("Try to use VideoEncoderAMF.\n");
        auto encoder = std::make_shared<VideoEncoderAMF>(d3dRender, encoderWidth, encoderHeight);
        encoder->Initialize();
        return std::unique_ptr<D3d11EncoderBackend>(new D3d11EncoderBackend(std::move(encoder)));
    } catch (Exception e) {
        vceException = e;
    }
    try {
        Debug("Try to use VideoEncoderNVENC.\n");
        auto encoder = std::make_shared<VideoEncoderNVENC>(d3dRender, encoderWidth, encoderHeight);
        encoder->Initialize();
        return std::unique_ptr<D3d11EncoderBackend>(new D3d11EncoderBackend(std::move(encoder)));
    } catch (Exception e) {
        nvencException = e;
    }
    try {
        Debug("Try to use VideoEncoderVPL.\n");
        auto encoder = std::make_shared<VideoEncoderVPL>(d3dRender, encoderWidth, encoderHeight);
        encoder->Initialize();
        return std::unique_ptr<D3d11EncoderBackend>(new D3d11EncoderBackend(std::move(encoder)));
    } catch (Exception e) {
        vplException = e;
    }
#ifdef ALVR_GPL
    try {
        Debug("Try to use VideoEncoderSW.\n");
        auto encoder = std::make_shared<VideoEncoderSW>(d3dRender, encoderWidth, encoderHeight);
        encoder->Initialize();
        return std::unique_ptr<D3d11EncoderBackend>(new D3d11EncoderBackend(std::move(encoder)));
    } catch (Exception e) {
        swException = e;
    }
    throw MakeException(
        "All VideoEncoder are not available. VCE: %s, NVENC: %s, VPL: %s, SW: %s",
        vceException.what(),
        nvencException.what(),
        vplException.what(),
        swException.what()
    );
#else
    throw MakeException(
        "All VideoEncoder are not available. VCE: %s, NVENC: %s, VPL: %s",
        vceException.what(),
        nvencException.what(),
        vplException.what()
    );
#endif
}

void D3d11EncoderBackend::Shutdown() {
    if (m_videoEncoder) {
        m_videoEncoder->Shutdown();
        m_videoEncoder.reset();
    }
}

void D3d11EncoderBackend::Transmit(
    ID3D11Texture2D* pTexture,
    uint64_t presentationTime,
    uint64_t targetTimestampNs,
    bool insertIDR
) {
    m_videoEncoder->Transmit(pTexture, presentationTime, targetTimestampNs, insertIDR);
}
