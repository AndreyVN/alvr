use super::{
    NestingInfo, SettingControl,
    presets::{PresetControl, builtin_schema},
};
use crate::dashboard::ServerRequest;
use alvr_gui_common::{DisplayString, theme};
use alvr_session::{SessionSettings, Settings};
use eframe::egui::{Align, Align2, Frame, Grid, Layout, RichText, ScrollArea, Ui, Vec2};
#[cfg(target_arch = "wasm32")]
use instant::Instant;
use serde_json as json;
use settings_schema::SchemaNode;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

const DATA_UPDATE_INTERVAL: Duration = Duration::from_secs(1);
const MIN_COLUMN_SIZE: f32 = 300.0;

struct TopLevelEntry {
    id: DisplayString,
    control: SettingControl,
}

pub struct SettingsTab {
    selected_top_tab_id: String,
    resolution_preset: PresetControl,
    framerate_preset: PresetControl,
    encoder_preset: PresetControl,
    foveation_preset: PresetControl,
    codec_preset: PresetControl,
    game_audio_preset: PresetControl,
    microphone_preset: PresetControl,
    hand_tracking_interaction_preset: PresetControl,
    eye_face_tracking_preset: PresetControl,
    top_level_entries: Vec<TopLevelEntry>,
    session_settings_json: Option<json::Value>,
    last_update_instant: Instant,
    #[cfg(not(target_arch = "wasm32"))]
    metrics_test_result: Arc<Mutex<Option<String>>>,
    #[cfg(not(target_arch = "wasm32"))]
    metrics_test_running: bool,
    metrics_dialog_text: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    hw_test_result: Arc<Mutex<Option<String>>>,
    #[cfg(not(target_arch = "wasm32"))]
    hw_test_running: bool,
    hw_dialog_text: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    lhm_test_result: Arc<Mutex<Option<String>>>,
    #[cfg(not(target_arch = "wasm32"))]
    lhm_test_running: bool,
    lhm_dialog_text: Option<String>,
}

impl SettingsTab {
    pub fn new() -> Self {
        let nesting_info = NestingInfo {
            path: vec!["session_settings".into()],
            indentation_level: 0,
        };
        let schema = Settings::schema(alvr_session::session_settings_default());

        // Top level node must be a section
        let SchemaNode::Section { entries, .. } = schema else {
            unreachable!();
        };

        let top_level_entries = entries
            .into_iter()
            .map(|entry| {
                let id = entry.name;
                let display = super::get_display_name(&id, &entry.strings);

                let mut nesting_info = nesting_info.clone();
                nesting_info.path.push(id.clone().into());

                TopLevelEntry {
                    id: DisplayString { id, display },
                    control: SettingControl::new(nesting_info, entry.content),
                }
            })
            .collect();

        Self {
            selected_top_tab_id: "presets".into(),
            resolution_preset: PresetControl::new(builtin_schema::resolution_schema()),
            framerate_preset: PresetControl::new(builtin_schema::framerate_schema()),
            encoder_preset: PresetControl::new(builtin_schema::encoder_preset_schema()),
            foveation_preset: PresetControl::new(builtin_schema::foveation_preset_schema()),
            codec_preset: PresetControl::new(builtin_schema::codec_preset_schema()),
            game_audio_preset: PresetControl::new(builtin_schema::game_audio_schema()),
            microphone_preset: PresetControl::new(builtin_schema::microphone_schema()),
            hand_tracking_interaction_preset: PresetControl::new(
                builtin_schema::hand_tracking_interaction_schema(),
            ),
            eye_face_tracking_preset: PresetControl::new(builtin_schema::eye_face_tracking_schema()),
            top_level_entries,
            session_settings_json: None,
            last_update_instant: Instant::now(),
            #[cfg(not(target_arch = "wasm32"))]
            metrics_test_result: Arc::new(Mutex::new(None)),
            #[cfg(not(target_arch = "wasm32"))]
            metrics_test_running: false,
            metrics_dialog_text: None,
            #[cfg(not(target_arch = "wasm32"))]
            hw_test_result: Arc::new(Mutex::new(None)),
            #[cfg(not(target_arch = "wasm32"))]
            hw_test_running: false,
            hw_dialog_text: None,
            #[cfg(not(target_arch = "wasm32"))]
            lhm_test_result: Arc::new(Mutex::new(None)),
            #[cfg(not(target_arch = "wasm32"))]
            lhm_test_running: false,
            lhm_dialog_text: None,
        }
    }

