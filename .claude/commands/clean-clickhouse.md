Truncate all rows from the `alvr.streaming_metrics` table on the remote metrics server.

Steps:
1. Read `metrics/.env` to get SSH and ClickHouse credentials.
2. SSH into the server using the `_ssh` helpers from `metrics/_ssh.py`.
3. Run `TRUNCATE TABLE alvr.streaming_metrics` via `clickhouse-client`.
4. Report success or print the error output on failure.

Use this exact Python snippet (run with the Bash tool):

```python
import sys
from pathlib import Path
sys.path.insert(0, "metrics")
from _ssh import connect, load_env, run_script, fix_stdout
fix_stdout()

cfg = load_env(Path("metrics/.env"))
host   = cfg["SSH_METRICS_HOST"]
port   = int(cfg.get("SSH_METRICS_PORT", "22"))
user   = cfg["SSH_METRICS_USER"]
passwd = cfg["SSH_METRICS_PASSWORD"]
ch_user = cfg.get("CLICKHOUSE_USER", "default")
ch_pass = cfg.get("CLICKHOUSE_PASSWORD", "changeme")

print(f"Connecting to {user}@{host}:{port} ...")
client = connect(host, port, user, passwd)
print("Connected.")

script = f"""#!/usr/bin/env bash
set -euo pipefail
clickhouse-client --user {ch_user} --password '{ch_pass}' --query 'TRUNCATE TABLE alvr.streaming_metrics'
echo "Truncated alvr.streaming_metrics"
"""

rc = run_script(client, script, passwd, timeout=60)
client.close()
sys.exit(rc)
```

Run it as: `python - <<'EOF'\n<snippet>\nEOF`
