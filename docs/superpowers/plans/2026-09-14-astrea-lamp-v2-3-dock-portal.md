# Astrea Lamp v2.3: Dock Icon Portal Convergence

## Goal

Make an active Lamp converge through the frozen Eclipse Dock icon target and render behind the matching Dock surface, while leaving the v2.2 motion model and all lifecycle/effects/frame-pacing behavior unchanged.

## Invariants and design decisions

- `WindowLifecycleAnimator` remains the sole owner of raw linear lifecycle progress, transition identity, reversal, duration, and physical settlement. No lifecycle ownership or settlement code changes are planned.
- The CPU remains the sole authority for temporal Lamp channels. `lamp_motion_channels(raw_progress, bump_distance)` remains the only production timing/easing source and continues to provide the global `InOutCubic` temporal progress plus contraction, soft-start translation, and bounded raw-progress retreat channels.
- GLSL remains spatial-only. It will consume CPU-uploaded channels and frozen geometry; no timing, easing, aspect-fit, or portal calculation will be duplicated in the shader.
- Contraction and translation remain overlapping. No sequential stage machine may survive under renamed fields or helpers. The current v2.2 constants, 280 ms duration, opacity interval, mesh topology, and spatial equations are not retuned.
- `retreat_progress` remains a pure bounded function of raw progress: it is zero when `bump_distance <= epsilon`, otherwise the existing `smoothstep(raw_progress / RETREAT_END)`. It is never a pre-animation phase and remains bounded by `bump_distance`.
- `p = 0` must map every point exactly through the frozen canonical-visual to presented-source-visual affine. `p = 1` must map every normalized point exactly into the frozen `portal_rect`. Reversal samples the same pure function backward, so it must preserve exact positional continuity.
- `anchor_rect` remains the frozen Dock identity/direction/occlusion/damage authority. `portal_rect` is an additional frozen lifecycle geometry field and is never recomputed from mutable Dock state during a transition.
- The existing Eclipse stable resting icon authority is retained; no Eclipse production change is planned. The Dock remains a normal layer-shell `Top` surface outside an active matching Lamp.
- Only visible, mapped `namespace == "astrea-dock"`, committed `Layer::Top` roots whose complete root-owned rendered bounds intersect a Lamp's frozen `anchor_rect` are promoted. Matching root-owned subsurfaces follow the root. Other Top surfaces remain in base ordering; true Overlay surfaces remain external overlays.
- Promoted command order remains Dock Top surfaces followed by genuine Overlay surfaces, after Lamp and before cursor. The Dock is removed from the base batch while promoted, so it is drawn exactly once.
- Scene-cache identity explicitly includes the ordered external-overlay surface-ID partition. A base → promoted → base transition must rebuild at both boundaries.
- No changes to mesh density (`32` target cells, `64` subdivision cap, `65_536` vertex cap), duration, opacity, lifecycle ownership, SSD freezing, Effects/Blur, KMS, pacing, Eclipse behavior, or the known ignored live-subsurface snapshot-ownership gap.

## Files and concrete dependencies

The implementation is expected to touch only these Rust files:

- `src/window_lifecycle_animation.rs`: frozen portal derivation, portal-target warp, lifecycle signatures/validation, and CPU geometry/motion regressions.
- `src/egl_renderer/program.rs`: Lamp shader target uniform and spatial-domain contract tests. The client-only `u_anchor_rect` Lamp uniform will be removed if unused; v2.2 motion uniforms remain CPU-fed.
- `src/egl_renderer.rs`: portal uniform location/upload, geometry identity, and scene-cache partition signature plumbing. Existing dirty Effects code in `src/egl_renderer/effects/executor.rs` will not be edited.
- `src/compositor/layer_shell.rs`: lifecycle-aware classification of matching Dock roots and their complete root-owned visible surface trees.
- `src/compositor/server.rs`: pass the lifecycle sample into the authoritative overlay classification wrapper.
- `src/native_output/runtime/frame.rs`: provide the active lifecycle sample when resolving the frame partition and preserve the existing snapshot/render ordering.

No file outside this set is authorized by the current design. If a test or compiler evidence demonstrates a concrete dependency on another production file, stop before editing it and report that dependency; do not broaden scope implicitly. No Eclipse source change is expected.

## TDD sequence

### 1. RED: reproduce the current ordering defect

Add an integration-level layer-shell regression using the existing compositor test harness. Create a mapped `astrea-dock` root committed at `Layer::Top`, create an active Lamp lifecycle sample with a frozen anchor intersecting the Dock root's complete rendered bounds, and inspect the actual authoritative base/external classification used for frame resolution. The test must fail against the current implementation because the Dock Top root is absent from the post-Lamp partition. It must assert the actual partition, not a standalone desired helper.

Before changing the classifier, run the narrow test and record the expected RED failure.

### 2. Implement and test Dock promotion

