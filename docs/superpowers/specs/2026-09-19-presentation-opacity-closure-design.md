# Typhon Presentation Opacity Closure Design

## Goal

Close the remaining Presentation Engine v2 opacity correctness gaps without changing the established ownership model or introducing new rendering passes.

## Repository truth at design time

- Checkout: `/home/agony/GitHub/Typhon`
- Branch: `main`
- Starting HEAD: `6eaae14ec3dc4ea5a26ea6e1e44f643a79162f26`
- Codebase Memory project: `home-agony-GitHub-Typhon`, ready, full index, generation `2026-09-19T17:15:39Z`.
- Existing dirty work is preserved and will not be staged by path-scoped commits.
- Existing source-layout diagnostics include `src/egl_renderer/effects/executor.rs` at 7162 lines and `src/compositor/state/desktop_windows.rs` at 1581 lines; this closure will not broaden either module.

## Design

### Logical WindowGroup teardown

`CompositorState::remove_desktop_window` will capture the WindowGroup `SceneNodeId` before removing the logical window or its scene topology. After logical removal and before `remove_window_scene_nodes`, it will call `presentation_animator.cancel_all(scene_node_id)`. This retires Geometry and Opacity tracks and their exact transaction members while preserving the existing physical presentation ledger. Root-surface physical-history cleanup remains in `cancel_presentation_for_root` and is not used for logical destruction.

XWayland backing replacement continues to detach and attach frame adapters only; it does not cancel the WindowGroup owner or its presentation tracks.

### Final effect alpha

The composite shader will keep effect color conversion and `EffectAlphaMode` ordering intact, then multiply the resulting premultiplied value by `u_presentation_opacity`, and finally sanitize. This makes PresentationOpacity the outermost effect alpha and prevents `Opaque` from restoring alpha above the group opacity.

`effect_pass_blend_mode` will accept effective final presentation opacity. Internal targets retain replacement semantics. Final framebuffer passes use premultiplied source-over for `Preserve` or any opacity below `1.0`; opaque output at exactly `1.0` retains replacement.

`execute_fullscreen_pass` will resolve final-pass presentation opacity once and reuse that scalar for both the shader uniform and blend-policy decision. Zero opacity therefore submits a zero premultiplied source through source-over and leaves the destination unchanged.

### Regression coverage

Coverage will be added beside the existing focused modules without changing canonical authority:

- Presentation engine and logical-window teardown: mixed Geometry+Opacity and Opacity-only cancellation, exact transaction cleanup, and physical evidence preservation.
- Effect policy: all final/internal blend combinations and shader-order string contract.
- CPU rendering: premultiplied RGBA/ARGB scaling at `1.0`, `0.5`, and `0.0`, plus a source-over reference.
- Compositor integration: ordinary owner opacity, occlusion, popup/SSD inheritance, XWayland canonical and active-track continuity, input/layout independence, and direct-scanout opacity blockers/recovery.

Tests will use existing helpers and test-only state access. No new physical ledger, GPU pass, framebuffer, animation thread, timer, Clip property, or Lamp migration will be introduced.

## Verification

Focused tests will be run after each red/green change. Final verification will use the repository commands through `rtk`: format check, locked all-target check, locked clippy with `-D warnings`, locked full tests, and the source-layout gate. Existing baseline failures will be distinguished from closure regressions.

