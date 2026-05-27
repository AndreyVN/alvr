use crate::{
    InstallationInfo, Progress, ReleaseChannelsInfo, ReleaseInfo, UiMessage, WorkerMessage,
};
use alvr_common::{ToAny, anyhow::Result, semver::Version};
use anyhow::{Context, bail};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use std::{
    env,
    fs::{self, File},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{Receiver, Sender},
};

const APK_NAME: &str = "client.apk";

/// Serde variant names for `alvr_session::RuntimeMode`. Stable across the
/// schema — bumping these requires a session migration. Kept as bare consts
/// rather than re-exported from `alvr_session` so the launcher does not need
/// to depend on the (heavy) settings-schema crate just for two strings.
pub const RUNTIME_MODE_STEAMVR: &str = "Steamvr";
pub const RUNTIME_MODE_OPENXR: &str = "Openxr";

pub fn installations_dir() -> PathBuf {
    data_dir().join("installations")
}

pub fn worker(
    ui_message_receiver: Receiver<UiMessage>,
    worker_message_sender: Sender<WorkerMessage>,
) {
    tokio::runtime::Runtime::new()
        .expect("Failed to create tokio runtime")
        .block_on(async {
            let req_client = reqwest::Client::builder()
                .user_agent("ALVR-Launcher")
                .build()
                .unwrap();
            let version_data = match fetch_all_releases(&req_client).await {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("Error fetching version data: {e}");
                    return;
                }
            };

            worker_message_sender
                .send(WorkerMessage::ReleaseChannelsInfo(version_data))
                .unwrap();

            loop {
                let Ok(message) = ui_message_receiver.recv() else {
                    return;
                };
                let res = match message {
                    UiMessage::Quit => return,
                    UiMessage::InstallServer {
                        release_info,
                        session_version,
                    } => {
                        install_server(
                            &worker_message_sender,
                            release_info,
                            session_version,
                            &req_client,
                        )
                        .await
                    }
                    UiMessage::InstallClient(release_info) => {
                        install_and_launch_apk(&worker_message_sender, release_info)
                    }
                };
                match res {
                    Ok(()) => worker_message_sender.send(WorkerMessage::Done).unwrap(),
                    Err(e) => worker_message_sender
                        .send(WorkerMessage::Error(e.to_string()))
                        .unwrap(),
                }
            }
        });
}

async fn fetch_all_releases(client: &reqwest::Client) -> Result<ReleaseChannelsInfo> {
    Ok(ReleaseChannelsInfo {
        stable: fetch_releases_for_repo(
            client,
            "https://api.github.com/repos/alvr-org/ALVR/releases",
        )
        .await?,
        nightly: fetch_releases_for_repo(
            client,
            "https://api.github.com/repos/alvr-org/ALVR-nightly/releases",
        )
        .await?,
    })
}

async fn fetch_releases_for_repo(client: &reqwest::Client, url: &str) -> Result<Vec<ReleaseInfo>> {
    let response: serde_json::Value = client.get(url).send().await?.json().await?;

    let mut releases = Vec::new();
    for value in response.as_array().to_any()? {
        releases.push(ReleaseInfo {
            version: value["tag_name"].as_str().to_any()?.into(),
            assets: value["assets"]
                .as_array()
                .to_any()?
                .iter()
                .filter_map(|value| {
                    Some((
                        value["name"].as_str()?.into(),
                        value["browser_download_url"].as_str()?.into(),
                    ))
                })
                .collect(),
        })
    }
    Ok(releases)
}

pub fn get_release(
    release_channels_info: &ReleaseChannelsInfo,
    version: &str,
) -> Option<ReleaseInfo> {
    release_channels_info
        .stable
        .iter()
        .find(|release| release.version == version)
        .cloned()
        .or_else(|| {
            release_channels_info
                .nightly
                .iter()
                .find(|release| release.version == version)
                .cloned()
        })
}

