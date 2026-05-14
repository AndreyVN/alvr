"""ClickHouse insert path for ALVR metric snapshots.

Flattens a `Snapshot` into the column layout defined in
`metrics/clickhouse_schema.sql` and pushes one row per snapshot through
`clickhouse-connect`. The connection is process-wide and reused.
"""

from __future__ import annotations

import os
from functools import lru_cache
from typing import Any, List, Optional, Tuple

import clickhouse_connect
from clickhouse_connect.driver.client import Client

from .models import AccStats, Snapshot

TABLE = "alvr.streaming_metrics"

COLUMNS: Tuple[str, ...] = (
    "ts",
    "device",
    "session",
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
        s.device,
        s.session,
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
