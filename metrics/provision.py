"""Install nginx, ClickHouse, and Grafana on a fresh Debian/Ubuntu server.

Reads connection details from metrics/.env. Safe to re-run — every step
is idempotent.

Usage:
    python metrics/provision.py
"""
from __future__ import annotations

import sys
import textwrap
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from _ssh import connect, load_env, run_script, step

# ---------------------------------------------------------------------------
METRICS_DIR = Path(__file__).parent

def build_script(cfg: dict) -> str:
    host     = cfg["SSH_METRICS_HOST"]
    ch_pass  = cfg["CLICKHOUSE_PASSWORD"]
    ng_user  = cfg["SSH_METRICS_USER"]

    return textwrap.dedent(f"""\
        #!/usr/bin/env bash
        set -euo pipefail
        export DEBIAN_FRONTEND=noninteractive

        # ── 0. remove stale apt sources from any previous attempt ────────────
        rm -f /etc/apt/sources.list.d/grafana.list \\
              /etc/apt/keyrings/grafana.gpg

        apt-get update -qq

        # ── 1. nginx ─────────────────────────────────────────────────────────
        echo "[1/5] nginx"
        apt-get install -y nginx apache2-utils

        # ── 2. ClickHouse ─────────────────────────────────────────────────────
        echo "[2/5] ClickHouse"
        apt-get install -y apt-transport-https ca-certificates curl gnupg
        curl -fsSL https://packages.clickhouse.com/rpm/lts/repodata/repomd.xml.key \\
            | gpg --dearmor | tee /usr/share/keyrings/clickhouse-keyring.gpg >/dev/null
        echo "deb [signed-by=/usr/share/keyrings/clickhouse-keyring.gpg] \\
https://packages.clickhouse.com/deb stable main" \\
            | tee /etc/apt/sources.list.d/clickhouse.list
        apt-get update -qq
        apt-get install -y clickhouse-server clickhouse-client

        # password config (idempotent — overwrites the file each run)
        mkdir -p /etc/clickhouse-server/users.d
        cat > /etc/clickhouse-server/users.d/alvr_password.xml <<'CHEOF'
<clickhouse>
  <users>
    <default>
      <password>{ch_pass}</password>
      <networks><ip>::/0</ip></networks>
      <profile>default</profile>
      <quota>default</quota>
    </default>
  </users>
</clickhouse>
CHEOF
        systemctl enable --now clickhouse-server

        # ── 3. Grafana ────────────────────────────────────────────────────────
        # Direct .deb download — apt.grafana.com (Fastly) may be geo-blocked.
        echo "[3/5] Grafana"
        apt-get install -y wget adduser libfontconfig1

        # Latest non-security release from GitHub releases API
        GF_VER=$(curl -sf "https://api.github.com/repos/grafana/grafana/releases?per_page=20" \\
            | grep -oP '"tag_name": "v\\K[^"]+' \\
            | grep -vE 'security|beta|pre|rc' \\
            | head -1 || true)
        GF_VER="${{GF_VER:-12.0.1}}"
        echo "  Grafana $GF_VER"
        wget -q "https://dl.grafana.com/oss/release/grafana_${{GF_VER}}_amd64.deb" -O /tmp/grafana.deb
        dpkg -i /tmp/grafana.deb && rm /tmp/grafana.deb

        # subpath drop-in so Grafana works under /grafana/
        mkdir -p /etc/systemd/system/grafana-server.service.d
        cat > /etc/systemd/system/grafana-server.service.d/subpath.conf <<'GFEOF'
[Service]
Environment=GF_SERVER_ROOT_URL=http://{host}/grafana/
Environment=GF_SERVER_SERVE_FROM_SUB_PATH=true
GFEOF
        systemctl daemon-reload
        systemctl enable --now grafana-server

        # ── 4. nginx config ───────────────────────────────────────────────────
        # All services on port 80, different paths.
        # Internal ports:  Grafana=3000  ClickHouse=8123  FastAPI=8087
        echo "[4/5] nginx config"
        cat > /etc/nginx/sites-available/alvr-metrics <<'NGEOF'
server {{
    listen 80;

    # Grafana dashboard
    location /grafana/ {{
        proxy_pass         http://127.0.0.1:3000/;
        proxy_set_header   Host $host;
        proxy_set_header   X-Real-IP $remote_addr;
        proxy_set_header   X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_http_version 1.1;
        proxy_set_header   Upgrade $http_upgrade;
        proxy_set_header   Connection "upgrade";
    }}

    # ClickHouse HTTP API (basic-auth)
    location /clickhouse/ {{
        auth_basic           "ClickHouse";
        auth_basic_user_file /etc/nginx/clickhouse.htpasswd;
        proxy_pass           http://127.0.0.1:8123/;
        proxy_set_header     Host $host;
        proxy_set_header     X-Real-IP $remote_addr;
    }}

    # ALVR metrics ingest (FastAPI / uvicorn on 127.0.0.1:8087)
    location /metrics/ {{
        proxy_pass         http://127.0.0.1:8087/;
        proxy_set_header   Host $host;
        proxy_set_header   X-Real-IP $remote_addr;
        proxy_set_header   X-Forwarded-For $proxy_add_x_forwarded_for;
    }}
}}
NGEOF
        ln -sf /etc/nginx/sites-available/alvr-metrics \\
               /etc/nginx/sites-enabled/alvr-metrics
        rm -f /etc/nginx/sites-enabled/default

        # ClickHouse htpasswd (nginx basic-auth, same password as CH user)
        htpasswd -bc /etc/nginx/clickhouse.htpasswd '{ng_user}' '{ch_pass}'

        nginx -t
        systemctl enable --now nginx
        systemctl reload nginx

        # ── 5. firewall: open ports 80 and 22 if ufw is active ───────────────
        echo "[5/5] firewall"
        if command -v ufw &>/dev/null && ufw status | grep -q active; then
            ufw allow 22/tcp
            ufw allow 80/tcp
        fi

        echo
        echo "============================================================"
        echo "  Provision complete."
        echo "  Grafana    -> http://{host}/grafana/   (admin/admin)"
        echo "  ClickHouse -> http://{host}/clickhouse/ ({ng_user}/{ch_pass})"
        echo "============================================================"
    """)


def main() -> None:
    cfg = load_env(METRICS_DIR / ".env")
    host = cfg["SSH_METRICS_HOST"]
    port = int(cfg.get("SSH_METRICS_PORT", "22"))
    user = cfg["SSH_METRICS_USER"]
    passwd = cfg["SSH_METRICS_PASSWORD"]

    print(f"Connecting to {user}@{host}:{port} ...")
    client = connect(host, port, user, passwd)
    print("Connected.\n")

    step(1, 1, "Provision server (nginx + ClickHouse + Grafana)")
    rc = run_script(client, build_script(cfg), passwd)
    client.close()

    if rc != 0:
        print(f"\nERROR: script exited with code {rc}", file=sys.stderr)
        sys.exit(rc)
    print("\nProvision finished successfully.")


if __name__ == "__main__":
    main()
