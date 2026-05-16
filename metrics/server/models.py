"""Pydantic models for ALVR metrics ingest payloads.

Two endpoints, two top-level payloads:

* `Snapshot`  — streaming-stat snapshot pushed by
  `alvr/server_core/src/metrics_exporter.rs::Aggregator::flush`.
* `HwSnapshot` — host hardware telemetry pushed by
  `alvr/server_core/src/hwmonitor_exporter.rs::build_payload`. Each
  section maps to its own ClickHouse table; all are joined by `host`.

Fields the exporter may omit when the corresponding source is unavailable
are typed as Optional.
"""

from __future__ import annotations

from datetime import datetime
from typing import List, Optional

from pydantic import BaseModel, ConfigDict, Field


# ─────────────────────────── streaming metrics ──────────────────────────


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


class ClientTelemetry(BaseModel):
    """Optional headset resource/thermal sample. Wire shape mirrors
    `alvr_packets::ClientTelemetry`; any field can be omitted when the
    sensor is unavailable on the client device."""

    model_config = ConfigDict(extra="ignore")

    battery_temperature_c: Optional[float] = None
    thermal_status: Optional[int] = None
    thermal_headroom: Optional[float] = None
    mem_total_kib: Optional[int] = None
    mem_available_kib: Optional[int] = None
    process_rss_kib: Optional[int] = None
    cpu_total_pct: Optional[float] = None
    cpu_process_pct: Optional[float] = None
    gpu_busy_pct: Optional[float] = None
    gpu_freq_hz: Optional[int] = None


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
    host: str = ""
    window_ms: int
    frames: int
    dropped_samples: int
    latency_ms: LatencyMs = Field(default_factory=LatencyMs)
    fps: Fps = Field(default_factory=Fps)
    throughput: Throughput
    totals: Totals
    battery: Optional[Battery] = None
    client_telemetry: Optional[ClientTelemetry] = None
    bitrate_directives: BitrateDirectives
    exporter: ExporterHealth


# ─────────────────────────── hardware metrics ───────────────────────────


class HwCpu(BaseModel):
    model_config = ConfigDict(extra="ignore")

    total_pct: Optional[float] = None
    freq_mhz: Optional[int] = None
    vrserver_pct: Optional[float] = None
    package_temp_c: Optional[float] = None
    package_power_w: Optional[float] = None
    cores_power_w: Optional[float] = None


class HwCpuCore(BaseModel):
    index: int
    load_pct: Optional[float] = None
    temp_c: Optional[float] = None
    power_w: Optional[float] = None


class HwGpu(BaseModel):
    model_config = ConfigDict(extra="ignore")

    name: Optional[str] = None
    util_pct: Optional[float] = None
    encoder_util_pct: Optional[float] = None
    decoder_util_pct: Optional[float] = None
    mem_used_mb: Optional[int] = None
    mem_total_mb: Optional[int] = None
    temp_c: Optional[float] = None
    power_w: Optional[float] = None
    power_limit_w: Optional[float] = None
    clock_graphics_mhz: Optional[int] = None
    clock_memory_mhz: Optional[int] = None
    clock_video_mhz: Optional[int] = None
    fan_pct: Optional[float] = None


class HwDram(BaseModel):
    total_mb: int
    used_mb: int
    available_mb: int
    used_pct: float
    swap_total_mb: int
    swap_used_mb: int
    vrserver_working_set_mb: Optional[int] = None


class HwDimm(BaseModel):
    slot: str
    capacity_gb: Optional[float] = None
    temp_c: Optional[float] = None


class HwStorage(BaseModel):
    device: str
    temp_c: Optional[float] = None
    used_pct: Optional[float] = None
    life_left_pct: Optional[float] = None
    total_gb: Optional[float] = None
    free_gb: Optional[float] = None


class HwNetwork(BaseModel):
    adapter: str
    bytes_sent_per_sec: int
    bytes_recv_per_sec: int
    packets_sent_per_sec: int
    packets_recv_per_sec: int
    outbound_errors: int
    outbound_discarded: int
    current_bandwidth_bps: int


class HwSnapshot(BaseModel):
    model_config = ConfigDict(extra="ignore")

    ts: datetime
    host: str = ""
    cpu: Optional[HwCpu] = None
    cpu_cores: List[HwCpuCore] = Field(default_factory=list)
    gpu: Optional[HwGpu] = None
    dram: Optional[HwDram] = None
    dimms: List[HwDimm] = Field(default_factory=list)
    storage: List[HwStorage] = Field(default_factory=list)
    network: List[HwNetwork] = Field(default_factory=list)
