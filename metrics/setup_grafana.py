"""Set up Grafana for ALVR metrics.

Steps:
  1. Install grafana-clickhouse-datasource plugin on the server (via SSH)
  2. Create / update the ClickHouse data source in Grafana
  3. Create / update the ALVR dashboard:
       - $host variable: multi-select dropdown with an "All" option
       - Panel: total pipeline latency over time, one series per host

Usage:
    python metrics/setup_grafana.py
"""
from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from _ssh import connect, fix_stdout, load_env, run_script, step

METRICS_DIR = Path(__file__).parent


# ── Grafana HTTP helper ───────────────────────────────────────────────────────

def gf(base: str, path: str, user: str, password: str,
       method: str = "GET", body: object = None) -> tuple[int, dict]:
    url = base.rstrip("/") + path
    data = json.dumps(body).encode() if body is not None else None
    pw = f"{user}:{password}".encode()
    import base64
    token = base64.b64encode(pw).decode()
    headers = {"Authorization": f"Basic {token}", "Content-Type": "application/json"}
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=15) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


# ── 1. Plugin install ─────────────────────────────────────────────────────────

def install_plugin(client, passwd: str) -> None:
    print("  Installing grafana-clickhouse-datasource …")
    rc = run_script(client, r"""
grafana cli --homepath /usr/share/grafana plugins install grafana-clickhouse-datasource 2>&1 || true
systemctl restart grafana-server
for i in $(seq 1 20); do
    curl -sf http://127.0.0.1:3000/grafana/api/health >/dev/null 2>&1 && echo "grafana up" && exit 0
    sleep 2
done
echo "grafana did not come up" >&2; exit 1
""", passwd, timeout=120)
    if rc != 0:
        print("  WARNING: plugin install may have failed — continuing anyway")


# ── 2. Data source ────────────────────────────────────────────────────────────

def setup_datasource(grafana: str, gf_user: str, gf_pass: str,
                     ch_host: str, ch_port: int,
                     ch_user: str, ch_pass: str, ch_db: str) -> str:
    body = {
        "name": "ClickHouse-ALVR",
        "type": "grafana-clickhouse-datasource",
        "access": "proxy",
        "isDefault": True,
        "jsonData": {
            "server": ch_host,
            "port": ch_port,
            "username": ch_user,
            "defaultDatabase": ch_db,
            "protocol": "http",
            "tlsSkipVerify": True,
        },
        "secureJsonData": {"password": ch_pass},
    }

    status, existing = gf(grafana, "/api/datasources/name/ClickHouse-ALVR", gf_user, gf_pass)
    if status == 200:
        ds_id = existing["id"]
        gf(grafana, f"/api/datasources/{ds_id}", gf_user, gf_pass, "PUT", body)
        print(f"  Updated datasource (id={ds_id})")
    else:
        sc, resp = gf(grafana, "/api/datasources", gf_user, gf_pass, "POST", body)
        print(f"  Created datasource: HTTP {sc} — {resp.get('message', '')}")

    _, ds = gf(grafana, "/api/datasources/name/ClickHouse-ALVR", gf_user, gf_pass)
    uid = ds["uid"]
    print(f"  Datasource UID: {uid}")
    return uid


# ── 3. Dashboard ──────────────────────────────────────────────────────────────

# ── Shared dashboard primitives ──────────────────────────────────────────────

HF = "host IN ($host)"       # host filter (uses the $host template variable)
TF = "$__timeFilter(ts)"     # time filter
T  = "toStartOfMinute(ts) AS time"


def host_variable(ds_ref: dict, source_table: str) -> dict:
    """Standard $host multi-select dropdown, populated from `source_table`."""
    return {
        "name": "host",
        "label": "Host",
        "type": "query",
        "datasource": ds_ref,
        "query": f"SELECT DISTINCT host FROM {source_table} ORDER BY host",
        "multi": True,
        "includeAll": True,
        "allValue": "",
        "current": {"selected": True, "text": "All", "value": "$__all"},
        "options": [],
        "refresh": 2,
        "sort": 1,
        "hide": 0,
    }


