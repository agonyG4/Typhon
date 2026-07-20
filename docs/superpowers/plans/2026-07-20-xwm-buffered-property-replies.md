# XWM Buffered Property Replies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure XWM property replies already buffered inside `x11rb` are drained so managed X11 windows can be mapped and admitted.

**Architecture:** Keep the single XWM reactor and existing bounded budgets. After draining X events, invoke the existing nonblocking property reply poll unconditionally; unavailable replies remain pending through the current `WouldBlock` path.

**Tech Stack:** Rust, x11rb, Unix socket-pair X11 protocol fixtures, Cargo.

## Global Constraints

- Do not change association, buffer-readiness, adoption-timeout, or command-execution semantics.
- Preserve the existing maximum event and property reply budgets.
- Add no dependencies.

---

### Task 1: Drain Buffered Property Replies

**Files:**
- Modify: `src/xwayland/xwm/events.rs`
- Modify: `src/xwayland/xwm/mod.rs`
- Modify: `src/xwayland/xwm/properties.rs`

**Interfaces:**
- Consumes: `properties::poll_replies(&mut Xwm, usize) -> Result<usize, XwmError>` and `events::drain(&mut Xwm, usize) -> Result<XwmDrain, XwmError>`.
- Produces: `Xwm::drain_events` that attempts both bounded drains on every reactor dispatch.

- [ ] **Step 1: Write the failing regression**

In the `src/xwayland/xwm/events.rs` test module, use the existing socket-pair `test_fixture`. Observe a managed window so initial property requests are pending, write empty `GetPropertyReply` packets for each pending sequence followed by one benign X event to the peer, and call `events::drain` once so `x11rb` buffers the replies while returning the event. Assert `properties::socket_has_input(xwm.raw_fd)` is false, then call `xwm.drain_events(256)` and assert the window record has `properties_ready == true` and `pending_properties == 0`.

- [ ] **Step 2: Run the regression and verify RED**

Run:

```bash
cargo test --locked buffered_property_replies_are_drained_without_raw_socket_input -- --nocapture
```

Expected: FAIL because the raw-fd gate prevents `poll_replies` from consuming replies already buffered by `x11rb`.

- [ ] **Step 3: Implement the minimal fix**

Change `Xwm::drain_events` to always call:

```rust
let drain = events::drain(self, budget.min(XWM_EVENT_BUDGET))?;
self.poll_root_event_mask()?;
let _ = properties::poll_replies(self, budget.min(XWM_EVENT_BUDGET))?;
Ok(drain)
```

Remove `properties::socket_has_input` if it has no remaining callers and adjust test-only imports accordingly.

- [ ] **Step 4: Verify focused GREEN**

Run:

```bash
cargo test --locked buffered_property_replies_are_drained_without_raw_socket_input -- --nocapture
cargo test --locked xwayland::xwm -- --nocapture
WAYLAND_DISPLAY="$WAYLAND_DISPLAY" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" cargo test --locked x11_window_reaches_window_ready_without_direct_fd_polling -- --nocapture
```

Expected: all tests PASS.

- [ ] **Step 5: Run the full project verification**

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

Expected: every command exits 0.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/xwayland/xwm/events.rs src/xwayland/xwm/mod.rs src/xwayland/xwm/properties.rs
git commit -m "fix(xwayland): drain buffered property replies"
```

Expected: one scoped implementation commit with the regression and minimal fix.

- [ ] **Step 7: Verify natively after restart**

Restart Typhon with `target/release/oblivion-one`, launch Steam on its managed XWayland display, and verify the normal Steam window is `IsViewable`, its XID appears in `_NET_CLIENT_LIST`, and `session.log` contains `xwayland_window_admitted` for its associated surface. Ignore `SLSTEAM` and `CLOUDREDIRECT` messages.
