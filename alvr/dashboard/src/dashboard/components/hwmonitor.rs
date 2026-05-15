use alvr_hwmonitor::{
    CpuSample, GpuSample, Hwmonitor, HwmonitorConfig, MemorySample, NamedValue, NetSample,
    Snapshot, StorageSample,
};
use eframe::egui::{CollapsingHeader, Grid, RichText, ScrollArea, Ui};
use std::time::Duration;

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct HwmonitorTab {
    monitor: Hwmonitor,
}

impl HwmonitorTab {
    pub fn new() -> Self {
        Self {
            monitor: Hwmonitor::spawn(HwmonitorConfig::default()),
        }
    }

    pub fn ui(&self, ui: &mut Ui) {
        // The sampler ticks at its own cadence; we just ask egui to redraw at
        // the same rate so the displayed numbers stay fresh.
        ui.ctx().request_repaint_after(REFRESH_INTERVAL);

        let snapshot = self.monitor.latest();
        ScrollArea::new([false, true]).show(ui, |ui| {
            ui.add_space(4.0);
            self.draw_status_hints(ui, &snapshot);
            self.draw_cpu(ui, snapshot.cpu.as_ref());
            self.draw_gpu(ui, snapshot.gpu.as_ref());
            self.draw_memory(ui, snapshot.memory.as_ref());
            self.draw_storage(ui, &snapshot.storage);
            self.draw_network(ui, &snapshot.network);
        });
    }

    fn draw_status_hints(&self, ui: &mut Ui, snap: &Snapshot) {
        let mut hints = Vec::new();
        if snap.cpu.is_none() {
            hints.push("CPU: sysinfo not initialised yet");
        }
        let lhm_missing = snap
            .cpu
            .as_ref()
            .is_some_and(|c| c.package_temp_c.is_none() && c.package_power_w.is_none() && c.fans_rpm.is_empty());
        if lhm_missing {
            hints.push(
                "LibreHardwareMonitor not detected — temps, fan RPM and CPU power will be blank. \
                 Start LHM as Administrator to populate them.",
            );
        }
        if snap.gpu.is_none() {
            hints.push(
                "nvidia-smi unavailable and no LHM GPU sensors — install NVIDIA drivers or LHM to see GPU data.",
            );
        }
        for line in hints {
            ui.label(RichText::new(line).italics().weak());
        }
    }

