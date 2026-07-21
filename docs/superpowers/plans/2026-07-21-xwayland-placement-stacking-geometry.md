# Typhon XWayland Placement, Stacking, and Geometry Implementation Plan

> **For agentic workers:** Implement task-by-task with a failing regression before each behavior change. Keep placement, stacking, and ConfigureNotify/buffer changes in separate commits.

**Goal:** Give independent X11 windows compositor-owned placement and one authoritative layered stack while keeping X11 frame geometry separate from committed Wayland buffer content.

**Architecture:** Preserve one `WindowId` per X11 handle and keep Wayland surfaces replaceable. Add persistent X11 client/frame geometry and complete semantic window-type lists to `DesktopWindow`; derive placement, focus, layer, render order, hit testing, and `_NET_CLIENT_LIST_STACKING` from compositor state. ConfigureNotify updates frame/preview state only; Wayland commits remain the sole source of committed content dimensions.

**Tech Stack:** Rust, Wayland compositor state, XWayland/X11, x11rb, Cargo unit/integration tests.

## Global Constraints

- No Steam-specific branches, delays, retries, sleeps, or popup exceptions.
- Do not combine popup classification, stacking, placement, and ConfigureNotify changes in one commit.
- Every behavior change requires a failing regression before implementation.
- Preserve the existing replaceable Wayland attachment and persistent X11 identity model.

### Task 1: Persist X11 geometry and separate ConfigureNotify from buffers

**Files:** `src/compositor/desktop_window.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/server_backend.rs`, `src/compositor/tests/xwayland.rs`, `src/compositor/state/desktop_window_tests.rs`.

- Add `X11GeometryState { client, frame }` to X11 desktop windows.
- Seed it at admission and update it from X11 geometry/configure operations.
- Make partial configure/moveresize preserve persistent frame/client dimensions, never current `RenderableSurface` dimensions.
- Make `reconcile_x11_configure_notify()` update frame/preview geometry only; never mutate committed renderable width/height.
- Add red tests for 640x480 frame with 2x2 buffer and for ConfigureNotify-before-commit; commit independently.

### Task 2: Retain all EWMH window-type atoms

**Files:** `src/xwayland/xwm/window.rs`, `src/xwayland/xwm/properties.rs`, `src/xwayland/xwm/atoms.rs`, `src/compositor/desktop_window.rs`, role tests.

- Store the ordered atom list and expose semantic queries for popup, dialog, notification, and support types.
- Add Combo, Splash, Toolbar, Dock, Desktop, DND and existing menu/tooltip types.
- Classify by the first supported semantic type while retaining raw atoms for tracing.
- Add a red regression for `[unsupported atom, _NET_WM_WINDOW_TYPE_POPUP_MENU]`; commit separately.

### Task 3: Add role-aware initial placement

**Files:** `src/compositor/desktop_window.rs`, `src/compositor/state/desktop_windows.rs`, placement helpers, XWayland tests.

- Add `X11PlacementPolicy` and derive it from final semantic kind/type/transient metadata.
- Normal managed toplevels use compositor root/cascade placement.
- Dialogs/transients are floating and parent-constrained; popup/menu/tooltip/OR windows retain client positioning.
- Remaps retain persistent frame placement.
- Add a red regression proving two normal X11 windows requesting the same origin receive independent compositor positions; commit separately.

### Task 4: Establish one layered stack authority

**Files:** `src/compositor/desktop_window.rs`, `src/compositor/state/window_stacking.rs` or `desktop_windows.rs`, `src/compositor/server.rs`, `src/compositor/server_xwayland.rs`, XWM command integration, stack tests.

- Add `DesktopStackLayer` and a compositor-owned bottom-to-top `WindowId` order.
- Apply exact requested-window raises/restacks while preserving unrelated sibling order and enforcing child-above-parent constraints.
- Remove family-wide raise as the default mutation.
- Derive render order, hit-test order, X restack commands, and EWMH client-list stacking from the same order.
- Add red regressions for sibling popup raise, X sibling restack, layer precedence, and render/hit-test agreement; commit separately.

### Task 5: Complete attachment migration on the corrected model

**Files:** existing XWayland attachment bridge and resize/focus helpers.

- Retain persistent geometry, placement policy, stack slot, and role across surface replacement.
- Retire old map-local buffers/publication/feedback state before accepting the replacement.
- Transfer focus only to the active replacement attachment and preserve resize identity only for the valid map epoch.
- Add a cross-layer replacement regression after Tasks 1–4.

### Task 6: Deterministic and native qualification

- Add a multi-window Steam-like harness covering main window, dialogs, sibling popups, tooltip, OR helper, remap, and resize.
- Assert one identity per XID, independent normal placement, layer/render/hit-test agreement, popup focus denial, commit-only buffer extent changes, and no stale attachment state.
- Run `cargo fmt --check`, locked check/clippy/tests, source-layout, diff check, release build, and native TTY qualification when available.

