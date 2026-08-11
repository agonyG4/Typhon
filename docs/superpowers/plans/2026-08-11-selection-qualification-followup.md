# Wayland Selection Qualification Follow-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Completed steps use checkbox (`- [x]`) syntax.

**Goal:** Make the activated selection and Idle Inhibit protocols truthful, interoperable, and Linux-qualification ready in one focused follow-up to `e8350fd`.

**Architecture:** Keep `SelectionState` as the two-channel canonical broker, move live selection runtime out of `state/surfaces.rs`, and route every accepted offer through one source-key/backend dispatcher. Add a shared monotonic mutation epoch for normal input and Data Control mutations, preserve protocol-specific source-use rules, and qualify behavior with real Wayland client pipes and lifecycle operations.

**Tech Stack:** Rust 2024, `wayland-server`/`wayland-client`, `wayland-protocols`, `OwnedFd`, existing Typhon compositor test harness, Cargo.

## Global Constraints

- Preserve `e8350fd` unchanged in history and create one focused follow-up commit.
- `SelectionState` is the only mutable Clipboard/PRIMARY authority; DnD remains protocol-specific.
- Clipboard and PRIMARY have independent generations, offers, and mutation watermarks.
- Never order raw Wayland `u32` serials numerically; use a monotonic internal `SelectionMutationEpoch(u64)`.
- Rejected receives must deterministically consume/close their `OwnedFd` and must never send source data.
- Data Control sources are strict single-use with exact `UsedSource`; PRIMARY sources may reuse a live valid source.
- Keep normal Wayland Clipboard, HostBridge Clipboard, DnD, MIME bounds/deduplication, source cancellation, and teardown behavior intact.
- Do not implement XWayland selection bridging or unrelated compositor refactors.

---

### Task 1: Establish the truthful capability and baseline test contract

**Files:**
- Modify: `src/compositor/tests/lifecycle.rs`
- Modify: `src/compositor/tests/plan.rs`
- Modify: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`

**Interfaces:**
- Preserve `SelectionProtocolCapabilities::core_clipboard()` as the positive profile for Clipboard, PRIMARY, and Data Control.
- Preserve `InputProtocolCapabilities::desktop_baseline()` as the negative Idle Inhibit profile and `native_libinput()` as the positive profile.

- [x] **Step 1: Rename stale registry tests and change only assertions that contradict the profile constructors.**

  Rename the default/core tests to describe the full activated selection profile. Assert Clipboard, PRIMARY, and Data Control globals for `core_clipboard()`. Split the Idle Inhibit test into a baseline-hidden test and a native-enabled test. Keep the duplicate-global assertion.

- [x] **Step 2: Run the focused registry tests.**

  Run: `cargo test --locked compositor::tests::lifecycle --lib`

  Expected: the three known assertion failures are resolved; remaining failures, if any, identify a production registration mismatch rather than a stale expectation.

- [x] **Step 3: Update the compliance matrix wording without claiming untested transfer behavior.**

  Record the activated profiles and state that protocol compliance is pending the new end-to-end pipe tests until those tests pass.

- [x] **Step 4: Commit nothing yet; keep this slice in the final focused commit.**

### Task 2: Add canonical mutation epochs and protocol-specific source-use rules

**Files:**
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/surfaces.rs` or the extracted selection runtime module
- Modify: `src/compositor/protocols/primary_selection.rs`
- Modify: `src/compositor/protocols/data_control.rs`
- Modify: `src/compositor/selection.rs`
- Test: `src/compositor/state/selection_runtime.rs` and protocol tests

**Interfaces:**
- Add a nonzero `SelectionMutationEpoch(u64)` or equivalent internal type.
- Record the epoch with every eligible input serial and expose serial-to-epoch lookup after client/kind/focus validation.
- Add per-channel `last_mutation_epoch` state and an API that rejects an older epoch.

- [x] **Step 1: Add failing deterministic stale-race tests for Clipboard and PRIMARY.**

  Model a valid normal input epoch, then perform a newer Data Control set/clear on the same channel, then submit the delayed normal request. Assert rejection and preservation of the newer active source. Include serial values around `u32::MAX` and a separate test proving mutation of one channel does not stale the other.

- [x] **Step 2: Run the focused tests and confirm the stale request is currently accepted or the new API is absent.**

  Run: `cargo test --locked selection --lib`

  Expected: the new regressions fail before the epoch implementation.

- [x] **Step 3: Implement shared epoch allocation and channel watermarks.**

  Allocate a monotonic nonzero epoch when an eligible input serial is recorded. Allocate another epoch for every Data Control set/clear and any HostBridge/internal mutation that supersedes a channel. Compare epochs only within the affected channel. Preserve exact current-focus-generation validation.