fn install_and_launch_apk(
    worker_message_sender: &Sender<WorkerMessage>,
    release: ReleaseInfo,
) -> Result<()> {
    worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
        message: "Starting install".into(),
        progress: 0.0,
    }))?;

    let root = installations_dir().join(&release.version);
    let apk_name = "alvr_client_android.apk";
    let apk_path = root.join(apk_name);
    if !apk_path.exists() {
        let apk_url = release
            .assets
            .get(apk_name)
            .ok_or(anyhow::anyhow!("Unable to determine download URL"))?;
        let apk_buffer = alvr_adb::commands::download(apk_url, |downloaded, total| {
            let progress = total.map_or(0.0, |t| downloaded as f32 / t as f32);
            worker_message_sender
                .send(WorkerMessage::ProgressUpdate(Progress {
                    message: "Downloading Client APK".into(),
                    progress,
                }))
                .ok();
        })?;
        let mut file = File::create(&apk_path)?;
        file.write_all(&apk_buffer)?;
    }

    let layout = alvr_filesystem::Layout::new(&root);
    let adb_path = alvr_adb::commands::require_adb(&layout, |downloaded, total| {
        let progress = total.map_or(0.0, |t| downloaded as f32 / t as f32);
        worker_message_sender
            .send(WorkerMessage::ProgressUpdate(Progress {
                message: "Downloading ADB".into(),
                progress,
            }))
            .ok();
    })?;

    let device_serial = alvr_adb::commands::list_devices(&adb_path)?
        .iter()
        .find_map(|d| d.serial.clone())
        .ok_or(anyhow::anyhow!("Failed to find connected device"))?;

    let v = if release.version.starts_with('v') {
        release.version[1..].to_string()
    } else {
        release.version
    };
    let version = Version::parse(&v).context("Failed to parse release version")?;
    let stable = version.pre.is_empty() && !version.build.contains("nightly");
    let application_id = if stable {
        alvr_system_info::PACKAGE_NAME_GITHUB_STABLE
    } else {
        alvr_system_info::PACKAGE_NAME_GITHUB_DEV
    };

    if alvr_adb::commands::is_package_installed(&adb_path, &device_serial, application_id)? {
        worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
            message: "Uninstalling old APK".into(),
            progress: 0.0,
        }))?;
        alvr_adb::commands::uninstall_package(&adb_path, &device_serial, application_id)?;
    }

    worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
        message: "Installing new APK".into(),
        progress: 0.0,
    }))?;
    alvr_adb::commands::install_package(&adb_path, &device_serial, &apk_path.to_string_lossy())?;

    alvr_adb::commands::start_application(&adb_path, &device_serial, application_id)?;

    Ok(())
}

async fn download(
    worker_message_sender: &Sender<WorkerMessage>,
    message: &str,
    url: &str,
    client: &reqwest::Client,
) -> Result<Vec<u8>> {
    let res = client.get(url).send().await?;
    let total_size = res.content_length();
    let mut stream = res.bytes_stream();
    let mut buffer = Vec::new();
    while let Some(item) = stream.next().await {
        buffer.extend(item?);

        match total_size {
            Some(total_size) => {
                worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
                    message: message.into(),
                    progress: buffer.len() as f32 / total_size as f32,
                }))?
            }
            None => worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
                message: format!("{message} (Progress unavailable)"),
                progress: 0.5,
            }))?,
        }
    }

    Ok(buffer)
}

async fn install_server(
    worker_message_sender: &Sender<WorkerMessage>,
    release_info: ReleaseInfo,
    session_version: Option<String>,
    req_client: &reqwest::Client,
) -> Result<()> {
    worker_message_sender.send(WorkerMessage::ProgressUpdate(Progress {
        message: "Starting install".into(),
        progress: 0.0,
    }))?;

    let file_name = if cfg!(windows) {
        "alvr_streamer_windows.zip"
    } else {
        "alvr_streamer_linux.tar.gz"
    };

    let url = release_info
        .assets
        .get(file_name)
        .ok_or(anyhow::anyhow!("Unable to determine download link"))?;

    let buffer = download(
        worker_message_sender,
        "Downloading Streamer",
        url,
        req_client,
    )
    .await?;

    let installation_dir = installations_dir().join(&release_info.version);

    fs::create_dir_all(&installation_dir)?;

    let mut buffer = Cursor::new(buffer);
    if cfg!(windows) {
        zip::ZipArchive::new(&mut buffer)?.extract(&installation_dir)?;
    } else {
        tar::Archive::new(&mut GzDecoder::new(&mut buffer)).unpack(&installation_dir)?;
    }

    if let Some(session_version) = session_version {
        if !cfg!(windows) {
            unreachable!("The session copying code should only be hit on Windows!")
        }

        for inst in get_installations() {
            if inst.version == session_version {
                let source = alvr_filesystem::filesystem_layout_from_openvr_driver_root_dir(
                    &installations_dir().join(session_version),
                )
                .unwrap()
                .session();

                let destination = alvr_filesystem::filesystem_layout_from_openvr_driver_root_dir(
                    &installation_dir,
                )
                .unwrap()
                .session();

                fs::copy(source, destination)?;

                break;
            }
        }
    }

    Ok(())
}

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from(env::var("HOME").expect("Failed to determine home directory"))
            .join(".local/share/ALVR-Launcher")
    } else {
        env::current_exe()
            .expect("Unable to determine executable directory")
            .parent()
            .unwrap()
            .to_owned()
    }
}

/// Locate the session.json belonging to an installation, or `None` if the
/// installation tree isn't shaped like a streamer install (e.g. macOS or a
/// stripped-down deploy).
fn session_path_for(installation_dir: &Path) -> Option<PathBuf> {
    alvr_filesystem::filesystem_layout_from_openvr_driver_root_dir(installation_dir)
        .map(|layout| layout.session())
}

