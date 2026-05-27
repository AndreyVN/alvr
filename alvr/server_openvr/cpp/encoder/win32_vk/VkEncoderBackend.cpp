#include "VkEncoderBackend.h"

#include <stdexcept>
#include <string>

// Last-error diagnostic, shared by both build variants. Set at every Create()
// failure point so the Rust bridge can log *why* the encoder didn't come up.
namespace {
std::string g_vk_encoder_last_error = "no error recorded";
}
const char* VkEncoderBackendLastError() { return g_vk_encoder_last_error.c_str(); }

// 3.3b-2 implements the real CUDA-interop NVENC encoder when the CUDA Toolkit is
// present at build time (build.rs defines ALVR_OXR_HAVE_CUDA). Without it — CI's
// NVIDIA-less windows-2022 runner — the stubs below compile and Create() reports
// "unavailable", so the cdylib and `cargo test -p alvr_server_openxr` stay green.
//
// Verification ceiling: this builds on an AMD host but cannot run NVENC here.
// Runtime correctness (the CUDA external-memory array import in particular —
// the array descriptor must match Monado's scratch image layout) is shaken out
// on the remote RTX 3090 with a headset in Slice 3.3d.

#ifdef ALVR_OXR_HAVE_CUDA

#include "CudaDriverApi.h"
#include "NvEncoder.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <vector>

namespace {

// This translation unit deliberately avoids the OpenVR-side Logger; the Rust
// bridge logs the bool/nullptr returns. OutputDebugString traces help bring-up,
// and we also stash the message as the last-error so the Rust warning on a null
// Create() can report the specific failing step.
void trace(const char* msg) {
    OutputDebugStringA(msg);
    g_vk_encoder_last_error = msg;
}

// NVENC encoder over a CUDA context. Allocates pitched CUDA device memory as the
// input pool (NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR), mirroring how
// NvEncoderD3D11 allocates D3D11 textures. The CUDA context must be current on
// the calling thread for every method.
class NvEncoderCuda : public NvEncoder {
public:
    NvEncoderCuda(
        CudaApi* cuda, CUcontext ctx, uint32_t width, uint32_t height, NV_ENC_BUFFER_FORMAT fmt
    )
        : NvEncoder(NV_ENC_DEVICE_TYPE_CUDA, ctx, width, height, fmt, 0, false, false)
        , m_cuda(cuda) { }

    ~NvEncoderCuda() override { ReleaseInputBuffers(); }

    size_t InputPitch() const { return m_pitch; }

private:
    void AllocateInputBuffers(int32_t numInputBuffers) override {
        if (!IsHWEncoderInitialized()) {
            NVENC_THROW_ERROR("Encoder not initialized", NV_ENC_ERR_ENCODER_NOT_INITIALIZED);
        }
        std::vector<void*> inputFrames;
        for (int i = 0; i < numInputBuffers; i++) {
            CUdeviceptr dptr = 0;
            size_t pitch = 0;
            // ABGR is 4 bytes/pixel; element size 16 lets the driver pick a
            // texture-friendly pitch alignment.
            CUresult r = m_cuda->cuMemAllocPitch(
                &dptr,
                &pitch,
                static_cast<size_t>(GetMaxEncodeWidth()) * 4,
                GetMaxEncodeHeight(),
                16
            );
            if (r != CUDA_SUCCESS) {
                NVENC_THROW_ERROR("cuMemAllocPitch failed", NV_ENC_ERR_OUT_OF_MEMORY);
            }
            if (m_pitch == 0) {
                m_pitch = pitch;
            }
            m_devicePtrs.push_back(dptr);
            inputFrames.push_back(reinterpret_cast<void*>(dptr));
        }
        RegisterInputResources(
            inputFrames,
            NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
            GetMaxEncodeWidth(),
            GetMaxEncodeHeight(),
            static_cast<int>(m_pitch),
            GetPixelFormat(),
            false
        );
    }

