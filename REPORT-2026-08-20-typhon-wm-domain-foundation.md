# Typhon WM Domain Foundation v1

Date: 2026-08-20

## Status

Implemented in the current Typhon checkout without resetting, stashing, cleaning, staging, committing, or creating another worktree. The existing dirty working tree was preserved as user-owned work.

## Baseline and repository state

- Baseline `HEAD`: `0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6`.
- Initial `git status --short`: a substantially dirty checkout with tracked changes across compositor, native-output, renderer, control, and support code, plus untracked closure reports, plans, and source files.
- Initial `git diff --stat`: 87 files, `6104 insertions(+), 829 deletions(-)`.
- The current checkout, rather than `HEAD`, remained authoritative throughout.
- The final status remains intentionally dirty. It contains the same broad pre-existing tracked and untracked work, the task plan, the new WM-domain files, and this report; no unrelated work was staged or altered.

The codebase-memory graph was checked before and after the implementation. The final operated-on source paths had matching metadata and no recorded coverage gaps. This is a best-effort coverage signal, not proof of source completeness.

## Architecture found and resulting ownership

The actual live-window authority is the compositor's `CompositorState::desktop_windows: HashMap<WindowId, DesktopWindow>`, with root-surface and X11-handle reverse indexes. `DesktopWindow` owns backend identity, protocol metadata, relationships, state, and geometry-facing compositor data. The WM layer now owns only neutral management policy/state.

The ownership chain is now:

```text
core::WindowId
    -> compositor::DesktopWindow and its existing registry
    -> wm::WindowManagementState
       -> WorkspaceId + LayoutMembership
    -> existing compositor geometry and WindowVisual mechanisms
```

No Wayland, XWayland, EGL, renderer, DRM/KMS, surface, or input object was moved into `src/wm`.

## Canonical window identity

Before this task, the real compositor identity was a `NonZeroU64`-backed `compositor::WindowId`, while `src/wm/mod.rs` also contained a separate prototype identity and registry model.

After this task:

- `src/core/window_id.rs` contains the one concrete `WindowId(NonZeroU64)` definition.
- It preserves `Debug`, `Copy`, equality, hashing, ordering, `from_raw`, `get`, and `raw` semantics.
- Public construction rejects zero; unchecked construction remains crate-private.
- The compositor allocator remains the owner of monotonic session-local allocation and does not reuse IDs.
- `crate::compositor::WindowId` and `crate::wm::WindowId` are compatibility re-exports of the core type.
- Search found exactly one `pub struct WindowId` and no `WmWindowId`, `CompositorWindowId`, conversion table, or second allocator.

## Retirement of the WM prototype

The old `ManagedWindow`, duplicate `WindowId`, rectangle authority, focus authority, and `WindowManager` prototype were removed from `src/wm/mod.rs`. The module is now a small facade over focused `window.rs` and `workspace.rs` modules. The compositor `DesktopWindow` registry remains the only live-window registry.

## Protocol mode and layout membership

`ToplevelMode::Floating` was renamed to `ToplevelMode::Normal` throughout the compositor, XDG paths, XWayland paths, restore paths, decorations, and tests. The enum now contains only:

```text
Normal, Maximized, Fullscreen
```

The XDG publication semantics are explicit and tested:

- `Normal` publishes no maximized/fullscreen state.
- `Maximized` publishes `xdg_toplevel::State::Maximized`.
- `Fullscreen` publishes `xdg_toplevel::State::Fullscreen`.

Restore geometry is captured only while protocol mode is `Normal`. It is not coupled to layout membership, so the future state `layout = Tiled, protocol mode = Normal` remains representable.

## WM domain model

`src/wm/workspace.rs` adds `WorkspaceId(NonZeroU32)` with explicit non-zero construction, numeric accessors, `Display`, and `Copy`/`Eq`/`Hash`/`Ord` behavior. It is not limited to ten IDs.

