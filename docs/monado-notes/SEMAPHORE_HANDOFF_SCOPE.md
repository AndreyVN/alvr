# Squash → encoder semaphore handoff — scope

Replacing `comp_alvr`'s `compose_via_squasher` `vkQueueWaitIdle` CPU stall with a
GPU→GPU handoff so the Monado compositor thread stops blocking on the squash/FFR
GPU work each frame. Two slices.

- **Slice A — landed** (master `20d03683`, openxr `2482288a6`, bridge **ABI v9**).
  Additive plumbing: an exported timeline semaphore is created in `comp_alvr`,
  signalled from the squash submit (`compose_via_squasher`) and the FFR submit
  (`run_ffr`), and its native handle + signalled value are forwarded through
  `alvr_oxr_submit_layers` into `VkSubmitDesc.sync_semaphore_{handle,value}`.
  **No runtime behaviour change** — the CPU `vkQueueWaitIdle` / FFR fence-wait
  stay authoritative and the encoder ignores the new fields.
- **Slice B — this doc.** The actual latency win. RTX-host-gated for any
  before/after measurement.

## Why Slice A alone wins nothing (the honest framing)

Today, per frame, on the **Monado compositor submit thread** (`comp_alvr_layer_commit`):

```
squash submit → vkQueueWaitIdle ──┐ (FFR on: + run_ffr submit → fence-wait)
                                  ├─ all blocking, on the compositor thread
bridge → cuMemcpy2D → cuCtxSynchronize → EncodeFrame ──┘
```

`encoder_bridge::submit` (lib.rs) calls the C++ `VkEncoderBackend::Submit`
**synchronously under the `ENCODER` lock**, on that same thread, and `Submit`
ends with `cuCtxSynchronize` + `EncodeFrame` — both blocking.

If Slice B only swapped `vkQueueWaitIdle` for `cuWaitExternalSemaphoresAsync` and
kept `Submit` synchronous, the CPU would still block at `cuCtxSynchronize` for
~the same squash duration. **The stall would relocate, not disappear.** The win
requires moving the encode **off** the compositor thread so `layer_commit`
returns right after `vkQueueSubmit`.

So Slice B is three coupled changes, not one:

1. **Forward GPU→GPU wait** — encoder waits on the Slice-A timeline (already
   plumbed) instead of the compositor CPU-draining the queue.
2. **Async encode** — `Submit` runs on a dedicated encoder thread; the bridge
   call becomes a non-blocking enqueue.
3. **Scratch-reuse safety** — async encode breaks the "only one frame in flight"
   invariant the round-robin scratch ring silently relies on (see below).

…and on the `comp_alvr` side, removing the CPU waits forces an explicit
**cross-queue FFR ordering** fix.

## The scratch-reuse hazard (the hard part)

`comp_alvr` writes the encoder's input into a 4-deep scratch ring
(`COMP_SCRATCH_NUM_IMAGES == 4`, separate rings for the squash scratch and the
FFR output). `comp_scratch.c::indices_get` is a **blind round-robin** —
`last+1 mod 4`, **no in-use / fence tracking**. It is safe today only because
synchronous encode guarantees slot N is fully consumed (copied into NVENC's own
input buffer) before `layer_commit` returns and the ring advances.

With async encode, the compositor advances through the ring and wraps every 4
frames. If the encoder's CUDA copy of slot N hasn't finished by the time the
compositor re-renders slot N (4 frames later), Vulkan overwrites a buffer CUDA is
still reading → corruption. At 90 Hz, 4 frames ≈ 44 ms of headroom and the
`cuMemcpy2D` is sub-millisecond, so it is *probably* safe in steady state — but
"probably" under a momentary stall is not correctness. Two ways to make it sound:

- **B-safe (recommended): reverse "consumed" signal.** A second exported timeline
  semaphore, imported into CUDA, that the encoder **signals** right after the
  per-view `cuMemcpy2D` completes ("scratch slot free"). The compositor's *next*
  squash/FFR submit that reuses that ring slot adds a **wait** on it. The hazard
  window is just the copy (not the whole encode), so back-pressure is minimal.
  Note the scratch handle is only needed until the copy lands — NVENC reads from
  its own input buffer afterwards — so the reverse signal fires early.
