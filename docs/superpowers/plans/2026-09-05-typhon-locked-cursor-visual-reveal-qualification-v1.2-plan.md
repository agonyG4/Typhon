# Typhon Locked Cursor Visual Reveal Qualification v1.2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close Gate 1 reveal attribution by freezing reveal, revision, and source ownership with the cursor presentation state through delayed KMS submission.

**Architecture:** Store the immutable `CursorRevealTraceSnapshot` on `RenderedOutputFrame`, expose it at the ready boundary, and move it into `KmsBundleOwners` or an existing cursor sidecar at worker admission. Immediate cursor-only and direct-worker paths capture the same snapshot before submission preparation. The KMS formatter retains exact raw DRM coordinates while adding unambiguous pointer, hotspot, and signed plane-origin fields.

**Tech Stack:** Rust, Cargo, existing Typhon native output/KMS worker pipeline, unit/integration tests, local `rtk` command wrapper.

## Global Constraints

- Preserve the exact ledger key `(output_generation, crtc_id, PageFlipToken)`.
- Preserve v1.1 first-visible matching, tri-state comparisons, bounded ledger capacity, worker/sync paths, and sidecar replacement semantics.
- Freeze reveal ownership at the same boundary as the cursor presentation state it describes.
- Physical token binding must never consult mutable current reveal or current cursor-source state for an already-frozen presentation.
- With cursor presentation tracing disabled, no trace-only authority or snapshot may be created or propagated.
- Preserve raw atomic cursor values and do not change atomic request semantics.
- No Gate 2 semantic cursor correction, scheduling, input, policy, or application-specific changes.
- Use the repository checkout and local target directory; run all commands through `/home/agony/.local/bin/rtk`.
- Do not use subagents; execute this plan inline.

---

### Task 1: Establish the design and plan artifacts

**Files:**
- Create: `docs/superpowers/specs/2026-09-05-typhon-locked-cursor-visual-reveal-qualification-v1.2-design.md`
- Create: `docs/superpowers/plans/2026-09-05-typhon-locked-cursor-visual-reveal-qualification-v1.2-plan.md`

**Interfaces:**
- Produces the approved architecture and the ordered implementation/test checkpoints below.

- [x] **Step 1: Write the design artifact**

  Record frame-owned freeze-time capture, worker/sidecar transport, disabled neutrality, KMS vocabulary, tests, and Gate 2 non-goals.

- [x] **Step 2: Self-review the design and plan**

  Check for placeholders, contradictory ownership rules, missing disabled/no-visible behavior, and any semantic cursor change. The documents contain no unresolved placeholders and explicitly retain the v1.1 boundaries.

- [x] **Step 3: Commit the planning artifacts**

  Run:

  ```bash
  /home/agony/.local/bin/rtk git add docs/superpowers/specs/2026-09-05-typhon-locked-cursor-visual-reveal-qualification-v1.2-design.md docs/superpowers/plans/2026-09-05-typhon-locked-cursor-visual-reveal-qualification-v1.2-plan.md
  /home/agony/.local/bin/rtk git commit -m "docs: design cursor reveal freeze-time ownership"
  ```

  Expected: one commit containing only the design and plan.

### Task 2: Add RED tests for freeze ownership and coordinate identity

**Files:**
- Modify: `src/native_output/presentation/cursor_trace.rs`
- Modify: `src/native_output/presentation/plane.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native/kms/tests.rs`
- Modify: `src/native_output/tests/scanout.rs`

**Interfaces:**
- Tests will require a `CursorRevealTraceSnapshot` to be copyable through frozen frame ownership and `PresentedCursorState.source` to remain authoritative.
- Tests will require KMS assignment fields `pointer_x`, `pointer_y`, `plane_origin_x`, and `plane_origin_y`, plus formatter labels `CRTC_X_RAW`, `CRTC_Y_RAW`, `pointer_position`, and `plane_origin_signed`.

- [ ] **Step 1: Write failing frozen-state tests**

  Add tests that construct a presented state with `source=Client`, call the snapshot constructor, then verify the snapshot source remains `Client` even when a later mutable source value differs. Add a ready-frame ownership test that stores and retrieves an optional snapshot without changing the existing frame identity or plan.