`WorkspaceManager` owns the active workspace and a deterministic collection of known workspaces. Its default is workspaces 1 through 10 with workspace 1 active. Construction of zero workspaces is rejected, unknown-workspace queries are safe, and there is no switching behavior yet.

`src/wm/window.rs` adds:

- `LayoutMembership::{Floating, Tiled}`;
- `WindowManagementState { workspace, layout }`, defaulting new state to `Floating`.

The management state is independent of protocol mode, minimized state, decorations, backend objects, and geometry storage. `src/wm` has no frame tick, polling, timer, thread, lock, renderer, KMS, or protocol dependency.

## Workspace eligibility and DesktopWindow integration

On insertion into the existing desktop-window registry, eligible windows receive the active workspace and `LayoutMembership::Floating`:

- normal XDG desktop windows;
- normal compositor-managed X11 toplevels;
- managed X11 dialogs.

The following remain without independent workspace membership and retain their existing ownership rules:

- XDG popups;
- X11 override-redirect windows;
- menus, tooltips, notifications, helper/support windows;
- layer-shell surfaces, cursor surfaces, drag icons, and subsurfaces.

X11 metadata reclassification refreshes eligibility, so an existing managed window becoming override-redirect loses management state, while a later eligible reclassification can receive it. Existing cascade placement and stacking/transient behavior remain in the compositor path.

## Control snapshots

The existing `WindowSnapshot.workspace` field is now populated from `DesktopWindow.management`. Managed default windows publish `Some("1")`; auxiliary/non-managed windows publish `None`. No new control protocol or switching command was added.

## Preservation of recent compositor behavior

The implementation did not create a competing geometry authority or layout transaction system. `ToplevelVisualGeometry`, immediate interactive visual resize, bounded/coalesced configure flow, resize ownership/capture, pointer-scene ownership, render/damage behavior, KMS scheduling, and input routing remain in their existing compositor/native-output paths. `MAX_IN_FLIGHT_RESIZE_CONFIGURES` remains 3. Workspace assignment does not enter the render loop and does not create per-frame workspace allocations, scans, locks, or configure traffic.

## Tests added and focused validation

New or updated deterministic coverage includes:

- canonical `WindowId` zero rejection, raw round-trip, and compositor/core type identity;
- `WorkspaceId` construction/display and `WorkspaceManager` defaults/queries;
- `WindowManagementState` explicit workspace, Floating default, and representable Tiled membership;
- Normal/Maximized/Fullscreen XDG state publication and Normal-only restore capture;
- managed XDG and managed X11 admission into workspace 1 as Floating;
- auxiliary X11 admission and later override-redirect reclassification without management state;
- control snapshot workspace publication.

Focused successful runs included:

```text
cargo fmt --check                                      PASS
cargo check --locked --all-targets                    PASS
git diff --check                                       PASS
cargo test --locked workspace -- --nocapture           5 passed
cargo test --locked canonical_window_id -- --nocapture 1 passed
cargo test --locked xdg_toplevel_creation -- --nocapture 1 passed
protocol mode test                                     1 passed
restore geometry test                                  1 passed
cargo test --locked desktop_window -- --nocapture     59 passed
cargo test --locked window_interaction -- --nocapture  64 passed
cargo test --locked window_decoration -- --nocapture   19 passed
cargo test --locked native_geometry -- --nocapture      6 passed
cargo test --locked resize -- --nocapture              compositor: 166 passed
```

The final resize-filtered run also exercised the native-output test target: 8 tests passed and 1 current-checkout/order-sensitive native input test failed at an existing exact-cursor assertion (`left: 0`, `right: 1`) followed by its worker `RecvError`. No WM-domain test failed.

The XWayland-filtered run completed with 372 passed, 33 failed, and 1 ignored. The failures begin in existing test setup with `path must be shorter than SUN_LEN`; subsequent display-lease tests report poisoned test locks. The relevant XWayland window/focus/resize behavior tests that reached execution passed.

