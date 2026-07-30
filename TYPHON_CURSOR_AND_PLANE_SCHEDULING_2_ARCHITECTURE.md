# Typhon Cursor and Plane Scheduling 2.0

## Comparative Architecture, Final Design, and Two-Pass Review

**Date:** 2026-07-30
**Typhon baseline:** `d9ae1a28d601d3284aa59a3d277147a58c11fa03` (`fix(native): reject unsupported direct modifiers before import`)
**Reference archives:** `Typhon144.zip`, `kwin-master(1)(5).zip`, `Hyprland-main(1)(5).zip`
**Hyprland display backend reference:** Aquamarine `6d6e2384f381def4ea4ea81543cba4bbdac72457`, pinned by the supplied Hyprland archive
**Scope:** single output, ordinary VSync, Atomic KMS, the existing KMS Commit Worker, Direct Scanout 2.0, and Triple Buffering 2.0.
**Explicitly out of scope:** VRR activation, tearing, multi-output, hotplug, generic overlay promotion, and a generic KMS commit reordering framework.
**Implementation status:** implemented in the Typhon repository in nine reviewable stages. The final validation record and hardware-qualification status are documented in section 30.

---

## 1. Executive decision

Typhon should implement a **transactional plane scheduler with one KMS lane and one replaceable cursor sidecar**.

The system should combine:

- **KWin's layer abstraction, high-priority cursor assignment, full-state validation, partial-plane commits, fallback ladder, and asynchronous cursor intent**;
- **Hyprland's low-latency per-output cursor delivery, initial stale-plane clearing, conservative hardware eligibility, dedicated cursor buffers, exact software-cursor damage, and immediate fallback**;
- **Apple's publicly visible separation between desired/model state, in-flight/presentation state, private render state, explicit presentation targets, and hardware-offloaded movement of cached visual content**;
- **Typhon's stronger transaction identity, exact pageflip ownership, explicit buffer/fence leases, bounded worker queue, direct-scanout ownership, Triple Buffering 2.0 pipeline snapshot, and terminal settlement rules**.

Typhon must not copy any reference compositor wholesale. The final architecture is intentionally stricter than all three references:

1. Every visible output change belongs to an immutable transaction or an explicitly replaceable pre-submit sidecar.
2. There is exactly one authoritative KMS submission lane.
3. Cursor movement never allocates or consumes a compositor primary slot.
4. Cursor work never creates a second competing commit queue.
5. A cursor update can be attached late to the next queued Atomic commit, but only before a precise worker freeze point and without mutating an already submitted transaction.
6. Hardware cursor failure is cached by an exact capability key, not latched globally forever.
7. Software cursor mode is embedded in primary composition and temporarily forces reactive double buffering, preventing stale cursor images from being rendered multiple frames ahead.
8. Direct scanout remains active with a valid hardware cursor or hidden cursor, and exits atomically when software composition becomes necessary.

The core shape is:

```text
input / cursor surface / theme / compositor interaction
                         |
                         v
                Cursor Desired Model
      image epoch + motion epoch + visibility epoch
                         |
                         v
                 Pure Plane Scheduler
        hardware | software embedded | hidden
                         |
             +-----------+-----------+
             |                       |
             v                       v
  Replaceable Cursor Sidecar    Primary Transaction Builder
  no primary slot, latest wins  composed or direct assignment
             |                       |
             +-----------+-----------+
                         v
                   KMS Commit Bundle
        primary owner + optional cursor sidecar owner
                         |
                         v
                single KMS Commit Worker
       freeze sidecar -> TEST_ONLY if needed -> ioctl
                         |
                         v
                  exact pageflip event
                         |
                         v
               Presented Plane Snapshot
       settle all transaction owners and retire leases
```

This is the recommended architecture.

---

## 2. What the supplied Typhon already has

Typhon is not starting from a primitive cursor path. The supplied baseline already contains important pieces that must be preserved:

- `OutputTransactionId` and immutable `OutputTransaction` objects;
- `OutputTransactionContent::{Composited, Direct, CompatibilityImmediate, CursorOnly}`;
- `OutputPlanePlan` with primary, cursor, and currently rejected overlay assignments;
- `PrimaryPlaneAssignment::{CompositorFramebuffer, CompatibilityFramebuffer, ClientFramebuffer, Unchanged, Disabled}`;
- `CursorPlaneAssignment::{Atomic, Unchanged, Disabled}`;
- `KmsCommitJob` with transaction, token, generation, CRTC, primary update, cursor update, cursor framebuffer pin, direct lease, and exact payload validation;
- `AtomicCommitArbiter`, with one kernel-submitted commit and one worker-queued-next commit;
- `OutputPipelineSnapshot`, which is now the authoritative Triple Buffering 2.0 ownership read model;
- `NativeAtomicCursor`, with desired/submitted/current visual state, exact cursor epochs, worker-queued identity, framebuffer leases, and pageflip-token validation;
- `NativeCursorOutputArbitration`, which coalesces updates, gives primary work priority, and permits cursor-only submission at a refresh deadline;
- explicit handling for hardware cursor, client cursor, theme cursor, interaction override, hidden cursor, and software fallback;
- Direct Scanout 2.0 integration and a fixed three-slot composed output pool;
- a KMS Commit Worker with bounded admission, retry, shutdown, timeout, and ownership return paths.

The current design is already correct in several critical ways:

- queueing a worker job does not falsely advance the submitted cursor epoch;
- exact pageflip token and output generation are required before promotion;
- cursor framebuffer IDs are accompanied by an owned pin;
- a cursor-only transaction cannot change the primary plane;
- cursor-only work does not own compositor frame callbacks;
- primary transactions may piggyback a cursor assignment;
- EBUSY retry preserves job-owned input fences;
- direct framebuffer ownership is explicit.

The problem is not that cursor support is missing. The problem is that cursor behavior is still represented as a special subsystem beside the generalized presentation pipeline:

- `CursorOnly` exists as a special transaction content and commit kind;
- `NativeCursorOutputArbitration` mirrors scheduling policy outside the plane plan;
- desired, worker-queued, submitted, and current cursor state live inside `NativeAtomicCursor`, while the pipeline snapshot only partially represents cursor ownership;
- hardware failure is latched broadly;
- a primary job that is already queued in the worker cannot absorb a newer cursor revision without waiting another presentation opportunity;
- software cursor behavior and triple-buffer render-ahead are not governed by one explicit coupling contract;
- future overlay support would require another parallel special path if the present shape were extended directly.

Cursor and Plane Scheduling 2.0 should keep the proven ownership machinery while replacing those special cases with a coherent plane model.

---

## 3. Comparative extraction

## 3.1 KWin

### Relevant source areas

- `src/core/outputlayer.h`
- `src/core/outputlayer.cpp`
- `src/compositor.cpp`
- `src/backends/drm/drm_layer.cpp`
- `src/backends/drm/drm_pipeline.cpp`
- `src/backends/drm/drm_commit.cpp`
- `src/backends/drm/drm_commit_thread.cpp`

### What KWin does well

KWin models scanout resources as `OutputLayer` objects. A layer has a type, z-position, source rectangle, target rectangle, transform, color pipeline, hotspot, damage, and enable state. The cursor is a high-priority `CursorOnly` layer. That classification is explicit: it can only contain cursor or cursor-attached content because it may move asynchronously.

KWin separates two cursor operations:

- enabling, disabling, or repainting the cursor image remains part of normal composition and layer validation;
- moving an already valid cursor layer can use `presentAsync`, avoiding an unnecessary full scene composition.

KWin validates the combined layer configuration and uses a conservative fallback ladder:

1. ideal layer assignment;
2. composited primary plus hardware cursor;
3. composited primary only.

If disabling the cursor layer is what makes presentation succeed, KWin can quarantine the hardware cursor for that output rather than repeatedly retrying a known-bad plan.

Its DRM backend also has a powerful distinction:

- test the complete combined Atomic state;
- submit only the changed layers/properties.

The commit thread can merge ready commits, preserve order for commits touching the same planes, and test whether disjoint-plane reordering remains valid. It also adapts its safety margin from real ioctl timing.

### What Typhon should take

- A first-class plane/layer assignment model.
- Cursor as a high-priority independent plane, not a separate rendering universe.
- Full combined-state validation with partial-delta submission.
- A deterministic fallback ladder.
- The distinction between cursor image/visibility changes and position-only movement.
- Buffer ownership per plane.
- A dedicated commit worker that knows plane write sets.
- Capability quarantine after a proven configuration rejection.

### What Typhon should not copy

- Generic multi-plane commit reordering in this cycle.
- Dynamically growing pending-frame or buffer counts.
- Mutable shared pending state without Typhon transaction identity.
- Permanent output-wide cursor disable from one unclassified error.
- A broad layer allocator before Typhon actually supports overlays.

KWin is the best reference for **plane abstraction and commit composition**, but Typhon should implement a much narrower and more explicit version.

## 3.2 Hyprland and Aquamarine

### Relevant Hyprland source areas

- `src/pointer/PointerManager.hpp`
- `src/pointer/PointerManager.cpp`
- `src/output/Monitor.cpp`
- `src/output/Monitor.hpp`
- `src/render/Renderer.cpp`

