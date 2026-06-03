# OpenXR-mode phase-sync pacer — scope

Plan to close the smoothness/latency gap between OpenXR mode and SteamVR mode
(measured 2026-06-03; see `[[openxr-pacing-smoothness]]` memory and the
known-limitation bullet in `NEXT_STEPS.md`). Companion to the code; when this
doc and the code disagree, the code wins.

## Problem (measured)

OpenXR mode runs at the right throughput (72.9 fps @ 72 Hz, encode 4.4 ms
off-thread, decoder keeps up) but the **frame interval jitters**: stddev
2.35 ms, min 0 / max 33 ms (bursting), ~8 likely-drops per 30 s — a hitch every
~3–4 s. SteamVR mode at the same 72 Hz is smoother. Absolute motion-to-photon
latency was not captured (dashboard/ClickHouse only) but the extra Monado
compositor hop (squash + FFR + residual `vkQueueWaitIdle`) is the structural
"laggy" contributor.

## Root cause

`comp_alvr_predict_frame` (comp_alvr.c:837) paces with Monado's generic
`u_pc_fake` helper (`compositor_init_pacing`, comp_alvr.c:736): a free-running
fixed-interval predictor anchored at compositor start. It has no relationship to
the client's display cadence, so the server's 72 Hz production clock and the
client's 72 Hz display clock drift → periodic early/late arrivals → judder.

SteamVR mode avoids this: `wait_for_vsync()` (server_openvr/lib.rs:444) sleeps
the producer to `ServerCoreContext::duration_until_next_vsync()` when
`settings().video.enforce_server_frame_pacing` is set, enforcing an even cadence
on the whole pipeline.

## Key enabler (already in place)

- `server_core` already computes `duration_until_next_vsync()` (lib.rs:569 →
  statistics.rs:507) from the statistics manager's frame-interval cadence.
- That statistics manager is fed by the **shared** connection pipeline, which is
  active in OpenXR mode too (this is why `[GRAPH]`/`[STATS]` flow on Monado).
- So the cadence signal exists in OpenXR mode today; nothing consumes it. The
  work is to plumb it into `comp_alvr` and enforce it, mirroring SteamVR mode.

Clock-domain note: pass a **relative** duration ("time until next vsync"), never
an absolute timestamp — `server_core` (Rust `Instant`) and `comp_alvr`
(`os_monotonic_get_ns`) share the OS monotonic clock on the same PC, but passing
a relative value sidesteps any drift/epoch mismatch. (Client↔server clocks are
NOT synced — never use client absolute time here; the cadence is server-derived.)

## Slices

Each slice is independently committable; the early ones are behaviour-preserving
or gated so master stays shippable. Re-run the 30 s jitter measurement (see
`[[openxr-pacing-smoothness]]` method) as the gate between slices.

- **Slice 0 — bridge the cadence (additive, inert).** Add
  `alvr_oxr_duration_until_next_vsync(uint64_t *out_ns) -> bool` to the bridge
  (server_openxr/src/lib.rs), wrapping `ServerCoreContext::duration_until_next_vsync()`
  exactly like `alvr_duration_until_next_vsync` in server_core/c_api.rs:496.
  Bridge ABI bump → **v11** (additive getter). Nothing consumes it yet → no
  behaviour change. Verify it returns sane values (~0–13.9 ms) in a session.

- **Slice 1 — enforce the cadence on the comp_alvr producer (the core win).**
  In `comp_alvr_predict_frame`, after `u_pc_predict`, override the wake/present
  phase using the bridged duration: align `out_wake_time_ns` /
  `out_predicted_display_time_ns` to `now + duration_until_next_vsync (+ period)`
  so the app is woken and targets the client's cadence rather than the
  free-running one. Keep `u_pc_fake` for `frame_id`/period bookkeeping. Gate on
  the existing `video.enforce_server_frame_pacing` setting (off → current
  behaviour, so it's opt-in and reversible). **Measure jitter; expect the
  bursting (min 0 / max 33) to shrink.**

- **Slice 2 — pace the submit, not just the prediction.** If Slice 1 isn't
  enough (the app render loop and squash scheduling can still bunch up), add an
  explicit sleep-to-cadence at the producer choke point — the natural analogue of
  SteamVR's `wait_for_vsync` — so `compose_via_squasher`/submit fire on an even
  beat. Account for the pipeline cost (squash + FFR + encode ≈ 5 ms measured) so
  the frame still lands before the target vsync. Measure.

- **Slice 3 — close the loop + latency budget.** Drive the target display time
  from the measured motion-to-photon / pipeline latency so the frame arrives
  *just* in time (minimize buffering). Capture **absolute latency**
  (dashboard/ClickHouse, `metrics/`) before/after to quantify the "laggy" half.
  Consider lowering `max_buffering_frames` (currently 2.0) once pacing is tight.
  **Gate: OpenXR-mode jitter + latency approach SteamVR-mode levels.**

- **Slice 4 (optional) — robustness + telemetry.** Fall back to `u_pc_fake` when
  no client cadence yet (pre-connection) or on dropout; extend `oxr_pacing`
  telemetry with a phase-error metric so regressions are visible.

## Open questions / risks

- **Open- vs closed-loop:** `duration_until_next_vsync` is a server-side
  *synthetic* steady cadence (free-running at `frame_interval`, anchored at
  connect — statistics.rs:511), not corrected from the client's actual vsync. It
  may be enough to enforce it evenly (Slices 1–2). If residual drift/judder
  remains, Slice 3's closed-loop correction from client frame-timing feedback is
  the deeper fix — decide based on the Slice 1–2 measurements.
- The extra Monado compositor hop's baseline latency is structural; pacing tightens
  jitter and lets buffering drop, but won't remove the hop itself.
- Verification is headset-gated (RTX 3090 + Quest 3); the `[GRAPH]`-timestamp
  jitter method is the smoothness gate, dashboard/ClickHouse the latency gate.

## ABI impact

Slice 0 adds one bridge getter → ABI **v11** (additive; Monado refuses to load
on mismatch, so bump `ALVR_OXR_BRIDGE_ABI_VERSION` + the Monado-side check
together). Slices 1–4 are comp_alvr-internal (no further ABI change).