- [x] **Step 4: Remove the PRIMARY generic-used rejection and keep Data Control strict.**

  PRIMARY source `Offer` and `SetSelection` accept a live valid source after prior use. Data Control source reuse continues to post `ext_data_control_device_v1::Error::UsedSource`; MIME registration after use remains rejected by its existing protocol policy. Do not add a PRIMARY error.

- [x] **Step 5: Run focused epoch, PRIMARY, and Data Control tests.**

  Run: `cargo test --locked selection --lib && cargo test --locked primary_selection --lib && cargo test --locked data_control --lib`

  Expected: the model and source-policy tests pass.

### Task 3: Extract the canonical selection runtime and dispatcher

**Files:**
- Modify: `src/compositor/state/selection_runtime.rs`
- Create if needed: `src/compositor/state/selection_transfer.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/clipboard_state.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/protocols/data_device.rs`
- Modify: `src/compositor/protocols/primary_selection.rs`
- Modify: `src/compositor/protocols/data_control.rs`
- Modify: `src/compositor/selection.rs`

**Interfaces:**
- Provide one runtime dispatcher equivalent to `request_selection_data(kind, source_key, mime_type, fd)`.
- Source records explicitly identify `wl_data_source`, PRIMARY source, Data Control source, or HostBridge backend.
- Offer handlers validate target, kind, generation, source key, active selection, and MIME before dispatch.

- [x] **Step 1: Add failing cross-protocol pipe tests for all five required paths.**

  Extend the client harness and add test helpers that create pipe FDs, issue `receive`, drain the read end, and record source-requested MIME. Cover normal Wayland Clipboard → Data Control, PRIMARY → Data Control, HostBridge Clipboard → Data Control, Data Control Clipboard → normal `wl_data_device`, and Data Control PRIMARY → PRIMARY.

- [x] **Step 2: Run the new tests to reproduce the missing backend resolution.**

  Run: `cargo test --locked cross_protocol_selection --lib`

  Expected: Data Control receives from normal/PRIMARY/HostBridge fail or the new test harness does not compile until the bindings are added.

- [x] **Step 3: Move Clipboard/HostBridge registration, mutation, publication, offer creation, and receive logic out of `surfaces.rs`.**

  Preserve actual surface lifecycle methods in `surfaces.rs`. Keep DnD branches and ownership untouched except for calls that now use the extracted selection runtime. Remove `active_clipboard`, `next_clipboard_generation`, and any duplicated current-selection representation from `CompositorState`.

- [x] **Step 4: Implement the common dispatcher from canonical source backend identity.**

  Resolve the source record by key, verify it is active for the requested channel, verify liveness/client ownership in the backend adapter, then issue exactly one source `Send` event or HostBridge request. Let `OwnedFd` drop on every rejected path and pass only a borrowed FD to protocol send events.

- [x] **Step 5: Route normal Clipboard, PRIMARY, and Data Control receive paths through the dispatcher.**

  Preserve DnD’s existing direct send path. Make Data Control receive resolve normal Wayland, PRIMARY, and HostBridge source keys through the dispatcher rather than scanning only `data_control_sources`.

- [x] **Step 6: Run focused normal clipboard/DnD and cross-protocol tests.**

  Run: `cargo test --locked cross_protocol_selection --lib && cargo test --locked data_device --lib && cargo test --locked selection --lib`

  Expected: all pipe paths pass, rejected/stale offers do not generate source sends, and DnD regressions remain green.

### Task 4: Complete real-client PRIMARY and Data Control qualification

**Files:**
- Modify: `src/compositor/tests/mod.rs`
- Modify: `src/compositor/tests/support/registry_state.rs`
- Create: `src/compositor/tests/primary_selection.rs`
- Create: `src/compositor/tests/data_control.rs`
- Modify: `src/compositor/tests/protocol_buffers.rs`
- Modify: `src/compositor/tests/support/client_setup.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`

**Interfaces:**
- Add client-side Dispatch implementations for all eight requested PRIMARY/Data Control protocol objects.
- Add reusable client helpers for source creation, MIME offer, selection mutation, offer event capture, pipe receives, and client teardown.

- [x] **Step 1: Add generated client imports and Dispatch implementations.**

  Bind `zwp_primary_selection_device_manager_v1`, `zwp_primary_selection_device_v1`, `zwp_primary_selection_source_v1`, `zwp_primary_selection_offer_v1`, `ext_data_control_manager_v1`, `ext_data_control_device_v1`, `ext_data_control_source_v1`, and `ext_data_control_offer_v1`. Record event ordering, offered MIME types, received source MIME, cancellation, and protocol errors in test state.

