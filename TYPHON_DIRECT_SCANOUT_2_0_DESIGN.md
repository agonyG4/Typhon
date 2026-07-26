# Direct Scanout 2.0 Design

**Date:** 2026-07-26
**Project:** Typhon / Oblivion One
**Target stage:** Stage 4 of the presentation roadmap
**Baseline:** `3fcb18f`
**Status:** Approved for implementation planning

## 1. Executive decision

Direct scanout must stop being a parallel presentation mode and become one possible primary-plane assignment of an `OutputTransaction`.

The design combines three proven ideas:

- KWin's model: build one complete plane state, validate it atomically, and let the commit retain every framebuffer until pageflip ownership ends.
- Hyprland's model: reject unsuitable candidates early, expose precise blocker diagnostics, and fall back to composition instead of treating scanout rejection as fatal.
- Apple's public presentation contracts: acquire scarce drawable-like resources late, keep the number of in-flight resources bounded, distinguish target timing from actual presentation, and never report a dropped frame as presented.

The Typhon-specific implementation must reuse the architecture already present at `3fcb18f`:

- `OutputTransaction`
- `OutputPlanePlan`
- `PrimaryPlaneAssignment::ClientFramebuffer`
- `AtomicCommitKind::DirectPrimary`
- `KmsCommitJob`
- `KmsTestOnlyPolicy::Required`
- `OutputTransactionLedger`
- `AtomicCommitArbiter`
- the bounded KMS commit worker

It must not create a second generic plane framework. The main work is to consolidate these pieces and delete the direct-scanout lifecycle authorities that duplicate them.

## 2. Scope

Stage 4 delivers conservative, single-output direct scanout for one opaque fullscreen dmabuf surface on the primary plane, optionally combined with the existing atomic hardware cursor plane.

Included:

- primary-plane client framebuffer assignment;
- exact candidate identity through content epochs;
- job-owned dmabuf and DRM framebuffer lifetime;
- worker-side atomic `TEST_ONLY` followed by the real commit using the same immutable job;
- exact pageflip-based transaction settlement;
- immediate composited fallback after rejection;
- transition handling for composed to direct, direct to direct, and direct to composed;
- same-content suppression without fake pageflips;
- scanout-capable linux-dmabuf feedback;
- blocker, ownership, timing, rejection, and settlement observability;
- deterministic tests and real TTY/DRM qualification.

Excluded:

- overlay-plane scheduling;
- underlays;
- VRR;
- tearing;
- multi-output and hotplug;
- output transforms;
- fractional or arbitrary scaling;
- color conversion, HDR, ICC, and non-identity color pipelines;
- multi-GPU import or cross-device copies;
- broad format expansion beyond the formats explicitly qualified by the existing primary-plane path;
- direct scanout without the Stage 3 KMS worker;
- making direct scanout default-on before qualification.

## 3. Prerequisite gate

Stage 4 implementation must not begin until Stage 3 has passed real TTY/DRM qualification with the worker forced on.

Required Stage 3 matrix:

- Palworld;
- Steam UI and popups;
- Firefox;
- Kitty;
- at least one additional Vulkan game;
- fullscreen enter and exit;
- cursor movement and cursor image changes;
- Alt+Tab under load;
- shutdown while a primary commit is active;
- `bin/qualify-kms-worker`.

The gate exists because Stage 4 depends on the worker's queue, retry, shutdown, cursor, pacing, and pageflip ownership guarantees. Direct scanout must not become a second debugging variable before those guarantees are proven on hardware.

## 4. Reference compositor findings

### 4.1 KWin

Relevant source paths in the reviewed KWin archive:

- `src/compositor.cpp`
- `src/scene/workspacescene.cpp`
- `src/backends/drm/drm_egl_layer.cpp`
- `src/backends/drm/drm_pipeline.cpp`
- `src/backends/drm/drm_commit.cpp`
- `src/backends/drm/drm_commit_thread.cpp`