    void ReleaseInputBuffers() override {
        if (m_devicePtrs.empty()) {
            return;
        }
        UnregisterInputResources();
        for (CUdeviceptr p : m_devicePtrs) {
            if (p && m_cuda->cuMemFree) {
                m_cuda->cuMemFree(p);
            }
        }
        m_devicePtrs.clear();
    }

    CudaApi* m_cuda;
    size_t m_pitch = 0;
    std::vector<CUdeviceptr> m_devicePtrs;
};

NV_ENC_BUFFER_FORMAT pickBufferFormat(const NvencConfig& cfg) {
    // Mirror VideoEncoderNVENC::Initialize. The squasher hands us RGBA8 scratch
    // images, so ABGR (memory order R,G,B,A) is the SDR match; NVENC does the
    // RGB->YUV conversion internally.
    if (cfg.use10bitEncoder) {
        return cfg.enableHdr ? NV_ENC_BUFFER_FORMAT_YUV420_10BIT : NV_ENC_BUFFER_FORMAT_ABGR10;
    }
    return cfg.enableHdr ? NV_ENC_BUFFER_FORMAT_NV12 : NV_ENC_BUFFER_FORMAT_ABGR;
}

} // namespace

struct VkEncoderBackend::Impl {
    CudaApi cuda;
    CUcontext ctx = nullptr;
    std::unique_ptr<NvEncoderCuda> encoder;
    NvencConfig cfg = {};
    std::atomic<bool> insertIdr { false };

    // Import one view's OPAQUE_WIN32 image, map it as a CUDA array, and copy it
    // into the [view] half of the combined NVENC input frame. CUDA context must
    // already be current. Returns false on any CUDA failure.
    bool importViewToInput(
        const VkEncoderBackend::SubmitDesc& desc,
        int view,
        uint64_t handle,
        uint64_t size,
        CUdeviceptr dstDevice,
        size_t dstPitch
    ) {
        if (handle == 0 || size == 0) {
            return false;
        }

        CUDA_EXTERNAL_MEMORY_HANDLE_DESC memDesc = {};
        memDesc.type = CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32;
        memDesc.handle.win32.handle = reinterpret_cast<void*>(static_cast<uintptr_t>(handle));
        memDesc.size = size;
        // Monado allocates the scratch images as dedicated allocations.
        memDesc.flags = CUDA_EXTERNAL_MEMORY_DEDICATED;

        CUexternalMemory extMem = nullptr;
        if (cuda.cuImportExternalMemory(&extMem, &memDesc) != CUDA_SUCCESS) {
            trace("cuImportExternalMemory failed\n");
            return false;
        }

        CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC mipDesc = {};
        mipDesc.offset = 0;
        mipDesc.numLevels = 1;
        mipDesc.arrayDesc.Width = desc.imageWidth;
        mipDesc.arrayDesc.Height = desc.imageHeight;
        mipDesc.arrayDesc.Depth = 0;
        mipDesc.arrayDesc.Format = CU_AD_FORMAT_UNSIGNED_INT8;
        mipDesc.arrayDesc.NumChannels = 4;
        mipDesc.arrayDesc.Flags = CUDA_ARRAY3D_SURFACE_LDST | CUDA_ARRAY3D_COLOR_ATTACHMENT;

        bool ok = true;
        CUmipmappedArray mipmap = nullptr;
        CUarray level = nullptr;
        if (cuda.cuExternalMemoryGetMappedMipmappedArray(&mipmap, extMem, &mipDesc) != CUDA_SUCCESS
            || cuda.cuMipmappedArrayGetLevel(&level, mipmap, 0) != CUDA_SUCCESS) {
            trace("cuExternalMemoryGetMappedMipmappedArray/GetLevel failed\n");
            ok = false;
        }

        if (ok) {
            CUDA_MEMCPY2D copy = {};
            copy.srcMemoryType = CU_MEMORYTYPE_ARRAY;
            copy.srcArray = level;
            copy.dstMemoryType = CU_MEMORYTYPE_DEVICE;
            copy.dstDevice = dstDevice;
            copy.dstPitch = dstPitch;
            copy.dstXInBytes = static_cast<size_t>(view) * desc.imageWidth * 4;
            copy.WidthInBytes = static_cast<size_t>(desc.imageWidth) * 4;
            copy.Height = desc.imageHeight;
            if (cuda.cuMemcpy2D(&copy) != CUDA_SUCCESS) {
                trace("cuMemcpy2D failed\n");
                ok = false;
            }
        }

        if (mipmap) {
            cuda.cuMipmappedArrayDestroy(mipmap);
        }
        cuda.cuDestroyExternalMemory(extMem);
        return ok;
    }
};

