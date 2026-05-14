"""Generate and POST 100 synthetic ALVR metric snapshots to the ingest server.

Usage:
    python metrics/gen_test_data.py [--url URL] [--count N]

Defaults:
    --url   reads METRICS_URL from metrics/.env, falls back to
            http://193.104.57.232/metrics/metrics
    --count 100
"""
from __future__ import annotations

import argparse
import json
import math
import random
import sys
import urllib.request
import urllib.error
from datetime import datetime, timezone, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from _ssh import load_env


DEVICES = ["Quest2", "Quest3", "QuestPro", "Quest3S"]


def acc(base: float, spread: float = 0.1) -> dict:
    lo = base * (1 - spread)
    hi = base * (1 + spread)
    avg = random.uniform(lo, hi)
    return {"min": round(lo, 3), "max": round(hi, 3), "avg": round(avg, 3), "n": random.randint(60, 90)}


def make_snapshot(ts: datetime, i: int) -> dict:
    # Simulate slight drift in latency and bitrate over time
    phase = i / 100 * 2 * math.pi
    pipeline_ms = 40 + 10 * math.sin(phase) + random.gauss(0, 1)
    bitrate_bps = 50_000_000 + 10_000_000 * math.cos(phase) + random.gauss(0, 500_000)

    return {
        "ts": ts.isoformat(),
        "device": DEVICES[i % len(DEVICES)],
        "session": "test",
        "window_ms": 1000,
        "frames": random.randint(70, 75),
        "dropped_samples": random.randint(0, 2),
        "latency_ms": {
            "total_pipeline": acc(pipeline_ms),
            "game_time":          acc(8.0),
            "server_compositor":  acc(2.5),
            "encoder":            acc(4.0),
            "network":            acc(pipeline_ms * 0.5),
            "decoder":            acc(6.0),
            "decoder_queue":      acc(1.5),
            "client_compositor":  acc(3.0),
            "vsync_queue":        acc(2.0),
        },
        "fps": {
            "client": acc(72.0, 0.02),
            "server": acc(72.0, 0.02),
        },
        "throughput": {
            "throughput_bps":     acc(bitrate_bps * 1.05),
            "bitrate_bps":        acc(bitrate_bps),
            "video_packets_per_sec": round(random.uniform(180, 220), 1),
            "video_mbits_per_sec":   round(bitrate_bps / 1_000_000, 2),
        },
        "totals": {
            "video_packets": i * 200 + random.randint(0, 50),
            "video_mbytes":  i * 6 + random.randint(0, 2),
        },
        "battery": {
            "hmd_pct":    max(5, 100 - i),
            "hmd_plugged": False,
        },
        "bitrate_directives": {
            "scaled_calculated_throughput_bps": round(bitrate_bps * 1.1),
            "decoder_latency_limiter_bps":      None,
            "network_latency_limiter_bps":      None,
            "encoder_latency_limiter_bps":      None,
            "manual_max_throughput_bps":        None,
            "manual_min_throughput_bps":        None,
            "requested_bitrate_bps":            round(bitrate_bps),
        },
        "exporter": {"failed_posts": 0},
    }


def post(url: str, payload: dict) -> int:
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        url, data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status
    except urllib.error.HTTPError as e:
        print(f"  HTTP {e.code}: {e.read().decode(errors='replace')}", file=sys.stderr)
        return e.code


def main() -> None:
    cfg = load_env(Path(__file__).parent / ".env")
    default_url = cfg.get("METRICS_URL", "http://193.104.57.232/metrics/metrics")

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url",   default=default_url)
    ap.add_argument("--count", type=int, default=100)
    args = ap.parse_args()

    now = datetime.now(timezone.utc)
    ok = 0
    for i in range(args.count):
        ts = now - timedelta(seconds=(args.count - i) * 5)
        snap = make_snapshot(ts, i)
        code = post(args.url, snap)
        if code in (200, 204):
            ok += 1
            print(f"  [{i+1:3d}/{args.count}] ok")
        else:
            print(f"  [{i+1:3d}/{args.count}] FAILED (HTTP {code})")

    print(f"\n{ok}/{args.count} records posted to {args.url}")


if __name__ == "__main__":
    main()
