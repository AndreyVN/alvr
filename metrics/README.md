# metrics/

Reference ingest for the ALVR streamer's telemetry exporters.

The streamer's `metrics_exporter` and `hwmonitor_exporter` (in `alvr/server_core/src/`) POST JSON snapshots to two HTTP endpoints when `Settings → Metrics → Metrics export` is enabled. The pieces here are the receiving side of that pipeline:

- `clickhouse_schema.sql` — ClickHouse table definitions (`alvr.streaming_metrics`, `alvr.headset`, `alvr.hw_*`).
- `server/` — FastAPI ingest that maps POSTed JSON onto the schema (`app.py`, `db.py`, `models.py`).
- `setup_grafana.py` — provisions the ClickHouse datasource and dashboard panels.
- `deploy.py` / `install.py` / `provision.py` / `_ssh.py` — host bootstrap helpers.
- `gen_test_data.py` — synthesizes plausible snapshots for dashboard work without a real headset.

## Field reference

- **`metrics.txt`** — per-column reference for `alvr.streaming_metrics` (latency, FPS, throughput, battery, client telemetry). Plain-text, ASCII layout — read it in a fixed-width viewer.
- `metrics_v1.txt` — historical reference kept for diffing against the current schema.
