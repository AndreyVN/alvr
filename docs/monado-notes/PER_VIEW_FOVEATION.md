# Per-view foveation — scoping (2026-05-22)

Status: **Slices 1–5 + producer wiring + real per-view FOV plumb + per-frame wire + client reprojection consumer LANDED** (1/2/3/5 + producer + FOV on 2026-05-22; **Slice 4 wire bump, per-frame `VideoPacketHeader` wire, and the client de-foveation consumer all 2026-05-27**) — eye-tracking → bridge cache is end-to-end on the alvr side; the params travel server→client both as a low-rate `RealTimeConfig` baseline and per-frame in `VideoPacketHeader`; and the client renderer now has a per-view de-foveation pipeline that consumes them. The **only remaining gap is Slice 6** (the OpenXR-mode encoder body in `alvr_oxr_submit_layers` that actually *produces* a per-view-foveated image, NVENC-hardware-blocked). Until it lands, the client consumer is dormant: no frame carries per-view foveation, so the renderer stays on its static path. See [`NEXT_STEPS.md`](NEXT_STEPS.md) per-view foveation bullet for commits.

## What "per-view foveation" means here

Make the encoder's foveation centre and edge-fall-off independent per eye, and let those parameters move per frame (driven by eye tracking) instead of being fixed at session start. Then expose the per-view params to the Monado-side encoder through a new bridge entry so the OpenXR-mode encoder produces a foveated image that the client's reprojection can correctly invert per eye.

**Why per-view, not just per-frame uniform?** Eye-tracked foveation only pays for itself when the high-resolution inset can follow each gaze independently. Today's FFR is symmetric: the inset sits at the same image-space coordinate in both eye halves, so even if you knew the gaze direction you couldn't aim a single shared inset at both fovea simultaneously when the eyes diverge (convergence on near objects, vergence shift, etc.).

Not in scope:
- The eye-tracking *capture* path. `FaceData.eyes_combined: Option<Quat>` and `eyes_social: [Option<Quat>; 2]` already arrive in `TrackingData`. No new sensor work.
- Client-side foveation (`ClientsideFoveationConfig`) — that's the Quest VRAPI display-time foveation override, runs on the headset, unrelated to the streamer's encoder FFR.
- Per-view foveation on the SteamVR mode encoder. The whole point of doing this in OpenXR mode first is that we can iterate without re-touching the OVR driver. A clean per-view path in `comp_alvr` + the bridge becomes the reference for a later SteamVR-mode port.

## Today's state

| Layer | Status | Where |
| --- | --- | --- |
| Session schema | Uniform per-stream | `alvr_session::settings::FoveatedEncodingConfig { center_size_{x,y}, center_shift_{x,y}, edge_ratio_{x,y} }`. All `steamvr-restart` — static within a session. |
| Wire — capability | ✅ boolean | `alvr_packets::VideoStreamingCapabilities.foveated_encoding: bool` + `NegotiatedStreamingConfig.enable_foveated_encoding: bool`. |
| Wire — params | ✅ static, uniform | `alvr_session::OpenvrConfig.foveation_*: f32` — six scalars, single set, shipped at stream init. |
| Wire — per-frame | ❌ | `VideoPacketHeader { timestamp, global_view_params, is_idr }` has no foveation slot. Foveation does not animate today. |
| Eye-tracking wire | ✅ unused for foveation | `FaceData.eyes_combined`, `eyes_social[2]` already in `TrackingData`. Used today for social presence and *optionally* for eye gaze rendering — not piped to the encoder. |
| SteamVR-mode encoder | Single FFR shader | `alvr/server_openvr/cpp/platform/win32/FFR.cpp` `CalculateFoveationVars()` reads from a singleton `Settings::Instance()`. Output is one `FoveationVars` struct applied identically to both eye halves of the rendered image. |
| OpenXR-mode encoder | Stubbed | `alvr_oxr_submit_layers` (bridge ABI v3, NOT implemented). Whatever encoder body lands here is the place to consume per-view foveation. |
| Client reprojection | Symmetric undo + dormant per-view path | Static path unchanged (one pipeline, baked spec-constants, both eyes identical). A second `FFE_RUNTIME` pipeline in `alvr_graphics::stream` (added 2026-05-27) derives the de-foveation constants at runtime from a per-view `center_shift` uniform, used only when `render` is given per-view shifts (from `VideoPacketHeader.per_view_foveation`). Dormant until Slice 6 produces per-view frames. |

