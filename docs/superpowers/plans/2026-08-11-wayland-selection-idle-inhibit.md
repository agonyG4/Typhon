# Wayland Selection and Idle Inhibition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement, qualify, and activate PRIMARY selection, ext-data-control, and surface-aware idle inhibition without regressing normal clipboard or DnD behavior.

**Architecture:** `SelectionState` becomes the canonical two-channel seat broker for Clipboard and Primary, while `CompositorState` stores generated Wayland resources and protocol-specific transfer adapters. PRIMARY and ext-data-control dispatches are extracted into focused modules and publish through the same broker. Idle inhibition stores target surfaces and reconciles effective state from live mapped visibility.

**Tech Stack:** Rust 2024, `wayland-server` 0.31, `wayland-protocols` 0.32, `OwnedFd`, existing compositor test harness, Cargo.

## Global Constraints

- Do not advertise any of the three globals until their end-to-end tests pass.
- Keep Clipboard and Primary as independent broker channels with independent generations.
- Keep `wl_data_source` DnD state protocol-specific; PRIMARY and ext-data-control sources never inherit DnD behavior.
- Validate selection serials with client ownership, eligible input kind, current focus generation, and an internal monotonic `u64` epoch; never order raw Wayland `u32` serials numerically.
- Reuse existing MIME bounds: 128 source MIME types maximum and 4096 bytes maximum per MIME type; ignore empty and duplicate MIME types.
- Rejected FDs must be consumed/closed deterministically.
- Do not implement XWayland selection bridging or the older wlr-data-control protocol.
- Preserve unrelated working-tree changes and current branch history.

---

### Task 1: Canonical broker model

**Files:**
- Modify: `src/compositor/selection.rs`
- Create: `src/compositor/state/selection_runtime.rs`
- Modify: `src/compositor/mod.rs`
- Test: `src/compositor/selection.rs` unit tests and `src/compositor/state/selection_runtime.rs` unit tests

**Interfaces:**
- Produces `SelectionKind::{Clipboard, Primary}`, `SelectionSourceKind`, canonical per-kind generations, source registration/removal, offer registration/validation, and channel-scoped invalidation.
- `SelectionState` owns active selection metadata for both channels; protocol maps only own generated resources and source transfer adapters.

- [ ] **Step 1: Write failing broker tests.** Add tests that register sources on both channels, verify independent generations, replace one source without invalidating the other channel, reject stale offers, reject stale-source destruction, enforce MIME bounds/deduplication, and clear only the active source’s channel.

```rust
let clipboard = broker.commit_selection(SelectionKind::Clipboard, source_a).unwrap();
let primary = broker.commit_selection(SelectionKind::Primary, source_b).unwrap();
let clipboard_offer = broker.register_offer(SelectionKind::Clipboard, 7, clipboard).unwrap();
broker.commit_selection(SelectionKind::Primary, source_c).unwrap();
assert!(broker.offer_is_current(clipboard_offer, SelectionKind::Clipboard, clipboard, "text/plain"));
assert!(!broker.offer_is_current(clipboard_offer, SelectionKind::Primary, primary, "text/plain"));
```

- [ ] **Step 2: Run the focused tests and confirm the expected missing-broker API failures.**

Run: `cargo test selection --lib`

Expected: FAIL because the two-channel broker API and Primary offer validation do not yet exist.

- [ ] **Step 3: Implement the minimal broker.** Replace the single clipboard-only active-selection/offer state with two channel records. Store source kind/key, owner identity, normalized MIME types, generation, and channel-scoped offers. Return the replaced source so callers can issue cancellation. Make source removal clear only if the source is still active for its channel.

- [ ] **Step 4: Run the focused tests and refactor only after green.**

Run: `cargo test selection --lib`

Expected: PASS with no warnings.

- [ ] **Step 5: Commit the broker slice.**

```bash
git add src/compositor/selection.rs src/compositor/state/selection_runtime.rs src/compositor/mod.rs
git commit -m "feat(selection): add canonical clipboard and primary broker"
```

### Task 2: Wrap-safe input epochs and normal clipboard migration

