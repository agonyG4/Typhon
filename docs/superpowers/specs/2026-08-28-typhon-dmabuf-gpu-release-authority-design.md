# Typhon DMA-BUF GPU Completion Release Authority v1

Date: 2026-08-28

## Scope

Move only composited DMA-BUF client release completion from the normal
physical-pageflip terminal to an asynchronous compositor-GPU completion
terminal. Preserve the existing O1 callback admission, SHM materialization,
regional damage, output buffer-age, KMS worker, and Direct Scanout authorities.

The current checkout stores a retired DMA-BUF's protocol completion token in
`SurfaceBufferRelease`, but does not retain its imported `BufferId`. Pending
releases are captured by `CompositorFrameBatch` and every terminal currently
completes them through the pageflip-oriented helper. `NoVisualChange` therefore
has no proof that a retired DMA-BUF's last compositor read has finished.

## Design

### Identity and ownership

Add an immutable `DmabufReleaseObligation` containing both the imported
`BufferId` and the exact `SurfaceBufferRelease`. The active surface map stores
the same pair. Equality and duplicate detection remain based on the exact
protocol release token, never only on `BufferId`.

`CompositorFrameBatch` owns obligations until one of these explicit transfers:

```text
frame batch
  -> GPU lease (eligible composited Atomic frame)
  -> pageflip fallback (Direct/KMS, compatibility, or unavailable proof)
  -> shutdown terminal
```

The compositor retains protocol resources in a keyed GPU-lease map. Native
runtime state retains only a lease ID, an owned completion FD, and the reactor
token. A lease has a source batch when it was transferred from a rendered
frame, so registration failure can restore the exact pageflip fallback owner.
NoVisualChange release-only leases requeue to pending work when registration
cannot be established.

On GPU completion, the compositor removes the lease before completing each
obligation. If the exact release token has become current again, the
obligation is requeued instead of sending an unsafe duplicate release. A
different explicit-sync point for the same `BufferId` remains independent.

### Native fence

`NativeRenderFence` gains a non-consuming `duplicate_completion_fd()` method.
It duplicates the same sync-file referenced by the existing submission/timing
descriptors. KMS submission and timing ownership remain unchanged and each
descriptor remains independently consumable.

### GPU completion

Atomic composited rendering exports one native fence after all scene sampling.
The runtime duplicates that fence before KMS consumes `submission_fd`, transfers
the exact frame-batch obligations to one GPU lease, and registers the duplicate
under `NativeEventSource::DmabufGpuRelease`. The event loop unregisters that
token once, then asks the compositor to complete that lease. This wake domain
does not participate in output submission, READY admission, callbacks, cursor
arbitration, or pageflip history.

An Atomic `NoVisualChange` terminal creates a release-only fence after the
retired obligations are identified and before logical batch settlement. It does
not draw, submit a framebuffer, advance output history, or emit presentation
feedback. If fence creation, duplication, or registration fails, obligations
remain owned and use the existing safe pageflip/pending fallback.

Compatibility EGL remains conservative because it has no required native-fence
export contract. Direct Scanout never transfers its KMS-owned obligations to a
GL lease; it continues to use `DirectReleaseProof` and existing out-fence/
pageflip ownership. A mixed or uncertain transition remains conservative.

### Failure and teardown

No obligation is released without a valid fence, existing Direct/KMS proof, a
safe legacy presentation terminal, or explicit compositor teardown. A rejected
KMS submission does not cancel a registered GPU lease: the GPU read proof is
still valid even though no physical presentation occurred. A render failure
before fence export restores the frame batch. A dead protocol resource is
discarded through the existing resource-liveness path. Session teardown
unregisters all release tokens and restores or terminates their compositor
ownership without waiting synchronously for the GPU.

## Verification model

Deterministic tests cover:

- retired composited buffers before and after a controllable fence signal;
- explicit-sync release points and legacy `wl_buffer.release`;
- release-only NoVisualChange with zero extra draws and no physical state;
- rejected output submission with valid GPU release;
- failed render/fence duplication/registration and teardown;
- current-buffer and exact-token reattachment guards;
- multiple obligations coalesced onto one fence;
- overlapping READY/in-flight frames;
- Direct Scanout isolation and Direct-to-composed transitions;
- never-sampled buffers and cache reuse after a new commit/acquire;
- unchanged SHM materialization and O1 callback paths.

The integrated deterministic oracle combines client A/B/C rotation, output
triple-buffer ages, topology changes, decorated overlap, a rejected candidate,
and a real retry, comparing every presented optimized frame with a full
reference renderer.

