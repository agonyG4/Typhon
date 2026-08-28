# Typhon Native Output Damage Authority v1 Plan

## Scope

Separate surface identity from logical output damage, output-slot repair, damage-journal settlement, and physical presentation. Preserve the existing buffer-age, Atomic slot quarantine, exact retry ownership, compatibility EGL invalidation, cursor-plane, and protocol FIFO policies.

## Implementation sequence

1. Add RED regressions for identity-only Empty transitions, geometry/map/unmap authority, valid retry ownership, Direct Scanout resource identity, more-than-capacity Empty journal settlement, native no-visual-change protocol behavior, Atomic renderer skip terminality, and hardware cursor-only preservation.
2. Remove the identity-only footprint fallback from `native_scene_surface_transition_damage()` while retaining geometry, map, removal, visibility, decoration, cursor, and topology authorities.
3. Add `SurfaceDamageSettlement::{Presented, NoVisualChange}` and route both public settlement wrappers through one keyed internal operation. Make no-visual-change frame-batch completion settle its owned token without producing a presentation.
4. Change the native no-primary-work path to capture exact resolved-scene lineage, settle it through no-visual-change batch completion, and avoid `FramePresentation::software_now()`.
5. Change explicit Atomic renderer `NoLogicalDamage` handling to cancel the unused slot, settle the exact token as no-visual-change, complete the batch terminally, and retain existing retry/failure behavior for real errors.
6. Correct the integrated physical swapchain oracle so Empty commits keep logical pixels unchanged while Direct Scanout resource identity may change independently.
7. Run focused tests, the full `rtk` verification workflow, and two manual adversarial reviews. Record exact output and classify unrelated dirty-checkout failures.

## Ownership invariants

- Surface identity and commit sequence identify content lineage only.
- Surface damage identifies logical changed pixels only.
- Buffer age, confirmed output history, and slot ownership identify repair work.
- `NoVisualChange` advances damage accounting and eligible protocol completion only; it never advances physical presentation state.
- Only confirmed backend presentation advances physical presentation history.
- Cursor-only hardware plane work remains independently owned by its PlaneDelta transaction.
