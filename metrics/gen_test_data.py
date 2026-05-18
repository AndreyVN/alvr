"""Generate and POST synthetic ALVR snapshots to the ingest server.

Per iteration, posts one streaming-metrics snapshot to `/metrics` and one
hardware-metrics snapshot to `/hw_metrics`, sharing host + timestamp so
they join cleanly in ClickHouse. Covers every field consumed by
`metrics/server/models.py` (`Snapshot` + `HwSnapshot`), including the
extended-telemetry fields the modern client emits when
`Settings.metrics.extended_headset_telemetry` is on (controller battery
and the bundled `ClientTelemetry` payload).

Each fake host has a stable hardware profile (GPU, DIMMs, storage,
network adapters) so the per-host time series looks like one real
machine sampled over time.

Usage:
    python metrics/gen_test_data.py [--url URL] [--hw-url URL] [--count N]

Defaults:
    --url     METRICS_URL from metrics/.env, falls back to
              http://193.104.57.232/metrics/metrics
    --hw-url  HW_METRICS_URL from metrics/.env, falls back to the same
              host as --url with the trailing path swapped to /hw_metrics
    --count   100
"""
from __future__ import annotations

import argparse
import json
import math
import random
import sys
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Optional

sys.path.insert(0, str(Path(__file__).parent))
from _ssh import load_env


HOSTS = [f"Host{n}" for n in range(1, 11)]

# Stable per-host hardware fingerprint. Picking from a small set so the
# fakes look like real machines rather than uniform-random noise.
GPU_MODELS = [
    ("NVIDIA GeForce RTX 4090", 24576, 450),
    ("NVIDIA GeForce RTX 4080", 16384, 320),
    ("NVIDIA GeForce RTX 3080", 10240, 320),
    ("NVIDIA GeForce RTX 3070", 8192, 220),
    ("NVIDIA GeForce RTX 2070 SUPER", 8192, 215),
]
STORAGE_MODELS = [
    ("Samsung SSD 990 PRO 2TB", 2000.0),
    ("WD_BLACK SN850X 1TB", 1000.0),
    ("Crucial T700 2TB", 2000.0),
    ("Seagate ST4000DM004", 4000.0),
]
NETWORK_ADAPTERS = [
    ("Intel Wi-Fi 6E AX211", 2_400_000_000),
    ("Realtek PCIe GbE Family Controller", 1_000_000_000),
    ("Intel I225-V 2.5GbE", 2_500_000_000),
]


def host_profile(host: str) -> dict:
    """Deterministic hardware fingerprint for `host` — same machine every call."""
    rng = random.Random(host)
    cores = rng.choice([6, 8, 12, 16])
    dimm_count = rng.choice([2, 4])
    dimm_cap = rng.choice([8.0, 16.0, 32.0])
    storage_devices = rng.sample(STORAGE_MODELS, k=rng.randint(1, 2))
    adapters = rng.sample(NETWORK_ADAPTERS, k=rng.randint(1, 2))
    gpu_name, gpu_mem_mb, gpu_power_limit = rng.choice(GPU_MODELS)
    has_gpu = rng.random() > 0.1  # ~10% of hosts: no nvidia-smi available
    has_lhm = rng.random() > 0.2  # ~20%: LHM not running → no temps / per-DIMM
    # Extended telemetry toggle: ~70% of hosts have it enabled.
    extended_telemetry = rng.random() > 0.3
    return {
        "cores": cores,
        "total_mem_mb": int(dimm_count * dimm_cap * 1024),
        "dimms": [
            {
                "slot": f"DIMM_{i} {rng.choice(['Corsair', 'G.Skill', 'Kingston'])} {int(dimm_cap)}GB",
                "capacity_gb": dimm_cap,
                "has_temp": has_lhm,
            }
            for i in range(dimm_count)
        ],
        "storage": [
            {"device": name, "total_gb": total_gb, "has_temp": has_lhm}
            for name, total_gb in storage_devices
        ],
        "adapters": [
            {"adapter": name, "bw_bps": bw_bps}
            for name, bw_bps in adapters
        ],
        "gpu": (
            {
                "name": gpu_name,
                "mem_total_mb": gpu_mem_mb,
                "power_limit_w": gpu_power_limit,
            }
            if has_gpu
            else None
        ),
        "has_lhm": has_lhm,
        "extended_telemetry": extended_telemetry,
    }