def ts_panel_factory(ds_ref: dict):
    def ts_panel(title: str, sql: str, unit: str, pid: int,
                 x: int = 0, y: int = 0, w: int = 24, h: int = 8) -> dict:
        return {
            "id": pid,
            "type": "timeseries",
            "title": title,
            "gridPos": {"x": x, "y": y, "w": w, "h": h},
            "datasource": ds_ref,
            "fieldConfig": {
                "defaults": {
                    "unit": unit,
                    "custom": {"lineWidth": 2, "fillOpacity": 8},
                },
                "overrides": [],
            },
            "options": {
                "tooltip": {"mode": "multi", "sort": "desc"},
                "legend": {"displayMode": "table", "placement": "bottom",
                           "calcs": ["mean", "min", "max", "last"]},
            },
            "targets": [{
                "datasource": ds_ref,
                "rawSql": sql,
                "format": 0,
            }],
        }
    return ts_panel


def row_panel(title: str, pid: int, y: int) -> dict:
    return {
        "id": pid,
        "type": "row",
        "title": title,
        "collapsed": False,
        "gridPos": {"x": 0, "y": y, "w": 24, "h": 1},
        "panels": [],
    }


def per_host_ts(table: str, col: str, agg: str = "avg") -> str:
    """One series per host over time."""
    return (f"SELECT {T}, host, {agg}({col}) AS value\n"
            f"FROM {table}\n"
            f"WHERE {TF} AND {HF}\n"
            f"GROUP BY time, host\nORDER BY time")


def per_host_dim_ts(table: str, dim: str, col: str, agg: str = "avg") -> str:
    """One series per (host, dimension) over time — for per-core / per-device /
    per-adapter / per-slot tables. Series label format: `host / dim`."""
    return (f"SELECT {T}, concat(host, ' / ', toString({dim})) AS series, {agg}({col}) AS value\n"
            f"FROM {table}\n"
            f"WHERE {TF} AND {HF}\n"
            f"GROUP BY time, series\nORDER BY time")


def cross_host_breakdown(table: str, cols: dict[str, str]) -> str:
    """One series per metric column, averaged across hosts."""
    selects = ",\n    ".join(f"avg({col}) AS \"{label}\"" for col, label in cols.items())
    return (f"SELECT {T},\n    {selects}\n"
            f"FROM {table}\n"
            f"WHERE {TF} AND {HF}\n"
            f"GROUP BY time\nORDER BY time")


# ── Dashboard 1: ALVR Streaming Metrics (existing) ───────────────────────────

