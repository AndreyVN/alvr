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

        return ok;
    }

    void Unload() {
        if (module) {
            FreeLibrary(module);
            module = nullptr;
        }
    }
};
