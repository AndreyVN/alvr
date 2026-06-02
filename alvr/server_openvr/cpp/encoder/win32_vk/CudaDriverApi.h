#pragma once

// CUDA driver API dynamic loader.
//
// Include ONLY from a translation unit compiled with ALVR_OXR_HAVE_CUDA (it
// pulls in <cuda.h>, present only when the CUDA Toolkit is on the build host).
// Loads nvcuda.dll at runtime via LoadLibrary/GetProcAddress so the
// alvr_server_openxr cdylib never statically imports it — keeping the cdylib
// (and the `cargo test -p alvr_server_openxr` binary CI runs on a non-NVIDIA
// host) loadable everywhere. Mirrors how NvEncoder.cpp loads nvEncodeAPI64.dll.
//
// Only the entry points the Vulkan-input NVENC encoder needs are resolved.
// Driver-API functions whose ABI was revised carry a _v2 export name; we
// GetProcAddress the versioned name so the signatures below match nvcuda.dll.

#include <cuda.h>

#include <type_traits>
#include <windows.h>

struct CudaApi {
    HMODULE module = nullptr;

    CUresult(CUDAAPI* cuInit)(unsigned int) = nullptr;
    CUresult(CUDAAPI* cuDeviceGetCount)(int*) = nullptr;
    CUresult(CUDAAPI* cuDeviceGet)(CUdevice*, int) = nullptr;
    CUresult(CUDAAPI* cuCtxCreate)(CUcontext*, unsigned int, CUdevice) = nullptr;
    CUresult(CUDAAPI* cuCtxDestroy)(CUcontext) = nullptr;
    CUresult(CUDAAPI* cuCtxPushCurrent)(CUcontext) = nullptr;
    CUresult(CUDAAPI* cuCtxPopCurrent)(CUcontext*) = nullptr;
    CUresult(CUDAAPI* cuCtxSynchronize)() = nullptr;
    CUresult(CUDAAPI* cuMemAllocPitch)(CUdeviceptr*, size_t*, size_t, size_t, unsigned int)
        = nullptr;
    CUresult(CUDAAPI* cuMemFree)(CUdeviceptr) = nullptr;
    CUresult(CUDAAPI* cuMemcpy2D)(const CUDA_MEMCPY2D*) = nullptr;
    CUresult(CUDAAPI*
                 cuImportExternalMemory)(CUexternalMemory*, const CUDA_EXTERNAL_MEMORY_HANDLE_DESC*)
        = nullptr;
    CUresult(CUDAAPI* cuDestroyExternalMemory)(CUexternalMemory) = nullptr;
    CUresult(CUDAAPI*
                 cuExternalMemoryGetMappedMipmappedArray)(CUmipmappedArray*, CUexternalMemory, const CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC*)
        = nullptr;
    CUresult(CUDAAPI* cuMipmappedArrayGetLevel)(CUarray*, CUmipmappedArray, unsigned int) = nullptr;
    CUresult(CUDAAPI* cuMipmappedArrayDestroy)(CUmipmappedArray) = nullptr;
    CUresult(CUDAAPI* cuGetErrorString)(CUresult, const char**) = nullptr;