def build_dashboard(ds_uid: str) -> dict:
    ds_ref = {"type": "grafana-clickhouse-datasource", "uid": ds_uid}

    variable = host_variable(ds_ref, "alvr.streaming_metrics")
    ts_panel = ts_panel_factory(ds_ref)

    def dev(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.streaming_metrics", col, agg)

    def hs(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.headset", col, agg)

    def breakdown(cols: dict[str, str]) -> str:
        return cross_host_breakdown("alvr.streaming_metrics", cols)

    STAGES = {
        "game_time":        "Game Time",
        "server_compositor": "Server Compositor",
        "encoder":          "Encoder",
        "network":          "Network",
        "decoder":          "Decoder",
        "decoder_queue":    "Decoder Queue",
        "client_compositor": "Client Compositor",
        "vsync_queue":      "VSync Queue",
    }

    def stage_breakdown(stat: str) -> str:
        return breakdown({f"{s}_{stat}_ms": label for s, label in STAGES.items()})

    panels = [
        # ── Latency ────────────────────────────────────────────────────────
        row_panel("Latency", 100, 0),

        ts_panel("Total Pipeline Latency — per host (avg ms)",
                 dev("total_pipeline_avg_ms"), "ms", 1, 0, 1, 24, 8),

        row_panel("Latency — Average", 110, 9),
        ts_panel("All Stages — Average (ms)",
                 stage_breakdown("avg"), "ms", 2, 0, 10, 24, 8),

        row_panel("Latency — Min", 111, 18),
        ts_panel("All Stages — Min (ms)",
                 stage_breakdown("min"), "ms", 3, 0, 19, 24, 8),

        row_panel("Latency — Max", 112, 27),
        ts_panel("All Stages — Max (ms)",
                 stage_breakdown("max"), "ms", 4, 0, 28, 24, 8),

        # ── FPS & Frames ───────────────────────────────────────────────────
        row_panel("FPS & Frames", 101, 36),

        ts_panel("Client FPS (avg)",
                 dev("client_fps_avg"), "short", 9, 0, 37, 12, 8),
        ts_panel("Server FPS (avg)",
                 dev("server_fps_avg"), "short", 10, 12, 37, 12, 8),

        ts_panel("Frames per Window",
                 dev("frames"), "short", 11, 0, 45, 12, 8),
        ts_panel("Dropped Samples",
                 dev("dropped_samples", "sum"), "short", 12, 12, 45, 12, 8),

        # ── Bitrate & Throughput ───────────────────────────────────────────
        row_panel("Bitrate & Throughput", 102, 53),

        ts_panel("Requested Bitrate (bps)",
                 dev("bd_requested_bitrate_bps"), "bps", 13, 0, 54, 12, 8),
        ts_panel("Measured Throughput (bps)",
                 dev("throughput_bps_avg"), "bps", 14, 12, 54, 12, 8),

        ts_panel("Video Throughput (Mbits/s)",
                 dev("video_mbits_per_sec"), "Mbits", 15, 0, 62, 12, 8),
        ts_panel("Video Packets/sec",
                 dev("video_packets_per_sec"), "pps", 16, 12, 62, 12, 8),

        ts_panel("Video Packets Total (cumulative)",
                 dev("video_packets_total", "max"), "short", 17, 0, 70, 12, 8),
        ts_panel("Video Data Total — MB (cumulative)",
                 dev("video_mbytes_total", "max"), "decmbytes", 18, 12, 70, 12, 8),

        # ── Headset (battery + extended telemetry) ────────────────────────
        row_panel("Headset", 103, 78),

        ts_panel("HMD Battery (%)",
                 hs("battery_hmd_pct"), "percent", 19, 0, 79, 8, 8),
        ts_panel("Left Controller Battery (%)",
                 hs("battery_ctl_left_pct"), "percent", 39, 8, 79, 8, 8),
        ts_panel("Right Controller Battery (%)",
                 hs("battery_ctl_right_pct"), "percent", 40, 16, 79, 8, 8),

        ts_panel("HMD Charging (1 = charging)",
                 hs("battery_hmd_plugged"), "short", 21, 0, 87, 8, 8),
        ts_panel("Left Controller Charging",
                 hs("battery_ctl_left_plugged"), "short", 41, 8, 87, 8, 8),
        ts_panel("Right Controller Charging",
                 hs("battery_ctl_right_plugged"), "short", 42, 16, 87, 8, 8),

        ts_panel("HMD Battery Temperature (°C)",
                 hs("hmd_battery_temp_c"), "celsius", 30, 0, 95, 12, 8),
        ts_panel("HMD Thermal Status (0=NONE … 6=SHUTDOWN)",
                 hs("hmd_thermal_status", "max"), "short", 31, 12, 95, 12, 8),

        ts_panel("HMD Thermal Headroom (1.0 ≈ throttling)",
                 hs("hmd_thermal_headroom"), "short", 32, 0, 103, 24, 8),

        ts_panel("HMD Memory Available (KiB)",
                 hs("hmd_mem_available_kib"), "kbytes", 33, 0, 111, 12, 8),
        ts_panel("HMD Process RSS (KiB)",
                 hs("hmd_process_rss_kib"), "kbytes", 34, 12, 111, 12, 8),

        ts_panel("HMD CPU — system (0..1)",
                 hs("hmd_cpu_total_pct"), "percentunit", 35, 0, 119, 12, 8),
        ts_panel("HMD CPU — alvr.client process (0..1)",
                 hs("hmd_cpu_process_pct"), "percentunit", 36, 12, 119, 12, 8),

        ts_panel("HMD GPU Busy (0..1)",
                 hs("hmd_gpu_busy_pct"), "percentunit", 37, 0, 127, 12, 8),
        ts_panel("HMD GPU Frequency (Hz)",
                 hs("hmd_gpu_freq_hz"), "hertz", 38, 12, 127, 12, 8),

        # ── Exporter Health ────────────────────────────────────────────────
        row_panel("Exporter Health", 104, 135),

        ts_panel("Failed POST Attempts",
                 dev("failed_posts", "sum"), "short", 20, 0, 136, 24, 8),
    ]

    return {
        "dashboard": {
            "uid": "alvr-streaming",
            "title": "ALVR Streaming Metrics",
            "tags": ["alvr"],
            "timezone": "browser",
            "schemaVersion": 38,
            "refresh": "30s",
            "time": {"from": "now-3h", "to": "now"},
            "templating": {"list": [variable]},
            "panels": panels,
        },
        "overwrite": True,
        "folderId": 0,
    }


# ── Dashboard 2: ALVR Hardware — Headset ─────────────────────────────────────

def build_dashboard_headset(ds_uid: str) -> dict:
    """Headset hardware telemetry only (alvr.headset). Battery, thermal,
    memory, CPU, GPU as reported by the on-headset `ClientTelemetry` sampler."""
    ds_ref = {"type": "grafana-clickhouse-datasource", "uid": ds_uid}
    variable = host_variable(ds_ref, "alvr.headset")
    ts_panel = ts_panel_factory(ds_ref)

    def hs(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.headset", col, agg)

    panels = [
        # ── Battery ────────────────────────────────────────────────────────
        row_panel("Battery", 100, 0),

        ts_panel("HMD Battery (%)",
                 hs("battery_hmd_pct"), "percent", 1, 0, 1, 8, 8),
        ts_panel("Left Controller Battery (%)",
                 hs("battery_ctl_left_pct"), "percent", 2, 8, 1, 8, 8),
        ts_panel("Right Controller Battery (%)",
                 hs("battery_ctl_right_pct"), "percent", 3, 16, 1, 8, 8),

        ts_panel("HMD Charging (1 = charging)",
                 hs("battery_hmd_plugged"), "short", 4, 0, 9, 8, 8),
        ts_panel("Left Controller Charging",
                 hs("battery_ctl_left_plugged"), "short", 5, 8, 9, 8, 8),
        ts_panel("Right Controller Charging",
                 hs("battery_ctl_right_plugged"), "short", 6, 16, 9, 8, 8),

        # ── Thermal ────────────────────────────────────────────────────────
        row_panel("Thermal", 101, 17),

        ts_panel("HMD Battery Temperature (°C)",
                 hs("hmd_battery_temp_c"), "celsius", 10, 0, 18, 12, 8),
        ts_panel("HMD Thermal Status (0=NONE … 6=SHUTDOWN)",
                 hs("hmd_thermal_status", "max"), "short", 11, 12, 18, 12, 8),
        ts_panel("HMD Thermal Headroom (1.0 ≈ throttling)",
                 hs("hmd_thermal_headroom"), "short", 12, 0, 26, 24, 8),

        # ── Memory ─────────────────────────────────────────────────────────
        row_panel("Memory", 102, 34),

        ts_panel("HMD Memory Available (KiB)",
                 hs("hmd_mem_available_kib"), "kbytes", 20, 0, 35, 8, 8),
        ts_panel("HMD Memory Total (KiB)",
                 hs("hmd_mem_total_kib", "max"), "kbytes", 21, 8, 35, 8, 8),
        ts_panel("HMD Process RSS (KiB)",
                 hs("hmd_process_rss_kib"), "kbytes", 22, 16, 35, 8, 8),

        # ── CPU / GPU ──────────────────────────────────────────────────────
        row_panel("CPU & GPU", 103, 43),

        ts_panel("HMD CPU — system (0..1)",
                 hs("hmd_cpu_total_pct"), "percentunit", 30, 0, 44, 12, 8),
        ts_panel("HMD CPU — alvr.client process (0..1)",
                 hs("hmd_cpu_process_pct"), "percentunit", 31, 12, 44, 12, 8),

        ts_panel("HMD GPU Busy (0..1)",
                 hs("hmd_gpu_busy_pct"), "percentunit", 32, 0, 52, 12, 8),
        ts_panel("HMD GPU Frequency (Hz)",
                 hs("hmd_gpu_freq_hz"), "hertz", 33, 12, 52, 12, 8),
    ]

    return {
        "dashboard": {
            "uid": "alvr-hw-headset",
            "title": "ALVR Hardware — Headset",
            "tags": ["alvr", "hardware", "headset"],
            "timezone": "browser",
            "schemaVersion": 38,
            "refresh": "30s",
            "time": {"from": "now-3h", "to": "now"},
            "templating": {"list": [variable]},
            "panels": panels,
        },
        "overwrite": True,
        "folderId": 0,
    }


# ── Dashboard 3: ALVR Hardware — PC ──────────────────────────────────────────

def build_dashboard_pc(ds_uid: str) -> dict:
    """PC-side host hardware telemetry sampled by alvr_hwmonitor and pushed
    by hwmonitor_exporter. One panel per resource group; per-dimension tables
    (cores, dimms, storage, network) plot one series per (host, dimension)."""
    ds_ref = {"type": "grafana-clickhouse-datasource", "uid": ds_uid}
    variable = host_variable(ds_ref, "alvr.hw_cpu")
    ts_panel = ts_panel_factory(ds_ref)

    def cpu(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.hw_cpu", col, agg)

    def gpu(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.hw_gpu", col, agg)

    def dram(col: str, agg: str = "avg") -> str:
        return per_host_ts("alvr.hw_dram", col, agg)

    panels = [
        # ── CPU (aggregate) ────────────────────────────────────────────────
        row_panel("CPU — Aggregate", 100, 0),

        ts_panel("CPU Load — total (%)",
                 cpu("total_pct"), "percent", 1, 0, 1, 12, 8),
        ts_panel("CPU Load — vrserver.exe (%)",
                 cpu("vrserver_pct"), "percent", 2, 12, 1, 12, 8),

        ts_panel("CPU Frequency (MHz)",
                 cpu("freq_mhz"), "rotmhz", 3, 0, 9, 12, 8),
        ts_panel("CPU Package Temp (°C)",
                 cpu("package_temp_c"), "celsius", 4, 12, 9, 12, 8),

        ts_panel("CPU Package Power (W)",
                 cpu("package_power_w"), "watt", 5, 0, 17, 12, 8),
        ts_panel("CPU Cores Power (W)",
                 cpu("cores_power_w"), "watt", 6, 12, 17, 12, 8),

        # ── CPU per-core ───────────────────────────────────────────────────
        row_panel("CPU — Per Core", 101, 25),

        ts_panel("Per-Core Load (%) — host / core",
                 per_host_dim_ts("alvr.hw_cpu_cores", "core_index", "load_pct"),
                 "percent", 10, 0, 26, 24, 9),
        ts_panel("Per-Core Temperature (°C) — host / core",
                 per_host_dim_ts("alvr.hw_cpu_cores", "core_index", "temp_c"),
                 "celsius", 11, 0, 35, 12, 9),
        ts_panel("Per-Core Power (W) — host / core",
                 per_host_dim_ts("alvr.hw_cpu_cores", "core_index", "power_w"),
                 "watt", 12, 12, 35, 12, 9),

        # ── GPU ────────────────────────────────────────────────────────────
        row_panel("GPU", 102, 44),

        ts_panel("GPU Utilization (%)",
                 gpu("util_pct"), "percent", 20, 0, 45, 8, 8),
        ts_panel("NVENC Encoder Utilization (%)",
                 gpu("encoder_util_pct"), "percent", 21, 8, 45, 8, 8),
        ts_panel("NVDEC Decoder Utilization (%)",
                 gpu("decoder_util_pct"), "percent", 22, 16, 45, 8, 8),

        ts_panel("GPU VRAM Used (MB)",
                 gpu("mem_used_mb"), "decmbytes", 23, 0, 53, 12, 8),
        ts_panel("GPU Temperature (°C)",
                 gpu("temp_c"), "celsius", 24, 12, 53, 12, 8),

        ts_panel("GPU Power (W)",
                 gpu("power_w"), "watt", 25, 0, 61, 12, 8),
        ts_panel("GPU Power Limit (W)",
                 gpu("power_limit_w", "max"), "watt", 26, 12, 61, 12, 8),

        ts_panel("GPU Graphics Clock (MHz)",
                 gpu("clock_graphics_mhz"), "rotmhz", 27, 0, 69, 8, 8),
        ts_panel("GPU Memory Clock (MHz)",
                 gpu("clock_memory_mhz"), "rotmhz", 28, 8, 69, 8, 8),
        ts_panel("GPU Video Clock (MHz)",
                 gpu("clock_video_mhz"), "rotmhz", 29, 16, 69, 8, 8),

        ts_panel("GPU Fan (%)",
                 gpu("fan_pct"), "percent", 30, 0, 77, 24, 8),

        # ── DRAM ───────────────────────────────────────────────────────────
        row_panel("DRAM", 103, 85),

        ts_panel("DRAM Used (%)",
                 dram("used_pct"), "percent", 40, 0, 86, 12, 8),
        ts_panel("DRAM Available (MB)",
                 dram("available_mb"), "decmbytes", 41, 12, 86, 12, 8),

        ts_panel("Swap Used (MB)",
                 dram("swap_used_mb"), "decmbytes", 42, 0, 94, 12, 8),
        ts_panel("vrserver.exe Working Set (MB)",
                 dram("vrserver_working_set_mb"), "decmbytes", 43, 12, 94, 12, 8),

        # ── DIMMs ──────────────────────────────────────────────────────────
        row_panel("DIMMs", 104, 102),

        ts_panel("DIMM Temperature (°C) — host / slot",
                 per_host_dim_ts("alvr.hw_dimms", "slot", "temp_c"),
                 "celsius", 50, 0, 103, 24, 9),

        # ── Storage ────────────────────────────────────────────────────────
        row_panel("Storage", 105, 112),

        ts_panel("Storage Temperature (°C) — host / device",
                 per_host_dim_ts("alvr.hw_storage", "device", "temp_c"),
                 "celsius", 60, 0, 113, 12, 9),
        ts_panel("Storage Used (%) — host / device",
                 per_host_dim_ts("alvr.hw_storage", "device", "used_pct"),
                 "percent", 61, 12, 113, 12, 9),

        ts_panel("Storage Life Left (%) — host / device",
                 per_host_dim_ts("alvr.hw_storage", "device", "life_left_pct"),
                 "percent", 62, 0, 122, 12, 9),
        ts_panel("Storage Free (GB) — host / device",
                 per_host_dim_ts("alvr.hw_storage", "device", "free_gb"),
                 "decgbytes", 63, 12, 122, 12, 9),

        # ── Network ────────────────────────────────────────────────────────
        row_panel("Network", 106, 131),

        ts_panel("Network — Bytes Sent/s (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "bytes_sent_per_sec"),
                 "Bps", 70, 0, 132, 12, 9),
        ts_panel("Network — Bytes Recv/s (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "bytes_recv_per_sec"),
                 "Bps", 71, 12, 132, 12, 9),

        ts_panel("Network — Packets Sent/s (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "packets_sent_per_sec"),
                 "pps", 72, 0, 141, 12, 9),
        ts_panel("Network — Packets Recv/s (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "packets_recv_per_sec"),
                 "pps", 73, 12, 141, 12, 9),

        ts_panel("Network — Outbound Errors (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "outbound_errors", "sum"),
                 "short", 74, 0, 150, 12, 9),
        ts_panel("Network — Outbound Discarded (host / adapter)",
                 per_host_dim_ts("alvr.hw_network", "adapter", "outbound_discarded", "sum"),
                 "short", 75, 12, 150, 12, 9),
    ]

    return {
        "dashboard": {
            "uid": "alvr-hw-pc",
            "title": "ALVR Hardware — PC",
            "tags": ["alvr", "hardware", "pc"],
            "timezone": "browser",
            "schemaVersion": 38,
            "refresh": "30s",
            "time": {"from": "now-3h", "to": "now"},
            "templating": {"list": [variable]},
            "panels": panels,
        },
        "overwrite": True,
        "folderId": 0,
    }


def push_dashboard(grafana: str, gf_user: str, gf_pass: str, dash: dict) -> str:
    title = dash["dashboard"]["title"]
    sc, resp = gf(grafana, "/api/dashboards/db", gf_user, gf_pass, "POST", dash)
    if sc in (200, 412):
        url = resp.get("url", "")
        print(f"  {title}: {url}")
        return url
    raise RuntimeError(f"Dashboard '{title}' save failed HTTP {sc}: {resp}")


def setup_dashboard(grafana: str, gf_user: str, gf_pass: str, ds_uid: str) -> str:
    return push_dashboard(grafana, gf_user, gf_pass, build_dashboard(ds_uid))


# ── main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    fix_stdout()
    cfg = load_env(METRICS_DIR / ".env")
    host    = cfg["SSH_METRICS_HOST"]
    port    = int(cfg.get("SSH_METRICS_PORT", "22"))
    user    = cfg["SSH_METRICS_USER"]
    passwd  = cfg["SSH_METRICS_PASSWORD"]
    gf_user = cfg.get("GRAFANA_USER", "admin")
    gf_pass = cfg.get("GRAFANA_PASSWORD", "admin")
    grafana = f"http://{host}/grafana"

    print(f"Connecting to {user}@{host}:{port} …")
    client = connect(host, port, user, passwd)
    print("Connected.\n")

    step(1, 3, "Install Grafana ClickHouse plugin + reset admin password")
    run_script(client,
        f"grafana cli --homepath /usr/share/grafana admin reset-admin-password '{gf_pass}' 2>&1 | tail -1",
        passwd)
    install_plugin(client, passwd)

    step(2, 3, "Create ClickHouse data source")
    ds_uid = setup_datasource(
        grafana, gf_user, gf_pass,
        ch_host=cfg.get("CLICKHOUSE_HOST", "127.0.0.1"),
        ch_port=int(cfg.get("CLICKHOUSE_PORT", "8123")),
        ch_user=cfg.get("CLICKHOUSE_USER", "default"),
        ch_pass=cfg.get("CLICKHOUSE_PASSWORD", ""),
        ch_db=cfg.get("CLICKHOUSE_DATABASE", "alvr"),
    )

    step(3, 3, "Create ALVR dashboards")
    urls = [
        push_dashboard(grafana, gf_user, gf_pass, build_dashboard(ds_uid)),
        push_dashboard(grafana, gf_user, gf_pass, build_dashboard_headset(ds_uid)),
        push_dashboard(grafana, gf_user, gf_pass, build_dashboard_pc(ds_uid)),
    ]

    print(f"\nDone.  Dashboards:")
    for u in urls:
        print(f"   -> http://{host}/grafana{u}")
    print(f"       Use the Host dropdown to filter by host or select All.")


if __name__ == "__main__":
    main()
