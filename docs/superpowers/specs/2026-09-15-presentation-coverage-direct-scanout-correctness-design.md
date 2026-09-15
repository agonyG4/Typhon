# Typhon Presentation Coverage / Borderless Direct Scanout Correctness Design

## Goal

Keep Presentation Coverage as the physical scene gate for borderless Direct Scanout while preventing it from granting semantic fullscreen tearing eligibility, using renderer-authoritative bounds for above-content checks, and making the disabled Direct Scanout path cheap.

## Constraints and invariants

- Presentation Coverage remains independent from semantic fullscreen state, async permission, and VRR permission.
- A normal output-sized borderless XDG or XWayland window may remain a DirectScanoutSceneCandidate without becoming semantically fullscreen.
- `candidate.is_some() <=> blockers.is_empty()` remains true.
- Existing DRM, generation, CRTC/plane, format/modifier, explicit-sync, cursor, presentation-mode, TEST_ONLY, ownership, pageflip, and composition-fallback authority remains unchanged.
- Fullscreen composition semantics, XDG/EWMH behavior, VRR, Predictive O1, KMS scheduling, generalized plane allocation, Eclipse, and the default Direct Scanout policy are unchanged.
- Build artifacts stay in the repository's existing `target` directory.

## Design

### Semantic fullscreen authority

The Atomic direct path will stop deriving `AsyncEligibility::solitary_fullscreen` from the candidate's single-surface shape. The compositor server will expose a read-only query backed by the existing `FullscreenCompositionPlan`: the candidate root is semantically solitary fullscreen only when it is the plan owner and the existing plan says `solitary_owner_only`.

This preserves the intended distinction:

- borderless output coverage: Direct Scanout allowed, solitary fullscreen false;
- real semantic solitary fullscreen: Direct Scanout allowed, solitary fullscreen true.

No new fullscreen state is introduced and the Direct Scanout scene gate is not tightened.

### Renderer-authoritative content bounds

The renderer already centralizes visual target dimensions in `surface_render_space_assignments()`. A small shared target helper will expose the same target rectangle construction while allowing callers with an `ActiveSceneView` origin cache to provide those origins. Presentation Coverage will pass the active scene's authoritative origins and use the shared render-space targets, including `render_target_size`, for above-content intersection.

The decoration path will accept already-derived active-scene origins for this coverage call so it does not calculate origins a second time. Existing decoration rendering callers continue to derive origins when they do not have an active-scene cache.

### Direct Scanout-off inspection gate

`DirectPresentationInputs` will carry `NativeDirectScanoutPreference`. Inspection will materialize a candidate only when Direct Scanout is enabled or a direct primary is currently presented and needs transition handling. When policy is off with no active direct assignment, candidate keys and coverage analysis are skipped; when an active direct assignment exists, the existing candidate path remains available and composition is forced by policy so fallback can complete correctly.

The frame loop gains no logging and the default policy remains off.

### Bounded scene analysis

`visual_stack_groups()` will bucket each computed visual root's surface indices during the existing one-pass root computation instead of rescanning all roots for every group. `window_visual_stack_order_with_popups()` will index decorations by root once instead of searching the full decoration list per visual group. These are narrow fixes in helpers directly exercised by Presentation Coverage; no generalized renderer rewrite is planned. Any remaining costs in unrelated scene/decoration derivation will be documented as pre-existing rather than described as O(N).

## Test strategy

1. Add a pure presentation-mode regression that an async-hinted, otherwise-qualified borderless candidate is Vsync with `AsyncBlocker::NotSolitaryFullscreen`, plus the existing semantic-solitary case remaining Async-eligible.
2. Add a Presentation Coverage regression where an above surface has a committed rect entirely off the output but a larger `render_target_size` intersects; assert Application content above, the scene blocker, and no candidate. Retain the off-output negative case.
3. Add an inspection-policy unit regression proving disabled/no-active-direct skips candidate derivation, with the active-direct transition case still inspecting.
4. Add or extend integration coverage for the normal XDG borderless case and a narrow XWayland state regression using `SurfaceRenderBackend::Xwayland`, X11 ownership, normal semantic mode, output-sized geometry, XRGB dmabuf metadata, identity viewport, and valid publication generation. Add XWayland negatives for SSD, render-target content above, and non-opaque/unknown opacity.
5. Verify the full requested Rust/build/source-layout commands, compare source-layout violations to baseline, inspect the final diff, and commit.

## Data flow

```text
semantic window state
    -> PresentationCoverageAnalysis
    -> DirectScanoutSceneAnalysis
    -> DirectScanoutSceneCandidate
    -> EffectivePresentation / AsyncEligibility
    -> physical Direct Scanout validation
    -> Atomic TEST_ONLY
    -> submit/pageflip or composition fallback
```

The key policy boundary is that only the semantic fullscreen authority feeds `solitary_fullscreen`; physical coverage only feeds candidate eligibility.
