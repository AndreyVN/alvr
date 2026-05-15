-- ClickHouse schema for ALVR streaming + host hardware metrics.
--
-- The streamer side has two exporters (`alvr/server_core/src/metrics_exporter.rs`
-- and `alvr/server_core/src/hwmonitor_exporter.rs`). Each POSTs a JSON
-- document every `extra.metrics_export.interval_ms`. The streaming exporter
-- targets `streaming_metrics`; the hardware exporter is fanned out into
-- the `hw_*` tables. All tables share a `host` column, which is the
-- aggregation key for joins/dashboards across resource types.
--
-- Usage:
--   clickhouse-client --multiquery < metrics/clickhouse_schema.sql

CREATE DATABASE IF NOT EXISTS alvr;

-- ─────────────────────────────────────────────────────────────────────
--                      STREAMING METRICS (per-frame stats)
-- ─────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS alvr.streaming_metrics
(
    -- ───── identity ─────
    ts                                      DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                                    LowCardinality(String),
    window_ms                               UInt64,
    frames                                  UInt32,
    dropped_samples                         UInt64,

    -- ───── latency_ms.<stage> (min / max / avg / n) ─────
    total_pipeline_min_ms                   Nullable(Float32),
    total_pipeline_max_ms                   Nullable(Float32),
    total_pipeline_avg_ms                   Nullable(Float32),
    total_pipeline_n                        UInt32 DEFAULT 0,

    game_time_min_ms                        Nullable(Float32),
    game_time_max_ms                        Nullable(Float32),
    game_time_avg_ms                        Nullable(Float32),
    game_time_n                             UInt32 DEFAULT 0,

    server_compositor_min_ms                Nullable(Float32),
    server_compositor_max_ms                Nullable(Float32),
    server_compositor_avg_ms                Nullable(Float32),
    server_compositor_n                     UInt32 DEFAULT 0,

    encoder_min_ms                          Nullable(Float32),
    encoder_max_ms                          Nullable(Float32),
    encoder_avg_ms                          Nullable(Float32),
    encoder_n                               UInt32 DEFAULT 0,

    network_min_ms                          Nullable(Float32),
    network_max_ms                          Nullable(Float32),
    network_avg_ms                          Nullable(Float32),
    network_n                               UInt32 DEFAULT 0,

    decoder_min_ms                          Nullable(Float32),
    decoder_max_ms                          Nullable(Float32),
    decoder_avg_ms                          Nullable(Float32),
    decoder_n                               UInt32 DEFAULT 0,

    decoder_queue_min_ms                    Nullable(Float32),
    decoder_queue_max_ms                    Nullable(Float32),
    decoder_queue_avg_ms                    Nullable(Float32),
    decoder_queue_n                         UInt32 DEFAULT 0,

    client_compositor_min_ms                Nullable(Float32),
    client_compositor_max_ms                Nullable(Float32),
    client_compositor_avg_ms                Nullable(Float32),
    client_compositor_n                     UInt32 DEFAULT 0,

    vsync_queue_min_ms                      Nullable(Float32),
    vsync_queue_max_ms                      Nullable(Float32),
    vsync_queue_avg_ms                      Nullable(Float32),
    vsync_queue_n                           UInt32 DEFAULT 0,

    -- ───── fps.{client,server} ─────
    client_fps_min                          Nullable(Float32),
    client_fps_max                          Nullable(Float32),
    client_fps_avg                          Nullable(Float32),
    client_fps_n                            UInt32 DEFAULT 0,

    server_fps_min                          Nullable(Float32),
    server_fps_max                          Nullable(Float32),
    server_fps_avg                          Nullable(Float32),
    server_fps_n                            UInt32 DEFAULT 0,

    -- ───── throughput ─────
    throughput_bps_min                      Nullable(Float32),
    throughput_bps_max                      Nullable(Float32),
    throughput_bps_avg                      Nullable(Float32),
    throughput_bps_n                        UInt32 DEFAULT 0,

    bitrate_bps_min                         Nullable(Float32),
    bitrate_bps_max                         Nullable(Float32),
    bitrate_bps_avg                         Nullable(Float32),
    bitrate_bps_n                           UInt32 DEFAULT 0,

    video_packets_per_sec                   Float64,
    video_mbits_per_sec                     Float64,

    -- ───── totals (cumulative end-of-window) ─────
    video_packets_total                     UInt64,
    video_mbytes_total                      UInt64,

    -- ───── battery (last value, NULL until first Battery sample) ─────
    battery_hmd_pct                         Nullable(UInt8),
    battery_hmd_plugged                     Nullable(UInt8),

    -- ───── bitrate_directives (last value) ─────
    bd_scaled_calculated_throughput_bps     Nullable(Float32),
    bd_decoder_latency_limiter_bps          Nullable(Float32),
    bd_network_latency_limiter_bps          Nullable(Float32),
    bd_encoder_latency_limiter_bps          Nullable(Float32),
    bd_manual_max_throughput_bps            Nullable(Float32),
    bd_manual_min_throughput_bps            Nullable(Float32),
    bd_requested_bitrate_bps                Float32,

    -- ───── exporter health ─────
    failed_posts                            UInt64,

    -- ───── ingestion bookkeeping ─────
    ingested_at                             DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- Rename `device` to `host` on existing deployments (no-op once renamed).
ALTER TABLE alvr.streaming_metrics RENAME COLUMN IF EXISTS device TO host;


-- ─────────────────────────────────────────────────────────────────────
--                         HARDWARE METRICS
-- One row per snapshot per host for the singleton tables, one row per
-- (snapshot, dimension) for the per-N tables. Joined by (host, ts).
-- ─────────────────────────────────────────────────────────────────────

-- CPU aggregate (one row per snapshot).
CREATE TABLE IF NOT EXISTS alvr.hw_cpu
(
    ts                  DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                LowCardinality(String),
    total_pct           Nullable(Float32),
    freq_mhz            Nullable(UInt32),
    vrserver_pct        Nullable(Float32),
    package_temp_c      Nullable(Float32),
    package_power_w     Nullable(Float32),
    cores_power_w       Nullable(Float32),
    ingested_at         DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- CPU per-core (one row per (snapshot, core_index)).
CREATE TABLE IF NOT EXISTS alvr.hw_cpu_cores
(
    ts                  DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                LowCardinality(String),
    core_index          UInt16,
    load_pct            Nullable(Float32),
    temp_c              Nullable(Float32),
    power_w             Nullable(Float32),
    ingested_at         DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, core_index, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- GPU (one row per snapshot).
CREATE TABLE IF NOT EXISTS alvr.hw_gpu
(
    ts                  DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                LowCardinality(String),
    name                LowCardinality(String) DEFAULT '',
    util_pct            Nullable(Float32),
    encoder_util_pct    Nullable(Float32),
    decoder_util_pct    Nullable(Float32),
    mem_used_mb         Nullable(UInt32),
    mem_total_mb        Nullable(UInt32),
    temp_c              Nullable(Float32),
    power_w             Nullable(Float32),
    power_limit_w       Nullable(Float32),
    clock_graphics_mhz  Nullable(UInt32),
    clock_memory_mhz    Nullable(UInt32),
    clock_video_mhz     Nullable(UInt32),
    fan_pct             Nullable(Float32),
    ingested_at         DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- DRAM aggregate (one row per snapshot).
CREATE TABLE IF NOT EXISTS alvr.hw_dram
(
    ts                          DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                        LowCardinality(String),
    total_mb                    UInt64,
    used_mb                     UInt64,
    available_mb                UInt64,
    used_pct                    Float32,
    swap_total_mb               UInt64,
    swap_used_mb                UInt64,
    vrserver_working_set_mb     Nullable(UInt64),
    ingested_at                 DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- DIMMs (one row per (snapshot, slot)).
CREATE TABLE IF NOT EXISTS alvr.hw_dimms
(
    ts                  DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                LowCardinality(String),
    slot                LowCardinality(String),
    capacity_gb         Nullable(Float32),
    temp_c              Nullable(Float32),
    ingested_at         DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, slot, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- Storage / HDD / SSD (one row per (snapshot, device)).
CREATE TABLE IF NOT EXISTS alvr.hw_storage
(
    ts                  DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                LowCardinality(String),
    device              LowCardinality(String),
    temp_c              Nullable(Float32),
    used_pct            Nullable(Float32),
    life_left_pct       Nullable(Float32),
    total_gb            Nullable(Float32),
    free_gb             Nullable(Float32),
    ingested_at         DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, device, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;

-- Network (one row per (snapshot, adapter)).
CREATE TABLE IF NOT EXISTS alvr.hw_network
(
    ts                      DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    host                    LowCardinality(String),
    adapter                 LowCardinality(String),
    bytes_sent_per_sec      UInt64,
    bytes_recv_per_sec      UInt64,
    packets_sent_per_sec    UInt64,
    packets_recv_per_sec    UInt64,
    outbound_errors         UInt64,
    outbound_discarded      UInt64,
    current_bandwidth_bps   UInt64,
    ingested_at             DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
PARTITION BY toYYYYMM(ts)
ORDER BY (host, adapter, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;