    fn draw_cpu(&self, ui: &mut Ui, cpu: Option<&CpuSample>) {
        CollapsingHeader::new(RichText::new("CPU").size(18.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                let Some(cpu) = cpu else {
                    ui.label("No data");
                    return;
                };
                Grid::new("hw_cpu").num_columns(2).striped(true).show(ui, |ui| {
                    row(ui, "Total load", &format!("{:.1} %", cpu.total_pct));
                    row(ui, "vrserver load", &opt_pct(cpu.vrserver_pct));
                    row(ui, "Frequency", &format!("{} MHz", cpu.freq_mhz));
                    row(ui, "Package temp", &opt_temp(cpu.package_temp_c));
                    row(ui, "Package power", &opt_watts(cpu.package_power_w));
                    row(ui, "Cores power", &opt_watts(cpu.cores_power_w));
                });

                if !cpu.per_core_pct.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Per-core load").strong());
                    Grid::new("hw_cpu_cores").num_columns(4).striped(true).show(ui, |ui| {
                        for (idx, load) in cpu.per_core_pct.iter().enumerate() {
                            let temp = cpu.per_core_temp_c.get(idx).copied();
                            let label = match temp {
                                Some(t) => format!("Core {idx}: {load:.0}% @ {t:.0} °C"),
                                None => format!("Core {idx}: {load:.0}%"),
                            };
                            ui.label(label);
                            if idx % 4 == 3 {
                                ui.end_row();
                            }
                        }
                    });
                }

                if !cpu.per_core_power_w.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Per-core power").strong());
                    Grid::new("hw_cpu_core_power")
                        .num_columns(4)
                        .striped(true)
                        .show(ui, |ui| {
                            for (idx, watts) in cpu.per_core_power_w.iter().enumerate() {
                                ui.label(format!("Core {idx}: {watts:.2} W"));
                                if idx % 4 == 3 {
                                    ui.end_row();
                                }
                            }
                        });
                }

                if !cpu.fans_rpm.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Fans").strong());
                    Grid::new("hw_cpu_fans").num_columns(2).striped(true).show(ui, |ui| {
                        named_rows(ui, &cpu.fans_rpm, |rpm| format!("{rpm} RPM"));
                    });
                }
            });
    }

    fn draw_gpu(&self, ui: &mut Ui, gpu: Option<&GpuSample>) {
        CollapsingHeader::new(RichText::new("GPU").size(18.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                let Some(gpu) = gpu else {
                    ui.label("No data");
                    return;
                };
                Grid::new("hw_gpu").num_columns(2).striped(true).show(ui, |ui| {
                    row(ui, "Name", gpu.name.as_deref().unwrap_or("—"));
                    row(ui, "Utilization", &opt_pct(gpu.util_pct));
                    row(ui, "Encoder (NVENC)", &opt_pct(gpu.encoder_util_pct));
                    row(ui, "Decoder (NVDEC)", &opt_pct(gpu.decoder_util_pct));
                    row(
                        ui,
                        "VRAM",
                        &format!(
                            "{} / {} MB",
                            opt_int(gpu.mem_used_mb),
                            opt_int(gpu.mem_total_mb),
                        ),
                    );
                    row(ui, "Temperature", &opt_temp(gpu.temp_c));
                    row(
                        ui,
                        "Power",
                        &format!(
                            "{} / {}",
                            opt_watts(gpu.power_w),
                            opt_watts(gpu.power_limit_w),
                        ),
                    );
                    row(ui, "Clock graphics", &opt_mhz(gpu.clock_graphics_mhz));
                    row(ui, "Clock memory", &opt_mhz(gpu.clock_memory_mhz));
                    row(ui, "Clock video (NVENC)", &opt_mhz(gpu.clock_video_mhz));
                    row(ui, "P-state", gpu.pstate.as_deref().unwrap_or("—"));
                    row(
                        ui,
                        "Throttle reasons",
                        gpu.throttle_reasons.as_deref().unwrap_or("—"),
                    );
                    row(ui, "Fan duty", &opt_pct(gpu.fan_pct));
                });

                if !gpu.fans_rpm.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Fans").strong());
                    Grid::new("hw_gpu_fans").num_columns(2).striped(true).show(ui, |ui| {
                        named_rows(ui, &gpu.fans_rpm, |rpm| format!("{rpm} RPM"));
                    });
                }
            });
    }

    fn draw_memory(&self, ui: &mut Ui, mem: Option<&MemorySample>) {
        CollapsingHeader::new(RichText::new("Memory").size(18.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                let Some(mem) = mem else {
                    ui.label("No data");
                    return;
                };
                Grid::new("hw_mem").num_columns(2).striped(true).show(ui, |ui| {
                    row(
                        ui,
                        "RAM",
                        &format!(
                            "{} / {} MB ({:.1} %)",
                            mem.used_mb, mem.total_mb, mem.used_pct,
                        ),
                    );
                    row(ui, "Available", &format!("{} MB", mem.available_mb));
                    row(
                        ui,
                        "Swap / page file",
                        &format!("{} / {} MB", mem.swap_used_mb, mem.swap_total_mb),
                    );
                    row(
                        ui,
                        "vrserver working set",
                        &mem.vrserver_working_set_mb
                            .map(|v| format!("{v} MB"))
                            .unwrap_or_else(|| "—".to_string()),
                    );
                });
            });
    }

    fn draw_storage(&self, ui: &mut Ui, drives: &[StorageSample]) {
        CollapsingHeader::new(RichText::new("Storage").size(18.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                if drives.is_empty() {
                    ui.label("No drives reported by LHM");
                    return;
                }
                for (i, d) in drives.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(8.0);
                    }
                    ui.label(RichText::new(&d.device).strong());
                    Grid::new(format!("hw_storage_{i}"))
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            row(ui, "Temperature", &opt_temp(d.temp_c));
                            row(ui, "Used space", &opt_pct(d.used_pct));
                            row(ui, "Life remaining", &opt_pct(d.life_left_pct));
                            let total = d.total_gb.map(|g| format!("{g:.1} GB"));
                            let free = d.free_gb.map(|g| format!("{g:.1} GB"));
                            row(
                                ui,
                                "Capacity",
                                &format!(
                                    "{} free of {}",
                                    free.unwrap_or_else(|| "—".to_string()),
                                    total.unwrap_or_else(|| "—".to_string()),
                                ),
                            );
                        });
                }
            });
    }

    fn draw_network(&self, ui: &mut Ui, adapters: &[NetSample]) {
        CollapsingHeader::new(RichText::new("Network").size(18.0).strong())
            .default_open(true)
            .show(ui, |ui| {
                if adapters.is_empty() {
                    ui.label("No adapters reported");
                    return;
                }
                for (i, a) in adapters.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(8.0);
                    }
                    ui.label(RichText::new(&a.adapter).strong());
                    Grid::new(format!("hw_net_{i}"))
                        .num_columns(2)
                        .striped(true)
                        .show(ui, |ui| {
                            row(
                                ui,
                                "Throughput TX / RX",
                                &format!(
                                    "{} / {}",
                                    format_bytes_per_sec(a.bytes_sent_per_sec),
                                    format_bytes_per_sec(a.bytes_recv_per_sec),
                                ),
                            );
                            row(
                                ui,
                                "Packets TX / RX",
                                &format!(
                                    "{} / {} pps",
                                    a.packets_sent_per_sec, a.packets_recv_per_sec
                                ),
                            );
                            row(ui, "Outbound errors", &a.outbound_errors.to_string());
                            row(ui, "Outbound discards", &a.outbound_discarded.to_string());
                            row(
                                ui,
                                "Link speed",
                                &format_bits_per_sec(a.current_bandwidth_bps),
                            );
                        });
                }
            });
    }
}

fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(label);
    ui.label(value);
    ui.end_row();
}

fn named_rows<T: Copy>(ui: &mut Ui, items: &[NamedValue<T>], fmt: impl Fn(T) -> String) {
    for item in items {
        ui.label(&item.name);
        ui.label(fmt(item.value));
        ui.end_row();
    }
}

fn opt_pct(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} %")).unwrap_or_else(|| "—".to_string())
}

fn opt_temp(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} °C")).unwrap_or_else(|| "—".to_string())
}

fn opt_watts(v: Option<f32>) -> String {
    v.map(|x| format!("{x:.1} W")).unwrap_or_else(|| "—".to_string())
}

fn opt_mhz(v: Option<u32>) -> String {
    v.map(|x| format!("{x} MHz")).unwrap_or_else(|| "—".to_string())
}

fn opt_int<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".to_string())
}

fn format_bytes_per_sec(bps: u64) -> String {
    let bytes = bps as f64;
    if bytes >= 1_000_000.0 {
        format!("{:.2} MB/s", bytes / 1_000_000.0)
    } else if bytes >= 1_000.0 {
        format!("{:.1} KB/s", bytes / 1_000.0)
    } else {
        format!("{bps} B/s")
    }
}

fn format_bits_per_sec(bps: u64) -> String {
    let bits = bps as f64;
    if bits >= 1_000_000_000.0 {
        format!("{:.2} Gb/s", bits / 1_000_000_000.0)
    } else if bits >= 1_000_000.0 {
        format!("{:.1} Mb/s", bits / 1_000_000.0)
    } else if bits >= 1_000.0 {
        format!("{:.1} Kb/s", bits / 1_000.0)
    } else if bps == 0 {
        "—".to_string()
    } else {
        format!("{bps} b/s")
    }
}
