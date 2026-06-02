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

/// Where to find Monado source. Resolution order (first non-empty wins):
/// 1. the `--monado-source <path>` xtask flag (`cli_override`),
/// 2. the `ALVR_MONADO_SOURCE_DIR` env var,
/// 3. the vendored submodule at `<workspace>/openxr/`.
///
/// The override points at a sibling clone (e.g., a local fork that hasn't been
/// pushed + submodule-bumped yet) without churning the alvr-repo submodule
/// pointer for every iteration. The flag is the discoverable surface (shows in
/// `--help`); the env var stays for shells/CI that already export it.
fn openxr_source_dir(cli_override: Option<&str>) -> PathBuf {
    if let Some(custom) = cli_override.filter(|s| !s.is_empty()) {
        return PathBuf::from(custom);
    }
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

/// Per `openxr/doc/winbuild.md`, vcpkg in manifest mode is Monado's officially
/// supported Windows build path: Monado's `vcpkg.json` declares the full dep
/// list (`pthreads`, `wil`, `cjson`, `eigen3`, `glslang`, `vulkan`, plus the
/// `usb`/`gui` features → `libusb`, `hidapi`, `sdl2`), and a single CMake
/// configure with `-DCMAKE_TOOLCHAIN_FILE=...\vcpkg.cmake` builds + installs
/// them on first run.
///
/// We mirror that path here: clone microsoft/vcpkg into
/// `build/_thirdparty/vcpkg` on first call, bootstrap it, and return the
/// toolchain file path. Idempotent — second and subsequent calls short-circuit
/// via the bootstrapped `vcpkg.exe` marker.
///
/// Returns None on non-Windows (system pkg manager owns the deps there) and
/// when `ALVR_OPENXR_SKIP_VCPKG=1` is set in the env (escape hatch for users
/// who manage Monado deps themselves or want to avoid the ~1GB vcpkg tree).
///
/// First-run cost: vcpkg clone (~150 MB) + bootstrap (~30s) + per-dep build
/// on the next CMake configure (~15–45 min for the full Monado dep set,
/// cached after that).
fn ensure_vcpkg_windows() -> Option<PathBuf> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    if env::var_os("ALVR_OPENXR_SKIP_VCPKG")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return None;
    }

    let vcpkg_root = thirdparty_dir().join("vcpkg");
    let toolchain = vcpkg_root.join("scripts/buildsystems/vcpkg.cmake");
    let bootstrap_marker = vcpkg_root.join("vcpkg.exe");

    // A partial / blobless clone left by earlier versions of this helper
    // poisons vcpkg's manifest-mode workflow: vcpkg's `checkout-index`
    // step on per-port trees triggers an on-demand promisor fetch that
    // hangs libcurl's DNS resolver on Windows ("getaddrinfo() thread
    // failed to start" cascade). Bail with a clear message rather than
    // silently reusing the broken state.
    if vcpkg_root.join(".git").exists() {
        let sh_check = Shell::new().unwrap();
        let vcpkg_str = vcpkg_root.to_string_lossy().into_owned();
        let filter = cmd!(
            sh_check,
            "git -C {vcpkg_str} config --get remote.origin.partialclonefilter"
        )
        .ignore_status()
        .read()
        .unwrap_or_default();
        if !filter.is_empty() {
            eprintln!(
                "error: existing vcpkg clone at {vcpkg_str} is a partial clone \
                 (filter={filter}). This breaks vcpkg's per-port checkout step \
                 on Windows. Delete the directory and rerun:\n\n\
                 \trd /s /q {vcpkg_str}\n\n\
                 (PowerShell's Remove-Item often fails on .git pack files held \
                 open by Defender — use cmd's rd instead.)"
            );
            process::exit(1);
        }
    }

    if toolchain.exists() && bootstrap_marker.exists() {
        return Some(toolchain);
    }

    let thirdparty = thirdparty_dir();
    fs::create_dir_all(&thirdparty).unwrap();
    let sh = Shell::new().unwrap();

    if !vcpkg_root.join(".git").exists() {
        println!("Cloning microsoft/vcpkg into {}...", vcpkg_root.display());
        let url = "https://github.com/microsoft/vcpkg.git";
        let dest = vcpkg_root.to_string_lossy().into_owned();
        // Full clone. Earlier iterations tried --depth=1 (missing the manifest
        // baseline commit) and --filter=blob:none (vcpkg's checkout-index
        // step triggers on-demand promisor fetches that destabilise libcurl's
        // threaded DNS resolver on this Windows host — observed reliably as
        // `getaddrinfo() thread failed to start`). A full clone trades disk
        // (~1 GB) for not needing network during per-port checkout, which is
        // the only knob that makes the failure mode go away.
        cmd!(sh, "git clone {url} {dest}").run().unwrap();
    }

    if !bootstrap_marker.exists() {
        println!("Bootstrapping vcpkg...");
        // Use the .bat's absolute path — cmd's PATH lookup doesn't include cwd
        // by default on this Windows host (`NoDefaultCurrentDirectoryInExePath`
        // policy), so a bare `bootstrap-vcpkg.bat` resolves to nothing.
        let bootstrap = vcpkg_root.join("bootstrap-vcpkg.bat");
        let bootstrap_str = bootstrap.to_string_lossy().into_owned();
        // -disableMetrics keeps vcpkg's telemetry quiet without prompting.
        cmd!(sh, "cmd /c {bootstrap_str} -disableMetrics")
            .run()
            .unwrap();
    }

    assert!(
        toolchain.exists(),
        "vcpkg bootstrap did not produce {}",
        toolchain.display()
    );

    Some(toolchain)
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
///
/// Note: when vcpkg is used (the default Windows path), Eigen3 also comes from
/// the vcpkg manifest, so this helper's install is redundant. It stays as a
/// fallback for `ALVR_OPENXR_SKIP_VCPKG=1` users.
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
fn check_source_present(src: &Path) {
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
///
/// `monado_source` is the optional `--monado-source <path>` override; see
/// [`openxr_source_dir`] for the full resolution order.
pub fn build_openxr_runtime(
    profile: Profile,
    enable_alvr_driver: bool,
    monado_source: Option<String>,
) {
    let src = openxr_source_dir(monado_source.as_deref());
    check_source_present(&src);

    let sh = Shell::new().unwrap();

    // Build the bridge cdylib first: the CMake build links monado-service
    // against its import library, and deploy_bridge_cdylib copies the resulting
    // DLL afterwards. Doing this here (rather than relying on a pre-existing
    // target/ artifact) guarantees the deployed DLL matches current source —
    // skipping it once shipped a stale pre-fix cdylib that emitted a zero
    // head-orientation quaternion and cost a multi-hour xrLocateViews
    // misdiagnosis.
    if enable_alvr_driver {
        build_bridge_cdylib(&sh, profile);
    }

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
    // Default Windows path: vcpkg in manifest mode reads Monado's vcpkg.json and
    // builds the whole dep set. Fallback (ALVR_OPENXR_SKIP_VCPKG=1): per-dep
    // ensure_*() helpers stage individual REQUIREDs locally.
    let mut extra_cmake_args: Vec<String> = Vec::new();
    if let Some(toolchain) = ensure_vcpkg_windows() {
        extra_cmake_args.push(format!(
            "-DCMAKE_TOOLCHAIN_FILE={}",
            toolchain.to_string_lossy()
        ));
    } else if let Some(prefix) = ensure_eigen3_windows() {
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

    deploy_bridge_cdylib(&build, profile);
    publish_active_runtime_manifest(&build);
}

/// Build the `alvr_server_openxr` cdylib for the matching cargo profile. Run
/// before the CMake build so the import library exists for monado-service to
/// link against and the DLL `deploy_bridge_cdylib` copies is freshly compiled.
fn build_bridge_cdylib(sh: &Shell, profile: Profile) {
    let mut flags = vec!["-p", "alvr_server_openxr"];
    if !matches!(profile, Profile::Debug) {
        flags.push("--release");
    }
    let flags_ref = &flags;
    println!("Building alvr_server_openxr cdylib ({profile})...");
    cmd!(sh, "cargo build {flags_ref...}").run().unwrap();
}

/// Copy the alvr_server_openxr cdylib next to monado-service.exe so the loader
/// resolves it at process start. Monado's CMake doesn't auto-deploy the cdylib
/// to the service target dir (it's an IMPORTED library, not built by CMake) —
/// without this step monado-service.exe exits with STATUS_DLL_NOT_FOUND on
/// startup, which is exactly the behaviour observed during the Gate B smoke
/// test before the manual copy.
///
/// Windows-only for now (.dll layout). On Linux the .so would normally live on
/// the linker's rpath via the same `target_link_directories` hint; revisit if
/// that ever breaks.
fn deploy_bridge_cdylib(build_dir: &Path, profile: Profile) {
    if !cfg!(target_os = "windows") {
        return;
    }

    let dll_name = "alvr_server_openxr.dll";
    let cargo_profile_dir = match profile {
        Profile::Debug => "debug",
        Profile::Release | Profile::Distribution => "release",
    };
    let src = afs::target_dir().join(cargo_profile_dir).join(dll_name);
    if !src.exists() {
        eprintln!(
            "warning: {} not found — build the bridge first via `cargo build -p alvr_server_openxr{}`. \
             monado-service.exe will exit with STATUS_DLL_NOT_FOUND until this is resolved.",
            src.display(),
            if matches!(profile, Profile::Debug) {
                ""
            } else {
                " --release"
            }
        );
        return;
    }

    // Multi-config generators (Visual Studio, Xcode) put the exe under a per-
    // config subdir of the service target dir; single-config puts it directly.
    let service_dir_root = build_dir.join("src/xrt/targets/service");
    let candidates = ["Debug", "Release", "RelWithDebInfo", "MinSizeRel"]
        .iter()
        .map(|cfg| service_dir_root.join(cfg))
        .chain(std::iter::once(service_dir_root.clone()))
        .filter(|p| p.join("monado-service.exe").exists());

    let mut deployed = 0;
    for dst_dir in candidates {
        let dst = dst_dir.join(dll_name);
        match fs::copy(&src, &dst) {
            Ok(_) => {
                println!("Deployed {} -> {}", src.display(), dst.display());
                deployed += 1;
            }
            Err(err) => eprintln!(
                "warning: failed to copy {} -> {}: {err}",
                src.display(),
                dst.display()
            ),
        }
    }

    if deployed == 0 {
        eprintln!(
            "warning: no monado-service.exe found under {} — cdylib deploy skipped.",
            service_dir_root.display()
        );
    }
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
/// for mutual exclusion (Phase 4 §3 of docs/openxr-migration.md).
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
    let query = cmd!(sh, "reg query {OPENXR_REGISTRY_KEY} /v ActiveRuntime")
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
    cmd!(sh, "reg delete {OPENXR_REGISTRY_KEY} /v ActiveRuntime /f")
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
        eprintln!(
            "error: neither $XDG_CONFIG_HOME nor $HOME is set; can't locate OpenXR config dir."
        );
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
        eprintln!(
            "error: failed to copy {} → {}: {err}",
            manifest.display(),
            dst.display()
        );
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