### Relevant Aquamarine source areas

- `include/aquamarine/backend/DRM.hpp`
- `src/backend/drm/DRM.cpp`
- `src/backend/drm/Atomic.cpp`
- `src/backend/drm/Legacy.cpp`

### What Hyprland does well

Hyprland separates cursor image/source management from per-monitor delivery. Each monitor tracks whether software rendering is forced, whether hardware failed, whether the plane was initially cleared, whether hardware is applied, the last software-drawn rectangle, and the current cursor buffer.

Important behavior:

- it clears a cursor plane left behind by a previous display manager even if Hyprland never installed its own cursor;
- it attempts hardware delivery per output;
- it moves the hardware cursor immediately on input;
- it can skip scheduling a full frame for movement;
- it uses a dedicated cursor swapchain;
- it validates cursor size against the plane's capabilities;
- it transforms the hotspot correctly;
- it can use a CPU/dumb cursor buffer for driver-specific compatibility, notably on NVIDIA paths;
- it disables the hardware plane before software fallback;
- it damages the previous software cursor rectangle so no trail remains;
- software cursor mode blocks direct scanout, while hardware cursor movement can coexist with direct scanout;
- a cursor update during an unchanged direct-scanout buffer still causes the output state to be tested and committed.

Aquamarine tracks primary and cursor framebuffers separately and carries front/back/last references for DRM planes. Cursor position, size, hotspot, visibility, and pending framebuffer are part of connector/output state. The cursor plane supports hotspot and input-fence properties when the driver exposes them.

### What Typhon should take

- Separate cursor source/image policy from output delivery policy.
- Per-output capability and fallback state.
- Initial stale-plane clear.
- Dedicated cursor buffers and a CPU-safe normalized ARGB path.
- Size, transform, hotspot, and format checks before submission.
- Immediate position coalescing without primary rendering.
- Exact old-plus-new software cursor damage.
- Direct scanout permitted only with hidden or hardware-plane cursor delivery.
- A fallback that disables hardware before composing software.

### What Typhon should not copy

- A collection of mutable booleans as the authoritative state machine.
- A broad `hardwareFailed` latch without an exact capability key and failure class.
- Cursor update flags coupled informally to direct-scanout state.
- Backend state mutation without immutable transaction ownership.
- Delegating correctness to the backend without a Typhon-level pageflip and settlement model.

Hyprland is the best reference for **latency, practical driver compatibility, and fallback behavior**. Typhon should retain those behaviors while replacing the ad hoc state representation.

## 3.3 macOS and Apple Silicon

macOS compositor internals are proprietary. No public source establishes the exact WindowServer cursor scheduler, so this architecture must not pretend otherwise.

The useful public evidence is architectural rather than source-level:

- Core Animation separates a mutable model tree, an in-flight presentation tree, and a private render tree.
- The presentation layer represents what is currently visible, not merely the latest requested state.
- `CAMetalDisplayLink` exposes both a target presentation timestamp and a render/present deadline.
- `preferredFrameLatency` explicitly requests a one-frame or two-frame rendering window.
- Core Animation moves cached layer content through dedicated graphics hardware instead of forcing application redraw for every property change.
- Apple Silicon's DCP is a display coprocessor. Public reverse-engineering documentation shows that it can layer, move, blend, and color-transform framebuffers. A classical use is moving a static cursor framebuffer without asking the GPU to redraw the desktop.

### What Typhon should take

- Desired/model state must be separate from submitted and actually presented state.
- Scheduling should use explicit presentation targets and deadlines.
- Cached visual content should be moved by the display engine whenever possible.
- Independent visual state may advance without rerendering unrelated content.
- When two pieces of content must be coherent, they need an explicit transaction coupling contract rather than an assumption.

### What Typhon should not claim or copy

- No claim should be made that macOS uses the exact state machines or APIs proposed here.
- DCP firmware architecture should not be imitated in a userspace compositor.
- Proprietary heuristics, hidden scheduling policy, and undocumented WindowServer behavior are not reliable design inputs.

The macOS contribution is therefore a set of principles: **separate desired from presented state, use target-aware scheduling, and offload static-layer movement to display hardware**.

---

## 4. Alternatives considered

### Option A — keep the existing cursor-only path and clean it up

This is the smallest implementation. It would rename a few states, improve failure handling, and add tests around the current arbitration.

**Advantages:** low risk, small diff.
**Disadvantages:** preserves cursor as a parallel special case, does not give future planes a common model, cannot late-attach cursor work to a queued primary, and leaves software/triple coupling implicit.

### Option B — copy KWin's generic layer allocator and commit optimizer

This would create a mature generic layer graph, general commit merging, and disjoint-plane reordering.

**Advantages:** powerful and future-friendly.
**Disadvantages:** far too broad for the current single-output stage, duplicates features needed only after overlays/multi-output, increases worker concurrency risk, and weakens Typhon's current ownership clarity.

### Option C — transactional plane scheduler plus cursor sidecar

This keeps Typhon's bounded transaction model, introduces first-class plane state and write sets, and adds one narrow optimization: a latest-state cursor sidecar that can join the next Atomic commit before the worker freeze point.

**Advantages:** low cursor latency, no competing commit queue, exact ownership, direct/triple integration, future plane vocabulary without premature overlay implementation.
**Disadvantages:** requires a deliberate `KmsCommitBundle` ownership extension and a new pre-submit sidecar lifecycle.

**Decision: Option C.**

---

## 5. Goals

1. Make primary, cursor, and future overlay assignments part of one output-plane vocabulary.
2. Keep exactly one KMS submission lane.
3. Preserve one kernel-submitted plus one worker-queued-next capacity.
4. Coalesce arbitrary cursor movement into at most one replaceable pre-submit sidecar.
5. Allow the latest cursor sidecar to join a primary commit before `TEST_ONLY` and ioctl.
6. Ensure cursor work never consumes a composed output slot or primary future-depth budget.
7. Preserve exact cursor framebuffer, transaction, fence, token, generation, and pageflip ownership.
8. Keep direct scanout active with hardware or hidden cursor delivery.
9. Exit direct scanout atomically when software cursor composition is required.
10. Prevent software cursor render-ahead from producing a chain of stale cursor frames.
11. Replace global hardware-failure latching with capability-keyed quarantine and classified retry.
12. Make fallback deterministic and observable.
13. Keep the design ready for future overlays without implementing overlay promotion now.
14. Preserve all Direct Scanout 2.0 and Triple Buffering 2.0 invariants.

## 6. Non-goals

- General overlay assignment or promotion.
- Generic z-order solving.
- Generic KMS commit reordering.
- More than one cursor sidecar.
- More than one worker-queued-next KMS job.
- Mutation of a job after its worker freeze point.
- Mutation of a submitted Atomic request.
- New VRR or tearing behavior.
- Multi-output cursor crossing architecture.
- DRM hotplug.
- Zero-copy client cursor dmabuf scanout as a requirement.
- Changing XWayland Present behavior in this stage.
- Replacing the existing normalized ARGB cursor buffer path with a driver-specific direct import path.

---

## 7. Core invariants

### 7.1 Ownership

1. Every primary presentation has exactly one `OutputTransactionId`.
2. Every materialized cursor delta has exactly one cursor transaction/ticket identity.
3. A cursor framebuffer visible in desired, sidecar, queued, submitted, or presented state has an exact lease owner.
4. No framebuffer is retired while any desired cache, sidecar, worker bundle, kernel submission, or presented snapshot can reference it.
5. The worker returns all owners on every pre-submit rejection, fatal path, quiesce, shutdown, and timeout path.
6. Pageflip promotion requires exact token, output generation, CRTC, and bundle identity.
7. A mismatched or stale pageflip cannot advance primary or cursor presented state.

### 7.2 Submission

8. There is one KMS lane: one kernel-submitted bundle and at most one worker-queued-next bundle.
9. Cursor sidecar state is not a second KMS job queue.
10. At most one replaceable cursor sidecar exists outside the worker bundle.
11. The worker may claim a sidecar only before the freeze point.
12. After the freeze point, the bundle is immutable.
13. No generic commit reordering is permitted.
14. Primary work has throughput priority, but cursor work has a bounded attachment deadline.
15. If a primary bundle is available before the cursor deadline, cursor state piggybacks.
16. If no primary is available by the deadline and the lane can accept work, the sidecar becomes a plane-delta-only bundle.
17. A cursor-only plane delta never modifies, leases, or retires a primary framebuffer.

### 7.3 Visual coherence

18. Hardware cursor position may advance independently from the primary content revision only when `CursorCoupling::IndependentPlane` is active.
19. Software cursor state is embedded in an exact primary render revision and cannot advance independently.
20. Hardware and software cursor delivery are mutually exclusive for one presented state.
21. The transition from hardware to software disables the cursor plane and presents the composed cursor in the same primary transaction.
22. The transition from software to hardware presents a valid hardware assignment and a primary frame without embedded cursor duplication in the same transaction.
23. Direct scanout is eligible only with `IndependentPlane` or hidden cursor delivery.
24. A visible software cursor forces direct-scanout exit before it is shown.
25. A hidden cursor does not generate repeated KMS position commits.
26. The previous display manager's cursor plane is cleared exactly once per output generation.