**Files:**
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/surface_focus.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/protocols/data_device.rs`
- Test: existing data-device tests plus new stale-request regressions in `src/compositor/state/surfaces.rs`

**Interfaces:**
- `InputSerial` gains a monotonic `epoch: u64`.
- `remember_input_serial` allocates an epoch without using raw serial ordering.
- `validate_set_selection_serial` accepts only a matching client, eligible input kind, and current focus generation.

- [ ] **Step 1: Add failing stale-clear and obsolete-focus tests.** Exercise a valid serial from client A, change desktop focus, install a newer selection, then submit the old clear/replacement request. Assert that the newer selection remains active. Include a serial value near `u32::MAX` followed by wrap to prove validation does not use numeric ordering.

- [ ] **Step 2: Run the focused data-device tests and observe failure.**

Run: `cargo test compositor::tests::data_device --lib`

Expected: FAIL because older focus generations are currently accepted.

- [ ] **Step 3: Implement epoch allocation and exact focus-generation validation.** Increment a nonzero `u64` epoch whenever an input serial is remembered, retain it with the serial record, and require `input.focus_generation == self.focus_generation` for selection requests. Keep activation, cursor, popup, and DnD validation semantics unchanged unless their existing tests require the shared epoch field.

- [ ] **Step 4: Migrate normal clipboard commits, host bridge state, offers, and receive validation to broker Clipboard.** Remove duplicate active clipboard generation/offer truth where possible; retain host bridge transfer payload only as a backend adapter. Ensure replacement cancels the old source and normal clipboard offers remain ordered `data_offer`, `offer*`, `selection`.

- [ ] **Step 5: Run focused normal clipboard and DnD tests.**

Run: `cargo test compositor::tests::data_device --lib && cargo test --test xwayland_dnd`

Expected: PASS; any failure is fixed in the implementation, not weakened in the test.

- [ ] **Step 6: Commit the serial and clipboard slice.**

```bash
git add src/compositor/input.rs src/compositor/mod.rs src/compositor/state/surfaces.rs src/compositor/state/surface_focus.rs src/compositor/state/input_resources.rs src/compositor/protocols/data_device.rs
git commit -m "fix(selection): reject stale focus serials"
```

### Task 3: PRIMARY selection protocol

**Files:**
- Create: `src/compositor/protocols/primary_selection.rs`
- Modify: `src/compositor/protocols/mod.rs` or the repository’s protocol module declaration
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/selection_runtime.rs`
- Modify: `src/compositor/state/surfaces.rs` or focused selection state module
- Test: `src/compositor/tests/primary_selection.rs`

**Interfaces:**
- `PrimarySourceData { client_id: ClientId, source_key: SelectionSourceKey }`
- `PrimaryDeviceData { client_id: ClientId, seat_id: ObjectId }`
- `PrimaryOfferData { target_client_id: ClientId, generation: u64, source_key: SelectionSourceKey }`
- State methods `register_primary_source`, `offer_primary_mime_type`, `set_primary_selection`, `publish_primary_to_focused_client`, `receive_primary_offer`, and channel-scoped cleanup.

- [ ] **Step 1: Write failing wire tests.** Add a test capability profile with only PRIMARY enabled and cover global gating, source MIME registration, focused set-selection, publication ordering, focus transition publication, clear, invalid/foreign/obsolete serial rejection, source cancellation, active/stale source destruction, withdrawn/unsupported-MIME receive rejection, deterministic FD close, duplicate publication suppression, MIME bounds, and client teardown.

- [ ] **Step 2: Run the tests and verify they fail because the handlers are scaffolding.**

Run: `cargo test primary_selection --lib`

Expected: FAIL at source/device behavior rather than merely failing to compile the test harness.

- [ ] **Step 3: Implement manager, source, device, and offer dispatch.** Use the exact generated request/event names from `wayland-protocols` 0.32. Validate requested seat ownership, retain source identity, and issue `source.cancelled()` on replacement. Publish `DataOffer`, all `Offer` MIME events, then `Selection`. For `Receive`, validate target, channel, generation, active source, and MIME before sending `zwp_primary_selection_source_v1::Event::Send` with `fd.as_fd()`.

- [ ] **Step 4: Add focus and teardown hooks.** Publish current PRIMARY only to the intended focused client’s PRIMARY devices, immediately publish state when a device is registered, retire offers on channel changes, and remove resources on explicit destruction and client disconnect.

