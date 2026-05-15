"""ClickHouse insert path for ALVR metric snapshots.

Flattens snapshots into the column layouts defined in
`metrics/clickhouse_schema.sql` and pushes rows via `clickhouse-connect`.
The connection is process-wide and reused.
"""

from __future__ import annotations

import os
from functools import lru_cache
from typing import Any, List, Optional, Tuple

import clickhouse_connect
from clickhouse_connect.driver.client import Client

from .models import AccStats, HwSnapshot, Snapshot

TABLE = "alvr.streaming_metrics"

COLUMNS: Tuple[str, ...] = (
    "ts",
    "host",
    "window_ms",
    "frames",
    "dropped_samples",
    "total_pipeline_min_ms", "total_pipeline_max_ms", "total_pipeline_avg_ms", "total_pipeline_n",
    "game_time_min_ms", "game_time_max_ms", "game_time_avg_ms", "game_time_n",
    "server_compositor_min_ms", "server_compositor_max_ms", "server_compositor_avg_ms", "server_compositor_n",
    "encoder_min_ms", "encoder_max_ms", "encoder_avg_ms", "encoder_n",
    "network_min_ms", "network_max_ms", "network_avg_ms", "network_n",
    "decoder_min_ms", "decoder_max_ms", "decoder_avg_ms", "decoder_n",
    "decoder_queue_min_ms", "decoder_queue_max_ms", "decoder_queue_avg_ms", "decoder_queue_n",
    "client_compositor_min_ms", "client_compositor_max_ms", "client_compositor_avg_ms", "client_compositor_n",
    "vsync_queue_min_ms", "vsync_queue_max_ms", "vsync_queue_avg_ms", "vsync_queue_n",
    "client_fps_min", "client_fps_max", "client_fps_avg", "client_fps_n",
    "server_fps_min", "server_fps_max", "server_fps_avg", "server_fps_n",
    "throughput_bps_min", "throughput_bps_max", "throughput_bps_avg", "throughput_bps_n",
    "bitrate_bps_min", "bitrate_bps_max", "bitrate_bps_avg", "bitrate_bps_n",
    "video_packets_per_sec",
    "video_mbits_per_sec",
    "video_packets_total",
    "video_mbytes_total",
    "battery_hmd_pct",
    "battery_hmd_plugged",
    "bd_scaled_calculated_throughput_bps",
    "bd_decoder_latency_limiter_bps",
    "bd_network_latency_limiter_bps",
    "bd_encoder_latency_limiter_bps",
    "bd_manual_max_throughput_bps",
    "bd_manual_min_throughput_bps",
    "bd_requested_bitrate_bps",
    "failed_posts",
)


# ─────────────────────────── hardware tables ───────────────────────────

HW_CPU_TABLE = "alvr.hw_cpu"
HW_CPU_COLS: Tuple[str, ...] = (
    "ts", "host", "total_pct", "freq_mhz", "vrserver_pct",
    "package_temp_c", "package_power_w", "cores_power_w",
)

HW_CPU_CORES_TABLE = "alvr.hw_cpu_cores"
HW_CPU_CORES_COLS: Tuple[str, ...] = (
    "ts", "host", "core_index", "load_pct", "temp_c", "power_w",
)

HW_GPU_TABLE = "alvr.hw_gpu"
HW_GPU_COLS: Tuple[str, ...] = (
    "ts", "host", "name", "util_pct", "encoder_util_pct", "decoder_util_pct",
    "mem_used_mb", "mem_total_mb", "temp_c", "power_w", "power_limit_w",
    "clock_graphics_mhz", "clock_memory_mhz", "clock_video_mhz", "fan_pct",
)

HW_DRAM_TABLE = "alvr.hw_dram"
HW_DRAM_COLS: Tuple[str, ...] = (
    "ts", "host", "total_mb", "used_mb", "available_mb", "used_pct",
    "swap_total_mb", "swap_used_mb", "vrserver_working_set_mb",
)

HW_DIMMS_TABLE = "alvr.hw_dimms"
HW_DIMMS_COLS: Tuple[str, ...] = (
    "ts", "host", "slot", "capacity_gb", "temp_c",
)

HW_STORAGE_TABLE = "alvr.hw_storage"
HW_STORAGE_COLS: Tuple[str, ...] = (
    "ts", "host", "device", "temp_c", "used_pct", "life_left_pct", "total_gb", "free_gb",
)

HW_NETWORK_TABLE = "alvr.hw_network"
HW_NETWORK_COLS: Tuple[str, ...] = (
    "ts", "host", "adapter",
    "bytes_sent_per_sec", "bytes_recv_per_sec",
    "packets_sent_per_sec", "packets_recv_per_sec",
    "outbound_errors", "outbound_discarded", "current_bandwidth_bps",
)


