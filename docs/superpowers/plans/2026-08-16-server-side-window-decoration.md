# Typhon Server-Side Window Decorations v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a native, themeable, safe server-side decoration subsystem with MacTahoe-Dark, correct XDG/XWayland policy, exact-window input actions, shared CPU/GLES render planning, and control-plane selection.

**Architecture:** Keep window behavior in Typhon state and interaction modules. Add pure decoration types/layout, immutable declarative theme snapshots, and a renderer-independent plan; pass those plans into both renderers. Track XDG preference by root surface and derive visible mode from negotiated preference plus fullscreen/role policy.

**Tech Stack:** Rust 2024, Serde/JSON, existing compositor state and interaction machinery, existing CPU/GLES renderers, existing bounded local control socket.

## Global Constraints

* JSON theme documents are <= 64 KiB.
* Theme metrics are bounded to titlebar 20..96 px, button 8..64 px, spacing 0..32 px, padding 0..64 px.
* Source assets are <= 256 KiB and raster cache entries are <= 128x128 physical px.
* SSD buttons are always RIGHT and ordered Minimize, Maximize/Restore, Close.
* CSD and clients without a decoration object retain zero server extents.
* Fullscreen hides all visible decorations.
* Do not add executable plugins, scripts, dynamic libraries, shell commands, or external SVG resource access.
* Preserve unrelated dirty files and stage only feature files.

---

### Task 1: Pure model and layout

**Files:**
- Create: `src/compositor/decoration/mod.rs`
- Create: `src/compositor/decoration/types.rs`
- Create: `src/compositor/decoration/layout.rs`
- Modify: `src/compositor/mod.rs`
- Test: `src/compositor/decoration/layout.rs` unit tests

**Interfaces:**
- Produces `DecorationMode`, `DecorationExtents`, `DecorationButtonKind`, `DecorationButtonVisualState`, `DecorationHit`, `DecorationMetrics`, `DecorationWindowState`, and `DecorationLayout::for_window`.

- [ ] Write tests for right-side order, 32/16/9/12 metrics, title-safe clipping, narrow windows, floating/maximized/fullscreen/CSD modes, resize-only extents, and 1.0/1.25/2.0 deterministic rounding.
- [ ] Run `cargo test decoration::layout` and confirm the new tests fail before implementation.
- [ ] Implement checked arithmetic and pure layout with distinct visual, input, title-safe, and resize regions.
- [ ] Run the targeted tests and `cargo fmt --check`.

### Task 2: Declarative theme snapshots and MacTahoe-Dark

**Files:**
- Create: `src/compositor/decoration/theme.rs`
- Create: `src/compositor/decoration/render_plan.rs`
- Create: `resources/decorations/MacTahoe-Dark/theme.json`
- Create: `resources/decorations/MacTahoe-Dark/LICENSE.md`
- Modify: `src/compositor/decoration/mod.rs`
- Modify: `Cargo.toml` only if an in-process bounded rasterizer is required
- Test: `src/compositor/decoration/theme.rs` unit tests

**Interfaces:**
- Produces `DecorationThemeSnapshot`, `DecorationThemeLoader`, `DecorationThemeError`, `DecorationRenderPlan`, and deterministic asset-state fallback.

- [ ] Test valid/unknown-field/version/color/metric/size/path/traversal/asset/SVG validation and last-known-good generation behavior.
- [ ] Implement schema validation, built-in fallback, package-relative asset resolution, bounded loading, and immutable snapshots without frame-loop I/O.
- [ ] Implement shared solid/image/text primitives and title ellipsizing against layout text-safe bounds.
- [ ] Add the MacTahoe-Dark package with attribution; if exact artwork redistribution is unavailable, use documented built-in vector-safe fallback assets.
- [ ] Run theme/layout tests and `./bin/check-source-layout`.

### Task 3: XDG mode negotiation and effective mode

