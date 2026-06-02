# Slice B2.2b — the handoff flip: implementation plan + RTX checklist

The behaviour flip that turns the semaphore handoff from inert plumbing into the
actual latency win. **Everything here is RTX-host-gated and locally
unverifiable** — bring it up on TESTHOST (RTX 3090 + Quest 3), not the AMD dev
host. Pairs with [`SEMAPHORE_HANDOFF_SCOPE.md`](SEMAPHORE_HANDOFF_SCOPE.md).

## State this builds on (B1 + B2.2a, landed)

Per frame, on the **Monado compositor submit thread** (`comp_alvr_layer_commit`):

1. `compose_via_squasher`: squash gfx submit signals forward timeline V, then
   **`vkQueueWaitIdle`** (CPU stall).
2. `run_ffr` (FFR on): compute submit signals forward V+1, then a **fence-wait**
   (CPU stall, via `ffr_end_signal_submit_wait_free_locked`).
3. `alvr_oxr_submit_layers` → `encoder_bridge::submit` → C++ `VkEncoderBackend::Submit`,
   **synchronous under the `ENCODER` lock on this thread**: waits forward V
   (instant — comp_alvr already CPU-waited), `cuMemcpy2DAsync` scratch→NVENC
   input on a stream, signals reverse V, `cuStreamSynchronize`, **`EncodeFrame`**
   (blocking), `onPacket` → `send_video_nal`.

Both forward + reverse timeline semaphores are exported (comp_alvr) and
imported (encoder); the reverse signal fires but **comp_alvr ignores it**. The
handle type is reported so imports don't probe.

Two stalls remain on the compositor thread: **(A)** the squash/FFR CPU waits
(steps 1–2) and **(B)** the synchronous `EncodeFrame` (step 3).

## Key facts that shape the design

- **The copy consumes the scratch, not the encode.** `cuMemcpy2DAsync` copies
  scratch → NVENC's own input pool. Once it lands, the scratch slot is free; the
  expensive `EncodeFrame` reads NVENC's input buffer, not the scratch. So the
  scratch-hold window is just the copy (sub-ms), not the whole encode.
- **The scratch ring has no reuse guard.** `comp_scratch.c::indices_get` is a
  blind `last+1 mod 4` (`COMP_SCRATCH_NUM_IMAGES`). Safe today only because
  synchronous encode keeps one frame in flight. Anything that reads the scratch
  off the compositor thread breaks that.
- **NVENC sessions are single-threaded.** All NVENC calls (`GetNextInputFrame`,
  `EncodeFrame`, `GetSequenceParams`, IDR force, `EndEncode`) must stay on one
  thread. `GetNextInputFrame` mutates session state (`m_iToSend`) — it cannot run
  on the compositor thread while a worker runs `EncodeFrame`.
- **Drops must still advance the reverse timeline.** If a frame V is dropped
  (never copied), comp_alvr will still eventually wait `reverse >= V` before
  reusing V's slot. So reverse must reach V for *every* assigned V, encoded or
  dropped, or comp_alvr deadlocks.

## Two variants — bring up partial first

### B2.2b-partial (recommended first flip) — moves only EncodeFrame off-thread

> **LANDED + headset-verified 2026-06-03** (master `567dd3ac`, encoder-only, no
> ABI/comp_alvr change). `VkEncoderBackend::Submit` copies the scratch into a
> 3-slot staging pool (drop-newest) and a single worker thread does
> staging→NVENC-input + `EncodeFrame` + packet callback. On TESTHOST (RTX 3090 +
> Quest 3): streamer survived 120 frames with no crash, import OK, client decoded
> + rendered, headset image clean. comp_alvr's squash `vkQueueWaitIdle` is
> untouched, so the squash-completion wait still sits on the compositor thread —
> only the encode moved off.
>
> **Pacing measured 2026-06-03 (RTX 3090, avg over 300 frames @ 2560×1184):**
> `bridge_call_us=740` (copy+enqueue, still on the compositor thread) +
> `encode_us=2657` (now on the worker, off-thread). So the bridge call dropped
> from ~3.4 ms (copy+encode) to ~0.74 ms — **partial removed ~2.66 ms/frame**
> (~24% of the 11.1 ms @90 Hz budget) from the compositor thread. `encode_us` ≪
> frame interval, so the worker keeps up and drop-newest rarely fires.
> **Still on the compositor thread:** comp_alvr's squash `vkQueueWaitIdle`
> (`cpu_us`, not yet instrumented — submodule) — that is what **full** targets.
> Decide on full by measuring `cpu_us`: if the squash-wait is small, partial may
> be enough; if large, full's reverse-wait complexity is justified.

Compositor thread keeps the scratch interaction (so the ring is consumed before
it returns → **no reuse hazard, reverse semaphore unused**); only `EncodeFrame`
moves to a worker.

