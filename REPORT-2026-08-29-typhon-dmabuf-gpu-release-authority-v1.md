# Typhon DMA-BUF GPU Release Authority v1.2

Date: 2026-08-29
Repository: `/home/agony/GitHub/Typhon`
Scope: v1.1 KMS-worker DirectPrimaryLease exclusion and deferred-release
liveness, plus exact current-token eligibility and retry quiescence.

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

The v1.2 correction keeps an exact release obligation ineligible whenever its
exact protocol token is current again. This is enforced at GPU-fence,
pageflip/direct-presentation, and safe-abandonment terminals. Deferred work is
partitioned by the same authority: current-token-blocked obligations remain
event-driven, while inactive obligations retain the v1.1 bounded retry debt.

## Checkout authority and source delta

The local checkout was authoritative for this work. At the v1.2 source audit,
`origin/main` was `a0d5b8a`, while the local branch was at the later
pointer-unlock closure `97fa2a4` and already contained the v1 DMA-BUF,
exact-lineage, regional-damage, O1, SHM, and v1.1 worker/liveness commits. No
public snapshot was substituted for the local source.

The concurrent pointer design/spec and implementation commits were preserved
and were not modified or staged by v1.2. The v1.2 working-tree changes are
limited to the exact-token release authority, retry gate, tests, report, and
plan listed below.

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

## v1.2 corrective closure

### Exact root cause

The v1.1 GPU-fence terminal already checked
`active_dmabuf_buffers` with `same_release_token()`. The later ordinary
frame-batch terminal did not: `complete_frame_batch_releases()` called the
low-level release method directly. A token requeued by a GPU lease could
therefore be captured by a later batch and incorrectly receive a legacy
`wl_buffer.release` at pageflip while the exact token was current again.

The retry scheduler had the same ownership distinction missing in a different
place. It treated every deferred obligation as retryable, so a token that was
blocked only by being current could repeatedly create release-only fences.

### Before and after ownership flow

Before v1.2:

```text
GPU terminal checks exact token
        |
        +-- current -> deferred
        |
        +-- inactive -> release

deferred list -> retry timer/fence machinery without eligibility partition
frame batch terminal -> direct protocol release without revalidation
```

After v1.2:

```text
every non-shutdown terminal
        |
        v
dmabuf_release_token_is_active(same_release_token)
        |
        +-- current -> deferred, no release
        |
        +-- inactive -> exact protocol terminal

deferred list
        |
        +-- current token -> event-driven only; no retry deadline/fence
        |
        +-- inactive token -> existing bounded retry debt/fence machinery
```

`complete_dmabuf_release_if_inactive()` is the single non-shutdown terminal
helper. GPU completion, normal pageflip, Direct presentation, and safe
abandonment all use it. Shutdown retains its explicit forced terminal after
renderer/KMS teardown. `take_frame_batch_for_render()` also leaves current-token
deferred obligations deferred, preventing avoidable release-only fences; the
terminal check remains mandatory for tokens that become current after capture.

Logical release eligibility remains separate from physical presentation. The
helper changes only protocol-release ownership and bounded release metrics. It
does not advance scene history, output-buffer presentation serials, pageflip
state, `wp_presentation`, or O1 callback admission.

### RED tests and pre-fix failures

The tests were added before the production changes. The first focused RED run
failed at compilation because the planned exact-token and retryability APIs did
not exist. Against the pre-fix source, the relevant terminal behavior was also
directly visible in source: a later frame-batch completion called
`complete_dmabuf_release()` without checking the active token, and deferred
transfer moved every obligation.

The new deterministic tests now cover the former failure modes:

- `gpu_requeued_current_token_stays_protected_through_a_later_pageflip`:
  requeued current token remains unreleased through a later ordinary frame and
  releases once after final retirement.
- `frame_batch_pageflip_revalidates_a_token_that_becomes_current_after_capture`:
  capture-before-reattach is protected at the terminal.
- `distinct_explicit_release_token_remains_releasable_while_same_buffer_is_current`:
  same `BufferId` does not merge distinct explicit points.
- `deferred_transfer_skips_current_tokens_but_retries_inactive_tokens`:
  mixed deferred work transfers only inactive obligations.
- `safe_abandonment_revalidates_a_current_release_token` and
  `direct_presentation_revalidates_a_current_release_token`: both additional
  non-shutdown terminals use the same authority.
- `current_token_only_deferred_work_does_not_arm_retry_debt`: a current-only
  deferred set has no deadline or retry attempts despite one second of
  deterministic clock advancement.