- [ ] **Step 2: Write failing overlap tests**

  Add the four required orderings using two distinct snapshots (`A` and `B`) and verify the physical identity consumes the snapshot captured for that frame/token, not the latest authority. Include a revision/source mutation between capture and token binding.

- [ ] **Step 3: Write failing coordinate tests**

  Extend the positive hotspot case `(100,80),(10,5)` to assert `pointer_position=(100,80)`, `hotspot=(10,5)`, `plane_origin_signed=(90,75)`, and exact raw `CRTC_X_RAW=90`, `CRTC_Y_RAW=75`. Add the negative case `(2,3),(8,9)` asserting signed origin `(-6,-6)` and raw values equal to `(-6i64) as u64`.

- [ ] **Step 4: Run RED tests**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked cursor_trace
  /home/agony/.local/bin/rtk cargo test --locked cursor_plane_assignment
  ```

  Expected: compile or assertion failures because the new ownership and coordinate fields/labels do not yet exist. Do not alter production behavior to weaken the tests.

- [ ] **Step 5: Commit the RED tests**

  Run:

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/presentation/cursor_trace.rs src/native_output/presentation/plane.rs src/native_output/scanout/output_swapchain.rs src/native/kms/tests.rs src/native_output/tests/scanout.rs
  /home/agony/.local/bin/rtk git commit -m "test: specify cursor reveal freeze ownership"
  ```

### Task 3: Make snapshot data freeze-safe and propagate it through explicit frames

**Files:**
- Modify: `src/native_output/presentation/plane.rs`
- Modify: `src/native_output/presentation/cursor_trace.rs`
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`

**Interfaces:**
- `CursorRevealTraceSnapshot::from_presented(authority, expected_epoch, state)` uses `state.source` and has no mutable cursor-source argument.
- `cursor_reveal_trace_snapshot(..., source, ...)` receives the source captured at the freeze boundary and returns immediately when tracing is disabled.
- `RenderedOutputFrame.frozen_cursor_trace_reveal: Option<CursorRevealTraceSnapshot>` is available through `ready_cursor_trace_reveal()`.

- [ ] **Step 1: Move the snapshot carrier into the plane module**

  Define the snapshot beside `PresentedCursorState`, import `CursorRevealAuthority`, keep ledger methods in `cursor_trace.rs`, and update imports. Remove the `from_presented` source override; use `state.source` exactly.

- [ ] **Step 2: Add the frame field and accessors**

  Add `frozen_cursor_trace_reveal` to every `RenderedOutputFrame` constructor, expose the ready accessor, and keep test constructors neutral with `None`. Do not add a second map or state machine.

- [ ] **Step 3: Capture at render freeze**

  Add a helper that first checks `cursor_presentation_trace_enabled()`, then captures authority, epoch, revision, delivery, source, and either the frozen atomic state or promoted presented state. Extend both preadmitted and ordinary explicit render tuples so the snapshot enters `render_frame` with the same cursor plan/owner.

- [ ] **Step 4: Store the snapshot in the rendered frame**

  Extend `render_frame` and its `RenderedOutputFrame` initializer. The frame owns the exact snapshot through render-ahead and ready delay.

- [ ] **Step 5: Run focused GREEN tests**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked cursor_trace
  /home/agony/.local/bin/rtk cargo test --locked scanout
  ```

  Expected: all focused tests pass, including the new frozen source and frame ownership tests.

- [ ] **Step 6: Commit the frame freeze implementation**

  Run:

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/presentation/plane.rs src/native_output/presentation/cursor_trace.rs src/native_output/runtime/presentation_cursor.rs src/native_output/scanout/output_swapchain.rs src/native_output/scanout/atomic_egl_gbm.rs src/native_output/runtime/presentation_cycle.rs
  /home/agony/.local/bin/rtk git commit -m "feat: freeze cursor reveal ownership with rendered frames"
  ```

### Task 4: Replace late primary reconstruction in worker and synchronous paths

**Files:**
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/kms_worker/bundle.rs`
- Modify: `src/native_output/kms_worker/presentation_executor.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm_transactions.rs`
- Modify: `src/native_output/kms_worker/cursor_sidecar.rs`
- Modify: `src/native_output/runtime/plane_cycle.rs`
- Modify: `src/native_output/kms_worker/thread.rs`

