# Direct Scanout Candidate-Scoped Presentation Closure

## Goal

Make temporary Presentation Engine Geometry and Opacity blockers causal to the
WindowGroup that owns the output-covering Direct Scanout candidate, while
preserving canonical opacity, physically presented Geometry/Opacity, scene
content, scheduler, renderer, and immutable frame-history semantics.

## Current behavior

`CompositorState::direct_scanout_scene_analysis` currently asks
`ActiveScene`-derived helpers whether any visible Geometry or Opacity track is
pending before resolving `PresentationCoverageAnalysis`'s covering application
group. Those helpers project the broader native-frame presentation target set,
so an unrelated WindowGroup can manufacture a blocker for another candidate.

The same analysis already performs candidate-root checks for canonical opacity,
physically presented Geometry, and physically presented Opacity. XWayland
backing replacement preserves the logical WindowGroup and its SceneNodeId, but
the Direct Scanout consumer must resolve the replacement root through that
owner before querying active tracks.

`NativeSceneHistory` already stores complete `NativeFrameSceneSnapshot` values
in `ready` and `submitted` history and promotes the stored snapshot on a
matching pageflip. The regression test will prove that opacity transition
evidence and the exact root/backing identity remain frozen across promotion.

## Design

1. Add `PresentationEngine::has_geometry_track(SceneNodeId) -> bool` as a
   sparse `geometry_tracks` map lookup. Keep `has_opacity_track` as the
   property-specific counterpart. Do not expose track objects or add a generic
   dynamic-property API.

2. In `direct_scanout_scene_analysis`, keep lifecycle and all existing global
   composition checks unchanged, but defer temporary Geometry/Opacity track
   checks until `covering_application_group` exists. Resolve its
   `root_surface_id` through `presentation_scene_node_id_for_root`, then query
   the Presentation Engine by the resulting WindowGroup `SceneNodeId`:

   - Geometry track → `AnimationTransform`
   - Opacity track → `PresentationOpacity`

   No track is queried or stored by root surface ID. If there is no covering
   group, report the existing `NoOutputCoveringApplication` and other relevant
   blockers without adding an animation blocker for an unrelated scene node.

3. Remove the property-specific ActiveScene helpers when their only consumer is
   removed. Preserve `presentation_animation_has_pending_visible()` and the
   engine's generic pending-visible APIs for scheduler/general presentation
   work.

4. Extend the focused existing Direct Scanout/XWayland test area with isolated
   cases for candidate Geometry and Opacity tracks, unrelated off-output
   Geometry and Opacity tracks, XWayland root replacement, canonical opacity,
   and physical Geometry/Opacity recovery. Split the existing accumulated
   visible/hidden Opacity test so each scenario has independent state.

5. Extend `frame_scene_identity_tests.rs` with a direct `NativeSceneHistory`
   test that queues frame A with SceneNode G/root A/opacity O1 and real
   transaction T/revision R evidence, replaces ready state with frame B using
   the same G/revision R but root B/opacity O2, then promotes A and B by their
   exact tokens. Assertions inspect the stored presented snapshot after each
   promotion; no production promotion path changes.

## Non-goals

- No Presentation Engine transaction, revision, retargeting, settlement, ACK,
  canonical opacity, renderer, effect, framebuffer, or alpha changes.
- No Clip implementation.
- No LifecycleAnimation scoping or Lamp migration.
- No new Direct Scanout rejection enum or blocker-order redesign.
- No pageflip-time lookup of mutable compositor, XWayland, or Presentation
  Engine state.

## Verification

Use `rtk` in the same checkout. Run the focused Direct Scanout, XWayland,
Presentation Engine, Geometry, Opacity, NativeSceneHistory, and physical
publication tests, then fresh:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

Pre-existing failures are recorded separately from task regressions.
