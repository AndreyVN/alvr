#pragma once
#include "shared/d3drender.h"

#include "shared/threadtools.h"

#include "D3d11EncoderBackend.h"
#include "FrameRender.h"
#include "alvr_server/IDRScheduler.h"
#include "alvr_server/Utils.h"
#include <d3d11.h>
#include <d3d11_1.h>
#include <map>
#include <wincodec.h>
#include <wincodecsdk.h>
#include <wrl.h>

using Microsoft::WRL::ComPtr;

//----------------------------------------------------------------------------
// Blocks on reading backbuffer from gpu, so WaitForPresent can return
// as soon as we know rendering made it this frame.  This step of the pipeline
// should run about 3ms per frame.
//----------------------------------------------------------------------------
class CEncoder : public CThread {
public:
    CEncoder();
    ~CEncoder();

    void Initialize(std::shared_ptr<CD3DRender> d3dRender);

    void SetViewParams(
        vr::HmdRect2_t projLeft,
        vr::HmdMatrix34_t eyeToHeadLeft,
        vr::HmdRect2_t projRight,
        vr::HmdMatrix34_t eyeToHeadRight
    );

    bool CopyToStaging(
        ID3D11Texture2D* pTexture[][2],
        vr::VRTextureBounds_t bounds[][2],
        vr::HmdMatrix34_t poses[],
        int layerCount,
        bool recentering,
        uint64_t presentationTime,
        uint64_t targetTimestampNs,
        const std::string& message,
        const std::string& debugText
    );

    virtual void Run();

    virtual void Stop();

    void NewFrameReady();

    void WaitForEncode();

    void OnStreamStart();

    void InsertIDR();

    void CaptureFrame();

private:
    CThreadEvent m_newFrameReady, m_encodeFinished;
    std::unique_ptr<D3d11EncoderBackend> m_encoderBackend;
    bool m_bExiting;
    uint64_t m_presentationTime;
    uint64_t m_targetTimestampNs;

    std::shared_ptr<FrameRender> m_FrameRender;

    IDRScheduler m_scheduler;
};