- [x] **Step 2: Add PRIMARY real-client tests.**

  Cover manager/device/source creation, multiple MIME offers, focus/serial acceptance and rejection, focus handoff/clear behavior, replacement cancellation, active/stale destruction, stale and unsupported receive rejection, compatible live-source reuse, pipe transfer, requested MIME, and disconnect cleanup.

- [x] **Step 3: Add Data Control real-client tests.**

  Cover immediate Clipboard/PRIMARY publication, focus-independent set/clear, multiple observing devices, exact `UsedSource`, stale offer/source behavior, resource destruction, and disconnect cleanup.

- [x] **Step 4: Add independent-channel tests.**

  Keep a valid Clipboard offer while mutating PRIMARY and transfer it successfully; then keep a valid PRIMARY offer while mutating Clipboard and transfer it successfully. Assert generations remain independent.

- [x] **Step 5: Run the protocol suite.**

  Run: `cargo test --locked primary_selection --lib && cargo test --locked data_control --lib && cargo test --locked protocol_buffers --lib`

  Expected: real protocol resources and pipe data pass, not just broker assertions.

### Task 5: Finish Idle Inhibit lifecycle qualification

**Files:**
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/protocols/idle_inhibit.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/layer_shell.rs`
- Modify: `src/compositor/tests/support/window_ops.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`
- Modify: `src/compositor/tests/input_output/output_keyboard_cursor.rs`
- Modify: `src/compositor/idle.rs`

**Interfaces:**
- Keep `IdleInhibitorBinding { inhibitor, client_id, target_surface }`.
- Keep `CompositorState::reconcile_idle_inhibition()` as the idempotent derived-state update.

- [x] **Step 1: Add lifecycle tests using client operations and server commands.**

  Create inhibitors against mapped and unmapped surfaces, then map, minimize, restore, unmap, destroy resources, disconnect clients, and use subsurface targets. Add multiple-inhibitor and occlusion scenarios.

- [x] **Step 2: Run focused idle tests and identify missing lifecycle hooks.**

  Run: `cargo test --locked idle --lib && cargo test --locked input_output::output_keyboard_cursor --lib`

  Expected: any missed transition is visible as an effective-count mismatch rather than a counter-underflow symptom.

- [x] **Step 3: Call reconciliation from every authoritative transition.**

  Add calls after mapped/unmapped commits, minimize/restore, layer visibility changes, surface teardown, and client cleanup. Keep occlusion independent and do not directly mutate expected counts in tests.

- [x] **Step 4: Verify idempotence and teardown.**

  Reconcile repeatedly, destroy one of multiple inhibitors, destroy target/client resources, and assert remaining valid inhibitors still work without underflow or permanent inhibition.

### Task 6: Final documentation and deterministic qualification

**Files:**
- Modify: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`
- Modify: relevant protocol feature-status documentation
- Modify: any source-layout allowlist only if a focused module boundary requires it

- [x] **Step 1: Update compliance claims from passing deterministic behavior.**

  Document exact tested transfer directions, source-use rules, focus/epoch policy, independent channels, cleanup, and surface-aware Idle Inhibit. Explicitly leave XWayland selection bridging out of scope.

- [x] **Step 2: Run formatting and compilation checks.**

  Run:

  ```bash
  cargo fmt --check
  cargo check --locked --all-targets
  cargo clippy --locked --all-targets -- -D warnings
  ```

  Expected: all exit 0 with no warning-based failures.

- [x] **Step 3: Run the entire locked test suite and source-layout checks.**

  Run:

  ```bash
  XDG_RUNTIME_DIR=/run/user/1000 TMPDIR=/tmp/t cargo test --locked -- --test-threads=1
  ./bin/check-source-layout
  git diff --check
  ```

  Expected: all exit 0; `surfaces.rs` is below the source-layout limit for responsibility reasons, not by suppressing the checker. The dedicated 0700 `/tmp/t` root keeps generated test socket paths below Linux `SUN_LEN` while avoiding permission changes to shared `/tmp`.

- [x] **Step 4: Inspect the final diff and commit once.**

  Confirm `e8350fd` is an unchanged ancestor, no duplicate selection authority remains, DnD is intact, all five transfer paths use pipes, source-use policies are protocol-specific, and lifecycle cleanup is deterministic. Then run:

  ```bash
  git status --short
  git diff --stat e8350fd..HEAD
  git commit -am "fix(selection): complete cross-protocol protocol qualification"
  git rev-parse HEAD
  ```

  Include newly created files in the commit with `git add` before committing. Manual Linux `wayland-info`, native PRIMARY, Data Control, and Idle Inhibit smoke checks remain an acceptance gate when a real Wayland session is available.
