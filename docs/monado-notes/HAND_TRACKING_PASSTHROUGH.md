# Hand-tracking passthrough — scoping (2026-05-22)

Status: **All three slices LANDED 2026-05-22.** Slice 1 + 3 (alvr-side ABI v4 + `ServerCoreContext::get_hand_skeleton` wire-up) shipped first; Slice 2 (Monado-side `xrt_device::get_hand_tracking` consuming the bridge call) shipped same day after confirming the fork can be iterated locally without the push gate. End-to-end behavioural verification (an OpenXR app on the PC actually receiving the joint set via `xrLocateHandJointsEXT`) is hardware-gated alongside Gates C–G in [`NEXT_STEPS.md`](NEXT_STEPS.md). The recommended landing-order section at the bottom is preserved as a reference, but every step in it is now done.

## What "passthrough" means here

Pipe the client's existing 26-joint hand skeletons through `alvr_server_openxr` into Monado so PC-side OpenXR applications can read them via `XR_EXT_hand_tracking`. This is the OpenXR-mode analogue of what `alvr_server_openvr` already does for SteamVR Input 2.0.

Not in scope:
- Enriching the streaming wire shape (per-joint `radius` / `locationFlags`). Today's `Pose; 26` is enough to satisfy `XR_EXT_hand_tracking`'s required fields; a follow-up slice can add the extras once we have a real consumer asking for them.
- Forwarding gesture-driven controller emulation (`HandTrackingInteractionConfig`). Lives in `alvr_session` already; orthogonal.
- The camera-passthrough (`PassthroughMode`) plumbing. Despite the name collision, that's video see-through — unrelated.

## Today's state

| Layer | Status | Where |
| --- | --- | --- |
| Client capture | ✅ | `alvr_client_openxr/src/stream.rs:648` populates `TrackingData.hand_skeletons` from `xrLocateHandJointsEXT`. |
| Wire | ✅ | `alvr_packets::TrackingData.hand_skeletons: [Option<[Pose; 26]>; 2]` (bincode 2, `config::standard()`). |
| SteamVR-mode consumer | ✅ | `alvr_server_openvr::tracking::to_openvr_ffi_hand_skeleton` + `HandSkeletonConfig.steamvr_input_2_0` gate. |
| `alvr_server_core` API | ✅ | `ServerCoreContext::get_hand_skeleton(HandType, poll_timestamp) -> Option<[Pose; 26]>`. |
| Bridge ABI surface | ❌ | No hand-skeleton entry in `alvr/server_openxr/include/alvr_runtime_bridge.h`. Only head + 2 controllers + view params + haptics + session events + pacing telemetry. |
| Monado-side driver | ❌ | `openxr/src/xrt/drivers/alvr/` registers HMD + 2 controllers as `xrt_device`s. None of them implement `xrt_device::get_hand_tracking`. The state tracker's hand-tracking extension is gated on at least one device exposing `XRT_DEVICE_TYPE_HAND_TRACKER`. |

The wire and the server-core API already have everything we need. The whole gap is on the OpenXR-mode side: bridge + Monado driver.

## Wire-compat — the alvr_packets piece

`TrackingData.hand_skeletons` is sufficient. Zero break.

Specifically:
- bincode 2 with `config::standard()` is *positional* and *length-prefixed* for `Option`s and arrays. Adding new fields at the end of `TrackingData` would be safe on a coordinated bump but the basic feature does not need any. Reuse the existing field.
- `XR_EXT_hand_tracking` mandates the joint pose; `radius` and `locationFlags` have spec-defined behaviour when the runtime supplies a default. Monado-side driver fills:
  - `xrt_hand_joint_value.radius` from a static per-joint table mirroring upstream defaults (~0.01 m at the palm, ~0.008 m at finger tips). Matches what other Monado drivers do when the upstream tracker doesn't expose radii.
  - `xrt_space_relation.relation_flags` = `XRT_SPACE_RELATION_POSITION_TRACKED_BIT | XRT_SPACE_RELATION_ORIENTATION_TRACKED_BIT | _VALID_BIT` for every joint when `TrackingData.hand_skeletons[side].is_some()`, all-zero when None. No velocity for now (`*_LINEAR_VELOCITY_VALID_BIT` cleared).