KWin's useful properties are:

1. Direct scanout is an assignment prepared for a layer, not a separate output lifecycle.
2. Source rectangle, destination rectangle, transform, format, color state, primary plane, and cursor plane are validated as one KMS presentation state.
3. The exact framebuffer resources are retained by the atomic commit object.
4. Presentation feedback is emitted after pageflip completion.
5. If the ideal assignment fails, KWin degrades through simpler complete assignments rather than partially mutating an already-tested state.
6. Commit-thread ownership permits queueing, merging, and deadline-aware submission without allowing the compositor thread to release KMS resources early.

Typhon should copy the ownership and complete-state validation principles, but not KWin's current overlay, underlay, color-management, multi-output, or generic layer-optimizer breadth.

### 4.2 Hyprland

Relevant source paths in the reviewed Hyprland archive:

- `src/output/Monitor.cpp`
- `src/managers/fullscreen/handler/FullscreenHandler.cpp`
- `src/protocols/LinuxDMABUF.cpp`

Hyprland's useful properties are:

1. The hot path rejects candidates quickly.
2. Diagnostic code can report explicit reasons such as overlays, popups, recording, software cursor, transform, non-dmabuf content, and color-management incompatibility.
3. Direct scanout failure falls back to normal rendering.
4. The linux-dmabuf feedback advertises a scanout tranche before the general rendering tranche.

Typhon must not copy:

- early presentation feedback before the real KMS result;
- same-buffer special cases that pretend a new presentation occurred;
- mutable output-state rollback as the main ownership model;
- incomplete device, format, or modifier validation;
- direct scanout as a renderer-return shortcut outside the transaction system.

### 4.3 Apple public contracts

Apple does not publish WindowServer source code. This design therefore uses only public Metal and Core Animation behavior, not claims about private implementation.

The useful public principles are:

- an opaque fullscreen Metal layer can qualify for direct-to-display behavior on supported Apple Silicon systems;
- target submission timing and estimated presentation timing are distinct values;
- actual presentation is reported separately, and a drawable that is dropped does not receive a nonzero presented time;
- drawable resources are limited and should be acquired late and held briefly;
- frame latency is deliberately bounded rather than allowing unlimited rendering ahead.

These principles map to Typhon as follows:

- eligibility is conservative and transparent to clients;
- `PresentationTarget` remains a scheduling target, not proof of presentation;
- only a validated pageflip terminalizes an output transaction as `Presented`;
- direct framebuffer leases are acquired after eligibility and worker admission are known;
- the existing one-in-flight plus one-queued worker bound remains unchanged.

Official references:

- https://developer.apple.com/documentation/metal/managing-your-game-window-for-metal-in-macos
- https://developer.apple.com/documentation/quartzcore/cametaldisplaylink
- https://developer.apple.com/documentation/quartzcore/cametaldisplaylink/update/targettimestamp
- https://developer.apple.com/documentation/quartzcore/cametaldisplaylink/update/targetpresentationtimestamp
- https://developer.apple.com/documentation/quartzcore/cametaldisplaylink/preferredframelatency
- https://developer.apple.com/documentation/metal/mtldrawable/presentedtime
- https://developer.apple.com/documentation/metal/mtldrawable/addpresentedhandler(_:)
- https://developer.apple.com/library/archive/documentation/3DDrawing/Conceptual/MTLBestPracticesGuide/Drawables.html

## 5. Current Typhon state

Typhon already represents direct content correctly at the logical level:

```rust
OutputTransactionContent::Direct {
    frame_id,
    key,
}

PrimaryPlaneAssignment::ClientFramebuffer {
    key,
    framebuffer_id,
}

AtomicCommitKind::DirectPrimary {
    transaction_id,
    direct_token,
    framebuffer_id,
}
```

The worker already supports:

```rust
KmsTestOnlyPolicy::Required
```

and executes `TEST_ONLY` before the real commit from the same `KmsCommitJob`.

