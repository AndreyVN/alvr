"""Deploy the ALVR metrics FastAPI server to the remote host.

Uploads metrics/server/, creates a Python venv, applies the ClickHouse
schema, and installs/restarts the alvr-metrics systemd service.
Safe to re-run — all steps are idempotent.

Usage:
    python metrics/deploy.py
"""
from __future__ import annotations

import io
import sys
import textwrap
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from _ssh import connect, load_env, run_script, step, upload_tree

METRICS_DIR = Path(__file__).parent
DEPLOY_DIR  = "/opt/alvr-metrics"
UVICORN_PORT = 8087


def build_install_script(cfg: dict) -> str:
    ch_host = cfg.get("CLICKHOUSE_HOST", "127.0.0.1")
    ch_port = cfg.get("CLICKHOUSE_PORT", "8123")
    ch_user = cfg.get("CLICKHOUSE_USER", "default")
    ch_pass = cfg.get("CLICKHOUSE_PASSWORD", "changeme")
    ch_db   = cfg.get("CLICKHOUSE_DATABASE", "alvr")

    return textwrap.dedent(f"""\
        #!/usr/bin/env bash
        set -euo pipefail

        # ── venv + Python deps ────────────────────────────────────────────────
        echo "[1/4] Python venv"
        apt-get install -y python3-venv python3-pip
        python3 -m venv {DEPLOY_DIR}/venv
        {DEPLOY_DIR}/venv/bin/pip install --quiet \\
            fastapi "uvicorn[standard]" pydantic clickhouse-connect

        # ── runtime .env ──────────────────────────────────────────────────────
        echo "[2/4] Runtime env"
        cat > {DEPLOY_DIR}/.env <<'ENVEOF'
CLICKHOUSE_HOST={ch_host}
CLICKHOUSE_PORT={ch_port}
CLICKHOUSE_USER={ch_user}
CLICKHOUSE_PASSWORD={ch_pass}
CLICKHOUSE_DATABASE={ch_db}
ENVEOF
        chmod 600 {DEPLOY_DIR}/.env

        # ── apply ClickHouse schema ───────────────────────────────────────────
        echo "[3/4] ClickHouse schema"
        clickhouse-client --user {ch_user} --password '{ch_pass}' \\
            --multiquery < {DEPLOY_DIR}/clickhouse_schema.sql
        echo "  Schema applied."

        # ── systemd service ───────────────────────────────────────────────────
        echo "[4/4] systemd service"
        cat > /etc/systemd/system/alvr-metrics.service <<'SVCEOF'
[Unit]
Description=ALVR Metrics ingest (FastAPI/uvicorn)
After=network.target clickhouse-server.service

[Service]
Type=simple
WorkingDirectory={DEPLOY_DIR}
EnvironmentFile={DEPLOY_DIR}/.env
ExecStart={DEPLOY_DIR}/venv/bin/uvicorn server.app:app \\
    --host 127.0.0.1 --port {UVICORN_PORT} --no-access-log
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
SVCEOF

        systemctl daemon-reload
        systemctl enable alvr-metrics
        systemctl restart alvr-metrics
        sleep 3
        systemctl is-active alvr-metrics || {{
            journalctl -u alvr-metrics -n 40 --no-pager
            exit 1
        }}
        echo "  alvr-metrics: active"

        # ── smoke test ────────────────────────────────────────────────────────
        curl -sf http://127.0.0.1:{UVICORN_PORT}/health && echo "  /health: ok"
        echo "Deploy complete."
    """)


def main() -> None:
    cfg = load_env(METRICS_DIR / ".env")
    host   = cfg["SSH_METRICS_HOST"]
    port   = int(cfg.get("SSH_METRICS_PORT", "22"))
    user   = cfg["SSH_METRICS_USER"]
    passwd = cfg["SSH_METRICS_PASSWORD"]

    print(f"Connecting to {user}@{host}:{port} ...")
    client = connect(host, port, user, passwd)
    print("Connected.\n")

    # ── 1. Upload app + schema ─────────────────────────────────────────────
    step(1, 2, "Upload metrics/server/ and schema")
    rc = run_script(client, f"mkdir -p {DEPLOY_DIR} && chown {user} {DEPLOY_DIR}", passwd)
    if rc != 0:
        sys.exit(rc)

    sftp = client.open_sftp()
    upload_tree(sftp, METRICS_DIR / "server", f"{DEPLOY_DIR}/server")
    sftp.put(str(METRICS_DIR / "clickhouse_schema.sql"),
             f"{DEPLOY_DIR}/clickhouse_schema.sql")
    sftp.putfo(io.BytesIO(b""), f"{DEPLOY_DIR}/__init__.py")
    sftp.close()
    print(f"  uploaded schema -> {DEPLOY_DIR}/clickhouse_schema.sql")

    # ── 2. Install venv, schema, service ──────────────────────────────────
    step(2, 2, "Install Python deps, apply schema, register service")
    rc = run_script(client, build_install_script(cfg), passwd, timeout=300)
    client.close()

    if rc != 0:
        print(f"\nERROR: script exited with code {rc}", file=sys.stderr)
        sys.exit(rc)

    print(f"\nDeploy finished.")
    print(f"  Metrics ingest -> http://{host}/metrics/metrics  (POST)")
    print(f"  Health check   -> http://{host}/metrics/health")
    print(f"  Point ALVR extra.metrics_export.url at the ingest URL.")


if __name__ == "__main__":
    main()
