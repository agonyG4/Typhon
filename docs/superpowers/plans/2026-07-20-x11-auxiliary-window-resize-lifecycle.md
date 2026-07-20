# X11 Auxiliary Window and Resize Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove Steam's auxiliary black window, finalize X11 interactive resize correctly, and keep override-redirect helpers out of EWMH client lists.

**Architecture:** X11 interaction state is finalized in the compositor because X11 has no XDG configure serial. EWMH filtering remains in desktop-window state, while auxiliary support-window detection remains in XWM property and lifecycle ownership.

**Tech Stack:** Rust, Cargo unit and integration tests, X11 ICCCM/EWMH properties, Xwayland-shell associations.

## Global Constraints

- Do not use Steam-specific class or title rules.
- Preserve ordinary managed dialogs and typed/input-capable small windows.
- Preserve override-redirect rendering while excluding it from EWMH client lists.
- Do not change XDG resize configure/commit completion.
- Keep the running Typhon session alive until a verified release build is ready.

---

### Task 1: Finalize X11 interactive resize previews

**Files:**
- Modify: `src/compositor/state/window_resize.rs`
- Test: `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Consumes: `CompositorState::send_resize_end_configure`, `active_toplevel_resizes`, `toplevel_visual_geometries`, and `update_toplevel_visual_render_assignment`.
- Produces: non-XDG resize completion that queues one final backend configure and leaves no active preview or visual clip.

- [ ] **Step 1: Write the failing X11 interaction regression**

Create an X11 desktop window and renderable surface, install an active resize
and visual geometry with `active_resize: Some(interaction_id)`, end the
matching committed interaction, and assert:

```rust
assert!(!state.active_toplevel_resizes.contains_key(&surface_id));
assert_eq!(
    state.toplevel_visual_geometries[&surface_id].active_resize,
    None
);
assert_eq!(
    state.renderable_surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .and_then(|surface| surface.visual_clip),
    None
);
assert_eq!(state.surface_placement(surface_id), final_placement);
assert_eq!(state.take_backend_commands().len(), 1);
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked x11_resize_release_finalizes_preview_without_xdg_commit -- --nocapture
```

Expected: FAIL because `active_toplevel_resizes` and the visual clip remain
after X11 interaction release.

- [ ] **Step 3: Implement X11-only finalization**

In the non-XDG branch of `send_resize_end_configure`, after queuing the final
backend configure, commit `geometry.placement`, remove only the matching X11
active resize, clear `visual.active_resize`, and call
`update_toplevel_visual_render_assignment(surface_id)`.

- [ ] **Step 4: Verify the focused interaction suite**

Run:

```bash
cargo test --locked x11_resize_release_finalizes_preview_without_xdg_commit -- --nocapture
cargo test --locked compositor::state::window_interaction_tests -- --nocapture
```

Expected: all tests pass with zero failures.

- [ ] **Step 5: Commit the resize fix**

```bash
git add src/compositor/state/window_resize.rs src/compositor/state/window_interaction_tests.rs
git commit -m "fix(xwayland): finalize X11 resize previews on release"
```

### Task 2: Publish only managed X11 clients through EWMH

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs`
- Test: `src/compositor/state/desktop_window_tests.rs`

**Interfaces:**
- Consumes: `CompositorState::x11_client_lists() -> (Vec<X11WindowHandle>, Vec<X11WindowHandle>)`.
- Produces: the same interface filtered to `DesktopWindowKind::Managed` in identity and stacking order.

- [ ] **Step 1: Write the failing EWMH regression**

