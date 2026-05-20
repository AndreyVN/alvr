//! Build wiring for the OpenXR (Monado) runtime path.
//!
//! This subcommand drives CMake on the vendored Monado tree at `openxr/` to
//! produce the runtime artifacts (`libopenxr_monado`, `monado-service`) that
//! the OpenXR-mode of ALVR consumes. It is **separate** from the OpenVR/SteamVR
//! build path — invoking it does not affect `build-streamer` or `build-server-lib`.
//!
//! The Monado-side ALVR driver lives at `openxr/src/xrt/drivers/alvr/` and is
//! gated by the CMake option `XRT_BUILD_DRIVER_ALVR` (default OFF). Until the
//! cdylib produced by `alvr/server_openxr` is built and locatable, Monado
//! should be built with `XRT_BUILD_DRIVER_ALVR=OFF`.

use crate::build::Profile;
use alvr_filesystem as afs;
use std::{
    fs,
    path::{Path, PathBuf},
};
use xshell::{Shell, cmd};

fn openxr_source_dir() -> PathBuf {
    afs::workspace_dir().join("openxr")
}

fn openxr_build_dir(profile: Profile) -> PathBuf {
    afs::build_dir().join(format!("openxr-{profile}"))
}

/// Returns Ok(()) if `openxr/` exists and looks like the Monado source tree;
/// otherwise prints a helpful message and exits.
fn check_source_present() {
    let src = openxr_source_dir();
    if !src.join("CMakeLists.txt").exists() {
        eprintln!(
            "error: {src:?} does not look like a Monado source tree.\n\n\
             Expected `openxr/CMakeLists.txt` to exist. If `openxr/` has been \n\
             converted to a git submodule, run:\n\n\
             \tgit submodule update --init --recursive openxr\n\n\
             See docs/monado-notes/SUBMODULE_PIN.md for details."
        );
        std::process::exit(1);
    }
}

/// Configure + build Monado.
///
/// `enable_alvr_driver` toggles the `XRT_BUILD_DRIVER_ALVR` cmake option. Default
/// is `false` so the build works against an unmodified Monado snapshot.
pub fn build_openxr_runtime(profile: Profile, enable_alvr_driver: bool) {
    check_source_present();

    let sh = Shell::new().unwrap();

    let src = openxr_source_dir();
    let build = openxr_build_dir(profile);
    fs::create_dir_all(&build).unwrap();

    let cmake_build_type = match profile {
        Profile::Debug => "Debug",
        Profile::Release | Profile::Distribution => "RelWithDebInfo",
    };

    // XRT_BUILD_DRIVER_ALVR and XRT_FEATURE_COMP_ALVR are paired: the driver
    // produces the head/controllers, the compositor forwards layers to the
    // streamer. Building one without the other gives you a Monado that either
    // sees ALVR devices but presents locally (no streaming), or has no head
    // device for comp_alvr to attach to. There's no real use case for the
    // asymmetric combinations, so the xtask offers a single toggle.
    let alvr_driver_flag = if enable_alvr_driver {
        "-DXRT_BUILD_DRIVER_ALVR=ON"
    } else {
        "-DXRT_BUILD_DRIVER_ALVR=OFF"
    };
    let alvr_comp_flag = if enable_alvr_driver {
        "-DXRT_FEATURE_COMP_ALVR=ON"
    } else {
        "-DXRT_FEATURE_COMP_ALVR=OFF"
    };

    let src_str = src.to_string_lossy().into_owned();
    let build_str = build.to_string_lossy().into_owned();
    let cmake_type = format!("-DCMAKE_BUILD_TYPE={cmake_build_type}");

    println!(
        "Configuring Monado:\n  source  = {src_str}\n  build   = {build_str}\n  profile = {profile}\n  alvr_driver+compositor = {enable_alvr_driver}"
    );

    cmd!(
        sh,
        "cmake -S {src_str} -B {build_str} {cmake_type} {alvr_driver_flag} {alvr_comp_flag}"
    )
    .run()
    .unwrap();

    cmd!(sh, "cmake --build {build_str} --parallel")
        .run()
        .unwrap();

    println!("Monado build finished. Artifacts in {build_str}");

    publish_active_runtime_manifest(&build);
}

/// Stable filename for the runtime manifest. The launcher (Phase 4.2) will
/// copy this into the per-user OpenXR config dir; for development the
/// loader can also be pointed at it via `XR_RUNTIME_JSON=<path>`.
const ACTIVE_RUNTIME_FILENAME: &str = "active_runtime_alvr.json";

/// Monado's `CMakeLists.txt` for the OpenXR target generates a build-tree
/// development manifest at `${CMAKE_BINARY_DIR}/openxr_monado-dev.json`
/// (single-config) or `${CMAKE_BINARY_DIR}/$<CONFIG>/openxr_monado-dev.json`
/// (multi-config). Find the freshest one and republish it under a stable
/// name the launcher can rely on. Non-fatal: a missing manifest just means
/// the build didn't produce the OpenXR target this round (e.g. a custom
/// `XRT_FEATURE_OPENXR=OFF`).
fn publish_active_runtime_manifest(build_dir: &Path) {
    const MONADO_MANIFEST_NAME: &str = "openxr_monado-dev.json";

    let mut candidates: Vec<PathBuf> = vec![build_dir.join(MONADO_MANIFEST_NAME)];
    // Multi-config generators (Visual Studio, Xcode) emit per-config subdirs.
    for cfg in ["Debug", "Release", "RelWithDebInfo", "MinSizeRel"] {
        candidates.push(build_dir.join(cfg).join(MONADO_MANIFEST_NAME));
    }

    let Some(src) = candidates.into_iter().find(|p| p.exists()) else {
        eprintln!(
            "warning: Monado did not produce {MONADO_MANIFEST_NAME} under {}. \
             Active-runtime publish step skipped.",
            build_dir.display()
        );
        return;
    };

    let dst = build_dir.join(ACTIVE_RUNTIME_FILENAME);
    match fs::copy(&src, &dst) {
        Ok(_) => println!(
            "Published OpenXR runtime manifest:\n  {}\nPoint the loader at it with:\n  XR_RUNTIME_JSON={}",
            dst.display(),
            dst.display()
        ),
        Err(err) => eprintln!(
            "warning: failed to copy {} → {}: {err}",
            src.display(),
            dst.display()
        ),
    }
}
