# Per-view foveation — scoping (2026-05-22)

Status: design only. Sister doc to [`HAND_TRACKING_PASSTHROUGH.md`](HAND_TRACKING_PASSTHROUGH.md). Both pin down concrete next slices for Phase 7 stretch work while the Monado fork-hosting blocker stands.

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
| Client reprojection | Symmetric undo | `alvr_graphics::stream` computes `foveation_scale` from the same uniform `center_size`/`edge_ratio` and reprojects both eye halves with the same shader. |

So the foveation *params* travel only at stream-init in `OpenvrConfig` (effectively part of `StreamConfig`). The encoder and the client decoder both consult the same numbers, which is what makes the warp invertible. Anything that breaks that single-source-of-truth invariant — including per-frame variation — needs to ship the params alongside each frame.

## Wire-compat — the central decision

Per-frame, per-view params force a new field. Three options ordered by cost:

1. **Sidecar real-time path (no `alvr_packets` break).** `RealTimeConfig` already exists for low-rate config updates from server to client. Add an optional `per_view_foveation: Option<[FoveationView; 2]>` and ship one update per *N* frames (where *N* is small — e.g. every frame or every 4 frames). Bincode-additive at the end of `RealTimeConfig` — old clients ignore unknown trailing data, new clients keep the last value until the next update. Risk: `RealTimeConfig` is sent over the *control* socket (TCP-ish) at low cadence; pushing it to per-frame would change its packet rate characteristics. Reasonable for ~4–10 Hz updates that mirror real saccades; not for per-frame.

2. **Per-frame, in `VideoPacketHeader` (clean, breaks the wire).** Add `per_view_foveation: Option<[FoveationView; 2]>` directly to `VideoPacketHeader`. Pros: lockstep with the frame the params apply to; no client-side interpolation/buffer-by-one issues. Cons: bincode wire break — client and server must rebuild together (this is the same boundary `alvr_packets` has always been: lockstep version bumps acceptable when both halves ship together, but a coordination cost).

3. **Two-tier: static defaults in `OpenvrConfig`, per-frame deltas as a separate UDP stream.** Add a `FOVEATION_PARAMS = 5` stream ID alongside `TRACKING/HAPTICS/AUDIO/VIDEO/STATISTICS`. Drop-tolerant (last-known good wins). Avoids touching `VideoPacketHeader`. Complexity: a fourth stream socket payload and the ordering question vs the video frame it's meant to pair with.

**Recommended**: (1) for the v0 slice (eye-tracking → foveation centre at ~10 Hz, fixed inset size). Move to (2) once the v0 is shipping and we know whether sub-frame update is actually worth the wire break. (3) is overengineering for a single new param set.

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

Bump `ALVR_OXR_BRIDGE_ABI_VERSION` 3 → 5 in one go (4 is reserved for hand-tracking passthrough; both v4 and v5 may land in either order — but they're independent bumps either way, never a hybrid v4-with-foveation). Update `History:` block and `ALVR_OXR_BRIDGE_ABI_EXPECTED` on the Monado side.

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

- [ ] `alvr_packets::TrackingData` unchanged on the wire (confirmed by bincode round-trip fixture).
- [ ] `alvr_packets::RealTimeConfig` extended additively with `per_view_foveation: Option<[FoveationView; 2]>` at the *end* of the struct (bincode 2 standard config — trailing optional is forward-compatible only inside an additive bump on both halves).
- [ ] `alvr_session::FoveatedEncodingConfig` additively gains the new `Switch<PerViewFoveationConfig>` field. No migration entry.
- [ ] `ALVR_OXR_BRIDGE_ABI_VERSION` bumped 3 → 5 with both halves of the wire (Rust const + Monado-side `_EXPECTED`).
- [ ] cbindgen header regenerated via `ALVR_REGENERATE_BRIDGE_HEADER=1 cargo build -p alvr_server_openxr`.
- [ ] `cargo xtask clippy --ci` clean.
- [ ] Monado CTest clean (25/25 on host 101 — see [[reference-remote-test-host]]).

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
