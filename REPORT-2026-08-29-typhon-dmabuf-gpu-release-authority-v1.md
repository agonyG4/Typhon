# Typhon DMA-BUF GPU Release Authority v1.1

Date: 2026-08-29
Repository: `/home/agony/GitHub/Typhon`
Scope: KMS-worker DirectPrimaryLease exclusion and deferred-release liveness.

## Outcome

The v1 GPU-release authority now has one conservative Direct/KMS safety snapshot
covering both compositor Atomic state and the KMS worker's queued, executing, and
inflight primary leases. The snapshot is passed to both normal rendered-frame
release arming and Atomic `NoVisualChange` release-only fencing.

Deferred DMA-BUF obligations remain compositor-owned and now create bounded,
timer-backed retry debt on Atomic. Retry debt does not count as visual frame work,
does not create callbacks or KMS commits, and does not run a polling loop. A due
retry rechecks Direct/KMS ownership, creates one release-only GPU fence, transfers
the exact obligations to one GPU lease, and registers the independent completion
FD. Failure preserves the exact obligations and backs off.

Normal rendered-frame fence setup still has the existing pageflip fallback. The
compatibility backend remains conservative and does not enter an unsupported
native-fence retry loop.

## Exact root causes

### P0: incomplete Direct/KMS barrier

The v1 gate called `AtomicEglGbmScanout::has_live_direct_kms_ownership()`. That
method correctly observed Atomic `submitted`, `presented`, and `suspended` Direct
ownership, but it could not observe the worker-owned lease transferred by
`finish_direct_worker_queued()` into `KmsCommitJob::direct_primary_lease`.

The worker independently stores that lease in `queued`, `executing`, or
`inflight`; `KmsCommitWorkerHandle::direct_content_keys()` is the existing narrow
snapshot of those states. The scheduler intentionally permits composed
render-ahead while a Direct primary is queued, so the overlap is real and cannot
be eliminated by changing scheduling policy.

The new `dmabuf_gpu_release_safety()` combines both authorities. Any non-empty
worker state or Atomic Direct state makes compositor-GPU release ineligible. This
is intentionally a global conservative barrier for v1.1; it does not infer
BufferId-specific ownership from candidate keys.

### P1: deferred work had no production liveness authority

`pending_dmabuf_buffer_releases` and `deferred_dmabuf_buffer_releases` were both
included in the aggregate release count, but `has_unowned_frame_work()` only
considered the pending list. After a NoVisualChange terminal or release-only
fence setup failure, the deferred list could therefore become invisible when no
new surface or callback work arrived.

The fix keeps deferred releases out of visual work and adds a runtime retry debt
with the following schedule:

```text
initial retry: 1 ms
failure backoff: 2 ms, 4 ms, 8 ms, ...
cap: 250 ms
```

The deadline is merged into the existing runtime timer. A timer wake services
only release debt before normal work classification; it does not set redraw,
presentation, callback, or KMS state.

## Before and after ownership flow

Before this closure:

```text
retired DMA-BUF
    -> pending/deferred compositor ownership
    -> NoVisualChange or rendered frame
    -> no physical terminal for some paths
    -> deferred list with no production retry authority
```

and the GPU eligibility decision observed only Atomic Direct ownership:

```text
Atomic Direct state only
    -> compositor GPU release lease
```

After this closure:

```text
retired DMA-BUF
    -> exact compositor obligation
    -> CompositorFrameBatch or deferred retry debt
    -> complete Direct/KMS safety snapshot
         Atomic submitted/presented/suspended
         + worker queued/executing/inflight
    -> if safe: one release-only or rendered-frame GPU fence
    -> dedicated DmabufGpuRelease reactor watch
    -> exact protocol-token completion
```

If the snapshot is unsafe, the obligation is not transferred to a compositor GPU
lease. A normal rendered frame retains its existing physical-presentation
fallback. A NoVisualChange terminal defers the obligation and arms retry debt.

`DmabufReleaseObligation` and exact release-token equality remain unchanged. A
retry transfers deferred obligations directly into the compositor-owned GPU lease;
it does not manufacture a new visual frame batch.

## Direct/KMS barrier evidence

`src/native_output/runtime/dmabuf_release.rs` defines
`DmabufGpuReleaseSafety` and `dmabuf_gpu_release_safety()`. Its worker query is
the existing `KmsCommitWorkerHandle::direct_content_keys()` API, which reports the
three worker domains without exposing or duplicating queue internals.

