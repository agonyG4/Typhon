# Typhon WM Domain Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish one canonical window identity, separate protocol mode from layout membership, and attach explicit workspace policy state to the existing compositor `DesktopWindow` authority without changing visible floating behavior.

**Architecture:** Move the non-zero `WindowId` definition into `core`, re-export it from compositor and WM namespaces, and retain monotonic allocation in `CompositorState`. Replace the old WM prototype with pure `WorkspaceId`, `WorkspaceManager`, `LayoutMembership`, and `WindowManagementState` modules. Store `Option<WindowManagementState>` directly on `DesktopWindow`, classify managed XDG/X11 roles at insertion, and publish the state through the existing control snapshot.

**Tech Stack:** Rust, Cargo, existing Wayland/XWayland compositor state, serde control snapshots, deterministic unit tests.

## Global Constraints

- Preserve all pre-existing dirty work; do not reset, restore, checkout, stash, clean, stage unrelated files, or commit.
- Reuse the existing Cargo target directory; never run `cargo clean`.
- Keep `DesktopWindow` as the only live-window registry and keep WM free of protocol/render/input dependencies.
- `ToplevelMode` contains only `Normal`, `Maximized`, and `Fullscreen`; `LayoutMembership` owns Floating/Tiled.
- Preserve `ToplevelVisualGeometry`, immediate resize, configure coalescing, pointer scene ownership, render generations, damage, KMS scheduling, XDG/XWayland behavior, and decorations.
- Do not implement Dwindle, workspace switching, chrome policy, layout transactions, or per-frame workspace work.

### Task 1: Add failing domain and identity tests

**Files:**
- Modify: `src/compositor/state/desktop_window_tests.rs`
- Modify: `src/lib.rs`
- Create: `src/wm/workspace.rs` tests and `src/wm/window.rs` tests through module test blocks

- [ ] Add tests for zero rejection/raw round-trip, workspace identity validation/display, deterministic manager defaults, management-state defaults, and protocol/layout independence.
- [ ] Add lifecycle/snapshot assertions for managed XDG, managed X11, and auxiliary X11 roles.
- [ ] Run the focused tests and confirm the failures are caused by missing new APIs and the old `WindowManager`/`Floating` names.

### Task 2: Implement neutral identity and pure WM domain

**Files:**
- Create: `src/core/window_id.rs`
- Modify: `src/core/mod.rs`
- Replace: `src/wm/mod.rs`
- Create: `src/wm/workspace.rs`
- Create: `src/wm/window.rs`
- Modify: `src/lib.rs`

- [ ] Define `WindowId(NonZeroU64)` once in `core`, preserving `from_raw`, `get`, crate-local construction, and derives.
- [ ] Implement extensible non-zero `WorkspaceId`, deterministic `WorkspaceManager` with workspaces 1 through 10 and active workspace 1, `LayoutMembership`, and `WindowManagementState`.
- [ ] Remove the prototype registry, rectangle, focus, and allocator from `src/wm`.
- [ ] Make the new domain tests pass without importing compositor/protocol/render/input types.

### Task 3: Rename protocol mode without changing behavior

**Files:**
- Modify: `src/compositor/window_state.rs`
- Modify: all current Rust references to `ToplevelMode::Floating`
- Modify: focused mode/restore tests

- [ ] Rename every `Floating` protocol-mode use to `Normal`.
- [ ] Keep restore capture restricted to `Normal` and preserve XDG state bytes (`Normal=[]`, `Maximized=[Maximized]`, `Fullscreen=[Fullscreen]`).
- [ ] Run the focused compositor mode, restore, decoration, resize, interaction, XWayland, and XDG tests.

### Task 4: Integrate management state with DesktopWindow and snapshots

**Files:**
- Modify: `src/compositor/desktop_window.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/server_control.rs`
- Modify: `src/compositor/mod.rs` re-exports as needed
- Modify: `src/control_snapshots.rs` tests as needed
- Modify: lifecycle and control snapshot tests

- [ ] Add `management: Option<WindowManagementState>` to `DesktopWindow` and initialize it from explicit eligibility: XDG and normal/dialog managed X11 receive active-workspace/Floating state; auxiliary/override-redirect X11 does not.
- [ ] Preserve current cascade/client placement and transient/stacking behavior.
- [ ] Publish workspace as `Some("1")` only for managed windows and retain `None` for auxiliary windows.
- [ ] Verify there is no second window registry or render-loop workspace access.

### Task 5: Review and validation

**Files:**
- Create: `REPORT-2026-08-20-typhon-wm-domain-foundation.md`

- [ ] Perform correctness/ownership review and fix duplicate identities, eligibility mistakes, restore regressions, or protocol/layout coupling.
- [ ] Perform performance/future-compatibility review and fix per-frame work, locks, scans, speculative transactions, or fixed-10-workspace assumptions.
- [ ] Run `cargo fmt --check`, `cargo check --locked --all-targets`, `git diff --check`, focused tests, and attempt `cargo test --locked`.
- [ ] Record actual results, pre-existing blockers, final status, and explicit out-of-scope behavior in the report.
