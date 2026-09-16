# Typhon Visual Scanout Source Selection Design

## Goal

Allow Direct Scanout to select the exact committed surface that supplies the
output pixels while retaining the application root as the owner of window
policy, semantic fullscreen, workspace membership, and presentation geometry.

## Invariants

- `root_surface_id` identifies the logical application/window owner.
- `surface_id` identifies the exact visual surface whose buffer is physically
  proposed for scanout.
- Renderer painter order, obtained from
  `WindowVisualGroup::stack_order_with_popups`, remains the only visual-order
  authority.
- Rendered target rectangles come from `surface_render_space_targets`, so
  retained XWayland geometry is evaluated exactly as the renderer presents it.
- A proven opaque full-output source may occlude surfaces painted before it;
  surfaces painted after it and intersecting the output remain blockers.
- Presentation Coverage, semantic fullscreen/tearing policy, and physical KMS
  qualification remain separate decisions.
- `candidate.is_some() <=> blockers.is_empty()` remains true.
- Existing DMA-BUF, format/modifier, viewport, explicit-sync, TEST_ONLY,
  submit, lease, and pageflip ownership rules remain authoritative.

## Data flow

```text
active WindowVisualGroup
    -> ordered owner surfaces and renderer targets
    -> PresentationCoverageApplicationGroup {
         root_surface_id,
         surface_ids,
         covering_surface,
         visible_surface_ids_above_covering
       }
    -> DirectScanoutSceneAnalysis
         root-owned policy checks
         source-owned buffer/content checks
    -> DirectScanoutSceneCandidate {
         root_surface_id = owner,
         surface_id = covering source
       }
    -> DMA-BUF import and physical DRM qualification
    -> DirectPrimaryLease {
         root_surface_id = owner,
         surface_id = exact source
       }
    -> Atomic TEST_ONLY, submit, and pageflip
```

## Coverage source selection

Coverage chooses the highest painter-order member of the selected application
group whose renderer target contains the physical output. The selected member
is represented by `PresentationCoverageSurface`, including its target and
conservative opacity proof. Same-group members painted later than that source
are tracked separately when their rendered target intersects the output.

The proof remains limited to the existing strong case: DMA-BUF,
`DRM_FORMAT_XRGB8888`, output-sized buffer, full-output rendered target, and no
clip, retained resize projection, or incompatible placement/mapping. Unknown
opacity never authorizes occlusion. Popups, decorations, layer-shell content,
other application groups, effects, and animations continue through the
existing external blocker paths.

## Direct Scanout identity split

Direct scene analysis finds the root through the selected coverage group and
finds the source through `covering_surface.surface_id`. Root checks cover owner
existence, minimized state, workspace/window policy, semantic presentation
animation, resize policy, and owner geometry. Source checks cover the buffer,
DMA-BUF handle, DRM format, dimensions, scale, transform, viewport, visual
clip, rendered target, generation, commit sequence, content epoch,
presentation generation, presentation metadata, and direct-surface damage.

The blanket additional-surface rejection is removed. A same-tree surface above
the source has a precise `OwnerTreeContentAboveSource` blocker. A surface
behind an opaque source or entirely outside the output does not block merely
because it exists.

Async eligibility asks the existing semantic fullscreen authority whether the
candidate root is a solitary fullscreen owner. It never compares the root and
source IDs. Thus a borderless child-source candidate can remain Vsync-only,
while a semantically solitary fullscreen root may remain async-eligible.

## Diagnostics

The existing `astreactl doctor` direct-scanout detail is extended with a
bounded, one-shot visual-group snapshot: owner root, selected source, painter
order, backend, buffer source, DRM format, rendered target, relation to the
source, external content above, and truncation state. The snapshot is built
only in the doctor command and does not add frame-loop logging, file I/O, or a
second persistent scene model.

## Testing

RED regressions cover root SHM plus child XRGB DMA-BUF selection, harmless
behind/outside surfaces, same-tree content above, root content above a child,
external blockers, source metadata, borderless/semantic-fullscreen async
separation, native lease identity, and the doctor shape. Existing physical
fallback, explicit-sync, presentation-mode, pageflip, and renderer-order tests
remain unchanged and are rerun.

## Scope

No generalized overlay-plane allocator, ARGB opaque-region support, VRR/HDR
work, workspace semantic changes, XWayland hacks, KMS scheduling rewrite,
Direct Scanout bypass of visible external overlays, or game-specific logic.