- [ ] **Step 5: Run the focused PRIMARY tests and the normal clipboard regression suite.**

Run: `cargo test primary_selection --lib && cargo test compositor::tests::data_device --lib`

Expected: PASS.

- [ ] **Step 6: Commit PRIMARY.**

```bash
git add src/compositor/protocols/primary_selection.rs src/compositor/mod.rs src/compositor/state/selection_runtime.rs src/compositor/state/surfaces.rs src/compositor/tests/primary_selection.rs
git commit -m "feat(wayland): implement primary selection"
```

### Task 4: ext-data-control protocol

**Files:**
- Create: `src/compositor/protocols/data_control.rs`
- Modify: `src/compositor/protocols/mod.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/selection_runtime.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`
- Test: `src/compositor/tests/data_control.rs`

**Interfaces:**
- `DataControlSourceData { client_id: ClientId, source_key: SelectionSourceKey, used: bool }`
- `DataControlDeviceData { client_id: ClientId, seat_id: ObjectId }`
- `DataControlOfferData { target_client_id: ClientId, kind: SelectionKind, generation: u64 }`
- State methods `register_data_control_source`, `set_data_control_selection`, `publish_data_control_state`, and channel-aware data-control receive/cleanup.

- [ ] **Step 1: Inspect generated bindings and write failing tests.** Confirm exact `used_source` error and request/event variants from the installed generated server bindings. Test gating, immediate Clipboard/PRIMARY publication, focus-independent observation, setting both channels without serials, cross-protocol reads, single-use source errors, cancellation, stale offers, multiple-device broadcasts, and teardown.

- [ ] **Step 2: Run focused tests and confirm scaffold failure.**

Run: `cargo test data_control --lib`

Expected: FAIL because source/device/offer requests are currently ignored.

- [ ] **Step 3: Implement generated request handlers with exact protocol errors.** A source can be committed once total; second use for either channel posts the generated `used_source` error. Enforce MIME policy, commit through the broker, cancel the replaced source, and broadcast current state to every data-control device.

- [ ] **Step 4: Implement offers and receive.** Create an offer for each channel update, send MIME events in order, validate channel/generation/source/MIME/client, and transfer through the normal Wayland source or host bridge adapter. Drop rejected FDs deterministically.

- [ ] **Step 5: Run ext-data-control, PRIMARY, normal clipboard, and DnD regressions.**

Run: `cargo test data_control --lib && cargo test primary_selection --lib && cargo test compositor::tests::data_device --lib && cargo test --test xwayland_dnd`

Expected: PASS.

- [ ] **Step 6: Commit ext-data-control.**

```bash
git add src/compositor/protocols/data_control.rs src/compositor/mod.rs src/compositor/state/selection_runtime.rs src/compositor/state/client_lifecycle.rs src/compositor/tests/data_control.rs
git commit -m "feat(wayland): implement ext data control"
```

### Task 5: Cross-protocol selection qualification

**Files:**
- Modify: `src/compositor/tests/data_device.rs`
- Modify: `src/compositor/tests/primary_selection.rs`
- Modify: `src/compositor/tests/data_control.rs`
- Modify: `src/compositor/state/selection_runtime.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`

- [ ] **Step 1: Add failing interoperability tests.** Cover normal Clipboard to data-control receive, data-control Clipboard to focused normal device receive, PRIMARY to data-control receive, data-control PRIMARY to focused PRIMARY device receive, independent Clipboard/PRIMARY offer validity, host bridge correctness, and DnD regressions.

- [ ] **Step 2: Run the cross-protocol tests and identify the missing shared-broker path.**

Run: `cargo test cross_protocol_selection --lib`

Expected: FAIL until all four source/consumer combinations use the same broker channels.

- [ ] **Step 3: Route all four combinations through broker-backed offers and transfer adapters.** Ensure channel generation changes invalidate only matching offers and device broadcasts do not duplicate unchanged selection events.

- [ ] **Step 4: Run all selection tests.**

Run: `cargo test selection --lib && cargo test data_device --lib && cargo test data_control --lib && cargo test primary_selection --lib`

Expected: PASS.

### Task 6: Surface-aware idle inhibition