    pub fn update_session(&mut self, session_settings: &SessionSettings) {
        let settings_json = json::to_value(session_settings).unwrap();

        self.resolution_preset
            .update_session_settings(&settings_json);
        self.framerate_preset
            .update_session_settings(&settings_json);
        self.encoder_preset.update_session_settings(&settings_json);
        self.foveation_preset
            .update_session_settings(&settings_json);
        self.codec_preset.update_session_settings(&settings_json);
        self.game_audio_preset
            .update_session_settings(&settings_json);
        self.microphone_preset
            .update_session_settings(&settings_json);
        self.hand_tracking_interaction_preset
            .update_session_settings(&settings_json);
        self.eye_face_tracking_preset
            .update_session_settings(&settings_json);

        self.session_settings_json = Some(settings_json);
    }

    pub fn ui(&mut self, ui: &mut Ui) -> Vec<ServerRequest> {
        let mut requests = vec![];

        // Collect results from the background test threads.
        #[cfg(not(target_arch = "wasm32"))]
        if self.metrics_test_running
            && let Some(text) = self.metrics_test_result.lock().unwrap().take()
        {
            self.metrics_dialog_text = Some(text);
            self.metrics_test_running = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.hw_test_running
            && let Some(text) = self.hw_test_result.lock().unwrap().take()
        {
            self.hw_dialog_text = Some(text);
            self.hw_test_running = false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.lhm_test_running
            && let Some(text) = self.lhm_test_result.lock().unwrap().take()
        {
            self.lhm_dialog_text = Some(text);
            self.lhm_test_running = false;
        }

        let now = Instant::now();
        if now > self.last_update_instant + DATA_UPDATE_INTERVAL {
            if self.session_settings_json.is_none() {
                requests.push(ServerRequest::GetSession);
            }

            self.last_update_instant = now;
        }

        let mut path_value_pairs = vec![];
        ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
            Frame::group(ui.style())
                .fill(theme::DARKER_BG)
                .inner_margin(theme::FRAME_PADDING)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(
                            &mut self.selected_top_tab_id,
                            "presets".into(),
                            RichText::new("Presets").raised().size(15.0),
                        );
                        for entry in &mut self.top_level_entries {
                            ui.selectable_value(
                                &mut self.selected_top_tab_id,
                                entry.id.id.clone(),
                                RichText::new(entry.id.display.clone()).raised().size(15.0),
                            );
                        }
                    })
                })
        });

        if self.selected_top_tab_id == "presets" {
            ScrollArea::new([false, true])
                .id_salt("presets_scroll")
                .show(ui, |ui| {
                    Grid::new("presets_grid")
                        .striped(true)
                        .num_columns(2)
                        .min_col_width(MIN_COLUMN_SIZE)
                        .show(ui, |ui| {
                            path_value_pairs.extend(self.resolution_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.framerate_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.encoder_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.foveation_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.codec_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.game_audio_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.microphone_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.hand_tracking_interaction_preset.ui(ui));
                            ui.end_row();

                            path_value_pairs.extend(self.eye_face_tracking_preset.ui(ui));
                            ui.end_row();
                        })
                });
        } else {
            ScrollArea::new([false, true])
                .id_salt(format!("{}_scroll", self.selected_top_tab_id))
                .show(ui, |ui| {
                    Grid::new(format!("{}_grid", self.selected_top_tab_id))
                        .striped(true)
                        .num_columns(2)
                        .min_col_width(MIN_COLUMN_SIZE)
                        .show(ui, |ui| {
                            if let Some(session_fragment) = &mut self.session_settings_json {
                                let session_fragments_mut =
                                    session_fragment.as_object_mut().unwrap();

                                let entry = self
                                    .top_level_entries
                                    .iter_mut()
                                    .find(|entry: &&mut TopLevelEntry| {
                                        entry.id.id == self.selected_top_tab_id
                                    })
                                    .unwrap();

                                let response = entry.control.ui(
                                    ui,
                                    &mut session_fragments_mut[&entry.id.id],
                                    false,
                                );

                                if let Some(response) = response {
                                    path_value_pairs.push(response);
                                }

                                ui.end_row();
                            }
                        });

                    // Test buttons paired with the URL fields rendered above.
                    // Sit below the schema grid because the schema rows are
                    // auto-generated and we can't inject an extra widget into
                    // a specific row without touching the shared renderer.
                    #[cfg(not(target_arch = "wasm32"))]
                    if self.selected_top_tab_id == "metrics" {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let url_opt = self
                            .session_settings_json
                            .as_ref()
                            .and_then(|j| j.pointer("/metrics/metrics_export/content/url"))
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        let hw_url_opt = self
                            .session_settings_json
                            .as_ref()
                            .and_then(|j| j.pointer("/metrics/metrics_export/content/hw_url"))
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        let lhm_url_opt = self
                            .session_settings_json
                            .as_ref()
                            .and_then(|j| j.pointer("/metrics/metrics_export/content/lhm_url"))
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);

                        Grid::new("metrics_test_grid")
                            .num_columns(2)
                            .min_col_width(MIN_COLUMN_SIZE)
                            .show(ui, |ui| {
                                ui.label("Endpoint URL");
                                let label = if self.metrics_test_running {
                                    "Testing…"
                                } else {
                                    "Test"
                                };
                                let enabled = url_opt.is_some() && !self.metrics_test_running;
                                if ui
                                    .add_enabled(enabled, eframe::egui::Button::new(label))
                                    .clicked()
                                {
                                    let url = url_opt.clone().unwrap();
                                    let result_arc = Arc::clone(&self.metrics_test_result);
                                    let ctx = ui.ctx().clone();
                                    self.metrics_test_running = true;
                                    std::thread::spawn(move || {
                                        let payload = streaming_test_payload();
                                        let text = match ureq::post(&url).send_json(&payload) {
                                            Ok(resp) => format!("✓  HTTP {}  —  OK", resp.status()),
                                            Err(e) => format!("✗  {e}"),
                                        };
                                        *result_arc.lock().unwrap() = Some(text);
                                        ctx.request_repaint();
                                    });
                                }
                                ui.end_row();

                                ui.label("Hardware endpoint URL");
                                let hw_label = if self.hw_test_running {
                                    "Testing…"
                                } else {
                                    "Test"
                                };
                                let hw_enabled = hw_url_opt.is_some()
                                    && !hw_url_opt.as_deref().unwrap_or("").is_empty()
                                    && !self.hw_test_running;
                                if ui
                                    .add_enabled(hw_enabled, eframe::egui::Button::new(hw_label))
                                    .clicked()
                                {
                                    let url = hw_url_opt.clone().unwrap();
                                    let result_arc = Arc::clone(&self.hw_test_result);
                                    let ctx = ui.ctx().clone();
                                    self.hw_test_running = true;
                                    std::thread::spawn(move || {
                                        let payload = hw_test_payload();
                                        let text = match ureq::post(&url).send_json(&payload) {
                                            Ok(resp) => format!("✓  HTTP {}  —  OK", resp.status()),
                                            Err(e) => format!("✗  {e}"),
                                        };
                                        *result_arc.lock().unwrap() = Some(text);
                                        ctx.request_repaint();
                                    });
                                }
                                ui.end_row();

                                ui.label("LibreHardwareMonitor URL");
                                let lhm_label = if self.lhm_test_running {
                                    "Testing…"
                                } else {
                                    "Test"
                                };
                                let lhm_enabled = lhm_url_opt.is_some()
                                    && !lhm_url_opt.as_deref().unwrap_or("").is_empty()
                                    && !self.lhm_test_running;
                                if ui
                                    .add_enabled(lhm_enabled, eframe::egui::Button::new(lhm_label))
                                    .clicked()
                                {
                                    let url = lhm_url_opt.clone().unwrap();
                                    let result_arc = Arc::clone(&self.lhm_test_result);
                                    let ctx = ui.ctx().clone();
                                    self.lhm_test_running = true;
                                    std::thread::spawn(move || {
                                        // LHM serves its sensor tree as JSON at the configured URL.
                                        // A successful GET that parses as JSON proves the web server
                                        // is up and we're reading the right endpoint.
                                        let text = match ureq::get(&url).call() {
                                            Ok(mut resp) => {
                                                let status = resp.status();
                                                match resp
                                                    .body_mut()
                                                    .read_json::<serde_json::Value>()
                                                {
                                                    Ok(_) => format!(
                                                        "✓  HTTP {status}  —  LHM responding"
                                                    ),
                                                    Err(e) => format!(
                                                        "⚠  HTTP {status}  —  body not JSON: {e}"
                                                    ),
                                                }
                                            }
                                            Err(e) => format!("✗  {e}"),
                                        };
                                        *result_arc.lock().unwrap() = Some(text);
                                        ctx.request_repaint();
                                    });
                                }
                                ui.end_row();
                            });
                    }
                });
        }

        if !path_value_pairs.is_empty() {
            requests.push(ServerRequest::SetSessionValues(path_value_pairs));
        }

        // Dialog shown when a test result is ready.
        if let Some(text) = self.metrics_dialog_text.clone() {
            let mut open = true;
            let resp = eframe::egui::Window::new("Metrics connection test")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.button("Close").clicked()
                });
            if !open || resp.is_some_and(|r| r.inner == Some(true)) {
                self.metrics_dialog_text = None;
            }
        }
        if let Some(text) = self.hw_dialog_text.clone() {
            let mut open = true;
            let resp = eframe::egui::Window::new("Hardware metrics connection test")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.button("Close").clicked()
                });
            if !open || resp.is_some_and(|r| r.inner == Some(true)) {
                self.hw_dialog_text = None;
            }
        }
        if let Some(text) = self.lhm_dialog_text.clone() {
            let mut open = true;
            let resp = eframe::egui::Window::new("LibreHardwareMonitor connection test")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.button("Close").clicked()
                });
            if !open || resp.is_some_and(|r| r.inner == Some(true)) {
                self.lhm_dialog_text = None;
            }
        }

        requests
    }
}

// Minimal streaming `Snapshot` payload — every required field present, every
// optional field at zero. Mirrors `metrics/server/models.py::Snapshot`.
#[cfg(not(target_arch = "wasm32"))]
fn streaming_test_payload() -> json::Value {
    serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "host": "test",
        "window_ms": 1000_u64,
        "frames": 0_u32,
        "dropped_samples": 0_u64,
        "latency_ms": {},
        "fps": {},
        "throughput": {
            "video_packets_per_sec": 0.0_f64,
            "video_mbits_per_sec": 0.0_f64
        },
        "totals": {
            "video_packets": 0_u64,
            "video_mbytes": 0_u64
        },
        "bitrate_directives": {
            "requested_bitrate_bps": 0.0_f32
        },
        "exporter": { "failed_posts": 0_u64 }
    })
}

// Minimal hardware `HwSnapshot` payload — `cpu_cores` / `dimms` / `storage`
// / `network` default to empty lists, so only `ts` + `host` are mandatory.
// Mirrors `metrics/server/models.py::HwSnapshot`.
#[cfg(not(target_arch = "wasm32"))]
fn hw_test_payload() -> json::Value {
    serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "host": "test",
    })
}