### 7.4 Triple buffering

27. Cursor sidecars do not count toward future primary depth.
28. Hardware cursor movement does not allocate or render a composed slot.
29. A visible software cursor forces `ReactiveDouble` while the fallback is active.
30. A prepared but not worker-queued composed frame with stale embedded cursor state is superseded and repaired.
31. A worker-queued or kernel-submitted frame is immutable; a newer software cursor revision schedules the next reactive frame.
32. At most one stale software-cursor presentation can escape during a transition, and the next frame must contain the latest revision.

### 7.5 Failure

33. `EBUSY` is retry/defer evidence, not capability failure.
34. `EINVAL`, unsupported format/modifier/size/transform/hotspot, or a failed full-state `TEST_ONLY` may quarantine an exact capability key.
35. Quarantine is invalidated by output-generation change, CRTC/plane change, mode change, session recovery, or explicit requalification.
36. Hardware failure cannot silently hide the cursor; fallback must be software or explicitly hidden by policy.
37. Repeated failed plans cannot produce a test/submit loop on every pointer motion.

---

## 8. State model

## 8.1 Typed identities

Introduce typed wrappers rather than raw `u64` values:

```rust
pub(crate) struct PlaneStateRevision(NonZeroU64);
pub(crate) struct CursorImageEpoch(NonZeroU64);
pub(crate) struct CursorMotionEpoch(NonZeroU64);
pub(crate) struct CursorVisibilityEpoch(NonZeroU64);
pub(crate) struct CursorSidecarId(NonZeroU64);
pub(crate) struct KmsCommitBundleId(NonZeroU64);
```

A cursor revision is a product, not one overloaded epoch:

```rust
pub(crate) struct CursorRevision {
    pub(crate) image: CursorImageEpoch,
    pub(crate) motion: CursorMotionEpoch,
    pub(crate) visibility: CursorVisibilityEpoch,
}
```

This makes coalescing rules explicit:

- position-only updates replace `motion` while preserving image ownership;
- image changes advance `image` and may reuse the current motion;
- hide/show advances `visibility`;
- exact KMS equivalence can suppress a transaction even when a logical source event occurred.

## 8.2 Cursor desired model

The cursor source resolver remains responsible for choosing:

```text
InteractionOverride > ClientCursor > ThemeCursor > Hidden
```

It produces an output-local desired model:

```rust
pub(crate) struct CursorDesiredState {
    pub(crate) revision: CursorRevision,
    pub(crate) source: CursorSource,
    pub(crate) visible: bool,
    pub(crate) logical_position: Point,
    pub(crate) output_position: Point,
    pub(crate) hotspot: Point,
    pub(crate) size: Size,
    pub(crate) transform: OutputTransform,
    pub(crate) scale: u32,
}
```

This state contains no DRM framebuffer ID and no submission token. It is the model tree equivalent: the latest requested state.

## 8.3 Delivery plan

A pure planner maps desired cursor state, scene state, output capabilities, and current ownership into one delivery mode:

```rust
pub(crate) enum CursorCoupling {
    IndependentPlane,
    EmbeddedInPrimary,
    Hidden,
}

pub(crate) enum CursorDeliveryPlan {
    Hardware {
        revision: CursorRevision,
        state: AtomicCursorVisualState,
        lease: CursorFramebufferLease,
        capability_key: CursorCapabilityKey,
    },
    Software {
        revision: CursorRevision,
        snapshot: SoftwareCursorSnapshot,
    },
    Hidden {
        revision: CursorRevision,
        disable_hardware_plane: bool,
    },
}
```

The planner must be pure. It does not allocate, render, queue, test, submit, settle, or mutate the cursor.

## 8.4 Presented plane snapshot

Extend the authoritative output pipeline read model with presented cursor ownership:

```rust
pub(crate) struct PresentedCursorState {
    pub(crate) revision: CursorRevision,
    pub(crate) coupling: CursorCoupling,
    pub(crate) framebuffer_id: Option<u32>,
    pub(crate) visible: bool,
    pub(crate) output_position: Point,
    pub(crate) hotspot: Point,
}

pub(crate) struct PresentedPlaneSnapshot {
    pub(crate) revision: PlaneStateRevision,
    pub(crate) primary: Option<ConfirmedPrimaryState>,
    pub(crate) cursor: PresentedCursorState,
}
```

The existing `confirmed_primary_assignment` and `NativeAtomicCursor::current` should no longer be independent authorities. During migration, adapters may read both and assert equivalence. The final state has one authoritative presented snapshot and cursor resource ownership behind it.

## 8.5 Plane write set

```rust
bitflags! {
    pub(crate) struct PlaneWriteSet: u8 {
        const PRIMARY = 0b0001;
        const CURSOR  = 0b0010;
        const OUTPUT  = 0b0100;
    }
}
```

Do not add overlay bits until overlays are implemented. The public vocabulary may define `PlaneRole`, but runtime acceptance remains primary/cursor only.

## 8.6 Transaction kinds

Replace semantic dependence on `CursorOnly` with a general delta classification:

```rust
pub(crate) enum OutputTransactionContent {
    Composited { /* existing fields */ },
    Direct { /* existing fields */ },
    CompatibilityImmediate { /* existing fields */ },
    PlaneDelta {
        changed: PlaneWriteSet,
        cursor_sidecar_id: CursorSidecarId,
    },
}
```

During migration, `CursorOnly` may remain as a deprecated adapter. It must disappear from final runtime policy and tests.

The corresponding commit classification becomes:

```rust
pub(crate) enum AtomicCommitKind {
    Primary,
    PlaneDelta,
    Combined,
}
```

The bundle's owners, not the enum name, identify exact primary and cursor work.

---

## 9. The cursor sidecar

## 9.1 Purpose

The sidecar solves the hardest latency problem without creating a second commit queue:

- the kernel may already have one primary commit in flight;
- the worker may already hold the next primary job while waiting for that pageflip;
- pointer motion arrives after the primary job was built;
- waiting until another full primary presentation can add an avoidable refresh of cursor latency;
- mutating the queued primary transaction would violate immutability and ownership.

The answer is a separate immutable cursor transaction stored in one replaceable mailbox. The worker may combine it with the queued primary before the Atomic request is frozen.

## 9.2 Sidecar lifecycle

```text
Desired
  -> Materialized
  -> MailboxQueued
  -> ClaimedByWorker
  -> FrozenInBundle
  -> KernelSubmitted
  -> Presented
  -> Settled
```

Terminal alternatives:

```text
Materialized/MailboxQueued -> SupersededBeforeClaim
Claimed/Frozen             -> TestRejected | SubmitRejected | Quiesced
KernelSubmitted            -> Presented | TimeoutRecovery | TeardownQuarantine
```

## 9.3 Replaceable mailbox

```rust
pub(crate) enum CursorSidecarCoupling {
    Independent,
    MustBundleWith(OutputTransactionId),
}

pub(crate) struct CursorSidecar {
    pub(crate) id: CursorSidecarId,
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) revision: CursorRevision,
    pub(crate) assignment: CursorPlaneAssignment,
    pub(crate) lease: Option<CursorFramebufferLease>,
    pub(crate) coupling: CursorSidecarCoupling,
    pub(crate) created_at: MonotonicTimestampNs,
    pub(crate) deadline: PresentationTarget,
}

pub(crate) struct CursorSidecarMailbox {
    pending: Option<CursorSidecar>,
}
```

Rules:

- only a sidecar not yet claimed by the worker can be replaced;
- replacement settles the old transaction as `SupersededBeforeSubmit` and retires only its own lease;
- latest KMS-equivalent state replaces older state without creating another sidecar;
- a hidden-to-hidden position change is suppressed;
- a position-only sidecar may reuse a lease on the presented cursor framebuffer;
- image or visibility changes carry the exact new lease/state;
- an independent sidecar may become a plane-delta-only bundle at its deadline;
- a `MustBundleWith(primary_id)` sidecar can only join that exact primary transaction and can never be promoted alone;
- a coupled transition sidecar supersedes an unrelated unclaimed sidecar, while later motion coalesces into the coupled state rather than bypassing it;
- sidecars do not own compositor primary slots or frame batches.

## 9.4 Worker claim and freeze point

The worker's lifecycle gains a precise phase:

```text
DequeuedWaitingPredecessor
  -> CollectingSidecar
  -> FrozenForValidation
  -> TestOnly
  -> SubmitIoctl
  -> KernelInFlight
```

The sidecar may be claimed only in `CollectingSidecar`.

The freeze point is the transition to `FrozenForValidation`. After it:

- the bundle is immutable;
- a newer cursor sidecar remains in the mailbox for the next opportunity;
- `TEST_ONLY` validates the exact state that will be submitted;
- no framebuffer ID, hotspot, position, visibility, fence, or owner can change.

The worker must never read mutable `NativeAtomicCursor` state directly. It receives owned immutable assignments.

## 9.5 KMS commit bundle