VkEncoderBackend::VkEncoderBackend() = default;

VkEncoderBackend::~VkEncoderBackend() { Shutdown(); }

std::unique_ptr<VkEncoderBackend> VkEncoderBackend::Create(const NvencConfig& cfg) {
    auto impl = std::make_unique<Impl>();

    if (!impl->cuda.Load()) {
        trace("VkEncoderBackend: nvcuda.dll unavailable; OpenXR mode cannot stream\n");
        return nullptr;
    }
    CudaApi& cuda = impl->cuda;

    int deviceCount = 0;
    CUdevice device = 0;
    if (cuda.cuInit(0) != CUDA_SUCCESS || cuda.cuDeviceGetCount(&deviceCount) != CUDA_SUCCESS
        || deviceCount <= 0 || cuda.cuDeviceGet(&device, 0) != CUDA_SUCCESS) {
        trace("VkEncoderBackend: no CUDA device\n");
        return nullptr;
    }
    // cuCtxCreate makes the new context current on this thread, which the NVENC
    // session open + input-buffer allocation below require.
    if (cuda.cuCtxCreate(&impl->ctx, 0, device) != CUDA_SUCCESS) {
        trace("VkEncoderBackend: cuCtxCreate failed\n");
        return nullptr;
    }

    impl->cfg = cfg;

    try {
        impl->encoder = std::make_unique<NvEncoderCuda>(
            &cuda, impl->ctx, cfg.renderWidth, cfg.renderHeight, pickBufferFormat(cfg)
        );

        NV_ENC_INITIALIZE_PARAMS initializeParams = { NV_ENC_INITIALIZE_PARAMS_VER };
        NV_ENC_CONFIG encodeConfig = { NV_ENC_CONFIG_VER };
        initializeParams.encodeConfig = &encodeConfig;
        FillNvencConfig(cfg, impl->encoder.get(), initializeParams);
        impl->encoder->CreateEncoder(&initializeParams);
    } catch (const std::exception& e) {
        trace(e.what());
        impl->encoder.reset();
        cuda.cuCtxDestroy(impl->ctx);
        impl->ctx = nullptr;
        return nullptr;
    }

    auto backend = std::unique_ptr<VkEncoderBackend>(new VkEncoderBackend());
    backend->m_impl = std::move(impl);
    return backend;
}

void VkEncoderBackend::Shutdown() {
    if (!m_impl) {
        return;
    }
    if (m_impl->encoder) {
        m_impl->cuda.cuCtxPushCurrent(m_impl->ctx);
        std::vector<std::vector<uint8_t>> pending;
        try {
            m_impl->encoder->EndEncode(pending);
            m_impl->encoder->DestroyEncoder();
        } catch (...) { }
        m_impl->encoder.reset();
        CUcontext popped = nullptr;
        m_impl->cuda.cuCtxPopCurrent(&popped);
    }
    if (m_impl->ctx) {
        m_impl->cuda.cuCtxDestroy(m_impl->ctx);
        m_impl->ctx = nullptr;
    }
    m_impl->cuda.Unload();
    m_impl.reset();
}

void VkEncoderBackend::OnStreamStart() {
    if (m_impl) {
        m_impl->insertIdr.store(true);
    }
}

void VkEncoderBackend::InsertIDR() {
    if (m_impl) {
        m_impl->insertIdr.store(true);
    }
}

