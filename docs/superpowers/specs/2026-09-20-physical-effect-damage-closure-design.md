# Physical Effect Damage Closure Design

## Goal

Make confirmed-pageflip damage history include effect output pixels derived from the exact physical source transition and preserve continuous-effect dirty evidence from the resolved frame.

## Design

Freeze a renderer-independent recipe beside each resolved frame. It contains each non-empty visible effect instance's identity, region, and validated program aggregate footprint, plus the resolved scene's frame-local dirty region. A missing validated program marks the recipe conservatively incomplete. Freeze this while the resolved effect scene, captured registry generation, and output bounds are all available, then copy it into the immutable native frame snapshot that moves through READY and SUBMITTED.

At pageflip, keep `NativeSceneHistory`'s exact submitted-token lookup and actual presented predecessor. Reconstruct the current physical base transition from scene, effect, cursor, Presentation Opacity/Clip, and lifecycle evidence, then add only the current submitted frame's frozen local dirty region. Expand that same source region through every non-empty previous and current recipe instance with `plan_effect_damage()`, unioning each output contribution without feeding generated outputs back into later instances. If the source transition is non-empty and either recipe is incomplete, use full-output damage.

Keep this evidence and expansion in a focused `native_output::output::physical_effect_damage` module. The existing owner-scoped Presentation effect influence remains the source damage for Clip/Opacity changes; the compositor expansion may then damage a foreign effect only where its frozen source/output influence intersects that source region. Include `OutputPostProcess` because the physical recipe is unowned. Do not store renderer plans or query mutable compositor/registry state on pageflip.

## Alternatives considered

- **Use render-time final damage as pageflip history:** rejected because render-ahead candidates can use a predecessor that never physically presents.
- **Store effect recipes in EGL renderer state:** rejected because the renderer is not the immutable physical-frame authority and would couple frozen evidence to GLES state.
- **Damage all effects for any Presentation change:** rejected because it breaks owner isolation and over-invalidates unrelated outputs.

## Verification design

Add unit coverage for owner-scoped Clip input versus foreign-effect consequence, unrelated-effect exclusion, visible/unowned postprocess expansion, added/removed effects, conservative and empty/full paths, and parity with `compile_frame_execution_plan()` for the same source region and footprints. Extend physical scene-history coverage for rejected render-ahead predecessors and delayed pageflip evidence. Exercise the actual presented transition journal through age-2/age-3 repair, including a continuous frame-local dirty region.

Run focused history, Presentation damage, effect planning/render graph, partial repaint, and render-ahead/pageflip suites, followed by the broad Cargo and source-layout checks. Route every build artifact through `/mnt/Aether/Desktop/GitHub`.

## Boundaries

This change does not alter Direct Scanout, ACK ownership, `visual_clip`, shader semantics, effect dependency semantics, buffer-age slot ownership, or canonical effect/Presentation authority.
