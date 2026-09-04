# Typhon XWayland Decoration Semantics Closure Implementation Plan

> **For agentic workers:** This plan is executed inline in the current task; no sub-agents are used.

**Goal:** Keep `_NET_FRAME_EXTENTS`, structural X11 metadata, and active interaction ownership correct across every X11 mode and decoration transition.

**Architecture:** Extend the existing compositor-owned X11 backend configure translation so a full configure publishes the canonical server frame extents in the same XWM command. Gate transient/workspace reconstruction on the existing structural-delta classification. Carry one explicit frame-ownership bit from decoration hit-testing into `WindowInteraction` so mode changes cancel only frame-owned native interactions.

**Tech Stack:** Rust, Cargo, x11rb, existing Typhon compositor/XWayland test fixtures.

## Global Constraints

- Preserve `X11DecorationHints`, `_GTK_FRAME_EXTENTS`, `_MOTIF_WM_HINTS`, canonical effective decoration mode, and authoritative X11 geometry.
- Apply the frame-extents correction at the common compositor-owned X11 backend/configure boundary.
- Do not add application-specific Chromium/Electron/ChatGPT/WM_CLASS/app_id behavior.
- Add focused regression tests before production behavior changes.
- Use `rtk` for validation commands and commit the completed changes.

### Task 1: Add failing lifecycle and ownership tests

**Files:**
- Modify: `src/compositor/tests/xwayland_decoration.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/state/desktop_window_tests.rs`
- Modify: `src/xwayland/xwm/properties_decoration_tests.rs`

- [ ] Add tests for mode lifecycle frame-extents publication, fullscreen hint changes, and normal/maximized extents without authoritative geometry drift.
- [ ] Add tests proving decoration-only metadata leaves role, placement, stack, transient, workspace, and client-list state unchanged.
- [ ] Add paired interaction tests for frame-owned and client-content-owned `NativeBinding` interactions.
- [ ] Add window-type-before/after-Motif parity and complete `_GTK_FRAME_EXTENTS` parser-reply cases.
- [ ] Run the focused tests and confirm the new expectations fail for the current implementation.

### Task 2: Correct common X11 frame-extents publication

**Files:**
- Modify: `src/compositor/server_backend.rs`
- Modify: `src/compositor/tests/xwayland_decoration.rs`

- [ ] Translate a full compositor-owned X11 backend configure to `XwmCommand::ConfigureFrame` with `state.x11_decoration_frame_extents(handle)`.
- [ ] Preserve resize synchronization, position-only configure behavior, client-request configure behavior, and authoritative geometry.
- [ ] Verify no extra geometry configure is emitted for a mode transition.

### Task 3: Isolate decoration metadata reconciliation

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/tests/xwayland_decoration.rs`

- [ ] Run workspace-membership refresh, transient reconstruction, and workspace inheritance reconciliation only for `structure_dirty` metadata.
- [ ] Keep constraint reconciliation and structural dirty publication behavior unchanged for their existing delta classes.

### Task 4: Track frame interaction ownership explicitly

**Files:**
- Modify: `src/compositor/interaction.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: interaction and decoration tests that construct the state structs.

- [ ] Add a private frame-owned marker to interaction start/state data.
- [ ] Set it only for titlebar/border hit starts; client-content and X11 protocol starts remain false.
- [ ] Make decoration-transition reconciliation cancel only frame-owned move/resize interactions.
- [ ] Run focused interaction tests, then the complete validation suite.

### Task 5: Verify, review, and commit

**Files:**
- Review the final diff and all changed files.

- [ ] Run `rtk cargo fmt --check`.
- [ ] Run `rtk cargo check --locked --all-targets`.
- [ ] Run `rtk cargo clippy --locked --all-targets -- -D warnings`.
- [ ] Run focused XWayland decoration/property tests separately.
- [ ] Run `TMPDIR=/tmp/t rtk cargo test --locked`.
- [ ] Run `rtk run -- git diff --check` and review for duplicate configures, loops, geometry drift, structural regressions, and hacks.
- [ ] Run the managed XWayland live qualification if the environment is available; report if it is not.
- [ ] Commit the implementation and verification-ready result.
