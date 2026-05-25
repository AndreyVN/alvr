//! Build script for `alvr_server_openxr`.
//!
//! Two responsibilities:
//!   1. Compile the platform-specific encoder backend C++ that lives in
//!      `../server_openvr/cpp/encoder/`. Today only the Windows Vulkan-input
//!      skeleton (`win32_vk/VkEncoderBackend.cpp`) is in scope; Slice 3.3
//!      adds the real implementation against NVENC's Vulkan-image-input
//!      API. The Linux side reuses `cpp/encoder/linux/EncodePipeline*` via
//!      Sub-slice 2.4, deferred.
//!   2. (Opt-in) Regenerate `include/alvr_runtime_bridge.h` from the
//!      `extern "C"` surface of `src/lib.rs` using cbindgen, gated by
//!      `ALVR_REGENERATE_BRIDGE_HEADER=1` so unrelated `cargo check` runs
//!      don't churn the header on disk.

use std::{env, path::PathBuf};

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let crate_path = PathBuf::from(&crate_dir);

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ALVR_REGENERATE_BRIDGE_HEADER");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");

    let platform = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // --- (1) compile encoder backend C++ -----------------------------------
    //
    // The shared encoder C++ lives in the alvr_server_openvr tree under
    // cpp/encoder/. We reach into the sibling crate's path to compile the
    // pieces alvr_server_openxr needs (currently just the win32_vk skeleton
    // on Windows). Both crates share the same EncoderBackend.h interface.
    let cpp_root = crate_path.join("..").join("server_openvr").join("cpp");
    println!(
        "cargo:rerun-if-changed={}",
        cpp_root.join("encoder").to_string_lossy()
    );

    if platform == "windows" {
        let vk_dir = cpp_root.join("encoder").join("win32_vk");
        let vk_sources: Vec<PathBuf> = std::fs::read_dir(&vk_dir)
            .expect("cpp/encoder/win32_vk not found — alvr_server_openvr tree expected as sibling")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("cpp"))
            .collect();

        let mut build = cc::Build::new();
        build
            .cpp(true)
            .std("c++17")
            .files(vk_sources)
            .include(&cpp_root) // resolves `#include "encoder/EncoderBackend.h"`
            .define("NOMINMAX", None);

        // The Vulkan-input NVENC encoder (Slice 3.3b) reuses the device-agnostic
        // base NvEncoder and the shared FillNvencConfig from the D3D11 tree —
        // both are free of OpenVR-side deps (verified: no Logger/Settings/
        // bindings includes). Compile them into this crate's encoder lib and put
        // the win32_d3d11 dir on the include path so `#include "NvEncoder.h"` /
        // `"NvCodecUtils.h"` resolve. They link in regardless of CUDA presence;
        // VkEncoderBackend only references them under the ALVR_OXR_HAVE_CUDA guard.
        let d3d11_dir = cpp_root.join("encoder").join("win32_d3d11");
        build
            .file(d3d11_dir.join("NvEncoder.cpp"))
            .file(cpp_root.join("encoder").join("NvencConfig.cpp"))
            .include(&d3d11_dir);

        // Vulkan SDK include path. Required even by the skeleton header
        // (which holds Vulkan types opaque today) so that Slice 3.3 can
        // start referencing VkImage / VkFormat without another build.rs
        // round-trip. The SDK is the standard LunarG installer; sets
        // VULKAN_SDK env var.
        if let Some(sdk) = env::var_os("VULKAN_SDK") {
            let sdk_path = PathBuf::from(sdk);
            build.include(sdk_path.join("Include"));
            println!(
                "cargo:rustc-link-search=native={}",
                sdk_path.join("Lib").to_string_lossy()
            );
            // Don't link vulkan-1 — the CUDA-interop path (Slice 3.3) imports
            // Monado's OPAQUE_WIN32 external-memory handle straight into CUDA
            // and never stands up a VkDevice, so no Vulkan symbols are called.
            // The SDK include stays only for VkFormat enum values used when
            // mapping AlvrOxrLayer.image_format → an NVENC buffer format.
        } else {
            println!(
                "cargo:warning=VULKAN_SDK not set; the encoder skeleton will compile but \
                 Slice 3.3's real Vulkan-input NVENC integration will need it. \
                 Install the LunarG Vulkan SDK and re-build."
            );
        }

        // CUDA Toolkit include path. The NVENC Vulkan-input encoder (Slice 3.3)
        // bridges to NVENC through the CUDA driver API: it imports Monado's
        // OPAQUE_WIN32 image handle via `cuImportExternalMemory` and registers
        // the resulting array with NVENC as NV_ENC_INPUT_RESOURCE_TYPE_CUDAARRAY.
        // We need `cuda.h` (driver API types/structs) and `cudaTypedefs.h`
        // (PFN_cu* function-pointer typedefs) at compile time.
        //
        // We deliberately do NOT link `cuda.lib`: the driver entry points live
        // in `nvcuda.dll`, which the encoder loads dynamically at runtime
        // (mirroring how NvEncoder.cpp loads nvEncodeAPI64.dll). Static-linking
        // the import lib would make this cdylib — and the `cargo test -p
        // alvr_server_openxr` binary CI runs on the NVIDIA-less windows-2022
        // runner — depend on nvcuda.dll being present just to load, which it
        // isn't on the build host (AMD iGPU) or CI. Dynamic loading keeps both
        // green and degrades to a clean "NVENC unavailable" at Create() time.
        //
        // NVENC's own header is already reachable via the cpp_root include
        // (`#include "alvr_server/nvEncodeAPI.h"`), so no extra path for it.
        match env::var_os("CUDA_PATH") {
            Some(cuda) => {
                build.include(PathBuf::from(cuda).join("include"));
                // Drives the `#ifdef ALVR_OXR_HAVE_CUDA` guard in
                // VkEncoderBackend.cpp. When set, the CUDA/NVENC headers are
                // mandatory (a wrong include path fails the build — the point
                // of the compile gate). When unset (CI's NVIDIA-less
                // windows-2022 runner, where the CUDA Toolkit isn't installed),
                // the guard compiles the skeleton fallback instead and
                // `Create()` reports NVENC unavailable, so `cargo test -p
                // alvr_server_openxr` still builds and runs there.
                build.define("ALVR_OXR_HAVE_CUDA", None);
            }
            None => {
                println!(
                    "cargo:warning=CUDA_PATH not set; the encoder skeleton will compile but \
                     Slice 3.3's NVENC CUDA-interop encoder needs the CUDA Toolkit. \
                     Install it (see install.txt) and re-build."
                );
            }
        }

        build.compile("alvr_server_openxr_encoder");
    }

    // --- (2) cbindgen bridge-header regeneration (opt-in) ------------------
    if env::var("ALVR_REGENERATE_BRIDGE_HEADER").ok().as_deref() != Some("1") {
        return;
    }

    let out_header = crate_path.join("include").join("alvr_runtime_bridge.h");

    std::fs::create_dir_all(out_header.parent().unwrap()).ok();

    let cfg = cbindgen::Config {
        language: cbindgen::Language::C,
        cpp_compat: true,
        pragma_once: true,
        include_guard: Some("ALVR_RUNTIME_BRIDGE_H".to_owned()),
        header: Some(
            "/* ALVR is licensed under the MIT license. \
             https://github.com/alvr-org/ALVR/blob/master/LICENSE */"
                .to_owned(),
        ),
        autogen_warning: Some(
            "/* Warning, this file is autogenerated by cbindgen. \
             Regenerate via `ALVR_REGENERATE_BRIDGE_HEADER=1 cargo build -p alvr_server_openxr`. */"
                .to_owned(),
        ),
        documentation: true,
        documentation_style: cbindgen::DocumentationStyle::Doxy,
        // Match the convention used by alvr_server_core/cbindgen.toml so that
        // C consumers get globally-unique enumerator names like
        // ALVR_OXR_SIDE_LEFT instead of a bare Left that would collide with
        // any other enum variant.
        enumeration: cbindgen::EnumConfig {
            rename_variants: cbindgen::RenameRule::QualifiedScreamingSnakeCase,
            ..Default::default()
        },
        ..Default::default()
    };

    let builder = cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(cfg);

    match builder.generate() {
        Ok(bindings) => {
            bindings.write_to_file(&out_header);
        }
        Err(err) => {
            // Do not fail the build on header-generation issues during
            // scaffolding; just warn. The header in version control may be
            // regenerated by a maintainer separately.
            println!(
                "cargo:warning=cbindgen failed to generate alvr_runtime_bridge.h: {err}. \
                 Continuing build; the existing header (if any) will be used."
            );
        }
    }
}
