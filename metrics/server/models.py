"""Pydantic models for the ALVR metrics snapshot payload.

Mirrors the JSON produced by `Aggregator::flush` in
`alvr/server_core/src/metrics_exporter.rs`. Fields the exporter may omit
when the corresponding accumulator is empty are typed as Optional.
"""

from __future__ import annotations

from datetime import datetime
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field


class AccStats(BaseModel):
    """Min/max/avg/n for a single accumulator over a window."""

    min: float
    max: float
    avg: float
    n: int


class LatencyMs(BaseModel):
    model_config = ConfigDict(extra="ignore")

    total_pipeline: Optional[AccStats] = None
    game_time: Optional[AccStats] = None
    server_compositor: Optional[AccStats] = None
    encoder: Optional[AccStats] = None
    network: Optional[AccStats] = None
    decoder: Optional[AccStats] = None
    decoder_queue: Optional[AccStats] = None
    client_compositor: Optional[AccStats] = None
    vsync_queue: Optional[AccStats] = None


class Fps(BaseModel):
    client: Optional[AccStats] = None
    server: Optional[AccStats] = None


class Throughput(BaseModel):
    throughput_bps: Optional[AccStats] = None
    bitrate_bps: Optional[AccStats] = None
    video_packets_per_sec: float
    video_mbits_per_sec: float


class Totals(BaseModel):
    video_packets: int
    video_mbytes: int


class Battery(BaseModel):
    hmd_pct: int
    hmd_plugged: bool


class BitrateDirectives(BaseModel):
    scaled_calculated_throughput_bps: Optional[float] = None
    decoder_latency_limiter_bps: Optional[float] = None
    network_latency_limiter_bps: Optional[float] = None
    encoder_latency_limiter_bps: Optional[float] = None
    manual_max_throughput_bps: Optional[float] = None
    manual_min_throughput_bps: Optional[float] = None
    requested_bitrate_bps: float


class ExporterHealth(BaseModel):
    failed_posts: int


class Snapshot(BaseModel):
    model_config = ConfigDict(extra="ignore")

    ts: datetime
    device: str = ""
    session: str = ""
    window_ms: int
    frames: int
    dropped_samples: int
    latency_ms: LatencyMs = Field(default_factory=LatencyMs)
    fps: Fps = Field(default_factory=Fps)
    throughput: Throughput
    totals: Totals
    battery: Optional[Battery] = None
    bitrate_directives: BitrateDirectives
    exporter: ExporterHealth