Extend `x11_client_lists_follow_identity_and_generic_stacking` with one
override-redirect snapshot inserted between two managed snapshots. Require
both returned lists to contain only the managed handles in their existing
identity/stacking order.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked x11_client_lists_follow_identity_and_generic_stacking -- --nocapture
```

Expected: FAIL because the override-redirect handle is currently present.

- [ ] **Step 3: Implement the managed-kind filters**

Apply the same predicate before both backend matches:

```rust
.filter(|window| window.kind == DesktopWindowKind::Managed)
```

- [ ] **Step 4: Verify desktop-window state tests**

Run:

```bash
cargo test --locked x11_client_lists_follow_identity_and_generic_stacking -- --nocapture
cargo test --locked compositor::state::desktop_window_tests -- --nocapture
```

Expected: all tests pass with zero failures.

- [ ] **Step 5: Commit the EWMH fix**

```bash
git add src/compositor/state/desktop_windows.rs src/compositor/state/desktop_window_tests.rs
git commit -m "fix(xwayland): exclude popup helpers from client lists"
```

### Task 3: Suppress protocol-identified client-leader support windows

**Files:**
- Modify: `src/xwayland/xwm/atoms.rs`
- Modify: `src/xwayland/xwm/properties.rs`
- Modify: `src/xwayland/xwm/window.rs`
- Test: `src/xwayland/xwm/window.rs`
- Test: `src/xwayland/xwm/properties.rs`

**Interfaces:**
- Consumes: the existing bounded property request/parser pipeline and `X11WindowRegistry::try_ready`.
- Produces: `client_leader: Option<X11WindowHandle>`, `user_time_window: Option<X11WindowHandle>`, and `X11WindowLifecycle::Auxiliary`, without changing public compositor snapshots.

- [ ] **Step 1: Write failing property and lifecycle regressions**

Add parser coverage proving `WM_CLIENT_LEADER` and
`_NET_WM_USER_TIME_WINDOW` accept one `WINDOW` value. Add a registry test that
builds a ready 10x10 managed record with self leader, distinct user-time
window, absent window type, and absent input hint, then requires:

```rust
assert_eq!(registry.try_ready(window).expect("known window"), None);
assert_eq!(
    registry.get(window).expect("support window").lifecycle,
    X11WindowLifecycle::Auxiliary
);
```

Add a paired test where `window_type = Some(X11WindowType::Normal)` and
`accepts_input = Some(true)` still produces a ready snapshot.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test --locked client_leader_support_window_is_not_desktop_ready -- --nocapture
cargo test --locked tiny_typed_input_window_remains_desktop_ready -- --nocapture
```

Expected: the support-window test fails because the current registry emits a
normal `X11WindowSnapshot`; the control test passes.

- [ ] **Step 3: Extend bounded property collection**

Add `WmClientLeader` and `NetWmUserTimeWindow` to `XwmAtomName`,
`PropertyKind::ALL`, request types, parsed properties, staged/final property
commit, and snapshot storage. Parse exactly the first 32-bit window value and
generation-bind it with `X11WindowHandle::new(handle.generation(), xid)`.

- [ ] **Step 4: Implement the auxiliary predicate at readiness ownership**

Before creating `X11WindowSnapshot` in `try_ready`, evaluate the complete
record with an `is_auxiliary_client_leader` helper. Require managed kind,
width and height at most 16, self leader, distinct nonzero user-time window,
no window type, and no input hint. Set lifecycle to `Auxiliary` and return
`Ok(None)` only for that exact signature.

- [ ] **Step 5: Verify XWM behavior**

Run:

```bash
cargo test --locked client_leader_support_window_is_not_desktop_ready -- --nocapture
cargo test --locked tiny_typed_input_window_remains_desktop_ready -- --nocapture
cargo test --locked xwayland::xwm -- --nocapture
env WAYLAND_DISPLAY=oblivion-one-sddm XDG_RUNTIME_DIR=/run/user/1000 cargo test --locked x11_window_reaches_window_ready_without_direct_fd_polling -- --nocapture
```

Expected: all tests pass with zero failures and the normal native X11 window
still reaches `WindowReady`.

- [ ] **Step 6: Commit auxiliary classification**

```bash
git add src/xwayland/xwm/atoms.rs src/xwayland/xwm/properties.rs src/xwayland/xwm/window.rs
git commit -m "fix(xwayland): suppress auxiliary client-leader windows"
```

### Task 4: Repository and live Steam verification

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: the three verified implementation commits and the live Typhon Xwayland display.
- Produces: a release binary and exact live evidence for client publication and resize teardown.

- [ ] **Step 1: Run repository verification**

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

- [ ] **Step 2: Verify live behavior after restart**

Confirm the running compositor inode matches `target/release/oblivion-one`,
launch Steam on the managed Xwayland display, and inspect root properties and
the persistent main XID before and after one move and one resize.

Expected: `_NET_CLIENT_LIST` contains the persistent Steam main XID but not
the 10x10 leader or override-redirect helpers; no black cube is present; the
main XID remains a single viewable normal window; and resize release leaves
no clipped/duplicated visual representation.