```rust
pub(crate) struct KmsCommitBundle {
    pub(crate) id: KmsCommitBundleId,
    pub(crate) token: PageFlipToken,
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) target: PresentationTarget,
    pub(crate) primary: Option<KmsPrimaryOwner>,
    pub(crate) cursor: Option<KmsCursorOwner>,
    pub(crate) write_set: PlaneWriteSet,
    pub(crate) test_policy: KmsTestOnlyPolicy,
}

// Every changed cursor-plane property has a KmsCursorOwner. A primary
// transaction may require that owner to share its exact pageflip through
// CursorSidecarCoupling::MustBundleWith, but it never borrows an anonymous
// cursor update that cannot be returned and settled independently.
```

```rust
pub(crate) struct KmsPrimaryOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) update: KmsPrimaryUpdate,
    pub(crate) slot_or_direct_lease: PrimaryLease,
}

pub(crate) struct KmsCursorOwner {
    pub(crate) transaction: Arc<OutputTransaction>,
    pub(crate) sidecar_id: CursorSidecarId,
    pub(crate) revision: CursorRevision,
    pub(crate) update: KmsCursorUpdate,
    pub(crate) lease: Option<CursorFramebufferLease>,
}
```

A bundle may be:

- primary only;
- cursor delta only;
- combined primary plus cursor.

No vector of arbitrary owners is needed. This is intentionally narrower than KWin's generic commit merging.

## 9.6 Sidecar handoff mechanism

Use a dedicated worker-visible mailbox guarded by the same worker state mutex, not a second unbounded channel.

Required operations:

```rust
fn offer_cursor_sidecar(
    &self,
    sidecar: CursorSidecar,
) -> Result<Option<CursorSidecar>, CursorSidecarOfferError>;

fn claim_cursor_sidecar_before_freeze(
    state: &mut WorkerState,
    job_generation: u64,
    crtc_id: u32,
) -> Option<CursorSidecar>;
```

`offer_cursor_sidecar` returns the replaced sidecar to the runtime for exact settlement. It cannot silently drop ownership.

The worker may claim a sidecar for a primary bundle if:

- output generation and CRTC match;
- `MustBundleWith(id)` matches that exact primary transaction ID, while `Independent` accepts any otherwise compatible primary;
- the sidecar deadline is not after an incompatible target generation;
- the assignment is compatible with the primary plan;
- software delivery is not requested;
- the sidecar has not been quarantined;
- the worker has not crossed the freeze point.

If no primary job arrives by the deadline, runtime promotes an `Independent` mailbox entry into a cursor-delta-only bundle using the normal admission permit. A `MustBundleWith` entry instead keeps or triggers its required primary work and must never submit alone.

---

## 10. Plane scheduler

## 10.1 Inputs

```rust
pub(crate) struct PlaneSchedulingInput<'a> {
    pub(crate) desired_cursor: &'a CursorDesiredState,
    pub(crate) presented: &'a PresentedPlaneSnapshot,
    pub(crate) pipeline: &'a OutputPipelineSnapshot,
    pub(crate) scene: ScenePlaneRequirements,
    pub(crate) capabilities: &'a PlaneCapabilityCache,
    pub(crate) cursor_preference: NativeCursorPreference,
    pub(crate) direct_candidate: Option<DirectScanoutCandidateKey>,
    pub(crate) software_cursor_allowed: bool,
    pub(crate) output_generation: u64,
}
```

## 10.2 Output

```rust
pub(crate) struct PlaneSchedulingDecision {
    pub(crate) cursor: CursorDeliveryPlan,
    pub(crate) direct_scanout_compatible: bool,
    pub(crate) primary_action: PrimaryPlaneAction,
    pub(crate) cursor_action: CursorPlaneAction,
    pub(crate) pacing_constraint: CursorPacingConstraint,
    pub(crate) reason: PlaneSchedulingReason,
}
```

The decision is pure and exhaustively testable.

## 10.3 Policy order

1. Resolve explicit user policy: software, hardware, or auto.
2. Resolve hidden state before allocating/rendering any buffer.
3. Normalize cursor geometry: clip the destination to output bounds, adjust the source rectangle in fixed-point coordinates, and classify fully visible, edge-clipped, corner-clipped, and fully outside cases.
4. Validate transform, scale, hotspot, normalized buffer size, signed/property ranges, and the resulting geometry class.
5. Check the exact capability key.
6. If hardware is valid, choose independent-plane delivery.
7. If hardware is invalid or quarantined, choose software embedded delivery.
8. If software is required while direct is active, request an atomic direct-to-composed transition.
9. If software is active, force reactive double buffering.
10. If the desired state is KMS-equivalent to presented/submitted/claimed state, emit no plane action.
11. If only motion changed, reuse the current image buffer lease.
12. If image changed, ensure a prepared cursor buffer exists before materializing a sidecar.
13. If visibility changed, make enable/disable part of an exact transaction.
14. Mark cursor-plane changes that must be coherent with a primary transition as `MustBundleWith(primary_transaction_id)`.

---

## 11. Capability cache and failure classification

## 11.1 Capability key

```rust
pub(crate) struct CursorCapabilityKey {
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) plane_id: u32,
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    pub(crate) output_transform: u32,
    pub(crate) output_scale_milli: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) cursor_width: u32,
    pub(crate) cursor_height: u32,
    pub(crate) hotspot_property_available: bool,
    pub(crate) geometry_class: CursorGeometryClass,
}

pub(crate) enum CursorGeometryClass {
    FullyVisible,
    EdgeClipped,
    CornerClipped,
}

```

Driver/device identity may be stored at cache scope rather than repeated in every key.

## 11.2 Cache states

```rust
pub(crate) enum CursorCapabilityStatus {
    Unknown,
    Proven,
    Quarantined {
        reason: CursorQuarantineReason,
        failure_count: u32,
    },
}
```

## 11.3 Error classes

| Failure | Action |
| --- | --- |
| `EBUSY` before submit | Preserve owners, defer, coalesce newer motion, do not quarantine |
| Queue full/admission contention | Keep sidecar pending, do not lose epoch or lease |
| Unsupported size/format/modifier/transform | Quarantine exact key, software fallback |
| Full-state `TEST_ONLY` returns `EINVAL` | Quarantine exact key, software fallback |
| Submit returns permanent property/configuration error | Quarantine exact key after classification, software fallback |
| Transient I/O/interruption | Bounded retry or recovery; no permanent quarantine |
| Output generation/CRTC mismatch | Return owner, invalidate cache, rebuild from current output |
| Pageflip timeout | Enter existing recovery path; do not promote cursor state |
| Worker fatal/quiesce/shutdown | Return sidecar and bundle owners for deterministic settlement |

A single failure must not permanently disable all hardware cursor use for the process lifetime.

## 11.4 Test policy

Do not execute `TEST_ONLY` for every pointer movement.

Use `TEST_ONLY` when:

- the capability key is unknown;
- cursor image dimensions/format/modifier changed;
- enable/disable changes a previously unproven combined state;
- direct/composed mode changes;
- output generation or mode changed;
- a quarantined key is explicitly requalified.

Position-only motion with a proven key and the same geometry class may submit without another test. Crossing into an unproven clipped geometry class, exceeding signed/property ranges, or changing crop semantics requires validation or software fallback. A fully outside cursor is represented as hidden rather than a zero-area plane assignment.

---

## 12. Cursor buffers and synchronization

## 12.1 Buffer policy

Preserve the normalized compositor-owned ARGB8888 cursor path as the reliable default. The supplied Typhon already uses CPU-mapped DRM dumb buffers and exact framebuffer pins; this is conservative and driver-friendly.

Extract buffer ownership from the large `output/cursor.rs` into a focused pool:

```rust
pub(crate) struct CursorBufferPool {
    current: Option<CursorBuffer>,
    submitted: Option<CursorBuffer>,
    prepared: Option<CursorBuffer>,
    retired: Vec<CursorBuffer>,
    theme_cache: Option<CursorBuffer>,
    client_cache: Option<CursorBuffer>,
}
```

The conceptual capacity is three live roles: presented, submitted/claimed, and prepared. Cached buffers may exist only when their lease graph proves they are not aliased with an unsafe role.

## 12.2 Position-only updates

Position-only updates:

- do not upload or render a new cursor image;
- reuse an existing lease;
- advance only `CursorMotionEpoch`;
- may replace an unclaimed older motion sidecar;
- cannot replace an image/visibility owner already frozen in a bundle;
- do not send client buffer release or frame callback settlement by themselves.

## 12.3 Image updates

Image updates:

- normalize source pixels and transform/hotspot before materialization;
- advance `CursorImageEpoch`;
- own the exact replacement buffer lease;
- retain previous presented/submitted buffers until pageflip or terminal return proves they are unused;
- coalesce with newer motion and visibility state before claim;
- settle cursor-surface obligations only when the image transaction reaches its defined terminal state.

## 12.4 Explicit synchronization

If a cursor source has an acquire fence, the sidecar owns it. The fence must remain sidecar/bundle-owned across EBUSY retry and worker waiting.

