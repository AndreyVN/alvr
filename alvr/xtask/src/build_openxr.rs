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

use crate::{build::Profile, command};
use alvr_filesystem as afs;
use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process,
};
use sysinfo::System;
use xshell::{Shell, cmd};

/// Where to find Monado source. Defaults to the vendored submodule at
/// `<workspace>/openxr/`. Override with `ALVR_MONADO_SOURCE_DIR` to point at a
/// sibling clone (e.g., a local fork that hasn't been pushed + submodule-bumped
/// yet) without churning the alvr-repo submodule pointer for every iteration.
fn openxr_source_dir() -> PathBuf {
    if let Some(custom) = env::var_os("ALVR_MONADO_SOURCE_DIR").filter(|s| !s.is_empty()) {
        return PathBuf::from(custom);
    }
    afs::workspace_dir().join("openxr")
}

fn openxr_build_dir(profile: Profile) -> PathBuf {
    afs::build_dir().join(format!("openxr-{profile}"))
}

fn thirdparty_dir() -> PathBuf {
    afs::build_dir().join("_thirdparty")
}

/// Monado's CMake hard-requires Eigen3 (`find_package(Eigen3 REQUIRED NO_MODULE)`)
/// at version >= 3.3. On Linux distros this is normally `libeigen3-dev` or
/// `eigen3-devel`; on Windows there's no choco/install.txt entry today, so we
/// stage a local install under `build/_thirdparty/eigen-install` and inject it
/// into `CMAKE_PREFIX_PATH` for the Monado configure step.
///
/// Returns the install prefix on Windows, or None on platforms where we trust
/// the system package manager. Idempotent — second and subsequent calls
/// short-circuit via the `Eigen3Config.cmake` marker.
fn ensure_eigen3_windows() -> Option<PathBuf> {
    if !cfg!(target_os = "windows") {
        return None;
    }

    let install = thirdparty_dir().join("eigen-install");
    let marker = install.join("share/eigen3/cmake/Eigen3Config.cmake");
    if marker.exists() {
        return Some(install);
    }

    println!("Staging Eigen3 3.4.0 locally for Monado build...");

    let thirdparty = thirdparty_dir();
    fs::create_dir_all(&thirdparty).unwrap();

    let src_dir = thirdparty.join("eigen-3.4.0");
    if !src_dir.join("CMakeLists.txt").exists() {
        // Eigen3 is header-only, but Monado finds it via the installed
        // Eigen3Config.cmake — so we still go through configure + install.
        command::download_and_extract_zip(
            "https://gitlab.com/libeigen/eigen/-/archive/3.4.0/eigen-3.4.0.zip",
            &thirdparty,
        )
        .unwrap();
    }

    let sh = Shell::new().unwrap();
    let cfg_build = thirdparty.join("eigen-build");
    let src_str = src_dir.to_string_lossy().into_owned();
    let cfg_build_str = cfg_build.to_string_lossy().into_owned();
    let prefix_arg = format!("-DCMAKE_INSTALL_PREFIX={}", install.to_string_lossy());

    cmd!(
        sh,
        "cmake -S {src_str} -B {cfg_build_str} {prefix_arg} -DBUILD_TESTING=OFF -DEIGEN_BUILD_DOC=OFF"
    )
    .run()
    .unwrap();

    cmd!(sh, "cmake --install {cfg_build_str}").run().unwrap();

    assert!(
        marker.exists(),
        "Eigen3 install did not produce {}",
        marker.display()
    );

    Some(install)
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

    // Stage Monado's hard-required deps that aren't in install.txt / choco today.
    // The function is per-dep and per-platform; today only Eigen3 on Windows.
    let mut extra_cmake_args: Vec<String> = Vec::new();
    if let Some(prefix) = ensure_eigen3_windows() {
        extra_cmake_args.push(format!("-DCMAKE_PREFIX_PATH={}", prefix.to_string_lossy()));
    }

    println!(
        "Configuring Monado:\n  source  = {src_str}\n  build   = {build_str}\n  profile = {profile}\n  alvr_driver+compositor = {enable_alvr_driver}"
    );

    cmd!(
        sh,
        "cmake -S {src_str} -B {build_str} {cmake_type} {extra_cmake_args...} {alvr_driver_flag} {alvr_comp_flag}"
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

/// Detect a live SteamVR `vrserver` process. Mirrors the check in
/// `alvr/dashboard/src/steamvr_launcher/mod.rs` so the two modes share the
/// same "is the other runtime busy?" signal. Used by [`register_openxr_runtime`]
/// for mutual exclusion (Phase 4 §3 of openxr-migration.md).
fn is_steamvr_running() -> bool {
    System::new_all()
        .processes_by_name(OsStr::new(&afs::exec_fname("vrserver")))
        .count()
        != 0
}

/// Resolve the published manifest path for `profile`, exiting with an error if
/// the build artefact isn't present. Shared by register / unregister so they
/// both refer to exactly the same file the build produced.
fn locate_published_manifest(profile: Profile) -> PathBuf {
    let manifest = openxr_build_dir(profile).join(ACTIVE_RUNTIME_FILENAME);
    if !manifest.exists() {
        eprintln!(
            "error: {} not found.\n\
             Run `cargo xtask build-openxr-runtime --enable-alvr-driver` first.",
            manifest.display()
        );
        process::exit(1);
    }
    manifest
}

/// Register the built Monado-as-ALVR runtime as the per-user OpenXR active
/// runtime.
///
/// Platform behaviour follows the OpenXR loader spec:
/// * **Windows**: writes `HKCU\Software\Khronos\OpenXR\1\ActiveRuntime` (REG_SZ)
///   via `reg.exe`. The loader reads this key — there is *no* file-based
///   convention at `%LOCALAPPDATA%\openxr\1\active_runtime.json` despite what
///   the original NEXT_STEPS.md draft suggested.
/// * **Linux / BSD**: writes the manifest contents to
///   `$XDG_CONFIG_HOME/openxr/1/active_runtime.json`, falling back to
///   `$HOME/.config/openxr/1/active_runtime.json`.
///
/// macOS is intentionally unhandled — no released Monado/ALVR target.
///
/// This action is **system-modifying for the current user**: every OpenXR
/// application launched after this point will use ALVR's Monado runtime until
/// `unregister-openxr-runtime` runs (or another runtime overwrites it).
///
/// Mutual exclusion: refuses to register while SteamVR's `vrserver` is alive.
/// Both runtimes want the same headset connection from the client; letting
/// them race produces a stream that goes nowhere useful. The user-facing
/// fix is to close SteamVR (or use the dashboard's "Restart SteamVR" with
/// runtime set to OpenXR) first.
pub fn register_openxr_runtime(profile: Profile) {
    if is_steamvr_running() {
        eprintln!(
            "error: SteamVR (vrserver) is running. Both modes claim the headset \
             connection — close SteamVR before registering the OpenXR runtime."
        );
        process::exit(1);
    }

    let manifest = locate_published_manifest(profile);

    if cfg!(target_os = "windows") {
        register_runtime_windows(&manifest);
    } else if cfg!(target_os = "macos") {
        eprintln!("error: macOS active-runtime registration is not supported.");
        process::exit(1);
    } else {
        register_runtime_unix(&manifest);
    }
}

/// Reverse of [`register_openxr_runtime`]. Refuses to act when the currently
/// registered runtime is *not* the manifest this profile published — the
/// uninstall path must not stomp on a different vendor's runtime.
pub fn unregister_openxr_runtime(profile: Profile) {
    let manifest = locate_published_manifest(profile);

    if cfg!(target_os = "windows") {
        unregister_runtime_windows(&manifest);
    } else if cfg!(target_os = "macos") {
        eprintln!("error: macOS active-runtime registration is not supported.");
        process::exit(1);
    } else {
        unregister_runtime_unix(&manifest);
    }
}

#[cfg(target_os = "windows")]
const OPENXR_REGISTRY_KEY: &str = r"HKCU\Software\Khronos\OpenXR\1";

#[cfg(target_os = "windows")]
fn register_runtime_windows(manifest: &Path) {
    let manifest_str = manifest.to_string_lossy().into_owned();
    let sh = Shell::new().unwrap();
    cmd!(
        sh,
        "reg add {OPENXR_REGISTRY_KEY} /v ActiveRuntime /t REG_SZ /d {manifest_str} /f"
    )
    .run()
    .unwrap();
    println!(
        "Registered as the current user's OpenXR runtime.\n  Registry: {OPENXR_REGISTRY_KEY}\\ActiveRuntime\n  Manifest: {manifest_str}"
    );
}

#[cfg(target_os = "windows")]
fn unregister_runtime_windows(manifest: &Path) {
    let manifest_str = manifest.to_string_lossy().into_owned();
    let sh = Shell::new().unwrap();
    let query = cmd!(
        sh,
        "reg query {OPENXR_REGISTRY_KEY} /v ActiveRuntime"
    )
    .ignore_status()
    .read()
    .unwrap_or_default();
    if !query.contains(&manifest_str) {
        eprintln!(
            "warning: current ActiveRuntime value doesn't reference {manifest_str}; \
             leaving registry alone so we don't stomp on another vendor's runtime.\n\
             reg query output:\n{query}"
        );
        return;
    }
    cmd!(
        sh,
        "reg delete {OPENXR_REGISTRY_KEY} /v ActiveRuntime /f"
    )
    .run()
    .unwrap();
    println!("Unregistered ALVR runtime ({OPENXR_REGISTRY_KEY}\\ActiveRuntime cleared).");
}

#[cfg(not(target_os = "windows"))]
fn register_runtime_windows(_manifest: &Path) {
    unreachable!("register_runtime_windows called off Windows")
}

#[cfg(not(target_os = "windows"))]
fn unregister_runtime_windows(_manifest: &Path) {
    unreachable!("unregister_runtime_windows called off Windows")
}

fn openxr_unix_config_dir() -> PathBuf {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        PathBuf::from(xdg).join("openxr").join("1")
    } else if let Some(home) = env::var_os("HOME").filter(|s| !s.is_empty()) {
        PathBuf::from(home).join(".config").join("openxr").join("1")
    } else {
        eprintln!("error: neither $XDG_CONFIG_HOME nor $HOME is set; can't locate OpenXR config dir.");
        process::exit(1);
    }
}

fn register_runtime_unix(manifest: &Path) {
    let dir = openxr_unix_config_dir();
    fs::create_dir_all(&dir).unwrap_or_else(|err| {
        eprintln!("error: failed to create {}: {err}", dir.display());
        process::exit(1);
    });
    let dst = dir.join("active_runtime.json");
    fs::copy(manifest, &dst).unwrap_or_else(|err| {
        eprintln!("error: failed to copy {} → {}: {err}", manifest.display(), dst.display());
        process::exit(1);
    });
    println!(
        "Registered as the current user's OpenXR runtime.\n  Path:     {}\n  Source:   {}",
        dst.display(),
        manifest.display()
    );
}

fn unregister_runtime_unix(manifest: &Path) {
    let dst = openxr_unix_config_dir().join("active_runtime.json");
    if !dst.exists() {
        println!("Nothing to unregister: {} doesn't exist.", dst.display());
        return;
    }
    let installed = fs::read(&dst).unwrap_or_default();
    let ours = fs::read(manifest).unwrap_or_default();
    if installed != ours {
        eprintln!(
            "warning: {} doesn't match {}; leaving it alone so we don't stomp on another \
             vendor's manifest.",
            dst.display(),
            manifest.display()
        );
        return;
    }
    fs::remove_file(&dst).unwrap_or_else(|err| {
        eprintln!("error: failed to remove {}: {err}", dst.display());
        process::exit(1);
    });
    println!("Unregistered ALVR runtime ({} removed).", dst.display());
}