A later slice can enrich the wire if a consumer complains; this one stays additive-only.

## Bridge ABI v4 — proposed entry

Add a single pose-fetch entry mirroring `alvr_oxr_get_controller_state`. Read-side, polled by Monado once per frame for the active session.

```c
/**
 * Per-joint pose payload returned by alvr_oxr_get_hand_skeleton. Joints are
 * indexed by the XR_HAND_JOINT_*_EXT order, which matches Monado's
 * xrt_hand_joint enum 1:1 (XRT_HAND_JOINT_PALM .. XRT_HAND_JOINT_LITTLE_TIP).
 */
typedef struct AlvrOxrHandJoint {
  struct AlvrOxrPose pose;
} AlvrOxrHandJoint;

#define ALVR_OXR_HAND_JOINT_COUNT 26

/**
 * Query the predicted hand skeleton at `at_timestamp_ns`. Writes
 * `ALVR_OXR_HAND_JOINT_COUNT` joints into `out_joints` when the client has a
 * tracked hand on this side. Sets `*out_is_tracked = false` and leaves
 * `out_joints` untouched when the client reports `hand_skeletons[side] = None`
 * for the resolved frame.
 *
 * Result codes: Ok (data filled or is_tracked=false), NotInitialised, Failed
 * (out_* null, side out of range).
 *
 * # Safety
 * `out_joints` must be a writable buffer of at least
 * `ALVR_OXR_HAND_JOINT_COUNT` elements. `out_is_tracked` must be writable.
 */
AlvrOxrResult alvr_oxr_get_hand_skeleton(AlvrOxrSide side,
                                         int64_t at_timestamp_ns,
                                         struct AlvrOxrHandJoint *out_joints,
                                         bool *out_is_tracked);
```

Implementation notes for `alvr_server_openxr/src/lib.rs`:
- Calls `ServerCoreContext::get_hand_skeleton(side, target)` with `target` resolved from `at_timestamp_ns` the same way `alvr_oxr_get_head_pose` resolves head pose.
- Maps `[Pose; 26]` -> `AlvrOxrPose; 26` 1:1 (same coordinate convention; both OpenXR-spec).
- `is_tracked=false` is the cheap path: no copy, no allocation. Frames where the client isn't running hand tracking should pay nothing extra.

Bump `ALVR_OXR_BRIDGE_ABI_VERSION` 3 -> 4. Update the `History:` block in the header. **There is no separate `ALVR_OXR_BRIDGE_ABI_EXPECTED` constant on the Monado side** — `openxr/src/xrt/drivers/alvr/CMakeLists.txt` `target_include_directories` PRIVATE-includes the alvr-side `alvr_runtime_bridge.h` directly, so the macro is single-source-of-truth and the runtime mismatch check at `alvr_hub.c:189` compares the loaded cdylib's version against the same compile-time macro. (Earlier drafts of this doc and the per-view foveation scoping doc both said there was a separate `_EXPECTED` to bump; that was wrong.) See [[openxr-mode-integration]] for the bump protocol.

## Session-schema additions

`HandSkeletonConfig` already exists; today only `steamvr_input_2_0` and `predict`. The SteamVR Input 2.0 flag is mode-specific.

Two viable shapes:

1. **Mode-blind flag** — repurpose the existing `controllers.hand_skeleton: Switch<HandSkeletonConfig>` so it gates *both* modes. SteamVR mode keeps reading `steamvr_input_2_0`; OpenXR mode just checks the outer `Switch` for "enabled". Cleanest if the dashboard tooltip is updated to mention OpenXR mode.

2. **Per-mode flag** — extend `HandSkeletonConfig` with a new boolean (e.g. `openxr_advertise: bool`). Lets users keep hand-tracking off for OpenXR mode while leaving it on for SteamVR, but adds another knob.