For compositor-uploaded dumb cursor buffers, CPU write completion is known before sidecar materialization. If a future GPU cursor-buffer renderer is added, it must export an acquire fence and use the same ownership contract.

Cursor release occurs only after:

- a replacing cursor state has been presented; or
- the sidecar is terminally superseded/rejected before submit and no other owner references its buffer; or
- teardown reaches a KMS-safe boundary.

---

## 13. Scheduling policy

## 13.1 Response window

Keep the useful part of `NativeCursorOutputArbitration`: cursor motion opens one response window and newer motion coalesces into it.

Move the policy into `PlaneScheduler` plus `CursorSidecarMailbox` rather than maintaining a separate output-opportunity state machine.

The sequence is:

1. input updates desired cursor state;
2. if hardware delivery is selected and KMS state differs, materialize/replace one sidecar;
3. if primary work is being built now, attach the sidecar immediately;
4. if a primary job is already waiting in the worker, offer the sidecar mailbox so the worker can claim it before freeze;
5. if no primary opportunity consumes it by the next refresh deadline, submit a cursor-delta-only bundle;
6. if the lane is busy, keep only the latest sidecar and retry after the exact predecessor completion.

## 13.2 Priority

Priority is not “cursor always first” or “primary always first.” It is:

- never delay a ready primary solely to create a cursor-only commit;
- attach the latest cursor sidecar to that primary whenever possible;
- do not let an endless stream of primary work starve cursor updates, because every next primary before freeze is required to claim the newest compatible sidecar;
- when idle, submit the sidecar at its deadline;
- never submit two commits for cursor and primary when one combined commit can represent both.

## 13.3 Target identity

A sidecar has a deadline target, but when combined with a primary transaction it inherits the primary bundle's exact presentation target. The sidecar's original deadline remains diagnostic evidence for latency metrics.

A sidecar cannot retarget a primary transaction or change its pacing mode.

## 13.4 Commit worker timing

The worker should preserve its current predecessor wait and timing model. The new sidecar collection happens while the job is waiting and ends before validation/submission.

No cursor path may perform blocking `TEST_ONLY`, Atomic commit, or pageflip wait on the compositor/input thread.

---

## 14. Direct Scanout 2.0 interaction

## 14.1 Entering direct

A direct transaction must include or be combinable with the current cursor delivery decision.

Direct entry is legal when:

- cursor is hidden; or
- cursor is visible and hardware delivery is proven for the exact combined state.

The first direct `TEST_ONLY` must validate primary client framebuffer plus cursor assignment together. A cached primary-only direct plan is insufficient when cursor state is visible.

## 14.2 Steady direct

During steady direct scanout:

- position-only cursor updates use sidecars;
- the unchanged client primary buffer is not resubmitted as a new logical direct frame merely to move the cursor;
- a cursor sidecar may become a cursor-delta-only commit if no primary content transaction exists;
- direct surface callbacks and presentation feedback are not duplicated by cursor-only pageflips;
- direct framebuffer leases remain owned by the confirmed primary state, not by the cursor sidecar.

## 14.3 Hardware failure during direct

If hardware cursor becomes invalid while direct is active:

1. quarantine the exact hardware capability key when appropriate;
2. request a composed fallback frame containing the software cursor;
3. build one primary transaction that changes primary from client to compositor framebuffer and one cursor sidecar marked `MustBundleWith` that disables the hardware cursor plane;
4. do not show software cursor before that composed transaction is ready;
5. settle direct retirement only at the exact accepted/presented boundary already defined by Direct Scanout 2.0;
6. never present both hardware and software cursor in the same visible state;
7. never silently hide the cursor as the fallback.

## 14.4 Returning to direct

Direct may be reconsidered only after hardware cursor delivery is proven again or the cursor is hidden. The transition back must remove the embedded software cursor and install/retain the hardware cursor assignment atomically with direct primary promotion. Any required cursor enable sidecar is marked `MustBundleWith` that direct primary transaction.

---

## 15. Triple Buffering 2.0 interaction

## 15.1 Hardware cursor

Hardware cursor state is independent of primary render-ahead:

- cursor sidecars do not consume one of the three primary compositor slots;
- they do not increase `future_primary_depth`;
- a cursor delta can attach to a predictive primary without changing its target;
- Triple Buffering 2.0 may remain active while the hardware cursor is visible;
- cursor-only commits are represented in the KMS lane snapshot but excluded from primary depth.

## 15.2 Software cursor

Software cursor state is embedded in the primary framebuffer. The safest low-latency rule is:

```text
visible software cursor => ReactiveDouble
```

This is deliberate. Rendering multiple future primary frames with a cursor position captured too early creates visible lag and complex repair semantics. Hardware cursor is the optimized path; software cursor is the correctness fallback.

When software mode begins:

- adaptive triple buffering receives `SoftwareCursorVisible` as a capability blocker;
- any prepared but unqueued predictive frame with stale cursor state is cancelled/superseded using existing exact transaction settlement;
- the next frame damages the previous and new cursor rectangles;
- already queued/submitted frames are immutable;
- the next reactive frame carries the newest cursor revision.

When software mode ends, Triple Buffering 2.0 may re-enter through its normal hysteresis rather than immediately forcing predictive mode.

## 15.3 Why no late GPU cursor latch in this cycle

A late software-cursor pass into a prepared primary framebuffer could reduce fallback latency, but it would require reopening render fences, buffer mutability, damage history, and worker readiness after the primary transaction was already prepared. That is a separate rendering architecture and would weaken current explicit-sync guarantees.

It is intentionally rejected for this cycle. The best system here is the most correct bounded system, not the largest feature set.

---

## 16. Software cursor rendering contract

1. Damage the union of the old and new cursor rectangles.
2. Clip damage to output bounds.
3. Include scale, transform, and hotspot in the cursor snapshot identity.
4. Draw the cursor exactly once in the scene.
5. Disable/hide the hardware plane in the same transaction that first embeds software cursor pixels.
6. Remove old software cursor damage when returning to hardware or hidden mode.
7. A client cursor surface frame callback must continue to progress under both hardware and software delivery.
8. Position-only software movement schedules visual work but does not create cursor-sidecar KMS work.
9. Software cursor cannot be considered presented until its exact primary transaction pageflips.
10. The presented cursor revision for software mode is promoted from the primary transaction, not from independent cursor state.

---

## 17. Initial plane clearing and recovery

On every new output generation:

1. treat the kernel's existing cursor plane state as unknown;
2. issue exactly one explicit disable/clear assignment before considering the plane synchronized;
3. do not assume that Typhon was the previous DRM master;
4. record `initial_cursor_plane_cleared` only after exact successful pageflip or proven initial modeset state;
5. reset capability cache entries tied to the previous generation;
6. rebuild buffer/framebuffer ownership;
7. reconcile desired state after session recovery;
8. never reuse a framebuffer ID or plane proof from the old generation.

A seat suspend pauses admission and preserves logical desired state. Resume creates a new generation, invalidates KMS proofs, clears unknown plane state, and materializes the latest desired cursor again.

---

## 18. Protocol obligations

Cursor and primary protocol work must remain distinct even when one KMS bundle submits them together.

### Primary owner

Owns the existing:

- compositor frame batch;
- presentation feedback;
- direct surface identity;
- primary buffer release/retirement;
- damage-history advancement;
- pacing accounting.

### Cursor owner

May own:

- cursor surface image commit identity;
- cursor buffer lease/release;
- cursor image presentation timestamp;
- cursor-specific frame callback opportunity if Typhon chooses to represent it explicitly.

Position-only cursor motion owns no new client buffer release.

A combined pageflip settles each owner according to its own obligations. Combining KMS state must never merge or discard logical obligations.

---

## 19. Observability

Add structured fields and counters rather than debug-only prose.

### State identity

- `plane_snapshot_revision`
- `cursor_image_epoch`
- `cursor_motion_epoch`
- `cursor_visibility_epoch`
- `cursor_sidecar_id`
- `kms_bundle_id`
- `primary_transaction_id`
- `cursor_transaction_id`
- `output_generation`
- `pageflip_token`

### Scheduling

- `cursor_response_windows_opened`
- `cursor_updates_coalesced`
- `cursor_sidecars_materialized`
- `cursor_sidecars_replaced`
- `cursor_sidecars_claimed_by_primary`
- `cursor_sidecars_promoted_to_delta`
- `cursor_sidecars_missed_freeze_point`
- `cursor_primary_piggybacks`
- `cursor_delta_only_submissions`
- `cursor_deadline_misses`

### Latency

- input event to desired state;
- desired state to sidecar materialization;
- sidecar materialization to worker claim;
- worker claim to ioctl;
- ioctl to pageflip;
- total input-to-presented cursor age;
- cursor deadline lateness.

### Capability/fallback

- capability cache hit/miss;
- first full-state test;
- quarantine by exact reason;
- quarantine invalidation;
- EBUSY deferral;
- software fallback entry/exit;
- direct exit caused by cursor;
- initial plane clear attempts/successes;
- hardware/software duplicate-prevention assertions.

### Triple/direct

- sidecar excluded from primary depth;
- software cursor triple blocker active;
- prepared frame superseded for software cursor revision;
- direct steady cursor update;
- direct-to-composed cursor transition.