- comp_alvr: **delete** `vkQueueWaitIdle` (squash) and the FFR fence-wait; the
  forward-semaphore wait inside the encoder's copy stream-sync replaces them.
  Add the **cross-queue FFR wait** (run_ffr submit waits forward V before
  reading squash scratch; see below) — required once the squash CPU drain is
  gone. *No reverse-wait, no per-slot tracking.*
- encoder: split `Submit` into a **synchronous copy phase** (on the compositor
  thread: forward-wait + `cuMemcpy2DAsync` scratch → a **staging** CUDA buffer
  ring (depth 2–3) + `cuStreamSynchronize`) and an **async encode phase** (enqueue
  `{staging idx, picParams, timestamp}` to a single worker thread that does
  `GetNextInputFrame` + copy staging→input + `EncodeFrame` + `onPacket`). The
  staging buffer keeps **all** NVENC calls on the worker, sidestepping the
  thread-safety constraint, and the scratch is fully consumed before the
  compositor returns.
- Win: `EncodeFrame` (several ms) off the compositor thread. **Not** removed: the
  squash-completion wait (now the copy's `cuStreamSynchronize` blocking on
  forward V) stays on the compositor thread.
- Cost: one extra device→device copy (scratch→staging), sub-ms.

### B2.2b-full — moves the whole handoff off-thread (uses B2.2a's reverse semaphore)

Compositor returns right after `vkQueueSubmit`; the worker does forward-wait +
copy + reverse-signal + encode. Removes **both** stalls. Requires the
scratch-reuse state machine:

- comp_alvr tracks, per scratch-ring slot, the forward value last written
  (`squash_slot_value[4]`, and `ffr_slot_value[4]` when FFR on — the slot index
  comes back in `frame_state.scratch_state.views[v].index` after
  `chl_frame_state_init`). Before a squash/FFR submit writes slot S, add a
  **GPU wait on `reverse >= slot_value[S]`** to that submit's
  `VkTimelineSemaphoreSubmitInfo` wait list, then record `slot_value[S] = V`.
- Drop handling: the bridge must ensure `reverse` reaches V for dropped frames.
  Cleanest: on a drop-newest backpressure decision (worker queue full), the
  **compositor-side bridge signals reverse = V immediately** (scratch never read
  → safe to free). Needs a tiny CUDA signal path callable off the worker, or a
  dedicated "advance reverse to V" enqueue the worker honours in order.
- The encoder worker owns copy + encode; no staging buffer needed.

