# Typhon Locked Cursor Visual Reveal Qualification v1 Implementation Plan

Execution is inline in the current checkout. No sub-agents and no additional
worktree will be used.

## Goal

Instrument and qualify the complete locked-pointer cursor reveal path so the
first visible cursor state can be compared with the latest authoritative reveal
state. Add deterministic sequence tests for hidden/visible, source, worker,
primary, and hardware/software transitions. Apply only a correction justified
by the first failing evidence; otherwise report that the existing ownership
path is validated without changing scheduling semantics.

## Architecture

Use the existing constraint id/generation as the correlation key. Add a narrow
last-reveal context to compositor state and a lazy opt-in event emitter in the
existing pointer diagnostics module. Instrument the existing lifecycle owners:

- compositor pending reveal begin, backend settlement, client warp observation,
  and reveal finalization;
- native source and client cursor-surface geometry resolution;
- `NativeAtomicCursor` desired/current/submitted state and revisions;
- `RuntimePlanePlan`, delta classification, and frozen cursor ownership;
- worker queue/sidecar replacement and exact `KmsCursorUpdate` payload;
- pageflip promotion and the first visible presentation per reveal.

Keep ordinary pointer timing and cursor movement asynchronous. Visibility,
visual, and delivery-mode transitions must remain complete visual-state
publications; only stable position-only hardware movement may use the existing
fast path.

## Tech Stack

Rust, existing Typhon compositor/native-output modules, Rust unit/integration
tests, the existing KMS worker synchronization hooks, Markdown documentation,
and `rtk` for every shell command.

## Global Constraints

- Work from starting `HEAD` `2c4bb7e6c908c10d6d19312b17902eaf7cc0f2af` in the
  current folder.
- Use `apply_patch` for edits; compile in the same folder.
- Do not reopen pointer-constraint transaction semantics, input cadence,
  activation-anchor fallback, or pointer authority.
- Do not add delays, synthetic motion, Sober-specific branches, or a second
  generation/transaction system.
- Trace-disabled execution must not do trace-only clock reads, formatting,
  allocation, scheduling changes, or log emission.
- Follow TDD: each production behavior is preceded by a focused failing test,
  then the smallest implementation and focused green verification.
- Preserve all existing tests and run the exact required global commands before
  claiming completion.

## Task 1 — Add lazy trace primitives and reveal correlation

Files:

- `src/pointer_debug.rs`
- `src/compositor/mod.rs`
- `src/compositor/state/pointer_constraints.rs`
- the nearest existing compositor test module

Steps:

1. Add a RED unit test proving the cursor-presentation formatter is not
   evaluated when `TYPHON_CURSOR_PRESENTATION_TRACE` is disabled, mirroring
   the existing lazy pointer logger test.
2. Add the opt-in lazy event helper with a monotonic sequence only inside the
   enabled branch. Keep the environment lookup cached and do not share the
   pointer timing flag.
3. Add a narrow reveal correlation snapshot based on the existing
   `PointerConstraintBackendId`; record it at reveal begin and expose it to
   native presentation code through the existing server/state boundary.
4. Emit begin, backend-settled, client-warp-observed, and finalize events with
   the exact pending-reveal fields, including fallback origin and final result.
5. Run the pointer-debug and pointer-constraint focused tests plus `rtk git
   diff --check`.
6. Commit as `feat: trace locked cursor reveal lifecycle`.

## Task 2 — Add state snapshot formatting and source/surface events

Files:

- `src/native_output/output/cursor.rs`
- `src/native_output/output/cursor_state.rs`
- `src/native_output/runtime/cursor_cycle.rs`
- `src/native_output/runtime/presentation_cursor.rs`
- `src/native_output/runtime/presentation_cycle.rs`
- `src/native_output/runtime/presentation_worker.rs`

Steps:

1. Add RED tests for stable formatting/identity of cursor revision, desired,
   submitted, current, and pending/queued state, and for client cursor geometry
   resolving to pointer position plus surface offset plus hotspot.
2. Add trace-only accessors/formatters that reuse existing revision and state
   representations; do not alter revision advancement or KMS equivalence.
3. Emit `cursor_source_resolved` before KMS policy, including source, visibility
   inputs, pointer authority, client surface/buffer/commit identity, logical
   coordinates, surface offsets, hotspot, and resolved output coordinates.
4. Emit `cursor_desired` after image/source preparation with desired epoch,
   revision components, visible state, geometry, framebuffer/image identity,
   and reveal correlation.