The presentation trace ring should include both transaction owners for a combined bundle.

---

## 20. Configuration

Preserve existing user-facing policy:

```text
OBLIVION_ONE_CURSOR=auto|hardware|software
OBLIVION_ONE_CURSOR_SCHEDULING=auto|piggyback|software
```

Recommended final semantics:

- `OBLIVION_ONE_CURSOR=auto`: hardware when proven, software fallback.
- `hardware`: require hardware; if impossible, log a prominent failure and use software only if current project policy already promises fallback. Do not silently hide.
- `software`: never enable the hardware plane.
- scheduling `auto`: sidecar piggyback plus deadline delta.
- scheduling `piggyback`: diagnostic mode that still permits deadline delta when idle, matching current non-starvation behavior.
- scheduling `software`: force embedded software delivery.

Add only diagnostic toggles, not permanent product knobs:

```text
OBLIVION_ONE_CURSOR_PLANES_TRACE=0|1
OBLIVION_ONE_CURSOR_REQUALIFY=0|1
```

Do not expose sidecar capacity or arbitrary queue depth; both remain fixed at one.

---

## 21. File and module architecture

The current cursor and presentation runtime files are already large. Do not add the whole feature to `runtime/presentation.rs`, `runtime/kms_worker.rs`, or `output/cursor.rs`.

### Create

`src/native_output/presentation/plane.rs`

- typed plane revisions;
- `PlaneWriteSet`;
- `CursorCoupling`;
- plane snapshot and assignment vocabulary;
- pure validation helpers.

`src/native_output/presentation/plane_policy.rs`

- `PlaneSchedulingInput`;
- `PlaneSchedulingDecision`;
- pure hardware/software/hidden policy;
- direct/triple compatibility decision;
- capability failure classification.

`src/native_output/output/cursor_buffer.rs`

- cursor buffer allocation/upload/cache;
- framebuffer leases;
- safe retirement;
- no scheduling policy.

`src/native_output/output/cursor_state.rs`

- desired, sidecar-claimed, submitted, and presented cursor state;
- typed epochs;
- exact transition validation;
- no DRM ioctl.

`src/native_output/kms_worker/cursor_sidecar.rs`

- one replaceable mailbox;
- offer/replace/claim/freeze semantics;
- ownership return types;
- concurrency tests.

`src/native_output/kms_worker/bundle.rs`

- `KmsCommitBundle`;
- primary and cursor owners;
- payload validation;
- exact returned/submitted ownership.

`src/native_output/runtime/plane_cycle.rs`

- materialize plane decisions;
- create/replace sidecars;
- promote idle sidecar to delta bundle;
- handle combined completion and fallback transitions.

`src/native_output/tests/plane_scheduling_model.rs`

- deterministic state-machine/model tests across small exhaustive state spaces.

### Modify

`src/native_output/presentation/mod.rs`

- export plane and policy modules.

`src/native_output/presentation/transaction.rs`

- introduce `PlaneDelta`;
- carry plane write set and typed cursor revision;
- preserve immutable ownership and protocol obligations;
- deprecate then remove `CursorOnly`.

`src/native_output/presentation/pipeline.rs`

- add authoritative presented cursor/plane snapshot;
- represent combined bundle ownership;
- ensure cursor sidecars do not count as primary depth.

`src/native_output/presentation/ledger.rs`

- support combined bundle settlement and `SupersededBeforeSubmit` for cursor sidecars.

`src/native_output/output/cursor.rs`

- retain cursor source resolution and image conversion facade;
- delegate buffer and state ownership to new focused modules;
- remove broad failure latch.

`src/native_output/runtime/frame.rs`

- remove final authority from `NativeCursorOutputArbitration`;
- keep only compatibility adapters until migration completes.

`src/native_output/runtime/cursor_cycle.rs`

- retain source/path resolution and software damage helpers;
- move KMS plane scheduling to `plane_cycle.rs`;
- remove cursor-only completion special cases after bundle migration.

`src/native_output/runtime/atomic_commit.rs`

- track bundle identity and write set;
- route combined pageflip completion;
- preserve one kernel plus one queued-next invariant.

`src/native_output/runtime/kms_worker.rs`

- orchestrate bundle results, not build sidecar policy inline;
- handle sidecar return/retry/fallback;
- remain below source-layout limit by extracting logic.

`src/native_output/kms_worker/payload.rs`

- migrate to bundle validation;
- validate all transaction/lease/assignment identities.

`src/native_output/kms_worker/queue.rs`

- store one sidecar mailbox alongside, not inside, KMS queue capacity;
- return mailbox owner on quiesce/shutdown/fatal.

`src/native_output/kms_worker/thread.rs`

- add collect/freeze phase;
- claim compatible sidecar before test;
- report combined submitted ownership.

`src/native_output/runtime/presentation.rs`

- call plane-cycle interfaces only;
- remove duplicated cursor branching rather than growing the file.

`src/native_output/runtime/presentation_transactions.rs`

- build primary, plane-delta, and combined transaction owners;
- preserve callback/release settlement.

`src/native_output/runtime/presentation_pipeline.rs`

- derive snapshot from authoritative owners.

`src/native_output/scanout/atomic_direct.rs`

- include cursor capability key in direct plan validation where required.

`src/native_output/scanout/direct_validation.rs`

- enforce hidden/hardware cursor contract and software fallback reason.

`src/native/adaptive_buffering.rs`

- add `SoftwareCursorVisible` blocker.

`src/native_output/runtime/metrics.rs` and presentation metrics modules

- add sidecar, plane, latency, and fallback metrics.

### Tests to extend

- `src/native_output/output/cursor_tests.rs`
- `src/native_output/runtime/kms_worker_tests.rs`
- `src/native_output/kms_worker/tests.rs`
- `src/native_output/tests/frame.rs`
- `src/native_output/tests/presentation_transactions.rs`
- `src/native_output/tests/triple_buffering_model.rs`
- direct scanout tests under `src/native_output/scanout/`

---

## 22. Implementation stages

### Stage 0 — behavioral oracle

- Add tests that capture current hardware, software, direct, cursor-only, piggyback, EBUSY, and pageflip behavior.
- Record existing metrics and qualification commands.
- No behavior change.

### Stage 1 — plane vocabulary and snapshots

- Add typed revisions, write sets, coupling, presented plane snapshot, and pure validation.
- Adapt existing transaction/commit kinds without deleting old paths.
- Assert legacy and new snapshot equivalence in tests.

### Stage 2 — cursor state and buffer extraction

- Split buffer ownership from cursor policy.
- Replace raw epoch with image/motion/visibility epochs.
- Preserve exact worker queue/submission/pageflip tests.
- Introduce capability key and classified quarantine.

### Stage 3 — pure plane scheduler

- Implement hardware/software/hidden decision matrix.
- Add exhaustive tests for direct, triple, preference, failure, hidden, and geometry combinations.
- No worker sidecar yet; adapt current piggyback path to consume the decision.

### Stage 4 — KMS bundle ownership

- Replace single-owner job assumptions with primary plus optional cursor owner.
- Validate and return all leases/transactions on every outcome.
- Preserve one KMS queue and exact arbiter limits.

### Stage 5 — replaceable cursor sidecar

- Add mailbox, replacement settlement, worker collect/freeze phase, and combined submit result.
- Use existing dequeue pause hooks for deterministic race tests.
- Keep sidecar capacity exactly one.

### Stage 6 — plane-delta-only idle submission

- Replace `CursorOnly` runtime policy with `PlaneDelta` promotion at deadline.
- Remove old arbitration authority.
- Verify no primary slot/depth consumption.

### Stage 7 — direct and triple integration

- Validate direct plus hardware cursor combined state.
- Implement atomic direct-to-composed software fallback.
- Add software cursor triple blocker and prepared-frame repair.

### Stage 8 — recovery, shutdown, and cleanup

- Return mailbox sidecars during quiesce, shutdown, fatal worker, seat loss, and generation change.
- Clear unknown inherited cursor plane once per generation.
- Remove deprecated cursor-only paths and duplicate state.

### Stage 9 — observability and qualification

- Add metrics and trace fields.
- Run deterministic gates.
- Run real TTY/DRM matrix.
- Keep feature default behavior unchanged until qualification passes.

---

## 23. Deterministic test matrix

### Plane policy

- hidden cursor chooses hidden delivery without buffer allocation;
- auto chooses hardware for proven capability;
- auto chooses software for quarantined exact key;
- hardware size overflow falls back deterministically;
- transform/hotspot mismatch is classified;
- center, every edge, every corner, negative logical position, and fully outside geometry normalize correctly;
- software mode blocks direct and predictive triple;
- hardware mode permits direct and predictive triple;
- position-only update reuses image epoch and lease;
- image-only update preserves motion epoch;
- visibility-only update preserves image ownership.

### Sidecar mailbox

- first offer stores one sidecar;
- newer motion replaces older unclaimed sidecar;
- replacement returns the exact old owner;
- KMS-equivalent update creates no replacement;
- worker claim removes exact sidecar once;
- generation/CRTC mismatch refuses claim and returns nothing silently;
- sidecar arriving before freeze joins primary;
- sidecar arriving after freeze remains pending;
- no primary by deadline promotes sidecar to delta bundle;
- repeated motion while lane busy remains capacity one;
- no starvation under continuous primary transactions.

