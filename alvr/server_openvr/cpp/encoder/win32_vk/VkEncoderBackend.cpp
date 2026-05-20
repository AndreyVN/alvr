#include "VkEncoderBackend.h"

#include <stdexcept>

// Phase 3.0 Slice 3.1 skeleton. Every method is a stub. The real bodies
// arrive in Slice 3.2 (NVENC Vulkan-input integration); see the header
// comment for the prerequisites.
//
// Deliberately self-contained — no #include of alvr_server/Logger.h or
// alvr_server/Utils.h so this translation unit can live in any future
// alvr_server_openxr cc::Build without pulling in the OpenVR-side
// logging-callback glue (which depends on Rust-set function pointers
// owned by alvr_server_openvr only). Real logging arrives once the
// bridge ABI gains a unified log surface in Phase 3.1.

VkEncoderBackend::VkEncoderBackend() = default;

std::unique_ptr<VkEncoderBackend>
VkEncoderBackend::Create(uint32_t encoderWidth, uint32_t encoderHeight) {
    (void)encoderWidth;
    (void)encoderHeight;
    throw std::runtime_error(
        "VkEncoderBackend::Create: not yet implemented. Slice 3.2 wires NVENC "
        "Vulkan-input support; until then OpenXR mode on Windows cannot stream."
    );
}

void VkEncoderBackend::Shutdown() { }

void VkEncoderBackend::OnStreamStart() {
    // Skeleton stub: no encoder running.
}

void VkEncoderBackend::InsertIDR() {
    // Skeleton stub: no encoder running.
}

bool VkEncoderBackend::Submit(const SubmitDesc& desc) {
    (void)desc;
    return false;
}