The remaining architectural problem is duplicated authority in the direct path:

```text
OutputTransactionLedger
AtomicCommitArbiter
KMS worker queue

plus

PreparedDirectFrame
WorkerQueuedDirectFrame
SubmittedDirectFrame
PresentedDirectFrame
DirectScanoutState.current
DirectScanoutState.worker_queued
DirectScanoutState.pending
```

The current path also performs synchronous `TEST_ONLY` on the compositor thread, stores only a framebuffer ID in the worker job, and keeps the real dmabuf/framebuffer resource in `DirectScanoutState.worker_queued`.

This means logical state, queued state, submitted state, resource ownership, and presentation state are distributed across overlapping structures.

## 6. Core invariant

> Direct scanout is not a presentation mode. It is a `PrimaryPlaneAssignment::ClientFramebuffer` inside an immutable `OutputTransaction`.

Every direct attempt must have exactly one authority for each responsibility:

| Responsibility | Authority |
|---|---|
| Logical descriptor and protocol obligations | `OutputTransactionLedger` |
| Queued or executing physical resources | `KmsCommitJob` |
| Kernel-submitted identity | `AtomicCommitArbiter` |
| Currently scanned-out direct resource | `DirectPrimaryOwnership.presented` |
| Imported framebuffer reuse | `DirectFramebufferCache` |
| Scheduling and pacing | existing frame scheduler and `NativeFramePacing` |

No other structure may independently decide that a transaction is queued, submitted, presented, failed, or abandoned.

## 7. Direct eligibility and diagnostics

### 7.1 Hot-path decision

The native runtime uses a non-generic pure result:

```rust
pub(crate) enum DirectScanoutDecision {
    Eligible,
    Blocked(DirectScanoutRuntimeBlocker),
}
```

The hot path remains first-rejection and allocation-light.

It evaluates, in order:

1. direct-scanout policy is `ExperimentalAuto`;
2. the KMS worker is running and admission is available;
3. no output/session/shutdown transition is active;
4. no queued or submitted primary transaction blocks admission;
5. the compositor provides one solitary fullscreen candidate;
6. no popup, shell layer, drag icon, lock surface, visual clip, resize preview, or unpublished work is visible;
7. the candidate is an opaque XRGB dmabuf;
8. buffer size equals output size;
9. scale is `1`;
10. transform is normal;
11. viewport source and destination are identity;
12. buffer device, format, modifier, and plane layout are compatible with the selected DRM device and primary plane;
13. acquire synchronization is ready;
14. the current cursor assignment is representable by the primary-plus-cursor atomic state;
15. the `DirectScanoutCandidateKey` differs from the already presented content key.

Any rejection composes normally in the same cycle when it occurs before worker admission.

### 7.2 Diagnostic assessment

Qualification and debug modes need more than the first rejection. Add a diagnostics-only assessment that collects all applicable scene and runtime blockers without changing the hot path.

The diagnostics path is called only when:

- `TYPHON_DIRECT_SCANOUT_DEBUG=1`;
- structured qualification output is requested;
- a direct attempt is rejected and rate-limited detailed reporting is enabled.

It must report stable reason names. At minimum:

```text
policy_off
worker_unavailable
worker_queue_full
output_transition
session_inactive
shutdown_active
no_fullscreen_owner
overlay_visible
popup_visible
owner_tree_has_additional_surface
software_cursor_visible
non_dmabuf
format_unsupported
modifier_unsupported
buffer_device_unproven
buffer_size_mismatch
buffer_scale_unsupported
buffer_transform_unsupported
viewport_non_identity
visual_clip_present
placement_mismatch
resize_preview_active
pending_or_unpublished_work
acquire_not_ready
cursor_assignment_unsupported
same_content
```

Do not duplicate scene eligibility logic merely for metrics. Extract pure predicates from the existing candidate builder and call those predicates from both the first-rejection API and the diagnostics API.