/// Read `session_settings.extra.runtime.variant` from an installation's
/// session.json. Returns `None` when the file is missing, malformed, or the
/// field doesn't exist (e.g. a version that pre-dates `RuntimeMode`).
pub fn read_runtime_mode(installation_dir: &Path) -> Option<String> {
    let path = session_path_for(installation_dir)?;
    let bytes = fs::read(&path).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json.get("session_settings")?
        .get("extra")?
        .get("runtime")?
        .get("variant")?
        .as_str()
        .map(str::to_owned)
}

/// Set `session_settings.extra.runtime.variant` in-place in an installation's
/// session.json. Reads → mutates → writes the same file. The schema's other
/// fields are preserved byte-for-byte (we never round-trip through the typed
/// `SessionConfig` here, only through `serde_json::Value`).
///
/// Caller is responsible for passing a value the schema accepts — today that's
/// `"Steamvr"` or `"Openxr"` (see the `RUNTIME_MODE_*` consts).
pub fn write_runtime_mode(installation_dir: &Path, variant: &str) -> Result<()> {
    let path = session_path_for(installation_dir)
        .with_context(|| format!("No session layout for {}", installation_dir.display()))?;
    let bytes = fs::read(&path)
        .with_context(|| format!("Failed to read session.json at {}", path.display()))?;
    let mut json: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("Failed to parse session.json at {}", path.display()))?;

    let runtime = json
        .get_mut("session_settings")
        .and_then(|v| v.get_mut("extra"))
        .and_then(|v| v.get_mut("runtime"))
        .with_context(|| "session.json has no session_settings.extra.runtime field")?;
    runtime
        .as_object_mut()
        .with_context(|| "runtime field is not an object")?
        .insert(
            "variant".to_owned(),
            serde_json::Value::String(variant.to_owned()),
        );

    let serialized = serde_json::to_vec_pretty(&json)?;
    fs::write(&path, serialized)
        .with_context(|| format!("Failed to write session.json at {}", path.display()))?;
    Ok(())
}

pub fn get_installations() -> Vec<InstallationInfo> {
    match fs::read_dir(installations_dir()) {
        Ok(entries) => entries
            .into_iter()
            .filter_map(|entry| {
                entry
                    .ok()
                    .filter(|entry| match entry.file_type() {
                        Ok(file_type) => file_type.is_dir(),
                        Err(e) => {
                            eprintln!("Failed to read entry file type: {e}");
                            false
                        }
                    })
                    .map(|entry| {
                        let installation_dir = entry.path();
                        let (has_session_json, runtime_mode) = if cfg!(windows) {
                            let session_exists =
                                alvr_filesystem::filesystem_layout_from_openvr_driver_root_dir(
                                    &installation_dir,
                                )
                                .map(|layout| layout.session().exists())
                                .unwrap_or(false);
                            let mode = if session_exists {
                                read_runtime_mode(&installation_dir)
                            } else {
                                None
                            };
                            (session_exists, mode)
                        } else {
                            // On linux, the launcher does not need to manage the session files
                            (false, None)
                        };

                        InstallationInfo {
                            version: entry.file_name().to_string_lossy().into(),
                            is_apk_downloaded: installation_dir.join(APK_NAME).exists(),
                            has_session_json,
                            runtime_mode,
                        }
                    })
            })
            .collect(),
        Err(e) => {
            eprintln!("Failed to read versions dir: {e}");
            Vec::new()
        }
    }
}

pub fn launch_dashboard(version: &str) -> Result<()> {
    let installation_dir = installations_dir().join(version);

    let dashboard_path = if cfg!(windows) {
        installation_dir.join("ALVR Dashboard.exe")
    } else if cfg!(target_os = "linux") {
        installation_dir.join("alvr_streamer_linux/bin/alvr_dashboard")
    } else {
        bail!("Unsupported platform")
    };

    Command::new(dashboard_path).spawn()?;

    Ok(())
}

pub fn delete_installation(version: &str) -> Result<()> {
    fs::remove_dir_all(installations_dir().join(version))?;

    Ok(())
}