**Files:**
- Create: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/state_data.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/protocols/xdg.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`
- Test: `src/compositor/tests/xdg.rs`

**Interfaces:**
- Produces per-root requested/effective decoration state and cleanup helpers consumed by geometry, input, and rendering.

- [ ] Add client tests for explicit CSD/SSD, unset default, no decoration object, duplicate object, destroy, remap, and disconnect.
- [ ] Implement one-resource-per-toplevel validation and mode configure events with conservative default behavior.
- [ ] Ensure fullscreen suppresses visible mode without mutating the negotiated preference.
- [ ] Run XDG decoration tests and existing XDG protocol tests.

### Task 4: Geometry, exact actions, and decoration input

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/window_actions.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Test: `src/compositor/state/window_interaction_tests.rs`
- Test: `src/compositor/tests/windows.rs`

**Interfaces:**
- Produces exact-window maximize/restore outcomes and compositor-owned decoration press/hover/capture transitions.

- [ ] Add tests for button capture/release cancellation, destruction cleanup, titlebar move, double-click, exact rear-window targeting, and CSD client ownership.
- [ ] Implement effective decoration hit testing ahead of client dispatch while preserving resize precedence and existing interaction IDs.
- [ ] Implement exact maximize/restore and titlebar double-click using `WindowId`, not focused-window lookup.
- [ ] Add 100 maximize/restore transitions and verify geometry/frame extents do not drift.
- [ ] Run targeted input/window tests.

### Task 5: Shared CPU/GLES render plan and damage

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/compositor/presentation_modes.rs` or the existing scene signature owner as required
- Test: `src/compositor/render.rs`
- Test: `src/egl_renderer/geometry.rs`

**Interfaces:**
- Consumes `DecorationRenderPlan` and produces equivalent logical primitives in CPU and GLES paths.

- [ ] Add parity tests for primitive geometry, RGBA, button state, title bounds, and clipping.
- [ ] Thread an optional decoration plan through native frame requests without breaking existing renderer tests.
- [ ] Draw/emit decorations outside the frame hot path and include generation/state in cache keys.
- [ ] Add fine-grained hover/focus/title/mode damage coverage and preserve fullscreen Direct Scanout eligibility.
- [ ] Run renderer and Direct Scanout tests.

### Task 6: XWayland policy and frame extents

**Files:**
- Modify: `src/xwayland/xwm/mod.rs`
- Modify: `src/xwayland/xwm/properties.rs`
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/compositor/desktop_window.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Test: `src/compositor/tests/xwayland_resize_visual.rs`
- Test: `src/compositor/tests/xwayland_focus.rs`

**Interfaces:**
- Produces a role/hint-aware X11 decoration policy and `_NET_FRAME_EXTENTS` updates derived from the shared extents.

- [ ] Test normal managed, override-redirect, special, fullscreen, Motif no-decoration, conversion, maximize/restore, and destruction cases.
- [ ] Implement the minimal Motif hint parse needed for explicit no-decoration and keep special X11 roles undecorated.
- [ ] Publish/update frame extents on all relevant state transitions using existing client/frame geometry.
- [ ] Run the XWayland regression matrix.

### Task 7: Persistent selection and control commands

**Files:**
- Create: `src/decoration_persistence.rs`
- Modify: `src/compositor/decoration/theme.rs`
- Modify: `src/control.rs`
- Modify: `src/control_snapshots.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/astreactl/client.rs`
- Modify: `src/astreactl/output.rs`
- Modify: `src/bin/astreactl.rs`
- Test: `src/decoration_persistence.rs`
- Test: `src/control_tests.rs`

**Interfaces:**
- Adds `decoration.status`, `decoration.set-theme`, and `decoration.reload` with bounded JSON snapshots and last-known-good switching.

- [ ] Test CLI grammar, invalid arguments, atomic persistence, invalid selection fallback, failed reload retention, and generation changes.
- [ ] Implement dedicated XDG config/data search and safe atomic persistence consistent with cursor configuration.
- [ ] Wire control requests to validate/prepare/switch before persisting and damage visible decorations.
- [ ] Run control and persistence tests.

### Task 8: Documentation, qualification, and focused commits

**Files:**
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`
- Modify: `docs/superpowers/specs/2026-08-16-server-side-window-decoration-design.md`
- Modify: `docs/superpowers/plans/2026-08-16-server-side-window-decoration.md`

- [ ] Update docs only for paths that are actually implemented and tested.
- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo check --locked --all-targets`.
- [ ] Run `cargo clippy --locked --all-targets -- -D warnings`.
- [ ] Run `cargo test --locked`.
- [ ] Run `./bin/check-source-layout` and `git diff --check`.
- [ ] Perform a fresh diff review for geometry coupling, exact target use, path traversal, external SVG access, stale capture, renderer parity, Direct Scanout, and unrelated staging.
- [ ] Stage only feature files and create focused commits; report any unavailable native/GPU smoke tests honestly.