std::vector<uint8_t> VkEncoderBackend::GetSequenceParams() {
    std::vector<uint8_t> out;
    if (m_impl && m_impl->encoder) {
        m_impl->cuda.cuCtxPushCurrent(m_impl->ctx);
        try {
            m_impl->encoder->GetSequenceParams(out);
        } catch (...) { }
        CUcontext popped = nullptr;
        m_impl->cuda.cuCtxPopCurrent(&popped);
    }
    return out;
}

bool VkEncoderBackend::Submit(const SubmitDesc& desc, PacketCallback onPacket, void* ctx) {
    if (!m_impl || !m_impl->encoder) {
        return false;
    }
    CudaApi& cuda = m_impl->cuda;
    cuda.cuCtxPushCurrent(m_impl->ctx);

    bool ok = true;
    try {
        const NvEncInputFrame* inputFrame = m_impl->encoder->GetNextInputFrame();
        CUdeviceptr dst = reinterpret_cast<CUdeviceptr>(inputFrame->inputPtr);
        size_t dstPitch = inputFrame->pitch;

        ok = m_impl->importViewToInput(
            desc, 0, desc.imageHandleLeft, desc.imageSizeLeft, dst, dstPitch
        );
        if (ok && desc.imageHandleRight != 0) {
            ok = m_impl->importViewToInput(
                desc, 1, desc.imageHandleRight, desc.imageSizeRight, dst, dstPitch
            );
        }

        if (ok) {
            // Both array->linear copies are async on the null stream; make sure
            // they finish before NVENC reads the input.
            cuda.cuCtxSynchronize();

            bool idr = m_impl->insertIdr.exchange(false);
            NV_ENC_PIC_PARAMS picParams = {};
            if (idr) {
                picParams.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR;
            }

            std::vector<std::vector<uint8_t>> packets;
            m_impl->encoder->EncodeFrame(packets, &picParams);

            for (std::vector<uint8_t>& packet : packets) {
                uint8_t* buf = packet.data();
                int len = static_cast<int>(packet.size());

                // NVENC's AV1 output is IVF-wrapped; strip to the OBUs. Mirrors
                // VideoEncoderNVENC::Transmit.
                if (m_impl->cfg.codec == NVENC_CODEC_AV1) {
                    const uint8_t ivf_magic[4] = { 0x44, 0x4B, 0x49, 0x46 };
                    if (len >= 4 && !memcmp(buf, ivf_magic, 4)) {
                        buf += 32;
                        len -= 32;
                    }
                    if (len <= 12) {
                        continue;
                    }
                    buf += 12;
                    len -= 12;
                }

                if (len > 0 && onPacket) {
                    onPacket(ctx, buf, len, idr, desc.targetTimestampNs);
                }
            }
        }
    } catch (...) {
        trace("VkEncoderBackend::Submit threw\n");
        ok = false;
    }

    CUcontext popped = nullptr;
    cuda.cuCtxPopCurrent(&popped);
    return ok;
}

#else // !ALVR_OXR_HAVE_CUDA — built without the CUDA Toolkit (e.g. CI)

struct VkEncoderBackend::Impl { };

VkEncoderBackend::VkEncoderBackend() = default;
VkEncoderBackend::~VkEncoderBackend() = default;

std::unique_ptr<VkEncoderBackend> VkEncoderBackend::Create(const NvencConfig&) {
    // Built without CUDA: OpenXR-mode NVENC streaming is unavailable.
    g_vk_encoder_last_error
        = "alvr_server_openxr built without the CUDA Toolkit (ALVR_OXR_HAVE_CUDA undefined)";
    return nullptr;
}

void VkEncoderBackend::Shutdown() { }
void VkEncoderBackend::OnStreamStart() { }
void VkEncoderBackend::InsertIDR() { }
std::vector<uint8_t> VkEncoderBackend::GetSequenceParams() { return {}; }

bool VkEncoderBackend::Submit(const SubmitDesc&, PacketCallback, void*) { return false; }

#endif // ALVR_OXR_HAVE_CUDA