Evolve the compositor classification API to accept the current `LifecycleSceneSample`. Keep true `Layer::Overlay` promotion unconditional. When Lamps are active, collect eligible Dock root IDs by deterministic surface order and match each root's unioned visible root-owned rendered bounds against each Lamp anchor. Filter the final renderable surface list by those root IDs so all Dock subsurfaces move together. Preserve original scene order and ensure an unmatched/other-output Dock is not promoted.

Add/extend tests for ordinary Top, inactive Dock Top, matching Dock promotion, root-owned subsurface promotion, multi-output/negative coordinates, genuine Overlay ordering, post-settlement return to base, and exactly-one-batch membership.

### 3. Add cache partition identity

Add an order-sensitive stable hash/signature of external-overlay IDs to `EglSceneCacheKey` and every production candidate/current comparison used by `EglRenderer`. Thread the actual frame partition through scene-command rebuild and cache-current checks. Add deterministic base → promoted → base tests proving unchanged surface signatures with changed partitions invalidate/rebuild the command cache and never leave a Dock in both batches.

### 4. Add the frozen portal geometry

Add `portal_rect` to `LifecycleVisualGroup`, derive it once in `from_bounds` from the frozen presented visual source and anchor:

```text
scale = min(anchor.width / source.width, anchor.height / source.height)
portal.size = source.size * scale
```

The portal is cross-axis centered. Along the movement axis it touches the Dock-facing edge: Bottom/Right use the anchor's near minimum edge; Top/Left use the anchor's near maximum edge. Validate finite positive dimensions and containment. Include the field in group validation, lifecycle signatures, Lamp geometry keys, snapshots, and equality-sensitive test fixtures. Keep the entire outer anchor in `lamp_footprint`.

Change only the spatial target of the existing v2.2 warp from `anchor_rect` to `portal_rect`; leave all temporal constants/equations untouched. Ensure the public/reference path uses the frozen group portal and any convenience path derives the same deterministic geometry, with no mutable Dock reads.

### 5. TDD the motion and portal invariants

Before replacing any old regression, add the required RED v2.2 regression around the current internal Stretch→Squash boundary using finite differences of actual warped representative mesh points (near edge, center, trailing edge, and cross-axis edges). Assert normal aggregate movement before and after with a near-zero aggregate RMS velocity at the boundary. If an applicable bump fixture exists, cover the Bump→Stretch boundary similarly.

After the target implementation, replace/extend the regression with dense aggregate-motion checks that exclude endpoint neighborhoods and reject an internal full-window stop. Test finite-difference position/slope continuity at translation start, the quadratic→linear join, contraction end, and retreat end. Preserve existing no-overlap, bounded-retreat, monotonic no-retreat, spatial funnel, row-delay, rotations, exact endpoint, inverse restore, and reversal tests.

Add pure portal tests for 16:9, portrait, square, tiny valid anchor, all four directions, and negative coordinates. Assert finite contained aspect-fit geometry, cross-axis centering, near-edge contact, early near-row convergence, eventual exact trailing-row portal mapping, exact source at `p=0`, exact portal mapping at `p=1`, and reversal continuity.

### 6. Renderer contract tests

Update the existing shader-source/GLES-focused tests to assert `u_portal_rect` is located/uploaded, `u_anchor_rect` is absent from the Lamp shader/plumbing when no longer used, legacy sequential stage uniforms/terms are absent, and the shader uses `u_source_visual_rect` consistently for its deformation domain. Assert CPU motion channels drive the uploaded temporal uniforms, progress-only frames retain mesh topology, endpoint mapping is exact, and vertex limits remain unchanged. Do not add a second Rust timing implementation for GLSL.

## Self-review before production edits

- No v2.2 temporal model, stage timing, spatial exponent, retreat timing, duration, opacity, mesh, lifecycle, settlement, Effects, Blur, KMS, or pacing change is present in this plan.
- CPU-only temporal ownership and spatial-only GLSL are explicit.
- The first ordering RED test observes actual base/external classification, and the first motion RED test observes aggregate warped-mesh velocity rather than scalar channels.
- Portal endpoint/source and arbitrary reversal continuity are explicit.
- Retreat is bounded and overlapping, not a hidden stage.
- Matching uses compositor role namespace/layer plus complete rendered root-tree bounds and supports multi-output/negative coordinates without hard-coded positions.
- Partition identity and exactly-once drawing are covered at both transitions.
- The known subsurface snapshot-ownership gap and Lamp+Blur status remain out of scope.
- The four pre-existing dirty files are outside the planned edit set and must remain unchanged.

## Verification and handoff

Run focused tests first with `rtk`:

```text
rtk cargo test --locked lamp
rtk cargo test --locked lifecycle_animation
rtk cargo test --locked layer_shell
rtk cargo test --locked egl_renderer
rtk cargo test --locked native_output
```

Then run:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Report exact results, existing ignored tests, and any pre-existing repository source-layout failures. Native visual acceptance is not implied by automated tests and will be reported separately as performed or not performed. Commit only the v2.3 plan and implementation files; verify the four unrelated dirty files remain untouched before committing.
