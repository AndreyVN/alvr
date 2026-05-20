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
use std::{fs, path::PathBuf};
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

    let alvr_driver_flag = if enable_alvr_driver {
        "-DXRT_BUILD_DRIVER_ALVR=ON"
    } else {
        "-DXRT_BUILD_DRIVER_ALVR=OFF"
    };

    let src_str = src.to_string_lossy().into_owned();
    let build_str = build.to_string_lossy().into_owned();
    let cmake_type = format!("-DCMAKE_BUILD_TYPE={cmake_build_type}");

    println!(
        "Configuring Monado:\n  source  = {src_str}\n  build   = {build_str}\n  profile = {profile}\n  alvr_driver = {enable_alvr_driver}"
    );

    cmd!(
        sh,
        "cmake -S {src_str} -B {build_str} {cmake_type} {alvr_driver_flag}"
    )
    .run()
    .unwrap();

    cmd!(sh, "cmake --build {build_str} --parallel")
        .run()
        .unwrap();

    println!("Monado build finished. Artifacts in {build_str}");
}
