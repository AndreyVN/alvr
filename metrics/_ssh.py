"""Shared SSH/SFTP helpers for metrics provisioning scripts."""
from __future__ import annotations

import io
import os
import sys
import time
from pathlib import Path
from typing import Optional

import paramiko

sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", errors="replace")


def load_env(env_file: Optional[Path] = None) -> dict[str, str]:
    path = env_file or Path(__file__).parent / ".env"
    env: dict[str, str] = {}
    if path.exists():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            k, _, v = line.partition("=")
            env[k.strip()] = v.strip()
    # env file values do NOT override real env vars
    for k, v in env.items():
        os.environ.setdefault(k, v)
    return {k: os.environ.get(k, v) for k, v in env.items()}


def connect(host: str, port: int, user: str, password: str,
            timeout: int = 20) -> paramiko.SSHClient:
    c = paramiko.SSHClient()
    c.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    c.connect(host, port=port, username=user, password=password, timeout=timeout)
    return c


def run_script(client: paramiko.SSHClient, script: str,
               sudo_password: str, timeout: int = 600) -> int:
    """Upload script as /tmp/_alvr_run.sh, execute as root, stream output."""
    sftp = client.open_sftp()
    sftp.putfo(io.BytesIO(script.encode()), "/tmp/_alvr_run.sh")
    sftp.chmod("/tmp/_alvr_run.sh", 0o700)
    sftp.close()

    _, stdout, _ = client.exec_command(
        "sudo -S bash /tmp/_alvr_run.sh 2>&1",
        timeout=timeout,
        get_pty=True,
    )
    stdout.channel.sendall((sudo_password + "\n").encode())

    while not stdout.channel.exit_status_ready():
        if stdout.channel.recv_ready():
            chunk = stdout.channel.recv(8192).decode(errors="replace")
            sys.stdout.write(chunk)
            sys.stdout.flush()
        time.sleep(0.05)
    rest = stdout.read().decode(errors="replace")
    if rest:
        sys.stdout.write(rest)
        sys.stdout.flush()

    return stdout.channel.recv_exit_status()


def upload_tree(sftp: paramiko.SFTPClient, local: Path, remote: str) -> None:
    """Recursively upload a directory tree via SFTP."""
    try:
        sftp.mkdir(remote)
    except OSError:
        pass
    for item in sorted(local.iterdir()):
        if item.name in {"__pycache__", ".env", "*.pyc"}:
            continue
        rpath = f"{remote}/{item.name}"
        if item.is_dir():
            upload_tree(sftp, item, rpath)
        else:
            sftp.put(str(item), rpath)
            print(f"  > {rpath}")


def step(n: int, total: int, label: str) -> None:
    print(f"\n[{n}/{total}] {label}")
    print("-" * 60)