@lru_cache(maxsize=1)
def get_client() -> Client:
    return clickhouse_connect.get_client(
        host=os.environ.get("CLICKHOUSE_HOST", "127.0.0.1"),
        port=int(os.environ.get("CLICKHOUSE_PORT", "8123")),
        username=os.environ.get("CLICKHOUSE_USER", "default"),
        password=os.environ.get("CLICKHOUSE_PASSWORD", ""),
        database=os.environ.get("CLICKHOUSE_DATABASE", "alvr"),
        compress=True,
    )


def _acc(stat: Optional[AccStats]) -> Tuple[Optional[float], Optional[float], Optional[float], int]:
    if stat is None:
        return (None, None, None, 0)
    return (stat.min, stat.max, stat.avg, stat.n)


def snapshot_to_row(s: Snapshot) -> List[Any]:
    lat = s.latency_ms
    fps = s.fps
    thr = s.throughput
    bat = s.battery
    bd = s.bitrate_directives

    row: List[Any] = [
        s.ts,
        s.host,
        s.window_ms,
        s.frames,
        s.dropped_samples,
        *_acc(lat.total_pipeline),
        *_acc(lat.game_time),
        *_acc(lat.server_compositor),
        *_acc(lat.encoder),
        *_acc(lat.network),
        *_acc(lat.decoder),
        *_acc(lat.decoder_queue),
        *_acc(lat.client_compositor),
        *_acc(lat.vsync_queue),
        *_acc(fps.client),
        *_acc(fps.server),
        *_acc(thr.throughput_bps),
        *_acc(thr.bitrate_bps),
        thr.video_packets_per_sec,
        thr.video_mbits_per_sec,
        s.totals.video_packets,
        s.totals.video_mbytes,
        bat.hmd_pct if bat else None,
        (1 if bat.hmd_plugged else 0) if bat else None,
        bd.scaled_calculated_throughput_bps,
        bd.decoder_latency_limiter_bps,
        bd.network_latency_limiter_bps,
        bd.encoder_latency_limiter_bps,
        bd.manual_max_throughput_bps,
        bd.manual_min_throughput_bps,
        bd.requested_bitrate_bps,
        s.exporter.failed_posts,
    ]
    return row


def insert(snapshot: Snapshot) -> None:
    client = get_client()
    client.insert(TABLE, [snapshot_to_row(snapshot)], column_names=list(COLUMNS))


def insert_hw(snap: HwSnapshot) -> None:
    """Fan the hardware snapshot out into the per-resource tables."""
    client = get_client()
    ts = snap.ts
    host = snap.host

    if snap.cpu is not None:
        c = snap.cpu
        client.insert(
            HW_CPU_TABLE,
            [[ts, host, c.total_pct, c.freq_mhz, c.vrserver_pct,
              c.package_temp_c, c.package_power_w, c.cores_power_w]],
            column_names=list(HW_CPU_COLS),
        )

    if snap.cpu_cores:
        rows = [
            [ts, host, core.index, core.load_pct, core.temp_c, core.power_w]
            for core in snap.cpu_cores
        ]
        client.insert(HW_CPU_CORES_TABLE, rows, column_names=list(HW_CPU_CORES_COLS))

    if snap.gpu is not None:
        g = snap.gpu
        client.insert(
            HW_GPU_TABLE,
            [[ts, host, g.name or "", g.util_pct, g.encoder_util_pct, g.decoder_util_pct,
              g.mem_used_mb, g.mem_total_mb, g.temp_c, g.power_w, g.power_limit_w,
              g.clock_graphics_mhz, g.clock_memory_mhz, g.clock_video_mhz, g.fan_pct]],
            column_names=list(HW_GPU_COLS),
        )

    if snap.dram is not None:
        d = snap.dram
        client.insert(
            HW_DRAM_TABLE,
            [[ts, host, d.total_mb, d.used_mb, d.available_mb, d.used_pct,
              d.swap_total_mb, d.swap_used_mb, d.vrserver_working_set_mb]],
            column_names=list(HW_DRAM_COLS),
        )

    if snap.dimms:
        rows = [[ts, host, m.slot, m.capacity_gb, m.temp_c] for m in snap.dimms]
        client.insert(HW_DIMMS_TABLE, rows, column_names=list(HW_DIMMS_COLS))

    if snap.storage:
        rows = [
            [ts, host, s.device, s.temp_c, s.used_pct, s.life_left_pct, s.total_gb, s.free_gb]
            for s in snap.storage
        ]
        client.insert(HW_STORAGE_TABLE, rows, column_names=list(HW_STORAGE_COLS))

    if snap.network:
        rows = [
            [ts, host, n.adapter,
             n.bytes_sent_per_sec, n.bytes_recv_per_sec,
             n.packets_sent_per_sec, n.packets_recv_per_sec,
             n.outbound_errors, n.outbound_discarded, n.current_bandwidth_bps]
            for n in snap.network
        ]
        client.insert(HW_NETWORK_TABLE, rows, column_names=list(HW_NETWORK_COLS))