## 8. Owned direct primary lease

Introduce one job-owned resource bundle:

```rust
pub(crate) struct DirectPrimaryLease {
    key: DirectScanoutCandidateKey,
    surface_id: u32,
    _buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    surface_damage: Option<SurfaceDamagePresentation>,
}
```

The lease must be `Send` and must retain:

- the client's dmabuf handle;
- the imported DRM framebuffer and its GBM ownership;
- the exact candidate key;
- the direct surface identity;
- the sampled surface-damage presentation state needed when the pageflip is confirmed. The damage token is stored in an `Option` so settlement can consume it once while the dmabuf and framebuffer remain pinned as the currently scanned-out resource.

Protocol frame-batch ownership remains in the transaction ledger. The lease does not invent a second frame-batch authority.

`KmsCommitJob` gains:

```rust
pub(crate) direct_primary_lease: Option<DirectPrimaryLease>
```

Validation rules:

- `AtomicCommitKind::DirectPrimary` requires a lease;
- the lease framebuffer ID and key must match the transaction plane assignment;
- composited, compatibility, and cursor-only jobs must not contain a direct lease;
- every worker rejection returns the unchanged job and lease;
- every successful worker submission transfers the lease in the `Submitted` event;
- `EBUSY` retries keep the exact same job and lease;
- shutdown quiesce returns queued leases exactly once;
- forced abandonment retains a kernel-submitted lease until KMS restore makes release safe.

The job must own the lease while queued and while the ioctl is executing. The compositor thread must not keep an additional `worker_queued` direct frame.

## 9. Physical ownership after submission

Replace the lifecycle-heavy `DirectScanoutState` with two focused components:

```rust
pub(crate) struct DirectPrimaryOwnership {
    submitted: Option<SubmittedDirectPrimary>,
    presented: Option<PresentedDirectPrimary>,
    suspended: Vec<DirectPrimaryLease>,
}

pub(crate) struct DirectScanoutControl {
    framebuffer_cache: DirectFramebufferCache,
    validation_cache: DirectPlaneValidationCache,
    inhibit_until_composited_present: bool,
    counters: DirectScanoutCounters,
    drm_generation: u64,
}
```

`SubmittedDirectPrimary` owns only physical submission state:

```rust
pub(crate) struct SubmittedDirectPrimary {
    transaction_id: OutputTransactionId,
    token: PageFlipToken,
    lease: DirectPrimaryLease,
    submit_started_at: MonotonicTimestampNs,
    submit_returned_at: MonotonicTimestampNs,
    out_fence: Option<OwnedFd>,
}
```

`PresentedDirectPrimary` owns the lease currently scanned out by KMS and presentation metadata needed for diagnostics.

It must not duplicate:

- frame-batch ownership;
- transaction state;
- scheduler state;
- worker admission state;
- pageflip routing identity.

On a confirmed direct pageflip:

1. `AtomicCommitArbiter` returns the exact `DirectPrimary` identity.
2. `DirectPrimaryOwnership` verifies token and transaction identity.
3. The submitted lease becomes presented.
4. The previously presented direct lease becomes releasable.
5. The ledger transaction enters settlement.
6. Frame callbacks, presentation feedback, buffer release, and sampled damage are completed exactly once.

## 10. Worker-side atomic validation

The compositor thread must not call synchronous direct `TEST_ONLY`.

For every direct job that does not have an exact positive validation-cache hit:

```rust
KmsTestOnlyPolicy::Required
```

The worker must execute:

```text
same KmsCommitJob
→ TEST_ONLY primary + cursor assignment
→ real nonblocking atomic commit
```

The following values must be identical between test and commit:

- output generation;
- CRTC;
- primary framebuffer;
- cursor update;
- source/destination state represented by the current bounded submitter;
- input fence ownership;
- out-fence request;
- pageflip token-bearing job identity, excluding flags that are necessarily different between test and real submit.