**Interfaces:**
- `KmsPrimaryOwner` and `KmsCursorOwner` retain optional frozen trace ownership; `KmsBundleOwners::set_cursor_trace_reveal` stores on cursor when present or primary otherwise, and `trace_reveal()` reads the frozen snapshot.
- `take_ready_for_worker` returns the ready frame’s snapshot with the fence and frozen cursor owner.
- Worker and synchronous primary submission bind the returned physical token to that ready-frame snapshot; no helper reads current server/cursor reveal state.

- [ ] **Step 1: Write the worker/sync ownership tests**

  Add tests for ready-frame snapshot extraction, worker primary owner propagation, sidecar replacement of both cursor state and snapshot, synchronous primary frozen epoch/revision, and cursor-only capture before submit.

- [ ] **Step 2: Run the new tests RED**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked worker
  /home/agony/.local/bin/rtk cargo test --locked sidecar
  ```

  Expected: failures until owner fields, extraction, and submission wiring exist.

- [ ] **Step 3: Extend worker owner storage**

  Add the optional snapshot to the primary owner, add the fallback-aware setter/getter, return it from `take_ready_for_worker`, and use the owner snapshot when the worker executor emits epoch/revision. Keep cursor sidecar replacement atomic by replacing its trace snapshot with its state.

- [ ] **Step 4: Remove explicit-ready late binding**

  Delete `explicit_cursor_reveal_trace_snapshot`. Worker ready submission passes no current-derived snapshot; the queue extracts the frame-owned snapshot. Synchronous ready submission reads the ready accessor before `submit_ready_frame` and binds it after the returned token.

- [ ] **Step 5: Preserve sync KMS epoch/revision**

  In `submit_ready_frame`, read the frame snapshot before moving the frame and pass `expected_epoch` and `expected_revision` into `CursorKmsSubmitContext`. The physical token is the only new value added at submission.

- [ ] **Step 6: Run GREEN worker/sync tests and commit**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked worker
  /home/agony/.local/bin/rtk cargo test --locked sidecar
  /home/agony/.local/bin/rtk cargo test --locked synchronous
  /home/agony/.local/bin/rtk git diff --check
  ```

  Expected: focused tests pass and the diff is whitespace-clean. Commit with:

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/runtime/presentation_worker.rs src/native_output/runtime/kms_worker.rs src/native_output/kms_worker/bundle.rs src/native_output/kms_worker/presentation_executor.rs src/native_output/scanout/atomic_egl_gbm_transactions.rs src/native_output/kms_worker/cursor_sidecar.rs src/native_output/runtime/plane_cycle.rs src/native_output/kms_worker/thread.rs
  /home/agony/.local/bin/rtk git commit -m "feat: carry frozen reveal ownership through KMS worker"
  ```

### Task 5: Close immediate-path and disabled/no-visible neutrality

**Files:**
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation_transactions.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/server.rs`

**Interfaces:**
- Immediate snapshot helpers return `None` before trace-only construction when disabled.
- Direct-worker finalization accepts the snapshot captured before direct admission.
- Compositor reveal authority is absent when tracing is disabled; no-visible terminal evidence retires active trace authority after recording its own terminal event.

- [ ] **Step 1: Write RED tests for neutrality and no-visible retirement**

  Test disabled snapshot construction/propagation as `None`, test no-visible terminal authority retirement, and test that a later visible reveal cannot consume a no-visible reveal’s first-visible slot.

- [ ] **Step 2: Gate trace-only construction**

  Gate worker hidden-state cloning, all snapshot helpers, and authority storage/clearing/logging on `cursor_presentation_trace_enabled()`. Keep semantic pointer unlock/finalization code outside those gates.

- [ ] **Step 3: Freeze direct and cursor-only snapshots before handoff**

  Capture direct-worker and synchronous cursor-only snapshots before their physical boundary, pass the direct snapshot into `finish_direct_worker_queued`, and retain the existing cursor pin/transaction behavior.