### Bundle validation

- primary-only bundle validates;
- cursor-only delta bundle cannot modify primary;
- combined bundle retains both transaction IDs;
- cursor framebuffer requires exact lease;
- direct framebuffer requires exact direct lease;
- duplicate/mismatched IDs reject before ioctl;
- test rejection returns both owners;
- submit rejection returns both owners;
- EBUSY preserves both owners and fences;
- pageflip promotes both exact owners;
- stale pageflip promotes neither.

### Buffer lifetime

- replacing image while old sidecar waits keeps old lease until returned;
- replacing image while old bundle is frozen keeps both buffers;
- pageflip retires only no-longer-referenced buffers;
- shutdown without KMS-safe boundary quarantines resources;
- session recovery invalidates old framebuffer IDs;
- current framebuffer position-only lease remains alive.

### Direct scanout

- direct plus hidden cursor succeeds;
- direct plus proven hardware cursor succeeds;
- direct plus software cursor rejects eligibility;
- hardware failure during direct creates one composed transition;
- transition disables hardware and embeds software exactly once;
- cursor delta during unchanged direct primary does not duplicate direct callbacks;
- returning to direct removes software duplication atomically.

### Triple buffering

- hardware sidecars do not change future primary depth;
- hardware cursor remains compatible with predictive triple;
- software cursor activates `ReactiveDouble` blocker;
- stale prepared software-cursor frame is cancelled and repaired;
- worker-queued primary remains immutable;
- latest software cursor appears in the next reactive frame;
- triple re-entry follows normal hysteresis after hardware recovery.

### Recovery and shutdown

- inherited plane clear occurs once per generation;
- seat suspend stops sidecar admission;
- resume invalidates capability proofs and rematerializes latest desired state;
- quiesce returns queued job and pending sidecar;
- fatal worker returns/quarantines exact owners;
- shutdown cannot leak cursor framebuffer leases;
- duplicate pageflip ack remains harmless and observable.

### Model tests

Create a deterministic small-state explorer over:

- primary lane: idle, kernel submitted, worker queued next;
- sidecar: none, mailbox, claimed, frozen, submitted;
- cursor mode: hidden, hardware, software;
- direct state: composed, direct;
- outcome: success, EBUSY, test reject, submit reject, stale pageflip, exact pageflip, quiesce.

For every reachable transition assert:

- one owner per lease;
- no capacity overflow;
- no forbidden direct/software combination;
- no double hardware/software cursor;
- no terminal transaction remains in a live bundle;
- no presented revision advances without exact pageflip;
- no cursor sidecar increases primary depth.

---

## 24. Real qualification matrix

Run on the real TTY/DRM environment after deterministic gates pass.

### Applications

- Kitty;
- Firefox;
- Steam client;
- Palworld;
- at least one additional Vulkan game;
- one Wayland-native client with custom animated cursor;
- one XWayland game/client after confirming this stage does not regress current XWayland behavior.

### Modes

- composed fullscreen;
- direct scanout eligible fullscreen;
- forced direct off;
- Triple Buffering `off`, `auto`, and `force` where valid;
- cursor `auto`, `hardware`, and `software`;
- rapid cursor shape changes;
- rapid movement during continuous game frames;
- idle desktop movement;
- cursor hide/show;
- client cursor enter/leave;
- compositor resize/move interaction cursor;
- VT switch/session suspend and resume;
- orderly shutdown while cursor and primary work are queued.

### Acceptance thresholds

- zero KMS queue overflows caused by cursor sidecars;
- zero cursor transaction/epoch/token/lease mismatches;
- zero duplicate hardware-plus-software cursor frames;
- zero cursor trails in software mode;
- zero hidden-cursor stalls after fallback;
- zero direct frame callback duplication from cursor-delta pageflips;
- zero leaked cursor framebuffer leases at shutdown;
- callbacks requested equal callbacks completed for tested clients;
- no unpublished cursor-owned buffer obligation after terminal settlement;
- hardware cursor p95 input-to-present age no worse than one refresh plus the measured worker/driver safety budget under steady load;
- no unbounded cursor latency under continuous primary work;
- direct scanout remains stable with hardware cursor movement;
- software fallback exits direct once and does not oscillate;
- Triple Buffering remains active with hardware cursor and becomes ReactiveDouble with visible software cursor;
- no regression in primary frame cadence versus the Triple Buffering 2.0 baseline.

Metrics must be compared against a baseline run, not judged only by visual impression.

---

## 25. Rejected shortcuts

The implementation must not:

- add another cursor commit thread;
- perform Atomic ioctl on the input/main thread;
- mutate `KmsCommitJob` after `TEST_ONLY` starts;
- overwrite a sidecar without returning/settling the old owner;
- mark a cursor epoch submitted when merely queued;
- advance current/presented state on worker success before pageflip;
- treat EBUSY as hardware capability failure;
- globally disable hardware cursor forever after one error;
- run `TEST_ONLY` on every mouse movement;
- resubmit the same direct primary buffer as a new logical frame only to move the cursor;
- let cursor work consume a primary swapchain slot;
- permit software cursor with active direct scanout;
- render software and hardware cursor simultaneously;
- add generic overlays or commit reordering in this cycle;
- grow already near-limit runtime files instead of extracting focused modules;
- claim real TTY/DRM success without running it.

---

## 26. First review — architecture and ownership

The first review examined ownership, abstraction boundaries, and whether the design accidentally imported too much KWin complexity.

### Finding 1: a generic commit merger was too broad

An initial concept allowed arbitrary plane-disjoint commit merging and reordering. That would reproduce KWin's mature commit optimizer before Typhon has overlays or multi-output.

**Correction integrated:** the final design permits only one narrow operation: attach one cursor sidecar to the next compatible bundle before freeze. No generic reordering exists.

### Finding 2: late mutation would violate transaction immutability

Directly editing a worker-queued primary transaction with newer cursor state would make its plane plan, lease set, and trace identity mutable.

**Correction integrated:** cursor work remains its own immutable transaction. `KmsCommitBundle` combines owners physically while preserving logical identities. The worker has a precise freeze point.

### Finding 3: one cursor epoch hid incompatible replacement rules

A single epoch could not express whether a newer update was motion-only, replaced the image buffer, or changed visibility.

**Correction integrated:** separate image, motion, and visibility epochs define legal coalescing and lease behavior.

### Finding 4: “matching cursor and primary epochs” was too strict

A hardware cursor is intentionally independent from the primary plane. Requiring equal revisions would destroy the latency benefit.

**Correction integrated:** `CursorCoupling` defines when independent revisions are legal. Software cursor remains exactly coupled to a primary render revision.

### Finding 5: global hardware failure latch was too coarse

A permanent process-wide latch could turn one mode/size/driver failure into a lasting software fallback.

**Correction integrated:** capability-keyed quarantine with explicit invalidation and transient/permanent error classification.

### Finding 6: future overlay vocabulary risked premature implementation

A generic layer graph could expand the stage indefinitely.

**Correction integrated:** plane vocabulary is future-compatible, but accepted write sets are primary/cursor only and overlays remain a hard non-goal.

### Finding 7: software cursor with predictive triple created stale render-ahead

Keeping triple active during software fallback would allow cursor positions to be captured too far ahead or require unsafe late framebuffer mutation.

**Correction integrated:** visible software cursor is a Triple Buffering capability blocker and forces ReactiveDouble.

### First-review verdict

After these corrections, the architecture remains ambitious but bounded. Each new owner has a single purpose, and the KMS lane stays finite and auditable.

---

## 27. Second review — adversarial runtime and driver behavior

The second review treated the architecture as hostile to timing, driver, shutdown, and direct/triple edge cases.

### Scenario 1: cursor arrives while worker waits for predecessor pageflip

Without a sidecar, it waits an avoidable extra refresh. With unrestricted mutation, ownership breaks.

**Final rule:** worker claims the latest compatible sidecar in `CollectingSidecar`, then freezes the bundle before validation.

### Scenario 2: cursor arrives after worker validation begins

Applying it would mean `TEST_ONLY` did not validate submitted state.

**Final rule:** after `FrozenForValidation`, newer state remains in the mailbox for the next bundle.

### Scenario 3: thousands of pointer motions while a commit is blocked

An unbounded queue would grow and submit stale positions.

**Final rule:** mailbox capacity is one; unclaimed motion sidecars are replaced with exact settlement and latest state wins.

### Scenario 4: cursor image is replaced while the old image is frozen

Dropping the old buffer could leave KMS referencing a dead framebuffer.

**Final rule:** frozen bundle owns an exact lease; new image uses a separate prepared lease; retirement waits for pageflip/return.

### Scenario 5: EBUSY on a valid cursor commit

Treating it as failure would oscillate to software unnecessarily.

**Final rule:** EBUSY preserves owners/fences, defers through the existing worker policy, and never quarantines capability.

### Scenario 6: permanent `EINVAL` while direct scanout is active

