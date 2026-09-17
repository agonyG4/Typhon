# Native-Faithful Stacked Backdrop Regression Hardening

## Goal

Harden the existing GLES checkpoint regression so it reproduces the native heterogeneous stacked backdrop ordering, proves real scene advancement and compiler-derived checkpoint dependencies, adds Dock and TopBar pixel-level coverage, and records evidence that the regression detects the legacy presentation-only scene-work behavior.

## Current evidence

- `src/egl_renderer.rs` currently builds the stacked case by cloning one `ResolvedEffectInstance`; both effects therefore share `BeforeSurface(42)`, `VisualGroup`, visual group, geometry, and capture geometry.
- `compile_frame_execution_plan()` derives checkpoint dependencies by walking already compiled checkpoint instances and testing exact capture-region intersection. The fixture must therefore order A before B and make B's capture region intersect A's dependency influence.
- `scene_work_regions()` is used by both `plan_effect_surface_consumers_with_debug_config()` and `execute_graph_passes_inner()`, so the consumer planner and execution path already share one scene-work authority.
- `SceneWorkRegions` stores `presentation_work` and `framebuffer_checkpoint_work`, but the callers consume only `scene_work_rects` and `extra_scene_work`.
- The graph coverage check reports no recorded issue for the planned files: `src/egl_renderer.rs`, `src/egl_renderer/effects/executor.rs`, `src/effects/damage.rs`, `src/effects/render_graph.rs`, and `src/compositor/effects.rs`.

## Design

### Native-faithful fixture

Replace the synthetic stacked helper with a dedicated helper accepting independent A and B specifications:

- target bounds, anchor, anchor scope, visual group, and scene order for each instance;
- one shared immutable blur program/registry;
- independently allocated effect instance IDs and signatures.

The Dock graph will use A at `BeforeSurface(11)` with `VisualGroup` scope and B at `BeforeSurface(7)` with `Surface` scope. The helper will use explicit scene-order values that place A before B, and `ResolvedEffectScene::new()` will remain responsible for semantic sorting. No checkpoint dependency will be inserted by the fixture.

The A and B regions will be chosen so the normal damage planner produces the native domains:

- A capture: `(66, 120, 1000, 937)`;
- B capture: `(762, 976, 396, 104)`.

The fixture will assert the domains from compiled `GraphTexturePlan`s, the single B checkpoint dependency, and exact-region dependency influence coverage. Its commands will use `Surface(100)` for the non-uniform fullscreen background, `Surface(11)` for A-associated content, and `Surface(7)` for the Dock-associated content. A and B will have distinct command ranges and composition boundaries.

### GLES evidence

The Dock regression will execute at `1920x1080` with `TopLeftScanout`, replay capture, and partial Kawase. It will render a full frame, retain the previous framebuffer, update a small repair, optionally poison pooled textures with incompatible magenta/green patterns, render the partial candidate, and independently render a full-current reference. It will assert:

- B has `framebuffer_blit`, one checkpoint, replay capture policy, partial Kawase policy, and `missing_pixels=0` in trace events;
- candidate pixels equal the previous frame outside repair and the full-current reference within repair;
- poisoned candidates are equal to each other and to the full-current reference within repair;
- A's dependency influence intersected with B's required region is covered by A's semantically valid Composite output using `EffectRegion` operations;
- A and B scene cursors/ranges differ and at least one command advances between them;
- surfaces needed only by expanded checkpoint scene work are present in `SurfaceConsumerPlan`.

The TopBar regression will use a dedicated independent-identity scene and a native `(0, 0, 120, 65)` checkpoint domain. It will use the same two-frame, three-image, poison-independent GLES assertions and require an actual checkpoint dependency.

A Full-Kawase control will run the same heterogeneous Dock graph with replay capture and full Kawase. It is diagnostic evidence only; production remains partial Kawase.

### Sensitivity proof

After the native-faithful regression is written, a local-only temporary edit will make checkpoint scene work equal presentation work while leaving B's framebuffer capture behavior unchanged. The native Dock regression must fail through a missing-pixel invariant, pixel mismatch, or poison dependence. The temporary edit will then be reverted completely and will not be exposed through an environment variable or committed.

If it remains green, the task stops with evidence that the fixture does not prove the suspected root cause; no further production changes are made.

### Production scope

Production behavior remains unchanged except for removing the two unused stored `SceneWorkRegions` fields. The local `presentation_work` and `checkpoint_work` variables remain in `scene_work_regions()`, while only `scene_work_rects` and `extra_scene_work` are stored. No preservation optimization, cache, replay architecture replacement, blur-math change, or production Full-Kawase change is included. Existing semantic validity, coordinate mapping, ordering, consumer planning, and presentation damage logic remain authoritative.

### Verification

Run focused compiler, exact-region, scene-advancement, consumer-plan, capture-coordinate, fullscreen, ordering, four-policy diagnostic, Dock, TopBar, poison, and native-faithful tests. Then run the renderer/effects/compositor suites plus `cargo fmt --check`, `cargo check --workspace --all-targets`, and `cargo clippy --workspace --all-targets -- -D warnings`. Run the native gate only after the automated regression is both sensitive and green, and record whether a valid hardware session contains partial checkpoint captures with `missing_pixels=0`.

## Out of scope

- `PersistentBackdropCache` and all preservation bandwidth optimizations;
- changing production capture policy, Kawase policy, replay architecture, or blur formulas;
- changing checkpoint ordering, semantic validity, capture texture coordinate mapping, SurfaceConsumerPlan architecture, fullscreen filtering/repair, Composite clipping, presentation damage, alpha mode, or Shell blur behavior unless the new native-faithful regression exposes a concrete correctness defect.