// session.json read/write helpers are Windows-only (Linux launcher doesn't
// manage session files), so the tests live behind the same gate. They exercise
// the file-level round-trip + byte-preservation guarantee that the launcher
// UI depends on. No tempfile dev-dep — we use std::env::temp_dir() with a
// unique per-process+counter suffix so parallel `cargo test` invocations
// don't collide.
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn unique_tempdir(name: &str) -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "alvr-launcher-test-{}-{}-{}",
            name,
            std::process::id(),
            id
        ));
        // Start clean in case a previous failed run leaked a dir at this path.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_session_json(runtime_variant: Option<&str>) -> serde_json::Value {
        let runtime = match runtime_variant {
            Some(v) => json!({ "variant": v }),
            None => json!({}),
        };
        json!({
            "server_version": "21.0.0-dev13",
            "openvr_config": {
                "eye_resolution_width": 800,
                "extra_field_a": 42,
            },
            "client_connections": {},
            "session_settings": {
                "extra": {
                    "runtime": runtime,
                    "other_extra_field": "preserve_me",
                },
                "video": { "preserve_me_too": true },
            },
        })
    }

    fn write_fixture(dir: &Path, value: &serde_json::Value) {
        fs::write(
            dir.join("session.json"),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn read_runtime_mode_missing_file() {
        let dir = unique_tempdir("read_missing");
        assert_eq!(read_runtime_mode(&dir), None);
    }

    #[test]
    fn read_runtime_mode_malformed_json() {
        let dir = unique_tempdir("read_malformed");
        fs::write(dir.join("session.json"), b"not actually json {{ }}").unwrap();
        assert_eq!(read_runtime_mode(&dir), None);
    }

    #[test]
    fn read_runtime_mode_field_absent() {
        let dir = unique_tempdir("read_no_field");
        write_fixture(&dir, &fixture_session_json(None));
        // The fixture has `runtime` as an empty object — no variant inside.
        assert_eq!(read_runtime_mode(&dir), None);
    }

    #[test]
    fn read_runtime_mode_steamvr() {
        let dir = unique_tempdir("read_steamvr");
        write_fixture(&dir, &fixture_session_json(Some(RUNTIME_MODE_STEAMVR)));
        assert_eq!(
            read_runtime_mode(&dir).as_deref(),
            Some(RUNTIME_MODE_STEAMVR)
        );
    }

    #[test]
    fn read_runtime_mode_openxr() {
        let dir = unique_tempdir("read_openxr");
        write_fixture(&dir, &fixture_session_json(Some(RUNTIME_MODE_OPENXR)));
        assert_eq!(
            read_runtime_mode(&dir).as_deref(),
            Some(RUNTIME_MODE_OPENXR)
        );
    }

    #[test]
    fn write_runtime_mode_roundtrips_both_variants() {
        let dir = unique_tempdir("write_roundtrip");
        write_fixture(&dir, &fixture_session_json(Some(RUNTIME_MODE_STEAMVR)));

        write_runtime_mode(&dir, RUNTIME_MODE_OPENXR).unwrap();
        assert_eq!(
            read_runtime_mode(&dir).as_deref(),
            Some(RUNTIME_MODE_OPENXR)
        );

        write_runtime_mode(&dir, RUNTIME_MODE_STEAMVR).unwrap();
        assert_eq!(
            read_runtime_mode(&dir).as_deref(),
            Some(RUNTIME_MODE_STEAMVR)
        );
    }

    #[test]
    fn write_runtime_mode_preserves_other_fields() {
        let dir = unique_tempdir("write_preserve");
        let original = fixture_session_json(Some(RUNTIME_MODE_STEAMVR));
        write_fixture(&dir, &original);

        write_runtime_mode(&dir, RUNTIME_MODE_OPENXR).unwrap();

        let after: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("session.json")).unwrap()).unwrap();

        // Every field outside session_settings.extra.runtime.variant must be
        // bit-identical to what we wrote — that's the launcher's promise.
        assert_eq!(after["server_version"], original["server_version"]);
        assert_eq!(after["openvr_config"], original["openvr_config"]);
        assert_eq!(after["client_connections"], original["client_connections"]);
        assert_eq!(
            after["session_settings"]["video"],
            original["session_settings"]["video"]
        );
        assert_eq!(
            after["session_settings"]["extra"]["other_extra_field"],
            original["session_settings"]["extra"]["other_extra_field"]
        );
        // And the targeted field actually moved.
        assert_eq!(
            after["session_settings"]["extra"]["runtime"]["variant"],
            json!(RUNTIME_MODE_OPENXR)
        );
    }

    #[test]
    fn write_runtime_mode_errors_when_runtime_missing() {
        // If session.json doesn't even have a `runtime` object, refuse to
        // synthesise one — the caller should bring the session up to date via
        // the dashboard instead. The UI gates the ComboBox on read_runtime_mode
        // returning Some, so in practice this error path isn't hit; the test
        // pins the contract.
        let dir = unique_tempdir("write_no_runtime");
        let mut malformed = fixture_session_json(None);
        // Remove the empty `runtime` object entirely so the get_mut("runtime")
        // chain returns None.
        malformed["session_settings"]["extra"]
            .as_object_mut()
            .unwrap()
            .remove("runtime");
        write_fixture(&dir, &malformed);

        let res = write_runtime_mode(&dir, RUNTIME_MODE_OPENXR);
        assert!(res.is_err(), "expected error when runtime field absent");
    }
}
