# XWM Pre-Map Buffer Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow a first X11 map to consume buffer readiness retained before `MapRequest` while preserving the fresh-buffer requirement after a real unmap.

**Architecture:** Keep the change inside `X11WindowRegistry`, where mapping lifecycle state is owned. `mark_map_requested` preserves the current buffer flag; `mark_unmapped` remains the sole withdrawal boundary that clears it.

**Tech Stack:** Rust, Cargo unit tests, Xwayland/XWM lifecycle registry.

## Global Constraints

- Do not change association, compositor, timeout, or command interfaces.
- Real `UnmapNotify` must continue to require a fresh buffer before remap.
- Verify the exact Steam main XID in `_NET_CLIENT_LIST` after rebuilding and restarting.

---

### Task 1: Preserve retained readiness on the initial map

**Files:**
- Modify: `src/xwayland/xwm/window.rs`
- Test: `src/xwayland/xwm/window.rs`

**Interfaces:**
- Consumes: `X11WindowRegistry::mark_map_requested(X11WindowHandle) -> Result<(), &'static str>` and the existing association/buffer/map lifecycle methods.
- Produces: unchanged `mark_map_requested` interface with retained initial buffer semantics.

- [ ] **Step 1: Write the failing registry regression**

Add this test beside the existing mapping-gate tests:

```rust
#[test]
fn buffer_before_first_map_request_completes_mapping_gate() {
    let generation = generation(1);
    let window = handle(generation, 26);
    let mut registry = X11WindowRegistry::default();
    registry.insert_observed_with_kind(
        window,
        DesktopWindowKind::Managed,
        X11Geometry::default(),
    );
    registry
        .mark_associated(window, associated(generation, 9, 45))
        .expect("association");
    registry.mark_buffer_ready(window).expect("retained buffer");

    registry.mark_map_requested(window).expect("first map request");
    complete_properties(&mut registry, window);
    registry.mark_map_commanded(window).expect("map command");
    registry.confirm_map_notify(window).expect("map notify");

    let snapshot = registry
        .try_ready(window)
        .expect("known window")
        .expect("retained buffer completes first map");
    assert_eq!(snapshot.surface_id, 45);
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked buffer_before_first_map_request_completes_mapping_gate -- --nocapture
```

Expected: FAIL at `retained buffer completes first map` because `mark_map_requested` clears `buffer_ready`.

- [ ] **Step 3: Implement the minimal lifecycle fix**

In `X11WindowRegistry::mark_map_requested`, remove only the unconditional initial-map reset:

```rust
record.snapshot = None;
record.properties_ready = false;
```

Do not add a replacement `buffer_ready` assignment. `mark_unmapped` already sets it to `false` for remaps.

- [ ] **Step 4: Verify focused and remap behavior**

Run:

```bash
cargo test --locked buffer_before_first_map_request_completes_mapping_gate -- --nocapture
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

Expected: every command exits zero; the complete test suite has zero failures.

- [ ] **Step 6: Commit the implementation**

```bash
git add src/xwayland/xwm/window.rs
git commit -m "fix(xwayland): retain buffers across first map request"
```

- [ ] **Step 7: Verify live Steam admission after restart**

Confirm the running compositor inode matches `target/release/oblivion-one`, launch Steam on the managed Xwayland display, and inspect its exact main XID.

Expected: `xwininfo` reports `IsViewable` and non-override, `_NET_CLIENT_LIST` contains the exact XID, and the current session log contains `xwayland_window_admitted` for its associated surface.