- **B-interim: rely on ring depth.** Ship async with only the forward wait, bound
  in-flight encodes to < ring depth, and `log()` a dropped-frame counter if the
  encoder can't keep up. Simpler, but a documented soft guarantee, not a hard one.

This naturally suggests a **B1 / B2 split** (see commit boundaries).

## Must-resolve before coding: the export handle type

Monado's `vk_get_timeline_semaphore_handle_type` (vk_sync_objects.c) prefers
**`VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT`** over `OPAQUE_WIN32` on
Windows when `external.timeline_semaphore_d3d12_fence` is enabled. CUDA's import
enum must match the exporter:

| Monado exports | CUDA `cuImportExternalSemaphore` type |
| --- | --- |
| `D3D12_FENCE` | `CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE` |
| `OPAQUE_WIN32` timeline | `CU_EXTERNAL_SEMAPHORE_HANDLE_TYPE_TIMELINE_SEMAPHORE_WIN32` |

**Action:** at bring-up, log which bit `vk_create_timeline_semaphore_and_native`
actually used on the RTX host (instrument `comp_alvr` init or read the boot log),
then either hardcode the matching CUDA type or — cleaner — **carry the handle
type across the bridge** as a small enum field so the encoder picks correctly and
the design survives a different host. That field is the likely reason Slice B
bumps **ABI v9 → v10**.

## Work breakdown

### comp_alvr (`openxr/`)
- **Remove the CPU waits**: delete `vkQueueWaitIdle` in `compose_via_squasher`
  and switch `run_ffr` back off the signal-and-CPU-wait helper
  (`ffr_end_signal_submit_wait_free_locked`) to a **submit-only** path (no fence
  wait, no free-after-wait — the cmd buffer lifetime now extends past CPU return,
  so it needs a per-frame/ring-tracked free, e.g. tied to the reverse "consumed"
  signal or a small fence ring).
