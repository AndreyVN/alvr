"""Full one-shot install for a new ALVR metrics server.

Runs provision.py (nginx + ClickHouse + Grafana) then deploy.py
(FastAPI ingest server + ClickHouse schema + systemd service).

Usage:
    # 1. Fill in metrics/.env  (copy from metrics/.env and edit)
    # 2. Run:
    python metrics/install.py

Re-running on an already-provisioned server is safe (all steps are
idempotent). Use the individual scripts for partial updates:
    python metrics/provision.py   # server infrastructure only
    python metrics/deploy.py      # FastAPI app + schema only
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import provision
import deploy
from _ssh import fix_stdout, load_env
fix_stdout()

METRICS_DIR = Path(__file__).parent


def main() -> None:
    cfg = load_env(METRICS_DIR / ".env")
    host = cfg["SSH_METRICS_HOST"]
    user = cfg["SSH_METRICS_USER"]

    print("=" * 60)
    print(f"  ALVR metrics full install on {user}@{host}")
    print("=" * 60)

    print("\n>>> Phase 1: Provision server infrastructure\n")
    provision.main()

    print("\n>>> Phase 2: Deploy FastAPI ingest service\n")
    deploy.main()

    print()
    print("=" * 60)
    print("  Install complete. Service URLs:")
    print(f"  Grafana    -> http://{host}/grafana/   (admin / admin)")
    print(f"  ClickHouse -> http://{host}/clickhouse/")
    print(f"  Metrics    -> http://{host}/metrics/metrics  (POST)")
    print(f"  Health     -> http://{host}/metrics/health")
    print()
    print("  Set in ALVR dashboard:")
    print(f"  extra > Metrics export > URL: http://{host}/metrics/metrics")
    print("=" * 60)


if __name__ == "__main__":
    main()
