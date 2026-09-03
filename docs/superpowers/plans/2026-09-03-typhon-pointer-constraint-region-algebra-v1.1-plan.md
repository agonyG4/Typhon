# Typhon Pointer Constraint Region Algebra v1.1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Canonicalize effective pointer-constraint geometry by geometric set and attach locked resolver timing to the exact `LockedActivated` transition that consumed it.

**Architecture:** Preserve the existing `SurfaceRect`/`SurfaceRectRegion` ordered rectangle algebra, one committed input-region snapshot, test-only raster oracle, late-bound locked anchor, and `NativePointerTransitionEvidence` flow. Add an edge/band canonicalization stage before `OutputRegion` construction, and return locked resolution timing as internal metadata beside the unchanged semantic `ActivateLocked { id }` request.

**Tech Stack:** Rust, Smithay/Wayland protocol test fixtures, Cargo, `rtk`, existing native pointer timing instrumentation.

## Global Constraints

- The production path must remain free of per-pixel loops, per-row surface scans, and input-region mutex acquisition inside geometry loops.
- Canonicalization must use rectangle/edge complexity, never surface pixel area or allocations proportional to surface height.
- Preserve `NativeInputEpoch` semantics, surface transaction ownership, pointer motion, acceleration, scheduling, readiness, cursor restoration policy, and pointer-constraint lifecycle semantics.
- Keep `ActivateLocked { id }` as the semantic backend request; timing is internal metadata only.
- Keep `TYPHON_POINTER_TIMING_TRACE` disabled-path neutrality: no extra clocks, formatting, or timing-only allocations.
- Do not change unrelated native-frame-pacing work or claim the Sober/Roblox camera jump is fixed without native Linux qualification.

---

### Task 1: Add RED regressions for canonical geometry and locked timing

**Files:**
- Modify: `src/compositor/state/pointer_constraint_region.rs`
- Modify: `src/native_output/input/routing.rs`

**Interfaces:**
- Consume the current `SurfaceRectRegion` resolver and transition-evidence test helpers.
- Define failing expectations for canonical adjacent additions, equivalent operation histories, disconnected/hole preservation, and a resolved locked request carrying wall/CPU timing.

- [ ] **Step 1: Write the failing adjacent-geometry test**

Add a test that resolves `Add(0,0,2,1)+Add(0,1,2,4)` and `Add(0,0,2,5)` and asserts equal output rectangles plus equal `closest_point(OutputPosition { x: 0.0, y: 0.5 })`.

- [ ] **Step 2: Write equivalent-history, gap, and hole assertions**

Add paired histories for one rectangle versus vertically adjacent rectangles and for a 10×10 rectangle with a 2×2 hole versus four surrounding pieces. Assert equal canonical rectangles and equal closest-point results for integer, fractional, outside, between-island, and equal-distance probes; separately assert disconnected islands remain two components and the hole remains absent.

- [ ] **Step 3: Write the locked timing handoff regression**

Add a native-routing test that constructs the internal resolved locked-request result with timing `(37ns, 19ns)`, selects `LockedActivated(A)`, and asserts both timing fields are present. Add a second transition for B after `Deactivate(A)` and assert A cannot receive B's timing.

- [ ] **Step 4: Run the focused tests and record RED**

Run:

```bash
rtk cargo test --locked pointer_constraint_region::tests
rtk cargo test --locked selected_locked_activation_keeps_region_resolution_timing
```

Expected result: the new canonical-equivalence test fails because fragmented rectangles are still emitted, and the locked handoff test fails to compile or fails because no resolved-request timing metadata exists yet.

- [ ] **Step 5: Commit the RED tests**

```bash
rtk git add src/compositor/state/pointer_constraint_region.rs src/native_output/input/routing.rs
rtk git commit -m "test: cover canonical pointer constraint geometry and locked timing"
```

### Task 2: Implement geometry-set canonicalization

**Files:**
- Modify: `src/compositor/state/pointer_constraint_region.rs`

**Interfaces:**
- Consume the existing non-overlapping `SurfaceRectRegion` algebra.
- Produce a canonical `Vec<SurfaceRect>` before `OutputRegion` creation, with equal geometry yielding equal decomposition independent of operation history.