So the foveation *params* travel only at stream-init in `OpenvrConfig` (effectively part of `StreamConfig`). The encoder and the client decoder both consult the same numbers, which is what makes the warp invertible. Anything that breaks that single-source-of-truth invariant — including per-frame variation — needs to ship the params alongside each frame.

## Wire-compat — the central decision

Per-frame, per-view params force a new field. Three options ordered by cost:

1. **Sidecar real-time path (no `alvr_packets` break).** `RealTimeConfig` already exists for low-rate config updates from server to client. Add an optional `per_view_foveation: Option<[FoveationView; 2]>` and ship one update per *N* frames (where *N* is small — e.g. every frame or every 4 frames). Bincode-additive at the end of `RealTimeConfig` — old clients ignore unknown trailing data, new clients keep the last value until the next update. Risk: `RealTimeConfig` is sent over the *control* socket (TCP-ish) at low cadence; pushing it to per-frame would change its packet rate characteristics. Reasonable for ~4–10 Hz updates that mirror real saccades; not for per-frame.

2. **Per-frame, in `VideoPacketHeader` (clean, breaks the wire).** ✅ **LANDED 2026-05-27.** Added `per_view_foveation: Option<[FoveationView; 2]>` directly to `VideoPacketHeader`. Server side: `ConnectionContext` caches the latest emitter output in `latest_per_view_foveation: RwLock<Option<[PerViewFoveationView; 2]>>` (mirrors the `local_view_params` pattern), `tracking_loop` writes it alongside the `ServerCoreEvent::PerViewFoveation` emit, and `send_video_nal` stamps it into each header via `From<PerViewFoveationView> for FoveationView`. `Some` only when `per_view_eye_tracked` is enabled (dynamic per-frame centre), else `None`. Pros: lockstep with the frame the params apply to; no client-side interpolation/buffer-by-one issues. Cons: bincode wire break — `protocol_id` only gates the major version (`"21-dev13"` today), so client and server must be built from the same revision (the existing ALVR model — same boundary `RealTimeConfig` already is). **Client-side per-view reprojection consumer LANDED 2026-05-27** (dormant): `alvr_graphics::stream` gained a second `FFE_RUNTIME` pipeline that derives the de-foveation constants from a per-view `center_shift` uniform (the push-constant block was already at the 128-byte limit), and the wire value is plumbed `VideoPacketHeader → client_core per_view_foveation_queue → report_compositor_start → StreamRenderer::render`. The static path is byte-identical when no per-view data arrives (today's only state). **Verification ceiling**: no GPU on the dev host and no producer yet, so the runtime path is exercised only by a naga parse+validate test on `stream.wgsl`; true end-to-end correctness (encoder warp ⇄ client inverse) waits on Slice 6.

3. **Two-tier: static defaults in `OpenvrConfig`, per-frame deltas as a separate UDP stream.** Add a `FOVEATION_PARAMS = 5` stream ID alongside `TRACKING/HAPTICS/AUDIO/VIDEO/STATISTICS`. Drop-tolerant (last-known good wins). Avoids touching `VideoPacketHeader`. Complexity: a fourth stream socket payload and the ordering question vs the video frame it's meant to pair with.

**Recommended**: (1) for the v0 slice (eye-tracking → foveation centre at ~10 Hz, fixed inset size). Move to (2) once the v0 is shipping and we know whether sub-frame update is actually worth the wire break. (3) is overengineering for a single new param set. **Both (1) and (2) have now landed** (1 on 2026-05-27, 2 on 2026-05-27) — the producer + transport are in place on both the low-rate baseline channel and the per-frame channel; the remaining gap is the client-side per-view reprojection consumer.

Picking (1) means **zero `alvr_packets` break for the basic case**. Mirrors the hand-tracking-passthrough win.

## Bridge ABI v5 — proposed entries

Two entry points. One pose-shaped getter for the encoder to consume (mirrors `alvr_oxr_get_view_params`), one setter for the bridge's drain thread to update from the upcoming `RealTimeConfig` field:

```c
/**
 * Per-view foveation parameters. Lengths in image-space [0, 1]; centre at
 * (0.5, 0.5) when shifts are zero. `edge_ratio_{x,y}` are the per-axis
 * fall-off factors (matches alvr_session::FoveatedEncodingConfig). Set
 * `is_present = false` to mean "no foveation for this view this frame —
 * encode the whole half at full resolution" (useful when an eye loses
 * tracking confidence).
 */
typedef struct AlvrOxrFoveationView {
  bool is_present;
  float center_size[2];
  float center_shift[2];
  float edge_ratio[2];
} AlvrOxrFoveationView;

/**
 * Read the latest cached per-view foveation params. Drives the encoder side
 * inside `alvr_oxr_submit_layers`. Returns Ok with `is_present = false` for
 * both views when foveation is disabled session-wide; the encoder should
 * encode at full resolution in that case.
 *
 * # Safety
 * `out_views` must be a writable buffer of 2 `AlvrOxrFoveationView`s
 * (left = index 0, right = index 1).
 */
AlvrOxrResult alvr_oxr_get_foveation(struct AlvrOxrFoveationView *out_views);

/**
 * Push a new per-view foveation update into the bridge. Called by the
 * server_core drain thread when a new `ServerCoreEvent::PerViewFoveation`
 * arrives (which itself is fed by `RealTimeConfig.per_view_foveation`).
 *
 * # Safety
 * `views` must point to 2 `AlvrOxrFoveationView`s. Caller retains ownership;
 * the bridge copies into its internal cache.
 */
AlvrOxrResult alvr_oxr_set_foveation(const struct AlvrOxrFoveationView *views);
```

Implementation notes for `alvr_server_openxr/src/lib.rs`:
- Add a `FOVEATION: RwLock<[FoveationView; 2]>` next to the existing `LOCAL_VIEW_PARAMS` cache. Default both views to `is_present = false` so the encoder defaults to full-res before any update lands.
- `alvr_oxr_set_foveation` takes the write lock and replaces the cache.
- `alvr_oxr_get_foveation` takes the read lock and copies. No allocations on the hot path.
- The drain thread learns about new foveation via a new `ServerCoreEvent::PerViewFoveation` (sibling of the existing `LocalViewParams` event).

Bump `ALVR_OXR_BRIDGE_ABI_VERSION` 3 → 5 in one go (v4 landed 2026-05-22 for hand-tracking passthrough Slice 1; this slice bumps 4 → 5). Update the `History:` block in the alvr-side header. There is no separate `_EXPECTED` constant to bump on the Monado side — the openxr submodule includes the alvr-side `alvr_runtime_bridge.h` directly via `target_include_directories`, so the macro is single-source-of-truth (see [`HAND_TRACKING_PASSTHROUGH.md`](HAND_TRACKING_PASSTHROUGH.md) for the correction).

## Session-schema additions

Today's `FoveatedEncodingConfig` is uniform and `steamvr-restart` flagged. Two changes:

1. **Add a `per_view_eye_tracked: Switch<PerViewFoveationConfig>` field** (additive, defaults `Disabled`). The `PerViewFoveationConfig` carries the rate-limit for updates (e.g. `update_rate_hz: f32`, default 10.0), the maximum offset each view's centre may track from straight-ahead (e.g. `max_offset_normalized: f32`, default 0.25 — i.e. ±25% of the view), and a confidence floor below which we fall back to the static centre.

2. **Re-flag `center_size_{x,y}` and `edge_ratio_{x,y}` as `real-time`** when `per_view_eye_tracked.enabled = true`. The current `steamvr-restart` flag exists because the SteamVR FFR pipeline allocates fixed-size buffers; in OpenXR mode the encoder allocates its own (per `alvr_oxr_submit_layers` body when it lands), so it's free to honour real-time tweaks. Conditional flagging isn't supported by the schema today; punt on this if the schema gymnastics are non-trivial, accept restart for now.

A migration in `alvr_session` is not needed — both items are additive. Add an `alvr_session` test that round-trips a session with the new field defaulted and confirms the bincode wire stays length-additive.

## Server-core changes

- New `ServerCoreEvent::PerViewFoveation([FoveationView; 2])`. Fed by either:
  - A new `connection.rs` worker that computes foveation centres from incoming `TrackingData.face.eyes_*` and the static config (eye-tracking driven), OR
  - A direct passthrough from a future `RealTimeConfig.per_view_foveation` (option 1 in the wire-compat section). Both shapes converge here.

- The OpenVR-mode driver ignores this event (no per-view support there). The OpenXR-mode `alvr_server_openxr` drain thread translates it into `alvr_oxr_set_foveation`. This is the architectural reason the bridge has both a getter and a setter — the producer and consumer live in different processes (well, different threads, but the lifetime model is the same).

## Out of scope (named so the follow-up can pick them up)

- The actual eye-tracking → foveation-centre math. Will need a small filter (e.g. one-Euro low-pass on the gaze yaw/pitch, dead-band ±2° to avoid micro-jitter), built on top of the existing `FaceData.eyes_*` samples.
- SteamVR-mode parity. `alvr_server_openvr` would need its FFR.cpp restructured to take per-eye params; doable but bigger than this scope.
- Variable Rate Shading (VRS) on the *encoder*'s NVENC input. The current FFR is image-domain; per-view could also drive NVENC's VRS extensions (12.1+). Separate decision.
- Foveation-aware bitrate allocation. Today the dynamic bitrate adapter doesn't know which parts of the image got fewer pixels — it could.

## Wire-compat checklist (for the actual landing slice)

- [x] `alvr_packets::TrackingData` unchanged on the wire (Slice 4 touches only `RealTimeConfig`).
- [x] `alvr_packets::RealTimeConfig` extended additively with `per_view_foveation: Option<[FoveationView; 2]>` at the *end* of the struct (Slice 4, 2026-05-27). Note: the struct's own doc-comment states `RealTimeConfig` is sent without cross-version compatibility, so client+server ship together regardless — the additive placement keeps the encoding self-consistent rather than backward-compatible. `bincode` round-trip test in `alvr_packets` pins both the `Some` and `None` shapes.
- [x] `alvr_session::FoveatedEncodingConfig` additively gains the new `Switch<PerViewFoveationConfig>` field. No migration entry. (Landed with Slice 2, 2026-05-22.)
- [x] `ALVR_OXR_BRIDGE_ABI_VERSION` bumped 4 → 5 (Slice 1, 2026-05-22 — single-source-of-truth via the alvr-side cbindgen header). Slice 4 does **not** touch the bridge ABI: it's purely the server↔client wire surface.
- [x] cbindgen header regenerated (with Slice 1). Slice 4 needs no regeneration.
- [x] `cargo clippy -p alvr_packets --all-targets` clean (Slice 4 scope).
- [ ] Monado CTest clean (25/25 on host 101 — see [[reference-remote-test-host]]). Unaffected by Slice 4 (no Monado-side change); re-run only gates the bridge/compositor slices.

## Verification ceiling

Gate A (Monado-side compile) and Gate B (boot) are unaffected — the new bridge entries are just additional symbols. Gate C (real client driving foveation through the runtime) is hardware-gated alongside the rest. The math correctness (encoder-decoder round-trip on per-view params) can be unit-tested in Rust without hardware by holding the FFR forward+inverse passes in `alvr_graphics::stream` accountable to a property: "any params the encoder uses must reproduce the original image up to per-pixel error E when the decoder applies the inverse". That test doesn't exist today and would be a useful add as part of the landing slice.

## Recommended landing order (when the hosting blocker clears)

1. **Bridge ABI v5 stubs** in `alvr_server_openxr` — getter returns `is_present = false` for both views; setter writes the cache. Regenerate header. Land.
2. **`alvr_session` additive field** (`per_view_eye_tracked: Switch<PerViewFoveationConfig>`) — disabled by default. Schema test for the bincode round-trip. Land.
3. **`server_core` glue** — `ServerCoreEvent::PerViewFoveation` + drain-thread wiring to the bridge setter. Land.
4. **`RealTimeConfig.per_view_foveation`** — additive optional field, gated on the session switch. Land alongside a coordinated client+server bump (this is the only step that touches the cross-version wire surface).
5. **Eye-tracking-to-centre math** (Rust-side filter on `FaceData.eyes_*`). Pure-Rust unit-testable.
6. **Encoder body of `alvr_oxr_submit_layers`** consumes `alvr_oxr_get_foveation`. This is Phase 3.3 (NVENC body) territory and is hardware-blocked regardless of foveation — but once it lands, foveation is the immediately-next thing to wire in.
7. **Optional**: SteamVR-mode parity in `alvr_server_openvr` FFR.

Steps 1–3 + 5 are non-blocked even today (no submodule touch); step 4 is the wire-compat coordination point; step 6 is the hardware-gated piece. So the "iterate without the hosting blocker" cut is 1, 2, 3, 5 — most of the design.