Simply disabling hardware would make the cursor disappear; simply drawing software would conflict with direct scanout.

**Final rule:** quarantine exact key and schedule one atomic direct-to-composed transaction that disables hardware and embeds software cursor.

### Scenario 7: old display manager left a cursor plane enabled

Assuming the initial state is disabled can display a stale cursor.

**Final rule:** one generation-scoped initial clear must be confirmed before state is trusted.

### Scenario 8: session generation changes with sidecar or submitted bundle alive

Old framebuffer IDs and capability proofs are invalid.

**Final rule:** stop admission, return/quarantine owners through the existing KMS-safe teardown boundary, invalidate cache, clear unknown plane state, and rematerialize desired cursor.

### Scenario 9: shutdown while mailbox and queued primary exist

A sidecar outside normal queue accounting could leak.

**Final rule:** sidecar mailbox is part of `WorkerShared` shutdown snapshot and must be returned with queued job ownership.

### Scenario 10: stale or duplicate pageflip event

Promoting either plane would corrupt presented state.

**Final rule:** bundle ID, token, generation, and CRTC must all match; otherwise neither owner is promoted.

### Scenario 11: endless primary frames starve cursor-only deadline

A low-priority standalone cursor path could starve forever.

**Final rule:** every compatible primary bundle before freeze must claim the newest sidecar. Standalone delta is needed only when there is no suitable primary.

### Scenario 12: hidden cursor receives movement

Submitting hidden position changes wastes commits and may wake VRR/display paths in the future.

**Final rule:** hidden-to-hidden movement updates logical desired state but does not materialize KMS work.

### Scenario 13: `TEST_ONLY` on every move destroys latency

A literal full-state validation policy would be correct but slow.

**Final rule:** test capability transitions and unknown keys, cache proven position-only movement, and invalidate by exact output changes.

### Scenario 14: coupled hardware/software transition submits its cursor delta alone

A one-entry mailbox could incorrectly promote the hardware-disable sidecar before the composed fallback frame, briefly hiding the cursor, or attach a hardware-enable sidecar to the wrong primary.

**Final rule:** coherence-critical sidecars use `MustBundleWith(primary_transaction_id)`. They never promote alone and the worker validates the exact required primary ID before claim.

### Scenario 15: cursor crosses an output edge after a centered capability proof

A driver may accept a centered cursor but reject negative destination coordinates or a different crop shape. Reusing one global proof could turn edge movement into repeated submit failure.

**Final rule:** normalize source/destination geometry, classify visible/edge/corner states, include geometry class in the capability proof, and hide fully outside cursors.

### Scenario 16: combined bundle failure loses one logical owner

A primary-centric error path could return only the primary job and leak cursor transaction/lease state.

**Final rule:** all worker events carry complete `KmsSubmittedOwnershipBundle`; test, submit, busy exhaustion, quiesce, fatal, and timeout paths must return both owner sets.

### Second-review verdict

The final design has explicit answers for all identified races. Its main implementation risk is the bundle/sidecar ownership migration, so that work must be staged behind deterministic tests before old cursor-only state is removed.

---

## 28. Final acceptance decision

Cursor and Plane Scheduling 2.0 is complete only when:

- cursor is represented through the shared plane model;
- old cursor-only scheduling is no longer an independent authority;
- one replaceable sidecar can join a queued primary before freeze;
- all sidecar and bundle owners are returned on every failure path;
- hardware failure uses exact capability quarantine;
- software fallback is atomically coupled to primary composition;
- direct and triple behavior follow the stated contracts;
- deterministic model tests pass;
- formatting, source-layout, check, clippy, tests, release build, and diff gates pass;
- real TTY/DRM qualification is run and reported honestly;
- no regression is observed in Palworld, Steam, Firefox, Kitty, and another Vulkan game.

The architecture deliberately chooses bounded correctness over generic cleverness. It extracts the best proven ideas from KWin, Hyprland/Aquamarine, and Apple's public architecture, but keeps Typhon's strongest trait: every displayed buffer, plane state, callback, fence, and pageflip has an exact owner.

---

## 29. Source index

### Typhon baseline

- `src/native_output/presentation/transaction.rs`
- `src/native_output/presentation/pipeline.rs`
- `src/native_output/presentation/ledger.rs`
- `src/native_output/runtime/frame.rs`
- `src/native_output/runtime/cursor_cycle.rs`
- `src/native_output/runtime/atomic_commit.rs`
- `src/native_output/runtime/kms_worker.rs`
- `src/native_output/runtime/presentation.rs`
- `src/native_output/runtime/presentation_transactions.rs`
- `src/native_output/output/cursor.rs`
- `src/native_output/kms_worker/payload.rs`
- `src/native_output/kms_worker/queue.rs`
- `src/native_output/kms_worker/thread.rs`
- `src/native_output/scanout/atomic_direct.rs`
- `src/native_output/scanout/direct_validation.rs`
- `TYPHON_TRIPLE_BUFFERING_2_ARCHITECTURE.md`

### KWin archive

- `src/core/outputlayer.h`
- `src/core/outputlayer.cpp`
- `src/compositor.cpp`
- `src/backends/drm/drm_layer.cpp`
- `src/backends/drm/drm_pipeline.cpp`
- `src/backends/drm/drm_commit.cpp`
- `src/backends/drm/drm_commit_thread.cpp`

### Hyprland archive

- `src/pointer/PointerManager.hpp`
- `src/pointer/PointerManager.cpp`
- `src/output/Monitor.cpp`
- `src/output/Monitor.hpp`
- `src/render/Renderer.cpp`
- `flake.lock`

### Aquamarine pinned by Hyprland

- `include/aquamarine/backend/DRM.hpp`
- `src/backend/drm/DRM.cpp`
- `src/backend/drm/Atomic.cpp`
- `src/backend/drm/Legacy.cpp`

### Apple and Apple Silicon public references

- Apple Developer Documentation: Core Animation.
- Apple Core Animation Programming Guide: model, presentation, and render trees.
- Apple Developer Documentation: `CAMetalDisplayLink`, `targetTimestamp`, `targetPresentationTimestamp`, and `preferredFrameLatency`.
- Asahi Linux documentation: Apple Silicon Display Controllers and DCP.
- Asahi Linux Linux 6.19 progress report: DCP plane handling and the classical cursor-overlay use case.

---

## 30. Implementation record

The implementation follows the architecture with one bounded KMS worker lane, one replaceable cursor sidecar mailbox, immutable pre-validation freeze, typed cursor/plane identities, bounded primary-plus-cursor bundle ownership, `PlaneDelta` transactions, exact cursor capability keys, and an authoritative presented-plane snapshot.

The source was decomposed by responsibility:

- `presentation/plane.rs` owns typed identities, write sets, cursor coupling, and presented snapshots;
- `presentation/plane_policy.rs` owns the pure scheduler, geometry normalization, failure classification, and capability cache;
- `output/cursor_buffer.rs` owns framebuffer allocation, pinning, cache, and retirement;
- `output/cursor_state.rs` owns desired, queued, submitted, presented, and generation-scoped clear state;
- `kms_worker/bundle.rs` owns the bounded physical bundle identity and logical owners;
- `kms_worker/cursor_sidecar.rs` owns the one-entry mailbox and coupling rules;
- `runtime/plane_cycle.rs` owns sidecar materialization and runtime scheduling adapters.

Local naming intentionally retains `CursorFramebufferPin` for the cursor lease and the existing composed/direct primary lease types. `KmsCursorOwner` stores the immutable logical descriptor, typed revision, and optional sidecar identity while the send-only framebuffer pin remains on the immutable job payload; validation proves that the assignment and pin name the same framebuffer. This preserves the existing DRM resource registry and direct-primary lease semantics without erasing ownership.

The old `CursorOnly` transaction and commit authorities were removed. Idle cursor work is represented as `PlaneDelta { changed, cursor_sidecar_id }`; cursor-only write-set validation rejects primary mutation and protocol obligations. Hardware cursor sidecars never consume primary slots or future-primary depth. Visible software cursor adds the exact `SoftwareCursorVisible` triple-capability blocker and forces `ReactiveDouble`.

The deterministic suite covers the 384-case pure policy matrix, a 600-case bounded lane/sidecar state explorer, exact pageflip promotion, cursor lease reuse and retirement, direct-scanout compatibility, EBUSY classification, sidecar replacement/coupling, pre-freeze and post-freeze races, quiesce, shutdown, and stale pageflips. Worker metrics expose sidecar materialization, replacement, claim, and missed-freeze counts in addition to plane-delta, piggyback, queue, ioctl, pageflip, capability, and fallback diagnostics.

The repository did not contain `TYPHON_TRIPLE_BUFFERING_2_ARCHITECTURE.md` at the supplied baseline or in reachable history. Triple Buffering 2.0 behavior was therefore verified against the authoritative implementation, fixed three-slot pool, pipeline model tests, and baseline commit history rather than that missing document.

Real TTY/DRM qualification requires a DRM master session and the listed applications. Unit/model tests do not substitute for that acceptance gate; when such access is unavailable, qualification is reported as `not run`.