- **Cross-queue FFR ordering**: with the squash's `vkQueueWaitIdle` gone, the FFR
  compute (on the FFR pool's queue) must **wait** on the squash timeline value V
  before reading the squash scratch, and signal V+1. Add the timeline wait to the
  FFR submit's `VkTimelineSemaphoreSubmitInfo` (`pWaitSemaphoreValues`). Slice A
  relied on the CPU drain for this ordering.
- **(B-safe)** create the reverse "consumed" timeline semaphore; export it; add a
  **wait** on it to the squash/FFR submit for the ring slot being reused; forward
  its handle (+ type) over the bridge so CUDA can signal it.
- Free-list / fence ring for the now-async command buffers (squash uses the
  persistent `c->nr.cmd`; that can't be re-recorded until the prior submit
  completes — today the `vkQueueWaitIdle` guaranteed it. Need a small N-deep cmd
  rotation or a wait-on-timeline before re-record).

### Encoder (`alvr/server_openvr/cpp/encoder/win32_vk/`)
- `CudaDriverApi.h`: add `cuImportExternalSemaphore`, `cuWaitExternalSemaphoresAsync`,
  `cuSignalExternalSemaphoresAsync` (B-safe), `cuDestroyExternalSemaphore`, and a
  real stream (`cuStreamCreate`/`cuStreamSynchronize`/`cuStreamDestroy`) — the
  copies currently run on the null stream.
- `VkEncoderBackend.cpp`:
  - Import the forward semaphore **once**, cached by handle (it's session-stable),
    not per-frame.
  - In `Submit`/`importViewToInput`, move the copies onto a real stream and issue
    `cuWaitExternalSemaphoresAsync(stream, {handle, value})` before the first
    `cuMemcpy2D`; order `EncodeFrame` after.
  - (B-safe) `cuSignalExternalSemaphoresAsync` after the copies → the reverse
    "consumed" semaphore.
- **Async worker**: a single dedicated encoder thread (NVENC sessions are
  single-threaded — must serialize, so one thread, not a pool). `Submit` becomes
  enqueue + return; single-slot mailbox with **drop-newest** on backpressure;
  drain on `Shutdown`. The `ENCODER` lock contract and the IDR/`insertIdr`
  exchange need to stay correct across the thread hop.

### Bridge (`alvr/server_openxr/`)
- ABI **v9 → v10** (likely): carry the **handle type** for the forward semaphore,
  and (B-safe) the reverse semaphore's handle + type. Update the const + cbindgen
  header macro + history, `alvr_hub.c` picks it up via the macro.
- `encoder_bridge`: thread the new fields into `VkSubmitDesc`; wire the async
  enqueue.

## Risks / correctness checklist

- **Reverse-sync correctness** is the make-or-break. Without it (B-interim), a
  single encoder stall corrupts a frame silently (tearing/garbage, not a crash) —
  exactly the symptom to watch for in the headset.
- **Timeline monotonicity across queues**: squash (main_queue) signals V, FFR
  (FFR queue) waits V + signals V+1 — must stay strictly increasing; verify no
  value reuse across frames.
- **NVENC single-thread**: all NVENC calls (`EncodeFrame`, `GetSequenceParams`,
  IDR force, `EndEncode`) must stay on the one worker thread.
- **`c->nr.cmd` re-record**: persistent squash cmd buffer can't be re-recorded
  while a prior async submit is in flight — needs a rotation or timeline gate.
- **Handle-type mismatch**: silent `cuImportExternalSemaphore` failure → falls
  back / frames drop. Log the chosen Vulkan bit and the CUDA import result.
- **Shutdown/connection-loss ordering**: drain the worker and destroy CUDA
  semaphores before the Vulkan device goes away (`comp_alvr_destroy` already
  guards the forward semaphore; reverse + worker join must slot in).

## Verification (RTX-host-gated)

- `oxr_pacing` telemetry: the `SUBMIT_BEGIN→SUBMIT_END` delta on the compositor
  thread should **drop** once the encode is async (the whole point). Compare a
  before/after window on TESTHOST (RTX 3090).
- Visual integrity: no tearing/garbage under sustained streaming **and** under an
  induced encoder stall — the reverse-sync test. FFR-on and FFR-off both.
- STATS / FPS hold; no new dropped-frame inflation beyond the documented
  drop-newest backpressure.
- Mirror-capture harness (`alvr_capwin_*`) for SteamVR-vs-Monado parity if any
  regression is suspected.

## Recommended commit boundaries

1. **B1** — CUDA loader additions (stream + external-semaphore entry points) +
   forward `cuWaitExternalSemaphoresAsync` on a real stream, **encode still
   synchronous**. Compile-checkable on the AMD host (it's just the C++ + loader);
   no behaviour change until the comp_alvr CPU waits are removed, so it's safe to
   land and CI-verify alone. *(No measurable win yet — same reason Slice A had
   none.)*
2. **B2** — async encoder thread + remove comp_alvr CPU waits + cross-queue FFR
   wait + reverse "consumed" semaphore + ABI v10. This is the behaviour flip and
   the win; **must** be verified on the RTX host before it's trustworthy. Keep
   B-safe (reverse sync) in this commit rather than shipping B-interim to master.

## Files to read first (Slice B)

1. `alvr/server_openvr/cpp/encoder/win32_vk/VkEncoderBackend.cpp` — `Submit` /
   `importViewToInput` (the synchronous null-stream path to make async).
2. `alvr/server_openvr/cpp/encoder/win32_vk/CudaDriverApi.h` — loader to extend.
3. `openxr/src/xrt/compositor/alvr/comp_alvr.c` — `compose_via_squasher`,
   `run_ffr`, `ffr_end_signal_submit_wait_free_locked` (Slice A signal points).
4. `openxr/src/xrt/compositor/util/comp_scratch.c` — `indices_get` (the
   round-robin with no in-use tracking) — the reuse hazard.
5. `openxr/src/xrt/auxiliary/vk/vk_sync_objects.c` —
   `vk_get_timeline_semaphore_handle_type` (the D3D12_FENCE-vs-OPAQUE_WIN32
   decision the CUDA import type must match).
6. `alvr/server_openxr/src/lib.rs` — `encoder_bridge::submit` (sync→async) +
   `alvr_oxr_submit_layers`.