The pre-existing GPU requeue test continues to pass, proving that the v1.1
GPU terminal behavior was preserved rather than replaced.

### Retry liveness and metrics

`retryable_deferred_dmabuf_release_count()` applies the exact-token authority
to the deferred list. The runtime deadline arming, due-retry service, and
NoVisualChange fallback use this count. `update_retry_for_deferred_work()`
clears retry debt when only current tokens remain and records bounded
`retry_skipped_current_token` visibility. Inactive deferred obligations still
use v1.1's 1 ms initial delay, capped exponential backoff, and asynchronous
GPU-fence registry. No refresh-rate polling, draw, callback, KMS commit, or
busy wait is introduced.

Terminal metrics now expose `dmabuf_release_terminal_revalidated` and
`dmabuf_release_terminal_requeued_current`. The v1.1 worker Direct/KMS safety
snapshot remains the source of truth for normal rendered release, NoVisualChange
release-only fencing, and deferred retry.

### Integrated and protocol evidence

The existing v1.1 deterministic integrated topology/buffer-age oracle remains
unchanged and passed its targeted full-reference pixel comparison. It covers
rotating client/output buffers, output ages, popup/subsurface transitions,
rejected-candidate retry, and overlapping SSD visuals. v1.2 adds the exact
reattachment/pageflip ownership cases around that oracle; it does not weaken
regional damage or buffer-age history.

The focused state tests use exact release-token fixtures, including distinct
explicit-sync points. Existing protocol and explicit-sync suites were retained.
No new native DRM/KMS run or new real unsignaled-GPU-fence Wayland integration
run was performed in this environment, so this report does not claim native
pre-pageflip release qualification.

### Files changed in v1.2

- `src/compositor/frame_batch.rs`: bounded terminal revalidation metrics.
- `src/compositor/server_frames.rs`: server access to retryable deferred count.
- `src/compositor/state/frames.rs`: centralized exact-token terminal authority,
  frame-capture filtering, inactive deferred transfer, and shutdown separation.
- `src/compositor/state/frame_tests.rs`: current-token terminal, distinct-token,
  mixed-deferred, and retry-quiescence coverage.
- `src/native_output/runtime/dmabuf_release.rs`: retry gate and skip metric.
- `src/native_output/runtime/metrics.rs`: retry deadlines based on retryable
  deferred obligations.
- `src/native_output/runtime/presentation_cycle.rs`: NoVisualChange retry gate.
- `src/native_output/runtime/mod.rs`: bounded shutdown metric output.
- `docs/superpowers/plans/2026-08-29-typhon-dmabuf-gpu-release-authority-v1-2-plan.md`:
  v1.2 implementation plan.
- This report.

No pointer-reposition file was modified or staged.

## Verification results

Focused checks completed during this closure:

- `rtk cargo test --lib frame_consumption_tests`: 59 passed;
- `rtk cargo test dmabuf_release::tests`: 7 passed;
- `rtk cargo test current_token_only_deferred_work_does_not_arm_retry_debt`:
  1 passed;
- `rtk cargo test xwayland_reactor_x11_window_reaches_window_ready_without_direct_fd_polling`:
  1 passed when replayed independently;
- `rtk cargo test one_child_exit_wakes_the_sigchld_signalfd_once`: 1 passed
  when replayed independently;
- the v1.1 worker Direct/KMS, scheduler render-ahead, native-fence,
  work-domain, integrated topology/buffer-age, Direct Scanout, SHM, O1, and
  explicit-sync-focused suites remain unchanged and were covered by the
  preceding qualification.

Final requested verification:

```text
rtk cargo fmt --check: passed
rtk cargo check: passed
rtk cargo clippy --all-targets --all-features -- -D warnings: passed
rtk cargo test --lib: 1,961 passed, 2 ignored
rtk cargo test: two full-run attempts each exposed one unrelated flaky test:
  run 1: native_output::runtime::xwayland_reactor_tests::xwayland_reactor_x11_window_reaches_window_ready_without_direct_fd_polling
    observed assertion: unrelated below parent below popup physically
  run 2: native::kms::tests::explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success
    observed assertion: left 1, right -1
  independent replay of both failures passed; no v1.2 source touches either
  module. Therefore the full aggregate command is recorded as not clean.
git diff --check: passed at final handoff
git status --short: clean after the v1.2 commit
```

The two full-run failures are unrelated to this closure and both passed in
isolation. The SIGCHLD test also passed independently. None of those modules
were modified.

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