The scene is not queried again between `TEST_ONLY` and the real commit.

If newer content arrives:

- a still-queued direct job may be superseded through the existing bounded queue policy;
- an executing or kernel-submitted direct job presents its immutable snapshot once;
- the newer content becomes a later transaction.

## 11. Validation cache

Replace `tested_plane_plan: Option<TestedDirectPlanePlan>` with a small positive-only cache.

```rust
pub(crate) struct DirectPlaneValidationKey {
    output_generation: u64,
    crtc_id: u32,
    primary_plane_id: u32,
    mode_width: u32,
    mode_height: u32,
    format: u32,
    modifier: u64,
    buffer_width: u32,
    buffer_height: u32,
    plane_layout_hash: u64,
    cursor_plan_key: Option<u64>,
    synchronization_key: u64,
}
```

Only include fields that are actually enforced by Stage 4. Do not add fake color, transform, overlay, or VRR epochs for unsupported systems.

Cache behavior:

- maximum eight positive entries;
- exact-key lookup only;
- no permanent negative cache;
- a short per-key rejection cooldown is allowed only to prevent repeated `TEST_ONLY` storms and must still compose normally;
- invalidate all entries on DRM/output generation change, modeset, session resume, primary/cursor plane capability change, or output reconstruction;
- invalidate the matching entry after a real commit rejection;
- a cache hit sets `KmsTestOnlyPolicy::Skip`, but the real commit still owns the same immutable resources and may still fail safely.

The cache optimizes validation. It never changes resource ownership or transaction settlement.

## 12. Fallback hierarchy

### 12.1 Before transaction construction

These failures compose in the same cycle:

- no eligible candidate;
- unsupported synchronization;
- framebuffer import failure;
- worker unavailable;
- worker admission unavailable;
- same content already presented.

No direct transaction is inserted for an ordinary eligibility rejection.

### 12.2 Worker `TEST_ONLY` rejection

The returned job still owns its lease.

The runtime must:

1. cancel the exact pacing and scheduler identity;
2. reject the arbiter's worker-queued reservation;
3. settle the direct transaction as a pre-submit failure without presentation feedback;
4. restore the frame batch for a composed retry;
5. release the returned lease exactly once;
6. record the validation rejection;
7. request an immediate composited redraw.

The previously presented framebuffer remains active, so there is no black frame.

### 12.3 Real commit rejection

The runtime performs the same cleanup, additionally invalidates the matching positive validation-cache entry, and schedules composition.

Direct rejection is not fatal unless the worker reports uncertain kernel ownership. The existing fatal path for uncertain submission remains unchanged.

### 12.4 Cursor fallback

A rejection of `client primary + hardware cursor` does not prove that the hardware cursor is globally broken.

The fallback order is:

1. direct primary plus hardware cursor;
2. composed primary plus hardware cursor;
3. composed primary plus software cursor only if the ordinary composed cursor path independently rejects or cannot represent the cursor.

Do not latch software cursor mode merely because direct `TEST_ONLY` rejected the combined assignment.

## 13. Assignment transitions

There is no special "enter" or "exit" ioctl.

### Composed to direct

```text
CompositorFramebuffer
→ confirmed pageflip
→ ClientFramebuffer
```

Entry metrics and direct-active state change only after the confirmed pageflip.

### Direct to direct

```text
ClientFramebuffer A
→ confirmed pageflip
→ ClientFramebuffer B
```

A remains pinned until B's pageflip confirms replacement.

### Direct to composed

```text
ClientFramebuffer
→ confirmed pageflip
→ CompositorFramebuffer
```

The previous direct lease is released only after the composed pageflip confirms that KMS no longer scans it out.

The compositor's presented-damage history is invalidated at confirmed assignment transitions, not at queue admission. This prevents a rejected direct attempt from corrupting composed damage history.

## 14. Same-content and metadata-only commits

`DirectScanoutCandidateKey` remains content-epoch authoritative.

