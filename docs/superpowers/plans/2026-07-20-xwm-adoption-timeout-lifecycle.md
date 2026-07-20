# XWM Adoption Timeout Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent an adoption diagnostic timeout from fabricating an X11 unmap and blocking late Steam window readiness.

**Architecture:** Keep the bounded `AdoptionTracker` deadline and expiration removal. Change expiration handling so it logs the expired wait but leaves property discovery and the X11 window record untouched; real X11 unmap/destroy events retain exclusive lifecycle ownership.

**Tech Stack:** Rust, x11rb XWM state machine, Cargo.

## Global Constraints

- Do not increase `ADOPTION_TIMEOUT_NS`.
- Do not query X11 or block while handling a deadline.
- Do not change real `UnmapNotify` or `DestroyNotify` handling.
- Add no dependencies.

---

### Task 1: Preserve Window State Across Adoption Expiration

**Files:**
- Modify: `src/xwayland/xwm/events.rs`
- Modify: `src/xwayland/xwm/mod.rs`

**Interfaces:**
- Consumes: `AdoptionTracker::expired(now_ns) -> Vec<(X11WindowHandle, AdoptionWait)>`.
- Produces: `Xwm::collect_adoption_expirations` that removes deadline entries without mutating `X11WindowRecord`.

- [ ] **Step 1: Write the failing regression**

Add `adoption_timeout_does_not_fabricate_unmap_before_late_readiness` to the existing `events.rs` XWM fixture. Prepare a managed window with properties ready, normalize its `MapNotify`, expire a `MapToAssociation` deadline, and assert `map_requested`, `mapped_notified`, and `properties_ready` remain true. Then provide matching X11/Wayland serials plus buffer readiness and assert exactly one `WindowReady` event is emitted.

- [ ] **Step 2: Verify RED**

```bash
cargo test --locked adoption_timeout_does_not_fabricate_unmap_before_late_readiness -- --nocapture
```

Expected: FAIL because current expiration calls `mark_unmapped` and clears the mapped lifecycle.

- [ ] **Step 3: Implement the minimal fix**

Replace property cancellation and `mark_unmapped` in `collect_adoption_expirations` with a diagnostic for each expired `(handle, wait)`:

```rust
eprintln!(
    "oblivion-one xwayland: event=adoption_timeout window={} wait={wait:?}",
    handle.xid(),
);
```

- [ ] **Step 4: Verify focused GREEN**

```bash
cargo test --locked adoption_timeout_does_not_fabricate_unmap_before_late_readiness -- --nocapture
cargo test --locked xwayland::xwm -- --nocapture
WAYLAND_DISPLAY=oblivion-one-sddm XDG_RUNTIME_DIR=/run/user/1000 cargo test --locked x11_window_reaches_window_ready_without_direct_fd_polling -- --nocapture
```

Expected: all commands PASS.

- [ ] **Step 5: Run the full verification and build**

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

- [ ] **Step 6: Commit**

```bash
git add src/xwayland/xwm/events.rs src/xwayland/xwm/mod.rs
git commit -m "fix(xwayland): preserve mapped windows after adoption timeout"
```

- [ ] **Step 7: Verify Steam after restart**

Launch Steam on the fresh managed display. Assert its main normal window is `IsViewable`, its XID appears in `_NET_CLIENT_LIST`, and Typhon logs `xwayland_window_admitted`; ignore `SLSTEAM` and `CLOUDREDIRECT` output.