**Files:**
- Modify: `src/compositor/idle.rs`
- Modify: `src/compositor/mod.rs`
- Create or modify: `src/compositor/protocols/idle_inhibit.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`
- Modify: relevant map/unmap/minimize/restore/layer-shell lifecycle modules
- Test: `src/compositor/tests/input_output/output_keyboard_cursor.rs` and idle model tests

**Interfaces:**
- `IdleInhibitorBinding { inhibitor, client_id, target_surface }`
- `IdleManager::reconcile_inhibited_count(count: usize)`
- `CompositorState::reconcile_idle_inhibition()` and `surface_tree_is_effectively_visible(root_surface_id)`.

- [ ] **Step 1: Write failing idle tests.** Cover exact target retention, unmapped/mapped, minimized/restored, unmap/remap, target/client destruction, multiple effective/ineffective inhibitors, subsurface root visibility, occlusion independence, repeated reconciliation, and no underflow.

- [ ] **Step 2: Run the focused idle tests and confirm shallow-count failure.**

Run: `cargo test idle_inhibit --lib`

Expected: FAIL because creation currently inhibits immediately even for an unmapped target and the target surface is discarded.

- [ ] **Step 3: Store exact target surface and derive eligibility.** Resolve a target to its root, require live resources and current renderable mapped content, reject minimized roots, accept restored roots, and treat occlusion as irrelevant. Keep layer-shell visibility in the existing authoritative mapped state.

- [ ] **Step 4: Reconcile idempotently on all lifecycle transitions.** Replace increment/decrement calls with reconciliation after create/destroy, client disconnect, surface teardown, map/unmap, minimize/restore, and layer visibility changes. Preserve the public `idle_inhibited()` query as a reconciliation point.

- [ ] **Step 5: Run focused idle and lifecycle tests.**

Run: `cargo test idle --lib && cargo test input_output::output_keyboard_cursor --lib && cargo test lifecycle --lib`

Expected: PASS.

- [ ] **Step 6: Commit idle inhibition.**

```bash
git add src/compositor/idle.rs src/compositor/mod.rs src/compositor/protocols/idle_inhibit.rs src/compositor/state/input_dispatch.rs src/compositor/state/client_lifecycle.rs src/compositor/state
git commit -m "feat(wayland): make idle inhibition surface aware"
```

### Task 7: Capability activation, documentation, and verification

**Files:**
- Modify: `src/compositor/plan.rs`
- Modify: `src/compositor/tests/lifecycle.rs`
- Modify: `src/compositor/tests/plan.rs`
- Modify: `src/compositor/tests/protocol_contract.rs` or relevant capability tests
- Modify: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`
- Modify: relevant protocol feature-status documentation

- [ ] **Step 1: Add failing capability assertions for the qualified profile.** Assert that `native_libinput()` enables `idle_inhibit` and `core_clipboard()` enables `primary_selection` and `data_control`, while safe baseline remains gated. Assert each global is registered exactly once at version 1.

- [ ] **Step 2: Run capability tests before activation and observe expected failures.**

Run: `cargo test plan --lib && cargo test lifecycle --lib`

Expected: FAIL because the production profile still leaves the three capabilities disabled.

- [ ] **Step 3: Activate capabilities only after protocol tests pass.** Set the three booleans in the production capability constructors and update intentional absence assertions. Do not alter safe baseline behavior.

- [ ] **Step 4: Update compliance documentation.** Mark the three protocols as implemented with the actual focus, generation, lifecycle, FD, and visibility guarantees. State that XWayland selection bridging remains out of scope and document the session-wide trust boundary of ext-data-control.

- [ ] **Step 5: Run the complete Windows-available verification.**

Run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
bin/check-source-layout
git diff --check
```

Expected: all commands exit 0. If Linux-only binaries or shell tests cannot run on Windows, record the exact command and reason without treating it as a passing result.

- [ ] **Step 6: Inspect the final diff and commit the completed work.** Check for duplicated selection state, stale source/offer mutation, FD leaks, missing teardown, DnD regressions, source-layout violations, and unrelated edits.

```bash
git status --short
git diff --stat
git diff --check
git commit -am "feat(wayland): qualify selection and idle inhibit protocols"
git rev-parse HEAD
```

Linux follow-up: run `wayland-info`, native PRIMARY copy/paste, an ext-data-control clipboard manager, and idle-inhibit map/minimize/unmap smoke checks in a real Wayland session.

