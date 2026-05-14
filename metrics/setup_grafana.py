"""Set up Grafana for ALVR metrics.

Steps:
  1. Install grafana-clickhouse-datasource plugin on the server (via SSH)
  2. Create / update the ClickHouse data source in Grafana
  3. Create / update the ALVR dashboard:
       - $device variable: multi-select dropdown with an "All" option
       - Panel: total pipeline latency over time, one series per device

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
        "name": "device",
        "label": "Device",
        "type": "query",
        "datasource": ds_ref,
        "query": "SELECT DISTINCT device FROM alvr.streaming_metrics ORDER BY device",
        "multi": True,
        "includeAll": True,
        "allValue": "",
        "current": {"selected": True, "text": "All", "value": "$__all"},
        "options": [],
        "refresh": 2,
        "sort": 1,
        "hide": 0,
    }

    def panel(title: str, sql: str, unit: str, pid: int, y: int) -> dict:
        return {
            "id": pid,
            "type": "timeseries",
            "title": title,
            "gridPos": {"x": 0, "y": y, "w": 24, "h": 9},
            "datasource": ds_ref,
            "fieldConfig": {
                "defaults": {
                    "unit": unit,
                    "custom": {"lineWidth": 2, "fillOpacity": 10},
                },
                "overrides": [],
            },
            "options": {
                "tooltip": {"mode": "multi", "sort": "none"},
                "legend": {"displayMode": "table", "placement": "bottom",
                           "calcs": ["mean", "min", "max", "last"]},
            },
            "targets": [{
                "datasource": ds_ref,
                "rawSql": sql,
                "format": 0,  # TIME_SERIES
            }],
        }

    device_filter = "device IN ($device)"

    panels = [
        panel(
            "Total Pipeline Latency (avg ms)",
            f"""SELECT
    toStartOfMinute(ts) AS time,
    device,
    avg(total_pipeline_avg_ms) AS value
FROM alvr.streaming_metrics
WHERE $__timeFilter(ts) AND {device_filter}
GROUP BY time, device
ORDER BY time""",
            "ms", 1, 0,
        ),
        panel(
            "Bitrate (Mbps)",
            f"""SELECT
    toStartOfMinute(ts) AS time,
    device,
    avg(video_mbits_per_sec) AS value
FROM alvr.streaming_metrics
WHERE $__timeFilter(ts) AND {device_filter}
GROUP BY time, device
ORDER BY time""",
            "mbits", 2, 9,
        ),
        panel(
            "Client FPS (avg)",
            f"""SELECT
    toStartOfMinute(ts) AS time,
    device,
    avg(client_fps_avg) AS value
FROM alvr.streaming_metrics
WHERE $__timeFilter(ts) AND {device_filter}
GROUP BY time, device
ORDER BY time""",
            "short", 3, 18,
        ),
        panel(
            "Battery (%)",
            f"""SELECT
    toStartOfMinute(ts) AS time,
    device,
    avg(battery_hmd_pct) AS value
FROM alvr.streaming_metrics
WHERE $__timeFilter(ts) AND {device_filter}
GROUP BY time, device
ORDER BY time""",
            "percent", 4, 27,
        ),
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
    print(f"       Use the Device dropdown to filter by device or select All.")


if __name__ == "__main__":
    main()