The required full `cargo test --locked` attempt completed with 1,675 passed, 36 failed, and 2 ignored. The failures were:

- Unix-socket path-length failures in Astrea discovery and XWayland setup;
- poisoned follow-on XWayland lease tests from that setup failure;
- one pre-existing direct-scanout eligibility failure.

These failures are outside the WM-domain changes and were not weakened or worked around in product code.

## Known pre-existing validation blockers

`cargo clippy --locked --all-targets -- -D warnings` is blocked by the existing `clippy::large_enum_variant` warning for `src/xwayland/xwm/event_types.rs::XwmEvent`; no task-owned warning was reported.

`bin/check-source-layout` still reports the known oversized files:

```text
src/compositor/tests/windows.rs       2002 lines (limit 2000)
src/compositor/state/windows.rs       1562 lines (limit 1500)
src/compositor/server.rs              1519 lines (limit 1500)
src/compositor/mod.rs                  816 lines (limit 800)
```

The first three were already over their limits. `src/compositor/mod.rs` was already over limit at the baseline; this task added only the minimal import/re-export/manager-field wiring required by the integration and did not perform an unrelated refactor. No new standalone WM file violates the layout limits. No native hardware qualification was claimed.

## Review Pass 1 — correctness and ownership

Checks performed:

- searched for duplicate identity definitions and retired WM prototype symbols;
- verified the sole live-window registry is the existing compositor `DesktopWindow` map;
- checked all mode/restore/XDG/XWayland references after the rename;
- checked managed XDG/X11 and auxiliary role classification;
- checked that management state is attached directly to `DesktopWindow` rather than a parallel map.

The review identified that the X11 metadata reclassification path needed an explicit regression assertion. That assertion was added: converting a managed X11 window to override-redirect leaves `management == None`. The workspace-manager zero-construction test was also made explicit. The review then found no remaining correctness or ownership issue.

## Review Pass 2 — performance and future compatibility

Checks performed:

- searched render, native-output, input, pointer-hit, and resize paths for workspace work;
- verified no per-frame workspace calculation, scan, allocation, lock, or thread was added;
- verified `ToplevelVisualGeometry` and the three-configure bound remain present;
- verified no speculative layout transaction, chrome policy, Dwindle, workspace-switching, or client barrier was introduced;
- verified the workspace identity is extensible beyond ten and is not tied to an output.

No performance or future-compatibility issue was found. Workspace state is event/state-driven and idle workspace CPU work remains zero.

## Final working-tree status

`git status --short` remains dirty by design. It includes the pre-existing broad tracked modifications and untracked closure/planning/source artifacts, plus these task-owned additions or edits:

At final verification it contained 126 entries: 95 tracked modifications and 31 untracked paths.

```text
M  src/compositor/desktop_window.rs
M  src/compositor/mod.rs
M  src/compositor/protocols/xdg.rs
M  src/compositor/server_control.rs
M  src/compositor/state/desktop_window_tests.rs
M  src/compositor/state/desktop_windows.rs
M  src/compositor/state/windows.rs
M  src/compositor/state/xwayland_mode.rs
M  src/compositor/window_state.rs
M  src/core/mod.rs
M  src/lib.rs
M  src/wm/mod.rs
?? src/core/window_id.rs
?? src/wm/window.rs
?? src/wm/workspace.rs
?? docs/superpowers/plans/2026-08-20-typhon-wm-domain-foundation.md
?? REPORT-2026-08-20-typhon-wm-domain-foundation.md
```

No commit was created, as requested.

## Explicitly out of scope

This foundation intentionally does not implement Dwindle trees, tiled geometry, Floating/Tiled toggles, workspace switching or keybindings, moving windows between workspaces, multi-window layout transactions, chrome policy, titlebar removal, layout animation, Spatial Canvas, camera transforms, snapping, Dock/Eclipse behavior, or Regulus configuration. No window is hidden because inactive workspaces are defined.