    // B1 (semaphore handoff): a real stream so the array->linear copies can be
    // ordered after a GPU-side wait on comp_alvr's squash/FFR timeline semaphore
    // (imported via cuImportExternalSemaphore) instead of relying on the
    // compositor's CPU vkQueueWaitIdle. cuMemcpy2DAsync replaces the synchronous
    // null-stream cuMemcpy2D so the wait actually gates the copy.
    CUresult(CUDAAPI* cuStreamCreate)(CUstream*, unsigned int) = nullptr;
    CUresult(CUDAAPI* cuStreamSynchronize)(CUstream) = nullptr;
    CUresult(CUDAAPI* cuStreamDestroy)(CUstream) = nullptr;
    CUresult(CUDAAPI* cuMemcpy2DAsync)(const CUDA_MEMCPY2D*, CUstream) = nullptr;
    CUresult(CUDAAPI*
                 cuImportExternalSemaphore)(CUexternalSemaphore*, const CUDA_EXTERNAL_SEMAPHORE_HANDLE_DESC*)
        = nullptr;
    CUresult(CUDAAPI*
                 cuWaitExternalSemaphoresAsync)(const CUexternalSemaphore*, const CUDA_EXTERNAL_SEMAPHORE_WAIT_PARAMS*, unsigned int, CUstream)
        = nullptr;
    // B2.2a: signal the reverse "consumed" semaphore once the input copies land,
    // so comp_alvr (Slice B2.2b) can wait before reusing the scratch ring slot.
    CUresult(CUDAAPI*
                 cuSignalExternalSemaphoresAsync)(const CUexternalSemaphore*, const CUDA_EXTERNAL_SEMAPHORE_SIGNAL_PARAMS*, unsigned int, CUstream)
        = nullptr;
    CUresult(CUDAAPI* cuDestroyExternalSemaphore)(CUexternalSemaphore) = nullptr;

    // Resolve every entry point. Returns false (and leaves the struct partially
    // filled) if nvcuda.dll is absent or any symbol is missing — the caller
    // treats that as "NVENC/CUDA unavailable on this host".
    bool Load() {
        module = LoadLibraryA("nvcuda.dll");
        if (!module) {
            return false;
        }

        bool ok = true;
        auto resolve = [&](auto& fnPtr, const char* name) {
            fnPtr = reinterpret_cast<std::remove_reference_t<decltype(fnPtr)>>(
                GetProcAddress(module, name)
            );
            if (!fnPtr) {
                ok = false;
            }
        };

        resolve(cuInit, "cuInit");
        resolve(cuDeviceGetCount, "cuDeviceGetCount");
        resolve(cuDeviceGet, "cuDeviceGet");
        resolve(cuCtxCreate, "cuCtxCreate_v2");
        resolve(cuCtxDestroy, "cuCtxDestroy_v2");
        resolve(cuCtxPushCurrent, "cuCtxPushCurrent_v2");
        resolve(cuCtxPopCurrent, "cuCtxPopCurrent_v2");
        resolve(cuCtxSynchronize, "cuCtxSynchronize");
        resolve(cuMemAllocPitch, "cuMemAllocPitch_v2");
        resolve(cuMemFree, "cuMemFree_v2");
        resolve(cuMemcpy2D, "cuMemcpy2D_v2");
        resolve(cuImportExternalMemory, "cuImportExternalMemory");
        resolve(cuDestroyExternalMemory, "cuDestroyExternalMemory");
        resolve(cuExternalMemoryGetMappedMipmappedArray, "cuExternalMemoryGetMappedMipmappedArray");
        resolve(cuMipmappedArrayGetLevel, "cuMipmappedArrayGetLevel");
        resolve(cuMipmappedArrayDestroy, "cuMipmappedArrayDestroy");
        resolve(cuGetErrorString, "cuGetErrorString");

        // B1: stream + external-semaphore entry points. Versioned exports carry
        // a _v2 suffix (cuStreamDestroy_v2, cuMemcpy2DAsync_v2) like the others.
        resolve(cuStreamCreate, "cuStreamCreate");
        resolve(cuStreamSynchronize, "cuStreamSynchronize");
        resolve(cuStreamDestroy, "cuStreamDestroy_v2");
        resolve(cuMemcpy2DAsync, "cuMemcpy2DAsync_v2");
        resolve(cuImportExternalSemaphore, "cuImportExternalSemaphore");
        resolve(cuWaitExternalSemaphoresAsync, "cuWaitExternalSemaphoresAsync");
        resolve(cuSignalExternalSemaphoresAsync, "cuSignalExternalSemaphoresAsync");
        resolve(cuDestroyExternalSemaphore, "cuDestroyExternalSemaphore");

        return ok;
    }

    void Unload() {
        if (module) {
            FreeLibrary(module);
            module = nullptr;
        }
    }
};