def acc(base: float, spread: float = 0.1, n_lo: int = 60, n_hi: int = 90) -> dict:
    lo = base * (1 - spread)
    hi = base * (1 + spread)
    avg = random.uniform(lo, hi)
    return {
        "min": round(lo, 3),
        "max": round(hi, 3),
        "avg": round(avg, 3),
        "n": random.randint(n_lo, n_hi),
    }


def maybe(prob: float):
    return random.random() < prob


def make_streaming_snapshot(ts: datetime, host: str, prof: dict, i: int) -> dict:
    phase = i / 100 * 2 * math.pi
    pipeline_ms = 40 + 10 * math.sin(phase) + random.gauss(0, 1)
    bitrate_bps = 50_000_000 + 10_000_000 * math.cos(phase) + random.gauss(0, 500_000)
    network_ms = max(1.0, pipeline_ms * 0.4 + random.gauss(0, 1))

    battery: dict = {
        "hmd_pct": max(5, 100 - i),
        "hmd_plugged": False,
    }
    client_telemetry: Optional[dict] = None
    if prof["extended_telemetry"]:
        # ~80% of intervals carry a controller battery sample (controllers can
        # disconnect / sleep mid-session).
        if maybe(0.8):
            battery["ctl_left_pct"] = max(10, 95 - i // 2)
            battery["ctl_left_plugged"] = False
        if maybe(0.8):
            battery["ctl_right_pct"] = max(10, 92 - i // 2)
            battery["ctl_right_plugged"] = False

        # Drift battery temperature with the session — climbs slowly as the
        # headset warms up. Thermal headroom shrinks correspondingly.
        battery_temp = 28.0 + i * 0.05 + random.gauss(0, 0.5)
        headroom = max(0.1, 0.95 - i * 0.003 + random.gauss(0, 0.02))
        thermal_status = 0 if headroom > 0.6 else 1 if headroom > 0.3 else 2

        mem_total_kib = 6 * 1024 * 1024  # 6 GiB Quest-class device
        mem_avail_kib = int(mem_total_kib * random.uniform(0.25, 0.45))
        rss_kib = random.randint(180_000, 320_000)

        client_telemetry = {
            "battery_temperature_c": round(battery_temp, 2),
            "thermal_status": thermal_status,
            "thermal_headroom": round(headroom, 3),
            "mem_total_kib": mem_total_kib,
            "mem_available_kib": mem_avail_kib,
            "process_rss_kib": rss_kib,
            "cpu_total_pct": round(random.uniform(0.25, 0.65), 3),
            "cpu_process_pct": round(random.uniform(0.30, 1.20), 3),
            "gpu_busy_pct": round(random.uniform(0.45, 0.85), 3),
            "gpu_freq_hz": random.choice([490_000_000, 525_000_000, 587_000_000]),
        }

    return {
        "ts": ts.isoformat(),
        "host": host,
        "window_ms": 1000,
        "frames": random.randint(70, 75),
        "dropped_samples": random.randint(0, 2),
        "latency_ms": {
            "total_pipeline":     acc(pipeline_ms),
            "game_time":          acc(8.0),
            "server_compositor":  acc(2.5),
            "encoder":            acc(4.0),
            "network":            acc(network_ms),
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
            "throughput_bps":        acc(bitrate_bps * 1.05),
            "bitrate_bps":           acc(bitrate_bps),
            "video_packets_per_sec": round(random.uniform(180, 220), 1),
            "video_mbits_per_sec":   round(bitrate_bps / 1_000_000, 2),
        },
        "totals": {
            "video_packets": i * 200 + random.randint(0, 50),
            "video_mbytes":  i * 6 + random.randint(0, 2),
        },
        "battery": battery,
        "client_telemetry": client_telemetry,
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


def make_hw_snapshot(ts: datetime, host: str, prof: dict, i: int) -> dict:
    phase = i / 100 * 2 * math.pi
    cpu_load = 0.45 + 0.25 * math.sin(phase) + random.gauss(0, 0.03)
    cpu_load = max(0.05, min(0.99, cpu_load))
    per_core_pct = [
        round(max(0.0, min(100.0, cpu_load * 100 + random.gauss(0, 8))), 1)
        for _ in range(prof["cores"])
    ]
    per_core_temp = (
        [round(55 + cpu_load * 25 + random.gauss(0, 1.5), 1) for _ in range(prof["cores"])]
        if prof["has_lhm"]
        else []
    )
    per_core_power = (
        [round(2.0 + cpu_load * 6.0 + random.gauss(0, 0.4), 2) for _ in range(prof["cores"])]
        if prof["has_lhm"]
        else []
    )
    cores_power_w = round(sum(per_core_power), 2) if per_core_power else None
    package_power_w = (
        round((cores_power_w or 0) + random.uniform(8, 18), 2)
        if cores_power_w is not None
        else None
    )

    cpu = {
        "total_pct": round(cpu_load * 100, 2),
        "freq_mhz": random.choice([3800, 4200, 4500, 5100]),
        "vrserver_pct": round(min(cpu_load * 100 + random.uniform(5, 20), 100.0), 2),
        "package_temp_c": round(55 + cpu_load * 25, 1) if prof["has_lhm"] else None,
        "package_power_w": package_power_w,
        "cores_power_w": cores_power_w,
    }

    cpu_cores = [
        {
            "index": idx,
            "load_pct": per_core_pct[idx],
            "temp_c": per_core_temp[idx] if per_core_temp else None,
            "power_w": per_core_power[idx] if per_core_power else None,
        }
        for idx in range(prof["cores"])
    ]

    gpu = None
    if prof["gpu"] is not None:
        gpu_util = max(0.0, min(100.0, 60 + 25 * math.cos(phase) + random.gauss(0, 5)))
        gpu_power = prof["gpu"]["power_limit_w"] * (0.4 + gpu_util / 250)
        gpu = {
            "name": prof["gpu"]["name"],
            "util_pct": round(gpu_util, 1),
            "encoder_util_pct": round(min(100.0, gpu_util * 0.6 + random.uniform(5, 15)), 1),
            "decoder_util_pct": round(random.uniform(0, 5), 1),
            "mem_used_mb": int(prof["gpu"]["mem_total_mb"] * random.uniform(0.3, 0.7)),
            "mem_total_mb": prof["gpu"]["mem_total_mb"],
            "temp_c": round(55 + gpu_util * 0.25 + random.gauss(0, 1.5), 1)
            if prof["has_lhm"]
            else None,
            "power_w": round(gpu_power, 1),
            "power_limit_w": float(prof["gpu"]["power_limit_w"]),
            "clock_graphics_mhz": random.choice([1800, 2100, 2400, 2700]),
            "clock_memory_mhz": random.choice([7000, 9500, 10500]),
            "clock_video_mhz": random.choice([1500, 1700, 1900]),
            "fan_pct": round(30 + gpu_util * 0.5 + random.gauss(0, 3), 1),
        }

    used_mb = int(prof["total_mem_mb"] * random.uniform(0.45, 0.75))
    available_mb = prof["total_mem_mb"] - used_mb
    dram = {
        "total_mb": prof["total_mem_mb"],
        "used_mb": used_mb,
        "available_mb": available_mb,
        "used_pct": round(used_mb / prof["total_mem_mb"] * 100, 2),
        "swap_total_mb": 8192,
        "swap_used_mb": random.randint(0, 2048),
        "vrserver_working_set_mb": random.randint(800, 1800),
    }

    dimms = [
        {
            "slot": d["slot"],
            "capacity_gb": d["capacity_gb"],
            "temp_c": round(36 + random.gauss(0, 2), 1) if d["has_temp"] else None,
        }
        for d in prof["dimms"]
    ]

    storage = [
        {
            "device": s["device"],
            "temp_c": round(38 + random.gauss(0, 3), 1) if s["has_temp"] else None,
            "used_pct": round(random.uniform(30, 85), 1),
            "life_left_pct": round(random.uniform(80, 99), 1),
            "total_gb": s["total_gb"],
            "free_gb": round(s["total_gb"] * random.uniform(0.15, 0.55), 1),
        }
        for s in prof["storage"]
    ]

    network = [
        {
            "adapter": a["adapter"],
            "bytes_sent_per_sec": random.randint(2_000_000, 8_500_000),
            "bytes_recv_per_sec": random.randint(50_000, 400_000),
            "packets_sent_per_sec": random.randint(2_500, 5_500),
            "packets_recv_per_sec": random.randint(500, 2_500),
            "outbound_errors": random.randint(0, 1),
            "outbound_discarded": random.randint(0, 3),
            "current_bandwidth_bps": a["bw_bps"],
        }
        for a in prof["adapters"]
    ]

    return {
        "ts": ts.isoformat(),
        "host": host,
        "cpu": cpu,
        "cpu_cores": cpu_cores,
        "gpu": gpu,
        "dram": dram,
        "dimms": dimms,
        "storage": storage,
        "network": network,
    }


def post(url: str, payload: dict, retries: int = 2, timeout: float = 10.0) -> int:
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        url, data=data,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    last_err: Optional[str] = None
    for attempt in range(retries + 1):
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.status
        except urllib.error.HTTPError as e:
            # 4xx/5xx — not worth retrying.
            print(f"  HTTP {e.code}: {e.read().decode(errors='replace')}", file=sys.stderr)
            return e.code
        except Exception as e:  # noqa: BLE001 — transient: timeout, conn reset, dns blip
            last_err = f"{type(e).__name__}: {e}"
    print(f"  POST failed after {retries + 1} attempts: {last_err}", file=sys.stderr)
    return 0


def derive_hw_url(metrics_url: str) -> str:
    """Swap the last path segment of `metrics_url` for `hw_metrics`."""
    parts = urllib.parse.urlsplit(metrics_url)
    path = parts.path.rstrip("/")
    if "/" in path:
        path = path.rsplit("/", 1)[0] + "/hw_metrics"
    else:
        path = "/hw_metrics"
    return urllib.parse.urlunsplit((parts.scheme, parts.netloc, path, parts.query, parts.fragment))


def main() -> None:
    cfg = load_env(Path(__file__).parent / ".env")
    default_url = cfg.get("METRICS_URL", "http://193.104.57.232/metrics/metrics")
    default_hw_url = cfg.get("HW_METRICS_URL", derive_hw_url(default_url))

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--url",     default=default_url, help="Streaming metrics endpoint (POST /metrics).")
    ap.add_argument("--hw-url",  default=default_hw_url, help="Hardware metrics endpoint (POST /hw_metrics).")
    ap.add_argument("--count",   type=int, default=100, help="Snapshots per endpoint (default 100).")
    ap.add_argument("--seed",    type=int, default=None, help="Optional RNG seed for reproducible runs.")
    args = ap.parse_args()

    if args.seed is not None:
        random.seed(args.seed)

    profiles = {host: host_profile(host) for host in HOSTS}
    now = datetime.now(timezone.utc)

    ok_streaming = 0
    ok_hw = 0
    for i in range(args.count):
        ts = now - timedelta(seconds=(args.count - i) * 5)
        host = HOSTS[i % len(HOSTS)]
        prof = profiles[host]

        snap = make_streaming_snapshot(ts, host, prof, i)
        hw_snap = make_hw_snapshot(ts, host, prof, i)

        code_s = post(args.url, snap)
        code_h = post(args.hw_url, hw_snap)
        ok_streaming += code_s in (200, 204)
        ok_hw += code_h in (200, 204)

        status = "ok" if code_s in (200, 204) and code_h in (200, 204) else f"FAILED ({code_s}/{code_h})"
        print(f"  [{i+1:3d}/{args.count}] {host:>6s}  metrics={code_s} hw={code_h}  {status}")

    print(f"\n{ok_streaming}/{args.count} streaming snapshots posted to {args.url}")
    print(f"{ok_hw}/{args.count} hardware snapshots posted to {args.hw_url}")


if __name__ == "__main__":
    main()
