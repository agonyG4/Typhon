# Typhon Presentation Effect Influence Damage Design

## Goal

Close the physical-damage gap where WindowGroup PresentationOpacity and PresentationClip changes omit final output from effects owned by that group.

## Frame evidence

Frame resolution will capture one trusted `EffectRegistryGeneration` and use its validated program footprints to compute each resolved owned effect's final output influence. That region expands the resolved visible effect region by aggregate output outsets, clamps it to the frame's output bounds, and excludes source sample radius. Anchored surface effects resolve through the anchor surface's presentation owner root to the stable WindowGroup `SceneNodeId`. All effects for one owner merge into one bounded region; `OutputPostProcess` contributes no owner evidence. An unexpected missing validated program conservatively assigns the output bounds to its resolved owner.

The merged owner evidence and that frame's root surface adapter are stored on `NativeSceneSnapshot`, so `NativeSceneHistory` carries it through ready, submitted, and presented states. No pageflip-time registry or compositor lookup is needed.

## Physical damage

Opacity compares stable WindowGroup IDs and unions the previous and current owner surface, SSD, and effect influence bounds when opacity changes. Clip uses the same stable owner IDs and intersects the previous influence with the previous physical Clip and the current influence with the current physical Clip. An unbounded Clip uses the complete region; a zero-area Clip contributes no current pixels. Both `NativeSceneHistory` and presentation-worker damage continue calling these same shared helpers. Global effect-transition damage remains separate.

## Scope and verification

This is CPU-side immutable frame metadata and damage calculation. It does not change effect rendering, presentation ordering, renderer signatures, GPU resources, or `NativeSceneHistory` authority. Focused regressions cover effect-only halos, Clip transitions, Opacity changes, unrelated owners, stable owner identity across root replacement, and submitted-frame immutability. Full Cargo verification and the existing source-layout checker run with all build output under `/mnt/Aether/Desktop/GitHub`.