Option 1 unless we hit a real reason for asymmetry. Add a migration entry in `alvr_session` only if we end up renaming; the additive option doesn't need one.

The OpenXR-mode driver advertises `XR_EXT_hand_tracking` unconditionally and just answers "untracked" when the switch is off — same pattern as how Monado's other drivers behave when the user disables tracking at the OS level.

## Monado-side wiring (as landed in Slice 2)

What actually shipped on the fork side:
1. New per-side `alvr_hand` xrt_device in `openxr/src/xrt/drivers/alvr/alvr_hand.c`, flagged `XRT_DEVICE_HAND_TRACKER` / `XRT_DEVICE_TYPE_HAND_TRACKER`, advertising a single `XRT_INPUT_HT_UNOBSTRUCTED_{LEFT,RIGHT}` input and `supported.hand_tracking = true`. Kept separate from `alvr_controller` per the original recommendation, so a touch-profile-only consumer doesn't pay the per-frame joint copy.
2. `alvr_hand_get_hand_tracking` calls `alvr_oxr_get_hand_skeleton`, copies the 26 joint poses 1:1 into `xrt_hand_joint_set::values.hand_joint_set_default[]`, stamps `XRT_SPACE_RELATION_{ORIENTATION,POSITION}_{VALID,TRACKED}_BIT` on every joint, applies the static per-joint radius table via `u_hand_joints_apply_joint_width`, sets `hand_pose` to the wrist relation, and sets `is_active = true`. When the bridge reports untracked the function zeroes `out_value` and returns Ok (the state tracker gates on `is_active`).
3. `alvr_hub.c::alvr_create_devices` wires the two devices into `xsysd->xdevs[]` and points `xsysd->static_roles.hand_tracking.unobstructed.{left,right}` at them. No `alvr_oxr_capabilities` bitfield was added — the device is always present; `is_active = false` is the cheap path when the client has hand-tracking off. If a later session needs per-feature gating (eye tracking, etc.) the bitfield can be added then.

## Wire-compat checklist (for the actual landing slice)

- [ ] `alvr_packets::TrackingData` unchanged on the wire. Confirmed by a bincode round-trip test using a serialized fixture from the current master build.
- [ ] `alvr_session::HandSkeletonConfig` unchanged or extended additively (no migration needed in option 1).
- [ ] `ALVR_OXR_BRIDGE_ABI_VERSION` bumped 3 -> 4 (single-source-of-truth via the alvr-side cbindgen header; no separate Monado-side constant).
- [ ] cbindgen header regenerated via `ALVR_REGENERATE_BRIDGE_HEADER=1 cargo build -p alvr_server_openxr`.
- [ ] `cargo xtask clippy --ci` clean.
- [ ] CTest suite on the openxr submodule clean (`build/openxr-debug` `ctest -C Debug --output-on-failure`, currently 25/25 on host 101 — see [[reference-remote-test-host]]).

## Verification ceiling (where this proves out)

Gate A/B equivalents stay achievable on this dev box (just compile + boot `monado-service.exe`, grep for the hand-tracker devices in the device list). Gate C — a real client driving `XR_EXT_hand_tracking` through the runtime — needs host 101 + a real headset. That is the same headset-blocked ceiling already on Gates C–G in `NEXT_STEPS.md`; no new verification debt.

## Recommended landing order (when the hosting blocker clears)

1. Bridge ABI v4 stub in `alvr_server_openxr` — returns `is_tracked=false` always. Regenerate header. Land.
2. Monado-side hand-tracker `xrt_device` reading from the stub. Land.
3. Wire the stub up to `ServerCoreContext::get_hand_skeleton`. Land.
4. Dashboard hint / launcher badge (optional polish — show "hand tracking forwarded" when the bridge advertises capability and the session flag is on).

Each step is testable in isolation: step 1 only needs Gate A; step 2 only needs Gate B; step 3 needs a connected client; step 4 needs the dashboard run.
