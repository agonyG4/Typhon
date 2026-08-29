# Pointer-Warp Enter-Serial Authority Implementation Plan

> **For inline execution:** This plan is executed in the current checkout without subagents. Steps use checkbox syntax for review tracking.

**Goal:** Validate `wp_pointer_warp_v1` against the live current pointer-enter authority instead of bounded generic input history.

**Architecture:** Preserve `pointer_has_current_enter_serial()` for `set_cursor`. Add a dedicated warp validator over `pointer_enter_serials`, enforcing same-client ownership while allowing the enter and target surfaces to differ. Keep the active-lock compositor and native guards untouched.

**Tech Stack:** Rust, wayland-server/client protocol tests, Cargo, existing `rtk` workflow.

## Global Constraints

- Do not increase `MAX_RECENT_INPUT_SERIALS`.
- Do not make historical enter serials valid forever.
- Do not weaken client/resource ownership checks.
- Do not modify active-lock behavior, relative deltas, acceleration, or capability advertisement.
- Do not add deferred pointer warp or application-specific behavior.

---

### Task 1: Add the protocol RED tests

**Files:**
- Create: `src/compositor/tests/input_output/pointer_warp_serial.rs`
- Modify: `src/compositor/tests/input_output/mod.rs`

- [ ] Add an integration test that records one real pointer-enter serial, emits more than 16 real pointer-button press/release pairs without changing focus, then issues a valid warp using the original serial. Assert the current behavior rejects the warp and capture the expected `invalid_serial` RED failure.
- [ ] Add a repeated lock/destroy cycle test that churns generic input history and then warps using the unchanged enter serial.
- [ ] Add a same-client target-surface test and a wrong-client resource/serial test, retaining current focus and valid coordinates where acceptance is expected.
- [ ] Run the new tests before production changes and save the RED output.
- [ ] Commit only the test and module-registration changes.

### Task 2: Implement the dedicated warp-enter validator

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/surfaces.rs` only if the old generic helper becomes unused after inspection.

- [ ] Add `pointer_has_valid_warp_enter_serial(pointer, serial, surface)` backed by the live `pointer_enter_serials` entry, requiring the pointer-enter record and target surface to share a client.
- [ ] Route `warp_pointer_protocol_request` through that helper and remove only the exact-surface dependency that conflicts with the protocol’s same-client enter-serial rule.
- [ ] Keep all dead-resource, pointer ownership, focus, finite-coordinate, bounds, and stale-current-enter checks.
- [ ] Keep `pointer_has_current_enter_serial` and `set_pointer_cursor` semantics unchanged.
- [ ] Run the new tests and existing pointer suites GREEN.
- [ ] Commit the production fix and directly related tests.

### Task 3: Verify lifecycle and repository gates

**Files:**
- No additional production files expected.

- [ ] Verify stale serial rejection after a real pointer focus transition.
- [ ] Verify the previous active-lock warp invariant and native backend defense remain green.
- [ ] Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `./bin/check-source-layout`, and `git diff --check` through `rtk`.
- [ ] Record any pre-existing gate blockers exactly and confirm the worktree is clean.
- [ ] Do not claim Sober runtime qualification unless it is actually run after this change.