`src/native_output/runtime/presentation_cycle.rs` computes one snapshot for the
Atomic render attempt and passes it into the rendered and NoVisualChange paths.
`src/native_output/scanout/atomic_egl_gbm.rs` uses the same snapshot for the
release-only fence condition. `arm_composited_dmabuf_release()` accepts the
snapshot instead of independently reading only Atomic ownership.

The deterministic safety test covers each worker tuple. Existing worker tests
also observe the real executing and inflight states. The scheduler test suite
continues to prove that queued Direct-worker ownership can coexist with
`RenderAhead`; the release snapshot remains conservative in that overlap.

No worker queue capacity, admission, ordering, render-ahead, or KMS worker policy
was changed.

## Deferred retry evidence

`src/native_output/runtime/dmabuf_release.rs` owns the bounded retry state and
its backoff. `src/native_output/runtime/metrics.rs` includes the retry deadline
in the existing timer arming only for Atomic deferred work. `run_cycle()` services
due retry debt before classifying visual work.

`CompositorState` now exposes a deferred-only count and a direct transfer into an
exact GPU lease. `has_unowned_frame_work()` deliberately still excludes deferred
releases. The state test proves that transfer drains the deferred list without
creating visual batch work.

Normal rendered-frame failure behavior remains conservative: if the ready-fence
FD cannot be duplicated or registered, the frame-batch obligation is requeued and
can use the existing physical presentation terminal. The retry debt is primarily
for NoVisualChange, where no physical frame terminal exists.

Compatibility backends do not schedule this Atomic retry path. They retain the
existing safe presentation-bound ownership.

## Physical presentation and protocol semantics

Logical release completion is still separate from physical output presentation.
A GPU completion watch only calls the existing exact lease completion method. It
does not advance:

- output scene history or presented serials;
- output swapchain history;
- pageflip sequence;
- `wp_presentation` feedback;
- O1 frame-callback admission;
- regional surface-damage history.

The GPU fence proves compositor reads have completed. It does not prove KMS
ownership has ended, which is why the complete Direct/KMS barrier is checked
before transfer. Direct Scanout remains governed by its existing
`DirectReleaseProof`/out-fence/pageflip ownership.

SHM remains materialization-bound and is not routed through this registry. The
independent render-fence FD ownership remains unchanged: the DMA-BUF completion
FD is duplicated independently of the KMS submission FD and timing FD.

## KWin and Hyprland comparison

The relevant KWin lesson remains separation of graphics-buffer lifetime from
output presentation: a renderer completion fence is the proof for compositor GPU
reads, while pageflip is a separate display event. Typhon applies that lesson at
its existing `CompositorFrameBatch`, `NativeRenderFence`, and release-registry
boundaries rather than adding a scene hierarchy.

The relevant Hyprland lesson is the same split in a different implementation:
render completion feeds buffer sync releasers, while Direct/KMS ownership uses a
different proof. Hyprland also contains pragmatic sibling-damage and tree-recheck
compatibility behavior; this closure does not copy those hacks or alter Typhon's
regional damage model.

The protocol lesson is that legacy `wl_buffer.release` and explicit sync release
timeline points have different protocol terminals but may share the same
asynchronous GPU-completion proof. Exact explicit release-token identity remains
distinct even when `BufferId` is equal.

## Files changed

- `src/native_output/runtime/dmabuf_release.rs`: complete Direct/KMS safety
  snapshot, retry debt/backoff, Atomic retry service, and focused tests.
- `src/native_output/runtime/presentation_cycle.rs`: pass the unified safety
  snapshot through rendered and NoVisualChange release decisions; classify retry
  causes.
- `src/native_output/scanout/atomic_egl_gbm.rs`: use the unified safety snapshot
  for release-only NoVisualChange fencing.
- `src/native_output/runtime/cycle.rs`: service due retry debt before normal work
  classification.
- `src/native_output/runtime/metrics.rs`: arm the retry deadline without marking
  deferred release debt as visual work.
- `src/native_output/runtime/mod.rs`: export the safety/retry types and include
  fence-creation failures in the bounded shutdown summary.
- `src/compositor/state/frames.rs`: expose deferred-only ownership and transfer
  it directly into an exact GPU lease; document the non-visual work boundary.
- `src/compositor/server_frames.rs`: server wrappers for deferred count/transfer.
- `src/compositor/state/frame_tests.rs`: deferred transfer and non-visual work
  regression coverage.
- `src/native_output/kms_worker/direct_lease_tests.rs`: real worker-state safety
  assertions for executing/inflight ownership.
