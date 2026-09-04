# Typhon Locked Pointer Release Cursor Restore Implementation Plan

> **For agentic workers:** This plan is executed inline in the current checkout. No subagents are used. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make locked-pointer release restoration explicit, constraint-scoped, and observable without treating the activation anchor as an implicit restore target.

**Architecture:** Preserve commit-exact constraint state and native settlement. Centralize the locked-release decision as either preserving the current compositor logical position or applying the exact committed hint for the active constraint generation. Carry an explicit warp origin through compositor and native requests, and attach transition state to the existing opt-in pointer timing evidence.

**Tech Stack:** Rust, Smithay/Wayland server resources, existing Typhon compositor test clients, native input backend, `rtk` verification commands.

## Global Constraints

- The solved region-algebra and input-backlog fixes remain unchanged.
- `NativeInputEpoch`, libinput dispatch cadence, readiness, scheduling, motion, acceleration, and cursor visibility timing remain unchanged except for the requested release-restore ownership.
- A pending hint is never a committed restore target.
- An activation anchor is never an implicit release restore target.
- A committed hint may restore only for the exact active constraint generation and valid current surface geometry.
- Compositor restoration never emits synthetic relative-pointer motion.
- Existing one-shot compatibility behavior remains covered.
- No application detection, Flatpak behavior, desktop automation, or Sober-specific code is added.
- Unrelated existing checkout edits remain unstaged and untouched.

---

### Task 1: Establish RED release-restore regressions

**Files:**
- Modify: `src/compositor/tests/input_output/pointer_cursor.rs`
- Modify: `src/native_output/tests/input.rs`

**Interfaces:**
- Exercise the existing real Wayland resource fixture and `NativePointerConstraintBackend` request seam.
- Preserve existing committed-hint and one-shot tests as independent controls.

- [ ] **Step 1: Change the persistent no-hint integration expectation** so release produces `restore_position: None`, leaves the logical pointer at its frozen position, and produces no warp request.
- [ ] **Step 2: Change the native backend no-hint expectation** so deactivation returns no restore position instead of falling back to its activation anchor.
- [ ] **Step 3: Run the two focused tests with `rtk cargo test --locked` filters** and record the expected failure against the current anchor fallback.

### Task 2: Add explicit restore decision and warp-origin data

**Files:**
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/tests/input.rs`

**Interfaces:**
- Add a copyable `PointerWarpOrigin` used by all compositor-applied reposition requests.
- Add a locked-release decision equivalent to `PreserveCurrentLogicalPosition` or `ApplyCommittedHint`.
- Extend deactivation/warp actions only with explicit origin metadata; do not change settlement boundaries.

- [ ] **Step 1: Implement the centralized locked-release decision** using the committed hint for the exact constraint id/generation when finite, in-bounds, and geometrically resolvable; otherwise preserve the current logical position.
- [ ] **Step 2: Remove the compositor fallback that writes `last_pointer_*` from `activation_anchor`** and make native deactivation honor `None` without substituting its anchor.
- [ ] **Step 3: Tag committed-hint restore, one-shot compatibility, pointer-warp protocol, and confined-region correction with explicit origins.** Keep the XWayland audit result explicit if no current production warp writer exists.
- [ ] **Step 4: Preserve exact surface-local hint translation and the existing delayed unlock/visibility settlement path.**

### Task 3: Extend opt-in transition observability

**Files:**
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/runtime/pointer_timing.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`

**Interfaces:**
- Extend `NativePointerTransitionEvidence` and the existing timing record with optional transition state fields.
- Use `unknown` for unavailable values and gate coordinate/state capture and formatting on the existing trace-enabled path.

- [ ] **Step 1: Capture constraint id/generation, logical before/after positions, activation anchor, committed/pending hint state, restore decision, warp request/origin/target/applied target, cursor visibility, and focus surface around locked activation/deactivation.**
- [ ] **Step 2: Include the fields in the existing transition summary without adding clocks, formatting, logs, or timing-only allocations when `TYPHON_POINTER_TIMING_TRACE` is disabled.**
- [ ] **Step 3: Add unit coverage for unknown fields, explicit origins, and disabled-trace neutrality.**

### Task 4: Expand real Wayland release ownership coverage

**Files:**
- Modify: `src/compositor/tests/input_output/pointer_cursor.rs`
- Modify: `src/compositor/tests/input_output/relative_and_constraints.rs`
- Modify: `src/compositor/tests/input_output/pointer_warp_serial.rs`
- Modify: `src/native_output/tests/input.rs`

- [ ] **Step 1: Cover committed hint restoration exactly once, including surface-local translation and no relative motion.**
- [ ] **Step 2: Cover no-hint preservation, stale hint isolation across A/B constraints, and anchor-versus-hint separation.**
- [ ] **Step 3: Re-run persistent and one-shot compatibility cases, including pending destroy cancellation and no events to destroyed resources.**
- [ ] **Step 4: Add or update deterministic non-trivial-origin coverage where the existing surface geometry seam supports it.**

### Task 5: Verify, document, and commit

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md`

- [ ] **Step 1: Run focused pointer, warp, native input, timing, and XWayland-related tests with `rtk`.**
- [ ] **Step 2: Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `rtk git diff --check`, recording blockers exactly.**
- [ ] **Step 3: Append the investigation report with starting/ending HEAD, mutation-site map, current observed path, mature-compositor comparison, selected fix, RED/GREEN evidence, blockers, and manual qualification instructions.**
- [ ] **Step 4: Stage only scoped files and commit the closure.**