Rules:

- same wl_buffer plus a new content epoch is new visual content and may produce a new direct transaction;
- same candidate key as the currently presented direct content is `NoVisualChange`;
- same candidate key as a queued or submitted transaction is not submitted twice;
- a metadata-only commit that does not change the content epoch does not cause an ioctl;
- `NoVisualChange` does not emit presentation feedback pretending that new content reached the display;
- callback obligations follow the existing no-visual-change settlement contract;
- `same_buffer_resubmissions` must remain zero in qualification.

No Hyprland-style same-buffer presentation shortcut is permitted.

## 15. Presentation and release semantics

Only a validated DRM pageflip may call the transaction's presented terminal path.

On presentation:

- use the actual DRM timestamp and sequence;
- record whether the path was direct or composed;
- complete presentation feedback once;
- complete the sampled direct surface damage once;
- complete the frame batch once;
- advance pacing once;
- release the replaced framebuffer only after KMS ownership has moved away from it.

On rejection, supersede, cancellation, suspension, output destruction, or safe abandonment:

- never emit presentation feedback;
- terminalize the transaction exactly once;
- settle frame-batch obligations according to the existing drop/failure reason;
- retain any framebuffer whose kernel ownership is uncertain until restore or explicit safe abandonment.

## 16. linux-dmabuf feedback

The scanout tranche must describe only combinations Typhon can actually import and assign to the selected primary plane.

Order:

1. primary-plane scanout tranche for the selected DRM device;
2. general renderer tranche.

The tranche must not advertise:

- unsupported modifiers;
- formats accepted only by the renderer but not by the primary plane;
- cross-device scanout without a qualified path;
- scaling, transform, or color-conversion capabilities Stage 4 does not implement.

Any change to selected DRM device, plane capability, or output generation rebuilds the feedback.

## 17. Observability

Structured metrics must expose:

- candidate checks;
- first blocker;
- full blocker set in qualification/debug mode;
- candidate accepted;
- import attempts, hits, failures, and live leases;
- worker admission rejection;
- validation cache hit and miss;
- `TEST_ONLY` attempts, duration, and rejection reason;
- real submit attempts, duration, and rejection reason;
- queued, submitted, presented, dropped, failed, superseded, and safely abandoned direct transactions;
- composed fallback requests and fallback latency in refresh cycles;
- direct entries, steady-state presentations, and exits;
- direct-to-direct replacements;
- same-content suppression and same-buffer resubmission;
- early release prevention assertions;
- duplicate feedback and duplicate settlement counters;
- current presented assignment type;
- current direct surface, buffer identity, framebuffer ID, and content epoch.

Debug logging must be rate-limited and disabled by default.

## 18. Failure policy

| Failure | Result |
|---|---|
| Eligibility rejection | Compose normally |
| Import rejection | Compose normally and record reason |
| Worker queue full | Compose normally or preserve queued visual work according to existing scheduler policy |
| `TEST_ONLY` rejection | Settle direct attempt, compose next cycle |
| Real commit rejection with known no-submit | Settle direct attempt, invalidate cache, compose next cycle |
| `EBUSY` within retry budget | Keep exact job and lease, retry |
| `EBUSY` exhausted | Settle direct attempt, compose next cycle |
| Uncertain kernel submission | Existing worker fatal/restore path |
| Pageflip timeout | Existing Stage 3 quarantine and safe-abandonment path |
| Session suspend | Quiesce worker, retain unsafe-to-release resources until restore |
| Client disconnect before submit | Returned/cancelled job settles without presentation |
| Client disconnect after kernel submit | Buffer lease remains valid through pageflip or safe restore |

## 19. Test strategy

### Pure tests

- scene rejection predicates;
- runtime blocker ordering;
- full diagnostic blocker collection;
- candidate-key equality and content-epoch behavior;
- validation-key equality, bounded eviction, and invalidation;
- direct lease/job payload validation;
- fallback classification;
- transition state machine;
- no-visual-change settlement.