- [ ] **Step 4: Run focused GREEN tests and commit**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked disabled
  /home/agony/.local/bin/rtk cargo test --locked no_visible
  /home/agony/.local/bin/rtk cargo test --locked cursor_trace
  /home/agony/.local/bin/rtk git diff --check
  ```

  Commit:

  ```bash
  /home/agony/.local/bin/rtk git add src/native_output/runtime/presentation_cursor.rs src/native_output/runtime/presentation_worker.rs src/native_output/runtime/presentation_transactions.rs src/native_output/runtime/presentation_cycle.rs src/compositor/state/pointer_constraints.rs src/compositor/server.rs
  /home/agony/.local/bin/rtk git commit -m "fix: keep disabled and immediate cursor traces neutral"
  ```

### Task 6: Add unambiguous KMS coordinate vocabulary

**Files:**
- Modify: `src/native/kms/atomic.rs`
- Modify: `src/native/kms/tests.rs`
- Modify: `src/native_output/presentation/cursor_trace.rs`

**Interfaces:**
- `AtomicCursorPlaneAssignment::Enabled` preserves `crtc_x/crtc_y` as exact raw `u64` values and adds signed plane-origin and pointer fields.
- The canonical formatter emits raw DRM coordinates under `CRTC_X_RAW/CRTC_Y_RAW` and human geometry under `pointer_position`, `hotspot`, and `plane_origin_signed`.

- [ ] **Step 1: Extend assignment construction**

  Compute `plane_origin_x/y = cursor.x/y.saturating_sub(cursor.hotspot_x/y)`, keep raw fields as `i64::from(origin) as u64`, and copy pointer/hotspot/signed-origin values into the assignment.

- [ ] **Step 2: Update all assignment literals and formatter tests**

  Supply the new fields in tests, remove ambiguous `position=(...)`, and assert disabled/unavailable paths report unknown geometry without inventing values.

- [ ] **Step 3: Run KMS GREEN tests and commit**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo test --locked cursor_plane_assignment
  /home/agony/.local/bin/rtk cargo test --locked synchronous_kms_payload_uses_canonical_exact_fields
  /home/agony/.local/bin/rtk git diff --check
  ```

  Commit:

  ```bash
  /home/agony/.local/bin/rtk git add src/native/kms/atomic.rs src/native/kms/tests.rs src/native_output/presentation/cursor_trace.rs
  /home/agony/.local/bin/rtk git commit -m "feat: label raw and signed cursor KMS geometry"
  ```

### Task 7: Full verification and v1.2 qualification report

**Files:**
- Modify: `docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1.1-report.md`

**Interfaces:**
- Produces the superseding English v1.2 report with starting `HEAD`, implementation commits, RED/GREEN evidence, exact closure claims, limitations, and complete verification output.

- [ ] **Step 1: Run focused qualification tests**

  Run the cursor trace, scanout, worker, sidecar, pageflip, sync, disabled, no-visible, and KMS geometry filters. Record exact pass counts and any ignored tests in the report.

- [ ] **Step 2: Run the required full verification**

  Run:

  ```bash
  /home/agony/.local/bin/rtk cargo fmt --check
  /home/agony/.local/bin/rtk cargo check --locked --all-targets
  /home/agony/.local/bin/rtk cargo clippy --locked --all-targets -- -D warnings
  /home/agony/.local/bin/rtk cargo test --locked
  /home/agony/.local/bin/rtk git diff --check
  ```

  Expected: every command passes in the checkout's local target directory.

- [ ] **Step 3: Write the v1.2 report**

  State that reveal ownership freezes with cursor presentation state; physical token binding never consults mutable current reveal/source; disabled tracing creates/propagates no trace-only authority/snapshot; `pointer_position` is hotspot position, `plane_origin_signed` is plane top-left, and raw `CRTC_X/Y` remain exact DRM representation; sync primary preserves frozen epoch/revision; no-visible reveals retire without contaminating future reveals; and no Gate 2 semantic correction was implemented. Include the next native A/B step without claiming teleport resolution.

- [ ] **Step 4: Commit the report and verify the worktree**

  Run:

  ```bash
  /home/agony/.local/bin/rtk git add docs/superpowers/specs/2026-09-04-typhon-locked-cursor-visual-reveal-qualification-v1.1-report.md
  /home/agony/.local/bin/rtk git commit -m "docs: update cursor reveal qualification to v1.2"
  /home/agony/.local/bin/rtk git status --short
  ```

  Expected: the report commit is created and `git status --short` is empty.
