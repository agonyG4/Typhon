# Typhon WindowVisual, Input, Fullscreen, Damage, and Scroll Corrective Closure

## Baseline journal

Investigation baseline:

- Repository: `/home/agony/GitHub/Typhon`
- HEAD: `3d835df4d4622580aafad67291f7d0d8ce440973`
- Subject: `fix: align SSD rendering with resolved surface origins`
- Working tree: intentionally dirty before this closure; preflight recorded 60 tracked modified files, 84 status entries, 2,726 insertions, and 692 deletions relative to HEAD.
- Existing dirty design: `docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md`.
- Existing dirty plan: `docs/superpowers/plans/2026-08-16-ssd-damage-mactahoe-closure.md`.

The preflight status, diff stat, and name list were captured before new closure changes. Existing SSD/theme/renderer/native-output work is not treated as authored by this closure unless a later diff explicitly makes it a direct dependency.

Before new edits, the focused baseline suites passed:

```text
rtk cargo test --locked --lib -- render::       47 passed
rtk cargo test --locked --lib -- fullscreen     45 passed
rtk cargo test --locked --lib -- pointer_axis    2 passed
```

The available focused filters `native_input_alt` and `native_output_damage` matched no tests at baseline; their neighboring native input/output suites remain the source of truth for adding coverage.

## Confirmed source defects

1. `DesktopSceneRenderer::compose_request_internal` copies the surface scene and then invokes `draw_decoration_instances` over all normal windows. `GlesSceneRenderer::rebuild_scene_commands` emits every surface and then invokes `push_egl_decoration_commands`. This is a global SSD post-pass and permits a lower window’s decoration to cover a higher window’s client content.
2. `begin_window_interaction_for_root` performs target/resource checks but does not reject fullscreen before resize IDs, resize-flow state, focus, and interaction state are allocated.
3. `NativeInputState::handle_key_event` returns early after a consumed Alt release, before reconciling a previously forwarded modifier key.
4. Current damage has `DecorationSceneSnapshot` old/current tracking, but it remains a parallel decoration snapshot rather than the complete stacked visual ownership unit.
5. `mark_xdg_buffer_commit` directly calls low-level `focus_surface` on map, without an explicit focus-policy decision.
6. `PointerAxisComponent` currently retains only `continuous`, legacy `discrete`, and `stopped`, even though repository protocol documentation and upstream input contain high-resolution v120 information.

## Runtime hypotheses, not proven causes

- Intermittent frame rollback requires bounded frame-history correlation with swapchain/buffer-age/presentation state.
- Firefox/Zen tear-off requires a native-session trace of map/configure/ack/focus/grab ownership; no application-specific fix is permitted.
- Kitty drag-selection requires a deterministic implicit-grab sequence and bounded live trace before changing pointer ownership.

## Chosen architecture

The closure extends the existing `DecorationSceneSnapshot` work into a stable WindowVisual ownership model. A normal window’s root identity, client tree/backing, SSD plan (when present), complete bounds, placement, and visual signature are ordered as one group keyed by `WindowId` and root surface. Explicit desktop layers remain outside normal-window order. CPU, GLES, hit testing, damage, XWayland extents, and Direct Scanout qualification derive from that same ownership/order contract.

Cached immutable decoration raster/text plans remain separate from small per-frame placement/state snapshots. No SVG/font/theme parsing is introduced in the frame loop.

## Alternatives rejected

- Keeping all decorations in a final global pass: fails ownership and stacking.
- Adding a second decoration z-index list: creates render/input/damage divergence.
- Full-output repaint or disabling buffer age: hides damage defects and regresses performance.
- Blocking fullscreen only in SSD titlebar handlers: leaves generic/XDG/X11 paths open.
- Special-casing Firefox or Kitty: unsupported without runtime evidence.
- Hard-coded scroll multipliers: distorts protocol semantics before v120 fidelity is preserved.
- Unconditional map-time focus: steals focus during existing grabs.

## Test evidence policy

Each production change in this closure is paired with a focused regression written before the implementation and observed failing for the new behavior. Tests distinguish pre-existing dirty behavior from closure-introduced behavior. Native-session qualification is reported only if a real Astrea/Typhon TTY/DRM session is actually exercised.
