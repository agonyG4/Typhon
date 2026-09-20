# Typhon Presentation Engine v2 — Clip Design

## Goal

Add Clip as the third transactional, WindowGroup-owned Presentation Engine property. Clip remains visual-only and does not activate window lifecycle animations.

## Authority and coordinate space

`DesktopWindow` owns persistent canonical Clip state. `PresentationEngine` owns only temporary interpolation on the stable WindowGroup `SceneNodeId`; surface roots remain frame adapters and visual groups remain frame-local rendering/effects groups. `PresentationClip::Unbounded` is the explicit identity value. A `Rect` uses finite WindowGroup-local logical coordinates relative to the canonical client top-left and permits zero dimensions and extents outside the client.

When an endpoint is `Unbounded`, the compositor freezes a finite visual envelope into that transition. This envelope is interpolation math only. At the exact semantic endpoint, sampling returns `Unbounded` so physical ACK and Direct Scanout qualification can observe identity precisely.

## Sampling and physical evidence

Geometry and Clip are sampled at one target presentation timestamp. The sampled Geometry maps the local Clip into output space before frame evidence is frozen. The frame snapshot carries both the semantic local Clip and its presented output-space mask; exact transaction/revision evidence travels separately from pixel identity. `NativeSceneHistory` remains the only ready/submitted/presented physical authority.

One transaction can contain Geometry, Opacity, and Clip members. Validation and ID reservation complete before any track changes; each effective property gets its own revision and can settle on an independent exact physical ACK.

## Rendering and effects

PresentationClip is an outer WindowGroup contribution mask. CPU and EGL rendering intersect it with the existing damage or scissor region while preserving `RenderableSurface.visual_clip` / `SurfaceVisualAperture` as independent surface/buffer aperture state. Popup VisualGroups and SSD resolve Clip through the same presentation owner. EGL command vertices remain intact so same-owner effect capture can bypass only its own final Clip; other owners remain clipped. Final owned effect composites use the owner Clip, while `OutputPostProcess` remains unowned.

## Damage, scanout, and scope

Physical Clip damage compares immutable previous and current frame snapshots and damages only the previous/current visible clipped contribution for changed owners. Direct Scanout rejects a candidate for candidate-scoped active, canonical, or last-physically-presented nonidentity Clip, with a diagnostic distinct from surface `visual_clip_present`.

Clip does not change input, layout, canonical Geometry, or XDG/X11 configure behavior. XWayland backing replacement preserves the `DesktopWindow`, SceneNode, canonical Clip, and active revision. This phase adds no protocol, settings, new mandatory render pass, animation thread, or lifecycle/Lamp migration.

## Verification

Tests cover Clip validation and interpolation, three-property transaction atomicity and settlement, Geometry-coherent projection, CPU/EGL and effects masking, immutable physical damage/ACK, XWayland backing continuity, and candidate-scoped Direct Scanout recovery. Full verification uses the commands in the task requirements and the existing source-layout checker without changing its limits.