### Worker tests

- direct job carries the lease through queue delay;
- `TEST_ONLY` and real submit use the same primary and cursor state;
- `EBUSY` preserves the same lease and input fence;
- `TestRejected`, `SubmitRejected`, and `BusyExhausted` return the lease once;
- successful submit transfers the lease in the worker event;
- shutdown quiesce returns queued leases;
- forced shutdown retains submitted ownership until safe restore.

### Runtime integration tests

- composed to direct;
- direct A to direct B;
- direct to composed;
- popup appearing over direct content;
- hardware cursor movement while direct remains active;
- direct combined-state rejection followed by composed hardware cursor;
- same-content suppression;
- same buffer with new content epoch;
- test rejection and real rejection fallback;
- late pageflip;
- client disconnect;
- session suspend/resume;
- shutdown under load;
- every transaction reaches one terminal state;
- no duplicate callback, feedback, damage, or release settlement.

### Real qualification

Applications:

- Palworld;
- Steam;
- Firefox fullscreen video and WebGL;
- Kitty fullscreen;
- another native Vulkan game;
- a Wine/Proton Vulkan game.

Scenarios:

- repeated fullscreen enter and exit;
- Alt+Tab loops;
- Steam popup and context-menu overlays;
- shell volume and notification overlays;
- hardware cursor movement and image changes;
- forced software cursor;
- same-buffer commits;
- resize immediately before fullscreen;
- client crash;
- VT switch;
- session suspend/resume;
- shutdown under load;
- injected `EBUSY`;
- injected `TEST_ONLY` rejection;
- injected real-submit rejection.

## 20. Acceptance criteria

Stage 4 is complete only when all of the following are true:

- direct scanout is represented only as `PrimaryPlaneAssignment::ClientFramebuffer` in an `OutputTransaction`;
- no synchronous direct `TEST_ONLY` runs on the compositor thread;
- every direct worker job owns the exact dmabuf and imported framebuffer resource it names;
- one in-flight plus one queued remains the worker bound;
- no direct resource is released before replacement pageflip or safe KMS restore;
- all direct transactions reach exactly one terminal state;
- only confirmed pageflips produce presentation feedback;
- same content is not resubmitted;
- same buffer with a new content epoch is not incorrectly suppressed;
- rejection never produces a black frame;
- normal rejection is nonfatal and returns to composition within at most one additional presentation cycle;
- direct combined-state cursor rejection does not unnecessarily disable the hardware cursor for composed frames;
- composed damage history remains valid after rejected direct attempts and is invalidated at confirmed assignment transitions;
- scanout feedback advertises only qualified primary-plane combinations;
- direct scanout remains default `Off` until the full hardware matrix passes;
- Stage 3 worker qualification and Stage 4 direct qualification reports are preserved as release evidence;
- VRR, tearing, overlays, multi-output, hotplug, scaling, transform, and color-management scope is not introduced.

## 21. Final architecture

```text
Wayland surface commit
        │
        ▼
solitary fullscreen candidate + content epoch
        │
        ▼
conservative eligibility / blocker decision
        │
        ▼
framebuffer import + DirectPrimaryLease
        │
        ▼
OutputTransaction
  primary = ClientFramebuffer
  cursor  = Atomic | Unchanged
        │
        ▼
KmsCommitJob owns lease
        │
        ├── TEST_ONLY rejected ──► settle without feedback ──► composed redraw
        │
        ▼
real atomic commit
        │
        ▼
AtomicCommitArbiter owns submitted identity
        │
        ▼
validated pageflip
        │
        ▼
transaction settlement + PresentedDirectPrimary
        │
        ▼
release replaced framebuffer and protocol obligations exactly once
```

This architecture is intentionally narrower than KWin, safer than Hyprland's special path, and consistent with the bounded-resource and actual-presentation principles exposed by Apple's public APIs.