- [ ] **Step 1: Add the edge/band canonicalizer**

Implement `SurfaceRectRegion::canonicalized` by collecting unique rectangle top/bottom edges, sorting them, deriving merged x intervals for each non-empty vertical band, and merging adjacent bands with identical intervals. Do not iterate over physical y or x pixels.

- [ ] **Step 2: Route final output through canonicalization**

Canonicalize in `into_output_region` after the algebra/intersection result and before translating to output coordinates. Preserve the one-rectangle default/full fast path and deterministic top-to-bottom/left-to-right ordering.

- [ ] **Step 3: Run the canonicalization tests**

Run:

```bash
rtk cargo test --locked pointer_constraint_region::tests
```

Expected result: adjacent-history and equivalent-history tests pass; disconnected and hole membership remain correct; existing area-independent operation tests remain green.

- [ ] **Step 4: Commit the geometry fix**

```bash
rtk git add src/compositor/state/pointer_constraint_region.rs
rtk git commit -m "fix: canonicalize pointer constraint geometry"
```

### Task 3: Carry exact locked resolver timing

**Files:**
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/native_output/input/routing.rs`

**Interfaces:**
- Add an internal `ResolvedPointerConstraintBackendRequest` containing the unchanged `PointerConstraintBackendRequest`, optional late-bound locked anchor, and optional resolver timing.
- Make the locked resolver call `pointer_constraint_output_region_with_timing`, and pass that timing to selected transition evidence; confined timing remains associated with its activation action.

- [ ] **Step 1: Add the internal resolved-request carrier**

Define a compositor-visible result with fields equivalent to `request`, `locked_anchor`, and `region_resolution_timing`. Keep the public semantic request enum unchanged.

- [ ] **Step 2: Resolve locked geometry exactly once at settlement**

Replace the untimed locked call with the timed resolver, use the returned region for the late-bound anchor, and return its timing in the carrier. Non-locked requests return no locked timing.

- [ ] **Step 3: Propagate timing into selected evidence**

Update the native request loop to destructure the carrier and pass locked timing, or confined action timing when present, to `select_pointer_transition_evidence`. Keep deactivation timing unknown and preserve first-selected transition ownership.

- [ ] **Step 4: Run focused timing and routing tests**

Run:

```bash
rtk cargo test --locked pointer_timing::tests
rtk cargo test --locked routing_transition_tests
rtk cargo test --locked pointer_constraint_transaction
rtk cargo test --locked confined_pointer
rtk cargo test --locked selected_locked_activation_keeps_region_resolution_timing
```

Expected result: locked and confined evidence carry only their own resolver timing; deactivation A does not receive activation B timing; existing causal timing behavior remains unchanged.

- [ ] **Step 5: Commit the timing fix**

```bash
rtk git add src/compositor/input.rs src/compositor/state/pointer_constraints.rs src/compositor/server.rs src/native_output/input/routing.rs
rtk git commit -m "fix: preserve locked pointer region resolver timing"
```

### Task 4: Audit, document, and verify the closure

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md`

**Interfaces:**
- Audit production call sites for all resolver and `OutputRegion::closest_point` paths and document the canonical-geometry refinement and locked timing ownership.

- [ ] **Step 1: Audit forbidden production behavior**

Use `rtk rg` to confirm no production pointer-constraint resolver contains per-pixel or per-row materialization, no input-region snapshot occurs inside a geometry loop, and every locked settlement resolution uses the timed path.

- [ ] **Step 2: Update the English closure report**

Record the fragmentation-dependent defect, adjacent-add reproduction, edge/band canonicalization, equivalent-history tests, why sorting alone was insufficient, locked timing ownership, focused results, full verification, blockers, and the exact non-claim about Sober/Roblox.

- [ ] **Step 3: Run the required verification**

Run:

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Report each actual result, including unrelated pre-existing blockers, without weakening production code or claiming full GREEN when a command is blocked.

- [ ] **Step 4: Commit the report and confirm repository state**

```bash
rtk git add docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md
rtk git commit -m "docs: close pointer constraint region algebra v1.1"
rtk git status --short
```

