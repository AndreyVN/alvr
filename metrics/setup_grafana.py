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

def build_dashboard(ds_uid: str) -> dict:
    ds_ref = {"type": "grafana-clickhouse-datasource", "uid": ds_uid}

    variable = {
        "name": "host",
        "label": "Host",
        "type": "query",
        "datasource": ds_ref,
        "query": "SELECT DISTINCT host FROM alvr.streaming_metrics ORDER BY host",
        "multi": True,
        "includeAll": True,
        "allValue": "",
        "current": {"selected": True, "text": "All", "value": "$__all"},
        "options": [],
        "refresh": 2,
        "sort": 1,
        "hide": 0,
    }

    HF = "host IN ($host)"       # host filter
    TF = "$__timeFilter(ts)"     # time filter
    T  = "toStartOfMinute(ts) AS time"

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

    def row_panel(title: str, pid: int, y: int) -> dict:
        return {
            "id": pid,
            "type": "row",
            "title": title,
            "collapsed": False,
            "gridPos": {"x": 0, "y": y, "w": 24, "h": 1},
            "panels": [],
        }

    # per-host time series (one series per host)
    def dev(col: str, agg: str = "avg") -> str:
        return (f"SELECT {T}, host, {agg}({col}) AS value\n"
                f"FROM alvr.streaming_metrics\n"
                f"WHERE {TF} AND {HF}\n"
                f"GROUP BY time, host\nORDER BY time")

    # multi-column breakdown (one series per metric column, averaged across hosts)
    def breakdown(cols: dict[str, str]) -> str:
        selects = ",\n    ".join(f"avg({col}) AS \"{label}\"" for col, label in cols.items())
        return (f"SELECT {T},\n    {selects}\n"
                f"FROM alvr.streaming_metrics\n"
                f"WHERE {TF} AND {HF}\n"
                f"GROUP BY time\nORDER BY time")

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

        # ── Battery ────────────────────────────────────────────────────────
        row_panel("Battery", 103, 78),

        ts_panel("HMD Battery (%)",
                 dev("battery_hmd_pct"), "percent", 19, 0, 79, 12, 8),
        ts_panel("HMD Charging (1 = charging)",
                 dev("battery_hmd_plugged"), "short", 21, 12, 79, 12, 8),

        # ── Exporter Health ────────────────────────────────────────────────
        row_panel("Exporter Health", 104, 87),

        ts_panel("Failed POST Attempts",
                 dev("failed_posts", "sum"), "short", 20, 0, 88, 24, 8),
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


def setup_dashboard(grafana: str, gf_user: str, gf_pass: str, ds_uid: str) -> str:
    dash = build_dashboard(ds_uid)
    sc, resp = gf(grafana, "/api/dashboards/db", gf_user, gf_pass, "POST", dash)
    if sc in (200, 412):
        url = resp.get("url", "")
        print(f"  Dashboard saved: {url}")
        return url
    raise RuntimeError(f"Dashboard save failed HTTP {sc}: {resp}")


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

    step(3, 3, "Create ALVR dashboard")
    url = setup_dashboard(grafana, gf_user, gf_pass, ds_uid)

    print(f"\nDone.  Dashboard -> http://{host}/grafana{url}")
    print(f"       Use the Host dropdown to filter by host or select All.")


if __name__ == "__main__":
    main()
