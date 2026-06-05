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

- **Slice 0 — bridge the cadence (additive, inert).** ✅ **LANDED + verified
  2026-06-03** (master `ec5f5c8e`). Added
  `alvr_oxr_duration_until_next_vsync(uint64_t *out_ns) -> bool` wrapping
  `ServerCoreContext::duration_until_next_vsync()`. Bridge ABI **v10 → v11**.
  Verified via a throttled probe: returns sane values cycling **0–13.9 ms** with
  a client connected. (Also fixed a latent xtask bug it surfaced — the
  `target/debug` vs `target/release` bridge import-lib mismatch, master
  `8c696861`.) comp_alvr's `U_LOG` lands in `%TEMP%\monado-service.stderr.log`.

- **Slice 1 — enforce the cadence on the comp_alvr producer (the core win).**
  ❌ **First approach (snap `u_pc_predict`'s outputs) ATTEMPTED + REVERTED — it
  REGRESSED pacing** (2026-06-03, measured: 72.9→**48.6 fps**, jitter
  2.35→**11.1 ms**, max 33→**151 ms**, drops 8→**47**/30s). Root cause: shifting
  `out_wake_time_ns` then letting Monado feed the *actual* wake back into the
  **same** `u_pc` via `mark_frame`→`u_pc_mark_point` makes u_pc see a constant
  ~3 ms misprediction every frame → its corrector oscillates. **Do not perturb
  u_pc's outputs.** The phase signal is fine (stable ~−2.92 ms offset); the
  application mechanism was wrong.
  **Corrected approach (Approach A) — ❌ BUILT + MEASURED-REGRESSED 2026-06-05,
  REVERTED.** Replaced `u_pc_predict`'s wake/display outputs with a cadence-locked
  computation (`display = now + duration_until_next_vsync`,
  `wake = display − 5 ms render_margin`, bumped a whole interval if wake went
  past), keeping `u_pc` only for `frame_id`; gate folded into the bridge getter.
  Built clean (CI green, comp_alvr + monado-service relinked). **Back-to-back A/B
  on RTX 3090 + Quest 3 (AK on Monado, H.264, 72 Hz, head moving, ~same session):**

  | | u_pc_fake (Slice 0) | Approach A |
  | --- | --- | --- |
  | jitter stddev | **2.33 ms** | **3.29 ms** (+41 %) |
  | max interval | 38 ms | **72 ms** (5-frame stall) |
  | drops >2× /30 s | ~9 | ~12 |
  | fps | 71.9 | 71.3 |

  The `u_pc_fake` arm reproduced the 2026-06-03 baseline (2.33 vs 2.35 ms), so the
  result is trustworthy. **Adjusting only the *prediction* is the wrong lever:**
  Monado's compositor loop, the app render loop, and squash scheduling all sit
  *between* `predict_frame` and the actual submit, so frames still bunch — and the
  per-frame `now`-sample + the `wake<now` whole-interval bump introduced a bimodal
  wake pattern that made the *spikes worse* (72 ms vs 33 ms). Reverted (unpushed
  local commits `5e2b8621c`/`481b499e` reset away; master back at `a6338048`).

  **Durable conclusion: BOTH Slice-1 variants (snap-outputs and replace-outputs)
  regressed.** The lever isn't the prediction — it's the *submit choke point*
  (Slice 2). In SteamVR mode `duration_until_next_vsync` helps precisely because
  `wait_for_vsync` sleeps the single submit point, not the prediction. So skip
  further Slice-1 prediction-editing (incl. Approach C re-anchoring) and go
  straight to Slice 2. The 5 ms `render_margin` value and the getter's
  `enforce_server_frame_pacing` gate are both reusable when Slice 2 is built.

- **Slice 2 — pace the submit, not just the prediction. ⭐ NOW THE PRIMARY PATH**
  (Slice 1 measured-regressed; the submit choke point is the real analogue of
  SteamVR's `wait_for_vsync`). Add an explicit sleep-to-cadence at the producer
  choke point so `compose_via_squasher`/`alvr_oxr_submit_layers` fire on an even
  beat regardless of when the app/compositor loop ran. Shape: just before the
  squash/submit, read `alvr_oxr_duration_until_next_vsync`; sleep
  `until_next_vsync − pipeline_cost` (squash + FFR + encode ≈ 5 ms measured) so
  the frame lands just before the target vsync. **Leave `u_pc_predict` untouched**
  (the lesson from Slice 1) — pace only the submit. Gate on
  `enforce_server_frame_pacing`; fall back to no-sleep when the getter returns
  false. Watch for: the sleep must not stall Monado's compositor thread in a way
  that starves the app (measure fps doesn't drop like Approach A's spikes did).
  **A/B vs the 2.33 ms `u_pc_fake` arm, same harness** (`cap_jitter.cmd` on the
  TESTHOST console + the `[GRAPH]`-interval analyzer; see `[[openxr-pacing-smoothness]]`).

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
