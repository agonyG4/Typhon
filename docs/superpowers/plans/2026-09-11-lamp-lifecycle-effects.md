# Lamp Lifecycle Effects Implementation Plan

**Goal:** Preserve the compositor-resolved visual appearance of effect-bearing windows while Astrea Lamp owns their minimize and restore presentation.

**Architecture:** Keep `resolved_effect_scene_for_presentation` and the ordinary Effects render graph unchanged for canonical rendering. At semantic lifecycle takeover, retain the resolved effect metadata for each affected root; frame resolution exposes it as lifecycle-only data. The renderer keeps the existing raw-surface Lamp fast path for roots without owned effects and creates a bounded, frozen resolved lifecycle texture for roots with owned effects. That texture is sampled by the same static Lamp mesh and released on physical settlement or teardown.

**Tech Stack:** Rust, Wayland state tests, GLES 3, existing pooled effect textures, existing native scene/pageflip history, `rtk` verification.

## Global Constraints

- Do not redesign Blur Policy, `EffectAnchorScope`, `EffectSceneOrder`, or the ordinary Effects graph.
- Do not modify Eclipse.
- Do not add effect-specific Lamp shader logic for only `system.background_blur`.
- Preserve the no-effects raw lifecycle path.
- Use bounded offscreen resources; no CPU readback, full-output snapshot, timer thread, or per-frame mesh upload.
- Reuse the existing Cargo `target/`; do not run `cargo clean` or create another build tree.
- Add the deterministic regression before changing runtime behavior and keep all documentation in English.

## Implementation Tasks

### Task 1: Gate 1 ownership regression

Add a compositor lifecycle/effects test using a real buffered XDG surface and the existing background-effect and controllable-server helpers. Assign the production blur effect to a transparent root, install a valid Dock anchor, minimize it, and inspect the production resolved presentation scene plus lifecycle source descriptor. The RED assertion must show that the current path has no lifecycle-resolved visual source even though the root owns a resolved effect. Include one trusted surface effect when the same fixture can bind it without unrelated setup. Run only the new filter and record the expected failure before production edits.

### Task 2: Lifecycle visual-source metadata

Add a lifecycle source decision with `NoOwnedEffects` and `ResolvedOwnedEffects` variants. Store the resolved effect instances/signature needed by an active transition, preserve them across reversal, and expose lifecycle-only effect metadata from `ResolvedNativeFrameScene`. Keep ordinary presentation filtering intact. Add state tests for minimize takeover, restore takeover, reversal identity, missing anchor fallback, and source release on teardown.

### Task 3: Resolved lifecycle renderer source

Extend the native EGL request with lifecycle effect metadata and add a renderer-owned bounded resolved-source cache keyed by exact window/root identity and transition source signature. Before Lamp draws an effected root, use the existing effect registry and graph execution to resolve that root over the already-composited background into a pooled texture, then freeze it for the transition. Keep resource consumers as the union of canonical, raw lifecycle, and resolved-source consumers. If source allocation or shader execution fails, clear lifecycle ownership/suppression and use the established immediate fallback path safely.

### Task 4: Lamp source selection and ordering

Make Lamp commands select either raw retained surface/decorations or the resolved lifecycle texture explicitly. Deform the resolved texture with the existing static mesh and uniforms. Preserve shared visual-group metadata, empty opaque regions, bounded lifecycle damage, and the existing base/effects → Lamp → external overlays/cursor order in both renderer paths. Ensure progress-only frames do not rebuild topology.

### Task 5: Settlement, policy reload, and regressions

Release resolved sources only after matching physical endpoint acknowledgement or root teardown. Keep Direct Scanout composition-aware and allow normal eligibility after settlement. Add tests for minimize/restore effects, reversal, effect-policy reload, retained updates, suppression without duplicate canonical drawing, source lifetime, and scanout recovery. Rerun existing effects, EGL, native-output, and full-suite regressions without changing unrelated failures.

### Task 6: Verification and handoff

Run the exact fresh `rtk` formatting, check, clippy, focused effects/EGL/native-output, full test, source-layout, and presentation dry-run commands. If a hardware session is available, perform the transparent effected-window and opaque fast-path acceptance matrix at 1920×1080/165 Hz; otherwise report native qualification unavailable. Commit only the implementation files and tests, leaving unrelated worktree changes untouched.