5. Emit a complete current/submitted/queued/pending snapshot at the same
   reveal boundary, keeping all trace-only work lazy.
6. Run focused cursor and source/surface tests; commit as `feat: trace cursor
   source and desired state`.

## Task 3 — Trace plan, freeze, worker, KMS, and presented ownership

Files:

- `src/native_output/runtime/presentation_cursor.rs`
- `src/native_output/runtime/plane_cycle.rs`
- `src/native_output/runtime/kms_worker.rs`
- `src/native_output/runtime/cursor_cycle.rs`
- `src/native_output/runtime/cycle/pageflip.rs`
- `src/native_output/kms_worker/payload.rs`
- `src/native_output/presentation/plane.rs`
- `src/native_output/presentation/plane_policy.rs`

Steps:

1. Add RED model tests asserting hidden-to-visible classification is never
   `PositionOnly`, and that the first visible promotion after hidden P0 -> P1
   carries P1's revision, geometry, and delivery.
2. Add `cursor_plane_plan` tracing for previous/next delivery, delta class,
   cursor/primary actions, test policy, desired epoch/revision, and presented
   state. Keep ordinary position-only movement eligible for the current fast
   path.
3. Add `cursor_freeze` tracing at both independent-plane and primary-bundle
   ownership points, including transaction/pageflip identity, assignment,
   frozen revision, source/capability keys, visible geometry, and framebuffer
   pin identity.
4. Add worker queue and sidecar replacement events that identify the incoming,
   replaced, selected, and final cursor states rather than only logging
   acceptance.
5. Emit `cursor_kms_submit` from the worker using the actual cursor update and
   normalized source/destination geometry, distinguishing cursor-only, primary
   bundle, sidecar, and initial modeset submissions.
6. Emit `cursor_presented` only from the existing exact pageflip/promotion
   ownership point. Add one `first_visible_cursor_presentation` event per
   reveal, comparing the presented state to the saved authoritative reveal
   state and recording `MATCH` or the first mismatch category.
7. Run plane-policy, cursor-output, KMS-worker, presentation-transaction, and
   pageflip-focused suites; commit as `feat: trace cursor presentation ownership`.

## Task 4 — Complete sequence-sensitive regression coverage

Files:

- `src/native_output/output/cursor_tests.rs`
- `src/native_output/tests/plane_scheduling_model.rs`
- `src/native_output/kms_worker/task4_tests.rs`
- `src/native_output/tests/presentation_transactions.rs`
- `src/compositor/tests/input_output/pointer_cursor.rs`
- `src/native_output/tests/input.rs`

Steps:

1. Add RED tests for hidden P0 -> hidden P1 -> visible, unchanged pointer
   hide/show, hidden image/hotspot changes, and client source/surface changes.
2. Add RED tests for valid post-unlock warp exactly once and no-warp logical
   preservation, using the current pending reveal state machine.
3. Add RED deterministic worker/sidecar and primary-in-flight tests proving an
   older hidden/visible revision cannot become newly visible before the latest
   reveal revision.
4. Add RED hardware/software transition and no-double-representation tests for
   Hidden/Hardware/Software and supported reverse handoffs.
5. Implement only the minimum behavior needed to turn each test green. If the
   existing code already passes a test, keep the test as an explicit proof and
   do not mutate unrelated architecture.
6. Run all focused suites and review the resulting trace fields against the
   qualification checklist; commit as `test: qualify first visible cursor state`.

## Task 5 — Manual qualification and evidence-backed correction

1. Run focused tests green before manual work.
2. Provide the user with the existing hardware/software commands using
   `TYPHON_CURSOR_PRESENTATION_TRACE=1`; do not automate Sober. Record whether
   the user supplied A/B results and logs.
3. Classify each available reveal as authority, source, desired, frozen/
   submitted, presented, hardware-only, or shared-path evidence. Identify the
   first divergence.
4. If deterministic or manual evidence proves a stale newly-visible state,
   patch the existing supersession/revalidation/bundle/sidecar edge and add a
   failing regression test first. Otherwise make no speculative correction.
5. Create the companion report
   `docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1-report.md`
   with starting/ending `HEAD`, KWin findings, trace/evidence, correction or
   no-correction result, focused/global verification, and manual results.

## Task 6 — Final verification and handoff

Run and preserve exact exit results for:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Inspect `rtk git status`, the final diff, and ending `HEAD`. Do not claim full
verification if any command fails. Commit the final report and any correction,
then use the finishing-development-branch workflow to present the integration
choice.

