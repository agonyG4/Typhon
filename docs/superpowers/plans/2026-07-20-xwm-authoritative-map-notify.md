# XWM Authoritative MapNotify Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Adopt an authoritative managed `MapNotify` even when Typhon did not observe the preceding `MapRequest`.

**Architecture:** Keep the mapping transition in `X11WindowRegistry`, where X11 lifecycle state is owned. Event normalization accepts the completed transition, refreshes properties as needed, emits readiness, and logs the external-map path without issuing a redundant X11 map command.

**Tech Stack:** Rust, Cargo unit tests, Xwayland/XWM lifecycle registry and event normalization.

## Global Constraints

- Apply identical authoritative `MapNotify` semantics to managed and override-redirect windows.
- Do not issue `WindowMapRequested` after consuming an unrequested `MapNotify`.
- Preserve real `UnmapNotify` as the boundary that clears mapping and retained-buffer state.
- Verify Steam's exact persistent main XID after rebuilding and restarting.

---

### Task 1: Adopt an unrequested managed MapNotify

**Files:**
- Modify: `src/xwayland/xwm/events.rs`
- Modify: `src/xwayland/xwm/window.rs`
- Test: `src/xwayland/xwm/events.rs`

**Interfaces:**
- Consumes: `X11WindowRegistry::confirm_external_map_notify(X11WindowHandle) -> Result<bool, &'static str>` and `Xwm::emit_ready_if_complete(X11WindowHandle) -> Result<bool, XwmError>`.
- Produces: unchanged interfaces with authoritative managed-map semantics and an `xwm_map_notify` diagnostic carrying `pending_map=false`.

- [ ] **Step 1: Write the failing event regression**

Add a test beside the existing external mapping tests that inserts an observed
managed window without calling `mark_map_requested`, marks its properties,
association, and buffer ready, and normalizes `map_event(handle.xid(), false)`.
The test must require exactly one `WindowReady`, no `WindowMapRequested`, and
`map_requested`, `map_authorized`, and `mapped_notified` all true.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked managed_map_notify_without_prior_request_is_adopted_as_mapped -- --nocapture
```

Expected: FAIL because the current managed external-map path converts the
consumed notification into a fresh map request and emits no readiness event.

- [ ] **Step 3: Implement the minimal lifecycle fix**

In `confirm_external_map_notify`, remove the desktop-kind restriction and
the false return for a missing map request. For every live record, set:

```rust
record.map_requested = true;
record.map_operation_pending = false;
record.map_authorized = true;
record.mapped_notified = true;
record.lifecycle = if record.associated.is_some() {
    X11WindowLifecycle::MappedAssociated
} else {
    X11WindowLifecycle::MappedAwaitingAssociation
};
```

Return `Ok(true)`. In the non-pending `MapNotify` event branch, keep property
refresh and readiness emission, remove the now-unreachable fallback that
calls `mark_map_requested`, and log `xwm_map_notify` with
`pending_map=false`, readiness result, and lifecycle.

- [ ] **Step 4: Verify focused and related behavior**

Run:

```bash
cargo test --locked managed_map_notify_without_prior_request_is_adopted_as_mapped -- --nocapture
cargo test --locked map_notify_before_map_command_is_classified_as_external_mapping -- --nocapture
cargo test --locked managed_unmap_requires_a_fresh_buffer_before_remap -- --nocapture
cargo test --locked xwayland::xwm -- --nocapture
env WAYLAND_DISPLAY=oblivion-one-sddm XDG_RUNTIME_DIR=/run/user/1000 cargo test --locked x11_window_reaches_window_ready_without_direct_fd_polling -- --nocapture
```

Expected: all commands pass with zero failures.

- [ ] **Step 5: Run repository verification**

Run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
cargo build --locked --release
```

Expected: every command exits zero and the complete test suite has zero failures.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/xwayland/xwm/events.rs src/xwayland/xwm/window.rs
git commit -m "fix(xwayland): adopt authoritative managed map notifications"
```

- [ ] **Step 7: Verify live Steam admission after restart**

Confirm the running compositor inode matches `target/release/oblivion-one`,
launch Steam on the managed Xwayland display, and inspect its exact persistent
main XID.

Expected: `xwininfo` reports `IsViewable` and non-override,
`_NET_CLIENT_LIST` contains the exact XID, and the current session log
contains matching `xwm_map_notify pending_map=false` and
`xwayland_window_admitted` diagnostics.