- `docs/superpowers/plans/2026-08-29-typhon-dmabuf-gpu-release-authority-v1-1-plan.md`:
  implementation plan updated with completed implementation steps and the
  protocol-boundary limitation.
- This report.

No pointer-reposition file was modified or staged.

## RED tests and pre-fix failures

The required TDD ordering was followed for the new v1.1 surfaces. Before the
production implementation, the focused DMA-BUF test command failed to compile
with the expected missing symbols:

```text
DmabufGpuReleaseSafety: undeclared type
DmabufReleaseRetryReason: undeclared type
retry_deadline_ns/retry_due/retry_after_failure: missing methods
```

The post-fix focused command passed seven DMA-BUF release tests. The worker
suite passed 28 tests, including the real queued, executing, and inflight
ownership observations, and the scheduler tests passed with render-ahead
behavior unchanged.

The new deterministic state coverage proves:

- every worker ownership phase blocks compositor-GPU release;
- an empty worker/Atomic ownership snapshot permits it;
- retry debt has a deadline, capped backoff, and no visual-work flag;
- deferred obligations can transfer to a GPU lease without a visual batch.

The repository already contains protocol-level Wayland and explicit-sync tests,
including real `wl_buffer` resources and syncobj timelines. Those existing tests
were re-run where available. A new real-DRM protocol test that holds an actual
native GPU fence unsignaled before pageflip was not run in this environment; the
native runtime requires a suitable DRM/KMS TTY. This report does not claim that
integration category as executed.

## Verification results

Focused checks completed during this closure:

- `rtk cargo test --locked dmabuf_release --bin oblivion-one`: 7 passed;
- `rtk cargo check --locked`: passed;
- `rtk cargo clippy --locked --all-targets --all-features -- -D warnings`:
  no issues reported;
- `rtk cargo test --locked compositor::state::frame_tests --lib`: 53 passed;
- `rtk cargo test --locked direct_lease --bin oblivion-one`: 28 passed;
- `rtk cargo test --locked scheduler --bin oblivion-one`: 7 passed;
- `rtk cargo test --locked --bin oblivion-one topology_transitions_match_full_reference_with_rotating_output_ages`: 1 passed;
- `rtk cargo test --locked --bin oblivion-one rejected_rendered_candidate_does_not_advance_history_and_retry_reuses_exact_pixels`: 1 passed;
- `rtk cargo test --locked wayland_client_syncobj_dmabuf_release_signals_release_point_after_present --lib`: 1 passed;
- existing native fence, work-domain, integrated topology/buffer-age, Direct
  Scanout, SHM, O1, and explicit-sync-focused suites were retained and focused
  checks were run as part of the preceding v1 qualification.

Final requested verification:

```text
rtk cargo fmt --check: passed
rtk cargo check: passed
rtk cargo clippy --all-targets --all-features -- -D warnings: passed
rtk cargo test: failed in unrelated tests/sigchld.rs::one_child_exit_wakes_the_sigchld_signalfd_once
  observed: left 0, expected 1
  all other reported suites passed; the failing test is unchanged and was
  reproduced in the preceding v1 verification
git diff --check: passed
git status --short: only the v1.1 source, plan, and report are modified/untracked
```

The known full-suite `sigchld::one_child_exit_wakes_the_sigchld_signalfd_once`
failure is unrelated to this closure and reproduced in isolation in the
preceding v1 verification. It was not modified.

No native DRM/KMS qualification was executed. Therefore this report makes no
claim about native pre-pageflip release latency, 165 Hz measurements, or actual
GPU-fence metrics.

## Non-regression boundaries

Source review and focused tests show no changes to:

- O1 `Captured -> RenderedAwaitingAdmission -> admission -> frame callback`;
- physical-pageflip `wp_presentation` completion;
- SHM materialization and early release;
- DMA-BUF Direct Scanout release proof or framebuffer identity;
- regional damage, buffer-age repair, or exact frame surface lineage;
- KMS worker scheduling, queue depth, or submission policy;
- pointer-reposition work.

## Remaining conservative follow-ups

- Actual native DRM/KMS qualification of compositor-GPU releases before pageflip.
- Real protocol-boundary qualification with a native unsignaled GPU fence and
  pageflip counters.
- Compatibility EGL remains presentation-bound when no safe native completion
  fence exists.
- GPU reset/context-loss terminal policy remains conservative and is not invented
  by this closure.
- BufferId-specific Direct-worker exclusion is intentionally not optimized;
  v1.1 uses the safe global barrier.
- DMA-BUF GPU-completion release is separate from future bounded pacing-trace
  volume work, resize convergence, and any pointer-reposition work.
