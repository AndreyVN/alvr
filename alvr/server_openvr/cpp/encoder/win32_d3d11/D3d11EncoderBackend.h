#pragma once

#include "VideoEncoder.h"
#include "encoder/EncoderBackend.h"
#include "shared/d3drender.h"

#include <memory>

// Windows OpenVR encoder backend. Wraps the existing VideoEncoder* hierarchy
// (NVENC / AMF / VPL / SW) behind a single typed handle so CEncoder no longer
// needs to know which concrete backend was selected.
//
// Backend selection (the AMF -> NVENC -> VPL -> SW try-fallthrough that used
// to live in CEncoder::Initialize) is encapsulated by Create() below. The
// ordering and fallthrough semantics are preserved byte-faithfully so the
// produced bitstream is identical to the pre-refactor path.
class D3d11EncoderBackend : public IEncoderBackend {
public:
    // Build a D3d11EncoderBackend by trying each video encoder backend in
    // the same order CEncoder::Initialize used. Throws if none succeed.
    static std::unique_ptr<D3d11EncoderBackend>
    Create(std::shared_ptr<CD3DRender> d3dRender, uint32_t encoderWidth, uint32_t encoderHeight);

    // IEncoderBackend
    void Shutdown() override;

    // D3D11-typed hot-path call. Not on IEncoderBackend because the input
    // texture type differs per backend family.
    void Transmit(
        ID3D11Texture2D* pTexture,
        uint64_t presentationTime,
        uint64_t targetTimestampNs,
        bool insertIDR
    );

private:
    explicit D3d11EncoderBackend(std::shared_ptr<VideoEncoder> videoEncoder);

    std::shared_ptr<VideoEncoder> m_videoEncoder;
};
