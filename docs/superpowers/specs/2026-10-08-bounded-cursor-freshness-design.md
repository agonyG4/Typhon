# Bounded Cursor Freshness Design

## Goal

Give a pending hardware cursor a bounded path to a legal KMS service opportunity during continuous unready scene activity, while preserving ready primary frames, transaction ownership, and bounded KMS admission.

## Current source evidence

The live CodebaseMCP index was refreshed against `main` at `13eeac181522f3c86a3d5de9ec2ead431d55b8ed`. It shows that `NativeCursorOutputArbitration::disposition` chooses `PiggybackPrimary` before checking its deadline, and the 1,024-epoch regression expects perpetual piggybacking. The `Auto` and `Piggyback` values are accepted but ignored by that helper. Cursor deadlines are seeded from `next_refresh_deadline_ns`; liveness, epoch settlement, and retry paths can seed or replace that deadline the same way.

The presentation cycle currently selects cursor-only targets from `scheduled_presentation_target` or `reactive_target[_after]` using full scene `prediction.total_cost_ns`. `PresentationTarget::authority()` distinguishes advisory `ReactiveDouble` targets from reserved targets. `KmsSubmitWindow` supplies worker wake, earliest submit, commit-complete deadline, dispatch budget, and apply guard. The existing runtime wake plan consumes the cursor deadline, and the KMS worker honors `worker_wake_at_ns`. Worker queue capacity is one; cursor sidecars are latest-wins until primary freeze; async/tearing validation and one `AtomicCommitArbiter` remain authoritative.

`run_cycle` drains pageflip/input and checks session/shutdown/output transitions before `process_acquire_and_prepare`, but that preparation happens before the later presentation-cycle cursor decision. Because preparation can perform non-interruptible scene work, Auto needs a small pre-prepare cursor-service checkpoint when its KMS service deadline has matured and there is no ready, attachable primary carrier.

## Design

Keep the existing cursor arbitration, presentation target planner, KMS timing model, atomic commit arbiter, single-slot worker, sidecar mailbox, output transaction ledger, and pageflip evidence as the only authorities. Extend cursor arbitration to retain absolute pending age and a KMS-derived service deadline separately from a transport retry deadline. Coalesced motion replaces only the desired epoch. A retry changes only `retry_not_before_ns`; it does not reopen the primary response window or move the service deadline.

Auto plans a physical cursor opportunity from the existing presentation clock and KMS timing. It uses zero scene-render cost, with a separately bounded reactor transaction-preparation guard. It observes actual primary carrier readiness and ownership. An attachable ready primary carries the latest cursor sidecar. A binding primary reservation remains owned even while rendering is incomplete; an independent cursor target must follow it. Advisory scheduled visual targets do not claim ownership. Cursor-only admission uses the existing arbiter and worker and is limited to one independent service per physical opportunity.

If a matured Auto opportunity can be endangered by primary acquire/prepare work, a checkpoint runs after pageflip drain, input dispatch, output/session/shutdown checks, and before expensive acquire/prepare. It only admits or concretely defers a cursor transaction; it performs no scene rendering or effects. Normal primary preparation then continues. The checkpoint is bounded to one service attempt per opportunity, so high-rate motion cannot spin the reactor or monopolize KMS admission.

## Policy semantics

- **Auto:** allow a short wait for a legal primary carrier; once the service opportunity matures, use an independent PlaneDelta if no legal carrier owns that opportunity and KMS admission permits it. Preserve debt under backpressure.
- **Piggyback:** retain primary-first behavior for the same-binary A/B baseline; primary work continues to select piggyback and does not inherit Auto's deadline fairness.
- **Software:** preserve software cursor rendering and never arm hardware PlaneDelta work.

## Boundaries and safety

No second KMS authority or direct DRM path is introduced. Do not change worker capacity, transaction/token/output-generation identity, framebuffer pinning, explicit-sync behavior, direct-scanout ownership, cursor-sidecar coupling, or async/tearing restrictions. Never preempt a queued, submitted, binding, or frozen primary transaction. A pageflip remains a presentation observation, not an optical visibility measurement.

## Verification

Add deterministic tests first for policy-specific arbitration, KMS-derived target selection, 500 ms scene-cost independence, retry debt, target authority, readiness/admission states, generation resets, and 60/120/144/165/240 Hz service. Model 1,000 Hz latest-wins motion against 165 Hz primary opportunities and assert one cursor service per opportunity, bounded queue occupancy, ready-primary preservation, and eventual service during unready scene work. Exercise the real runtime ordering around pageflip drain, input, session checks, acquire/prepare, and cursor admission. Update the existing bounded cursor trace and architecture/diagnostics documentation. Run the requested Rust, Python/tooling, and Clippy verification with build artifacts only under `/mnt/Aether/Desktop/GitHub`.

Hardware qualification remains an operator-run same-binary Auto/Piggyback comparison; implementation will provide exact launch settings and will not restart a compositor or claim optical latency from pageflip timestamps.