**Recommendation:** land **partial first**, measure `oxr_pacing` on RTX, and only
build **full** if the residual squash-wait is a meaningful slice of the
compositor-thread budget. Full's reverse-wait state machine + drop-consistency is
the highest-risk code in the whole feature; don't pay for it unprofiled
(NEXT_STEPS' "only if measured").

## The cross-queue FFR wait (needed by both variants)

Once the squash `vkQueueWaitIdle` is gone, `run_ffr` (on the FFR pool's queue)
can start before the squash (on `main_queue`) finishes writing the squash
scratch it samples. Fix: the FFR submit must **wait forward V** and **signal
forward V+1**, instead of today's signal-only + CPU fence-wait. Extend
`ffr_end_signal_submit_wait_free_locked` (or its B2.2b successor) with a
`pWaitSemaphores`/`pWaitSemaphoreValues = {forward, V}` and a
`pWaitDstStageMask = VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT`.

## Per-file implementation steps

### B2.2b-partial

1. `VkEncoderBackend.cpp`
   - Add a staging ring: `CUdeviceptr staging[N]` + `size_t stagingPitch`,
     allocated in `Create` (`cuMemAllocPitch`, size = 2·width × height), freed in
     `Shutdown`.
   - Split `Submit`: copy phase (existing forward-wait + `cuMemcpy2DAsync` into
     `staging[i]` + reverse-signal-if-present + `cuStreamSynchronize`), then push
     `{i, idr, timestamp}` to the worker mailbox; return true.
   - Worker thread (single, owns a `cuCtxPushCurrent`): pop → `GetNextInputFrame`
     → `cuMemcpy2D` staging[i] → input → `EncodeFrame` → strip NALs → `onPacket`.
     Join + drain on `Shutdown`.
   - Mailbox: bounded (depth N-1), `std::mutex` + `condition_variable`;
     drop-newest with a logged counter on overflow.
2. `comp_alvr.c`
   - `compose_via_squasher`: delete the `vkQueueWaitIdle` block.
   - `run_ffr`: replace the CPU fence-wait submit with a submit that waits
     forward V + signals forward V+1 (no CPU wait); defer cmd-buffer free (rotate
     a small cmd-buffer pool or free-after-N-frames, since it can't be freed while
     in flight).
   - Keep passing forward + reverse handles (reverse stays inert here).
3. No ABI change (B2.2a's surface suffices).

### B2.2b-full (adds, on top of partial or instead of staging)

1. `comp_alvr.c`: per-slot `slot_value[]` tracking + reverse-wait on squash/FFR
   submits (read slot index from `scratch_state.views[0].index`); remove the
   staging dependency (worker does the scratch copy directly).
2. `VkEncoderBackend.cpp`: move the scratch copy into the worker; bridge enqueue
   carries the scratch handles + values instead of a staging index.
3. Bridge: a path to advance reverse to V on drop (compositor-side signal or
   ordered worker "skip" token). Possibly ABI v11 if comp_alvr needs to send the
   per-frame "this is value V" explicitly for the drop path (it already has V).

## RTX bring-up checklist (TESTHOST, RTX 3090 + Quest 3)

Environment / deploy (per [[openxr_mode_quirks_windows]]):
- [ ] Run the Monado service **non-elevated**; `ALVR_ROOT=%LOCALAPPDATA%`.
- [ ] Force `XR_RUNTIME_JSON` to the built ALVR-Monado manifest (loaders walk
      HKLM first).
- [ ] **3-binary lockstep**: rebuild + deploy `alvr_server_openxr.dll`,
      `comp_alvr`/Monado, and the client from the **same** revision — bridge ABI
      v10 mismatch refuses to load (`alvr_hub.c` check). Grep the boot log for the
      `comp_alvr` factory fn, not just the driver name (static-link ≠ reachable).
- [ ] Quest 3 physically worn (ignores `prox_close`); bring it online via the
      `connect-quest` skill if `adb devices` is empty.

Functional bring-up (in order):
- [x] **Handle type confirmed (2026-06-03, RTX 3090)**: boot log of the deployed
      B2.2a build reports `exported (handle type 1)` = `OPAQUE_WIN32`;
      `vk_print_external_handles_info` shows `OPAQUE_WIN32_BIT(timeline): true`,
      `D3D12_FENCE_BIT(timeline): false`. Encoder imports as
      `TIMELINE_SEMAPHORE_WIN32`. (Doc's earlier "D3D12_FENCE preferred" was wrong
      for this driver — it isn't exportable here.)
- [x] **Import succeeds (2026-06-03, RTX 3090, full streaming session)**: Quest
      client connected (encoder `build_config` stood up), `oxr_overlay_smoke`
      rendered 30 frames through Monado, and the encoder logged
      `semaphore import: type=TIMELINE_WIN32 forward=OK consumed=OK`. Both the
      forward and reverse timeline semaphores import via `cuImportExternalSemaphore`
      as `TIMELINE_SEMAPHORE_WIN32`. The one-shot diagnostic that surfaced this
      (B2.2a's import success is otherwise invisible — CPU wait masks it) is the
      `alvr_vk_encoder_import_diag` path logged once by the Rust bridge. So both
      ABI-v10 semaphore imports are proven on hardware; B2.2b can rely on them.
- [ ] **Steady-state video**: `layer_commit diag` counters
      (`submit_ok` climbing, `submit_failed`/`submit_no_encoder` flat); STATS/FPS
      hold vs the pre-B2.2b baseline.
- [ ] **No corruption**: sustained streaming shows no tearing/garbage. Corruption
      = scratch reused before the copy landed → the reuse-protection is wrong
      (partial: staging copy/sync ordering; full: reverse-wait/slot tracking).
- [ ] **Induced-stall test (full only)**: throttle the encoder (lower clocks /
      heavier bitrate) to force backpressure; confirm drops advance reverse (no
      compositor hang) and no corruption. This is the make-or-break for full.
- [ ] FFR **on and off** both clean (FFR adds the second ring + V+1 chaining).

Measurement (the point of the slice):
- [ ] `oxr_pacing` `SUBMIT_BEGIN→SUBMIT_END` delta on the compositor thread
      **drops** vs baseline. Partial: by ~the `EncodeFrame` cost. Full: to near
      the bare `vkQueueSubmit` cost. Capture a before/after window via the
      metrics exporter.
- [ ] End-to-end latency (motion-to-photon, if measurable) not regressed.

Failure-signature quick map:
- Black/garbage frames, valid poses → reuse hazard (copy/reuse ordering) **or**
  semaphore import fell back silently. Check the import log first.
- Compositor hang / frame timer stalls → reverse deadlock (full): a dropped frame
  never advanced reverse, comp_alvr waits forever. Check the drop path.
- `submit_failed` climbing → per-frame CUDA error (wait/signal/copy); pull the
  `trace()` last-error string the Rust bridge logs.

## Safety / rollback

- The whole flip is gated on `c->squash_timeline != VK_NULL_HANDLE`. Keep a fast
  revert: if the semaphore export ever fails, comp_alvr must still fall back to a
  CPU-wait path (don't leave only the async path). Consider keeping the
  `vkQueueWaitIdle` behind a debug env flag for A/B on the RTX host.
- Land partial as its own commit (submodule + pointer bump) and verify before
  starting full; never bundle the two flips.
