# Direct Scanout Physical Presentation Owner Closure

## Goal

Make Direct Scanout qualify active, canonical, and physically presented Geometry/Opacity state against the same stable WindowGroup `SceneNodeId`, while retaining `root_surface_id` for render/input/frame identity.

## Current truth

Active Geometry and Opacity tracks are already queried by the candidate WindowGroup `SceneNodeId`. Canonical opacity remains owned by the candidate `DesktopWindow`. Physical Geometry and Opacity qualification still queries the current candidate root through `transform_for_root` and `opacity_for_root`, so XWayland backing replacement can hide previously presented nonidentity state.

`PresentationFrameSnapshot` already stores both `SceneNodeId` and `root_surface_id`, and already provides `transform_for_scene_node` and `opacity_for_scene_node`. `NativeSceneHistory` already promotes immutable submitted snapshots and must remain unchanged.

## Design

After coverage resolves the output-covering application group, Direct Scanout resolves its root to one candidate WindowGroup `SceneNodeId`. That value is reused for active-track checks and new physical presentation checks. Physical helpers read only the immutable `presented_presentation` snapshot:

- `presented_presentation_transform_for_scene_node` returns the matching stored transform, if any.
- `presented_presentation_geometry_is_non_identity_for_scene_node` treats a missing transform as identity and otherwise checks `is_identity()`.
- `presented_presentation_opacity_for_scene_node` returns the stored opacity or opaque identity when sparse evidence is absent.
- `presented_presentation_opacity_is_non_identity_for_scene_node` checks the SceneNode-qualified physical opacity.

The existing root-qualified helpers remain for input and interactive handoff. Current root B remains the Direct Scanout candidate and diagnostic source identity even when physical evidence came from root A.

## Tests

Add focused Direct Scanout regressions for:

1. physical nonidentity Geometry on G/root A continues blocking after A→B;
2. physical opacity 0.5 on G/root A continues blocking after A→B;
3. an identity physical G/root B frame clears both blockers;
4. unrelated SceneNode physical evidence does not block candidate G1;
5. existing canonical, active-track, immutable-history, and root-qualified input behavior remains intact.

The existing immutable A/B NativeSceneHistory regression remains unchanged. No renderer, effects, Presentation Engine transaction, input, lifecycle/Lamp, Clip, or NativeSceneHistory production changes are in scope.

## Verification

Run focused Direct Scanout, Presentation Engine, XWayland, physical recovery, and NativeSceneHistory tests, followed by `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `rtk run ./bin/check-source-layout`. Report unrelated concurrent failures without changing their files.
