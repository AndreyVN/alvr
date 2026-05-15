-- ClickHouse schema for ALVR streaming metrics.
--
-- Stores one row per snapshot pushed by the server-side metrics exporter
-- (alvr/server_core/src/metrics_exporter.rs). The exporter POSTs a JSON
-- document every `extra.metrics_export.interval_ms`; this schema mirrors
-- every numeric field in that document so a single INSERT per snapshot
-- captures the full payload without re-shaping at ingest time.
--
-- Usage:
--   clickhouse-client --multiquery < metrics/clickhouse_schema.sql
--
-- Snapshot shape (see metrics_exporter::Aggregator::flush):
--   {
--     "ts": "<RFC3339 ms>", "tags": {...}, "window_ms": u64,
--     "frames": u32, "dropped_samples": u64,
--     "latency_ms": { "<stage>": {"min","max","avg","n"} ... },
--     "fps":        { "client"|"server": {"min","max","avg","n"} | null },
--     "throughput": { "throughput_bps"|"bitrate_bps": {min/max/avg/n} | null,
--                     "video_packets_per_sec": f64, "video_mbits_per_sec": f64 },
--     "totals":     { "video_packets": u64, "video_mbytes": u64 },
--     "battery":    { "hmd_pct": u32, "hmd_plugged": bool } | null,
--     "bitrate_directives": {...},
--     "exporter":   { "failed_posts": u64 }
--   }

CREATE DATABASE IF NOT EXISTS alvr;

CREATE TABLE IF NOT EXISTS alvr.streaming_metrics
(
    -- ───── identity ─────
    ts                                      DateTime64(3, 'UTC') CODEC(DoubleDelta, ZSTD(1)),
    device                                  LowCardinality(String),
    window_ms                               UInt64,
    frames                                  UInt32,
    dropped_samples                         UInt64,

    -- ───── latency_ms.<stage> (min / max / avg / n) ─────
    -- A NULL min/max/avg means no frame sample arrived for this stage in the window.
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
    battery_hmd_plugged                     Nullable(UInt8),  -- 0/1; ClickHouse has no Bool

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
ORDER BY (device, ts)
TTL toDateTime(ts) + INTERVAL 90 DAY
SETTINGS index_granularity = 8192;
