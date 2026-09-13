# Astrea Lamp v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Lamp lifecycle presentation use one frozen complete visual group and a reversible, directional staged Genie deformation while preserving the existing lifecycle ownership and physical-settlement architecture.

**Architecture:** Extend the lifecycle sample/transition data in `window_lifecycle_animation.rs` with explicit canonical/presented client and visual rectangles, anchor, direction, and frozen shape parameters. Capture that data at semantic takeover from retained root-owned surfaces and the SSD scene snapshot; preserve it across reversal and discard it only after physical settlement. Keep one pure footprint helper for EGL/native damage, one CPU warp reference for tests, and mirror the same equations in the existing GLSL overlay.

**Tech Stack:** Rust unit tests, existing compositor lifecycle/render modules, GLES 3.0 GLSL, `rtk cargo`, existing repository `target` directory.

## Global Constraints

- Preserve `WindowLifecycleAnimator`, fresh `LifecycleTransitionId`, frozen Dock anchor, stale pageflip rejection, physical pageflip settlement, suppression, retained surfaces, Wayland/XWayland parity, Direct Scanout blocking, and Animation Control Plane speed semantics.
- Do not redesign lifecycle ownership, Effects execution/graph ordering, or add a plugin/state-machine framework.
- Do not fix Blur / `ResolvedOwnedEffects` invisibility.
- Do not modify KMS, commit worker, triple buffering, prediction, native-output pacing, or presentation scheduling unless a deterministic regression proves a separate defect.
- Use the existing repository `target` directory; never run `cargo clean` or create another target directory.
- Preserve all pre-existing dirty worktree modifications and stage only files belonging to this Lamp change or its design/plan commits.
- Use one scalar progress `p` in `[0, 1]`; derive bump/stretch/squash channels as pure functions and sample restore by reversing `p`.
- Keep the Lamp base duration in the 280–320 ms class and keep the final endpoint fully invisible.
- Keep the global `MAX_LAMP_VERTICES` budget and coarsen deterministically when concurrent meshes exceed it.

## File Map

- Modify `src/window_lifecycle_animation.rs`: explicit visual-group geometry, frozen transition data, direction/shape/stage channels, CPU warp reference, footprint contract, opacity, and focused unit tests.
- Modify `src/compositor/state/lifecycle_animation.rs`: capture and pass frozen visual-group bounds at takeover, preserve frozen state across reversal, and expose the same bounds to lifecycle samples.
- Modify `src/compositor/state/windows.rs`: assemble the takeover snapshot from retained root/subsurface render bounds and the current SSD render instance before canonical state is discarded.
- Read `src/compositor/render.rs`: use the existing `DecorationRenderInstance::scene_snapshot().bounds()` as the SSD bounds authority; do not add a second decoration-bounds representation.
- Modify `src/native_output/runtime/scene_history.rs`: consume the visual-group footprint contract and add SSD/subsurface/settlement/clipping regressions.
- Modify `src/egl_renderer.rs`: use visual-group lifecycle samples for geometry keys, decoration/client command grouping, denser bounded mesh admission, progress-only uniforms, and renderer tests.
- Modify `src/egl_renderer/program.rs`: replace the generalized pull shader with the axis-rotated staged warp and matching uniforms.
- Read `src/native_output/runtime/frame.rs`: verify the existing frame-scene handoff carries the expanded lifecycle sample; modify it only when compilation identifies a required field propagation, without changing presentation ownership.

### Task 1: Prove the SSD residual root cause with a RED regression

**Files:**
- Modify: `src/window_lifecycle_animation.rs` test module.

**Interfaces:**
- Consumes: existing `PresentationRect` and `lamp_footprint`.
- Produces: a deterministic failing contract showing client-only lifecycle bounds omit an SSD outer rectangle above the client.

- [ ] **Step 1: Write the failing test**

Add a test named `lamp_footprint_does_not_cover_ssd_above_client_before_visual_group_fix`:

```rust
#[test]
fn lamp_footprint_does_not_cover_ssd_above_client_before_visual_group_fix() {
    let client = rect(400.0, 100.0, 800.0, 600.0);
    let ssd_outer = rect(384.0, 60.0, 832.0, 640.0);
    let source = client;
    let anchor = rect(900.0, 900.0, 64.0, 64.0);

    let footprint = lamp_footprint(source, client, anchor).expect("valid footprint");

    assert!(footprint.y() <= ssd_outer.y());
    assert!(footprint.y() + footprint.height() >= ssd_outer.y() + ssd_outer.height());
}
```

The assertions deliberately describe the required visual group while the call still supplies only the current client/full-window rectangles.

- [ ] **Step 2: Run the RED test and verify the failure is causal**

Run:

```bash
rtk cargo test --locked lamp_footprint_does_not_cover_ssd_above_client_before_visual_group_fix
```

Expected: FAIL because the current footprint begins at `y = 100`, not `y = 60`. If it passes, stop production work and trace the actual stale-titlebar ownership boundary.

- [ ] **Step 3: Commit only the RED regression**

```bash
rtk git add src/window_lifecycle_animation.rs
rtk git commit -m "test: reproduce lamp SSD footprint omission"
```

### Task 2: Add the frozen visual-group geometry contract

**Files:**
- Modify: `src/window_lifecycle_animation.rs`.
- Modify: `src/compositor/state/lifecycle_animation.rs`.
- Modify: `src/compositor/state/windows.rs`.

**Interfaces:**
- Consumes: retained `RenderableSurface` bounds, `DecorationRenderInstance::scene_snapshot().bounds()`, the existing pageflip-confirmed source rectangle, and the existing Dock anchor.
- Produces: `LifecycleVisualGroup` with frozen `canonical_client_rect`, `canonical_visual_rect`, `presented_client_rect`, `presented_visual_rect`, `anchor_rect`, finite `direction`, `shape_factor`, and `bump_distance`; pure helpers `canonical_visual_rect(...)` and `presented_visual_rect(...)`; `LampWindowSample` and `LifecycleFrameLamp` expose these values without recapture.

- [ ] **Step 1: Add RED tests for union, CSD, subsurface, affine, and freeze semantics**

Add focused tests before changing implementation. Use the existing `rect` helper and real `WindowLifecycleAnimator` samples:

```rust
#[test]
fn canonical_visual_group_unions_client_subsurface_and_ssd_outer_bounds() {
    let actual = canonical_visual_rect(
        rect(400.0, 100.0, 800.0, 600.0),
        [rect(360.0, 120.0, 32.0, 760.0)],
        Some(rect(384.0, 60.0, 832.0, 640.0)),
    );
    assert_eq!(actual, Some(rect(360.0, 60.0, 856.0, 820.0)));
}

#[test]
fn canonical_visual_group_does_not_expand_csd_window() {
    let actual = canonical_visual_rect(
        rect(400.0, 100.0, 800.0, 600.0),
        [],
        None,
    );
    assert_eq!(actual, Some(rect(400.0, 100.0, 800.0, 600.0)));
}

#[test]
fn presented_visual_group_reuses_canonical_to_presented_client_affine() {
    let actual = presented_visual_rect(
        rect(400.0, 100.0, 800.0, 600.0),
        rect(360.0, 60.0, 832.0, 640.0),
        rect(200.0, 160.0, 960.0, 720.0),
    );
    assert_eq!(actual, Some(rect(152.0, 112.0, 998.4, 768.0)));
}
```

For `reversal_reuses_frozen_visual_group_and_anchor_with_fresh_id`, start a
real `WindowLifecycleAnimator` transition with a group whose visual `y` is 60,
sample at 100 ms, call `start_or_reverse` at that same timestamp with a new
request whose visual `y` is 20 and a different anchor, then assert the two
samples have equal canonical/presented visual rectangles and anchor but
different transition IDs. For `settled_new_transition_captures_new_visual_bounds`,
acknowledge the first transition at its exact endpoint, start a new transition
with the `y = 20` group, and assert the new sample has `canonical_visual_rect.y()
= 20.0`.

Use real `PresentationRect` values and `WindowLifecycleAnimator`; do not mock the animator or assert only internal fields.

- [ ] **Step 2: Run the new tests and verify expected RED failures**

Run:

```bash
rtk cargo test --locked canonical_visual_group
rtk cargo test --locked reversal_reuses_frozen_visual_group
rtk cargo test --locked settled_new_transition_captures_new_visual_bounds
```

Expected: failures for missing visual-group representation, incorrect affine mapping, and recapture/freeze behavior.

- [ ] **Step 3: Implement the pure visual-group data model and affine mapping**

Keep `LifecycleTransitionId` and `WindowLifecycleAnimator` ownership unchanged. Replace ambiguous `full_window_rect` use with explicit fields while preserving compatibility at call sites until all consumers migrate. Build the canonical visual union from the root/subsurface render bounds and SSD snapshot bounds. Apply the axis-aligned affine

```rust
presented = presented_client.origin
    + (canonical_visual - canonical_client.origin) * presented_client.scale
```

per axis, with finite validation and clamped positive extents. Store this group in the transition, copy it into samples, and when `start_or_reverse` finds an existing transition copy the old group/anchor/direction/shape values rather than any new request geometry.

- [ ] **Step 4: Capture the group at minimize/restore takeover**

Capture decorations while the window still has its canonical layout, union them with the retained root-owned `RenderableSurface` bounds, then pass that snapshot into `begin_lifecycle_minimize`. For restore, use the frozen group when a transition exists; only construct a fresh group when no settled transition is active. Keep canonical window state mutable and independent.

- [ ] **Step 5: Run the focused geometry tests GREEN**

Run:

```bash
rtk cargo test --locked canonical_visual_group
rtk cargo test --locked reversal_reuses_frozen_visual_group
rtk cargo test --locked settled_new_transition_captures_new_visual_bounds
rtk cargo test --locked lifecycle_transition_ids_are_fresh_for_reversal
```

Expected: PASS with no lifecycle ownership regressions.

- [ ] **Step 6: Commit the geometry contract**

```bash
rtk git add src/window_lifecycle_animation.rs src/compositor/state/lifecycle_animation.rs src/compositor/state/windows.rs src/compositor/render.rs
rtk git commit -m "feat: freeze complete visual group for lamp transitions"
```

### Task 3: Repair lifecycle damage through the visual-group footprint

**Files:**
- Modify: `src/window_lifecycle_animation.rs`.
- Modify: `src/native_output/runtime/scene_history.rs`.
- Modify: `src/egl_renderer.rs`.

**Interfaces:**
- Consumes: `LifecycleFrameLamp` frozen canonical/presented visual bounds and shape/bump data.
- Produces: one pure bounded footprint function used by EGL lifecycle damage, native scene-history repair, output-intersection admission, and bump excursion coverage.

- [ ] **Step 1: Add RED damage tests**

Add `lifecycle_footprint_repairs_ssd_titlebar_above_client` and assert the
returned footprint has `y <= 60.0` and a bottom edge at least 700.0 for a
client at `(400, 100, 800, 600)` and SSD outer bounds `(384, 60, 832, 640)`.
Add `lifecycle_footprint_covers_subsurface_outside_root` and assert a retained
subsurface at `(360, 120, 32, 760)` is contained by the returned footprint.
Add `csd_lifecycle_footprint_has_no_fake_decoration_expansion` and assert that
omitting SSD bounds leaves the footprint equal to the client/subsurface union.
Add `lifecycle_footprint_covers_anchor_and_bump_excursion` and assert both the
anchor rectangle and the positive conservative bump extent are contained.
Add `lifecycle_damage_clips_finite_to_every_output_edge` and invoke the native
damage conversion with footprints extending past each of the four output
edges; assert each result is finite, non-negative, and no larger than the
output dimensions.

- [ ] **Step 2: Run the RED damage tests**

```bash
rtk cargo test --locked lifecycle_footprint
rtk cargo test --locked lifecycle_damage
```

Expected: SSD/subsurface/bump tests fail against the client-only contract.

- [ ] **Step 3: Replace the ambiguous footprint inputs**

Define the footprint input as the immutable lifecycle visual group plus its conservative motion excursion. Union presented source visual bounds, canonical visual bounds where old pixels can remain, the anchor, and bump bounds; reject non-finite values; return a finite `PresentationRect`. Keep clipping in the existing output-specific damage conversion, not in the pure union helper.

- [ ] **Step 4: Route both damage users through the same contract**

Update `src/native_output/runtime/scene_history.rs::lifecycle_damage_rects`, `src/egl_renderer.rs::lifecycle_damage_for_samples`, and Lamp output-intersection checks to call the same helper. Do not escalate to full-output repaint. Ensure settlement uses the frozen old group so the SSD titlebar is included in the final repair.

- [ ] **Step 5: Run damage tests GREEN**

```bash
rtk cargo test --locked lifecycle_footprint
rtk cargo test --locked lifecycle_damage
rtk cargo test --locked native_output::runtime::scene_history
```

Expected: PASS, including existing physical scene promotion and stale lifecycle tests.

- [ ] **Step 6: Commit the damage authority**

```bash
rtk git add src/window_lifecycle_animation.rs src/native_output/runtime/scene_history.rs src/egl_renderer.rs
rtk git commit -m "fix: damage the complete lamp visual group"
```

### Task 4: Replace the CPU pull with directional staged Genie math

**Files:**
- Modify: `src/window_lifecycle_animation.rs`.

**Interfaces:**
- Consumes: frozen visual source/anchor geometry and scalar progress `p`.
- Produces: `LampDirection`, centralized Lamp constants, pure `lamp_stage_channels(p, shape_factor, bump_distance)`, and `lamp_warp_point(group, direction, shape_factor, bump_distance, bump_progress, stretch_progress, squash_progress, point)`.

- [ ] **Step 1: Add RED CPU warp tests**

Add tests for exact identity at `p=0`, exact anchor mapping at `p=1`, finite intermediate values, all four directions under rotation, monotonic movement toward the anchor, continuous stage boundaries, reversal continuity, tiny/huge windows, tiny anchors, and overlapping/close anchors. Add a test that an already separated window has zero bump distance.

- [ ] **Step 2: Run the RED CPU tests**

```bash
rtk cargo test --locked lamp_warp
rtk cargo test --locked lamp_stage
rtk cargo test --locked lamp_direction
```

Expected: failures because the current pull has no direction enum, stage channels, bump handling, or axis-rotation invariant.

- [ ] **Step 3: Implement centralized constants and direction selection**

Use finite direction selection from the closest output edge to the frozen anchor. When the anchor is not clearly edge-associated, choose the dominant source-center-to-anchor-center axis and sign. Keep `Top`, `Right`, `Bottom`, and `Left` as one enum. Freeze direction and shape values in the transition.

Use documented constants for initial shape factor near `0.20`, stretch weight near `0.7 * shape_factor`, dominant squash weight, narrow opacity endpoint, and a 300 ms-class base duration. Compute a finite geometry-dependent shape factor that increases as the moving extent approaches the anchor along the selected axis. Compute bump distance only when source and anchor overlap/cross along that axis.

- [ ] **Step 4: Implement the axis-rotated staged warp**

Normalize the point into the source group, rotate into a Bottom-axis frame, apply a continuous InOutCubic spatial curve, delay the far edge relative to the Dock-nearest edge, form the neck during stretch, and converge normalized coordinates into the anchor during squash. Rotate back for the selected direction. Clamp normalized values and use finite fallbacks for every denominator. At `p=0` return the presented source point exactly; at `p=1` return the corresponding normalized anchor point exactly.

- [ ] **Step 5: Update pure opacity and channel sampling**

Keep opacity at `1.0` until the narrow final interval near `0.98`, then use a continuous easing curve to reach `0.0` exactly at `p=1`. Derive all channels directly from `p`; do not add mutable renderer stage state.

- [ ] **Step 6: Run the CPU tests GREEN**

```bash
rtk cargo test --locked lamp_warp
rtk cargo test --locked lamp_stage
rtk cargo test --locked lamp_direction
rtk cargo test --locked lamp_progress_and_opacity_are_monotonic
```

Expected: PASS for all directions, endpoint/reversal invariants, and finite edge cases.

- [ ] **Step 7: Commit the CPU reference model**

```bash
rtk git add src/window_lifecycle_animation.rs
rtk git commit -m "feat: add directional staged lamp deformation"
```

### Task 5: Mirror the warp in GLSL and integrate the complete group

**Files:**
- Modify: `src/egl_renderer/program.rs`.
- Modify: `src/egl_renderer.rs`.

**Interfaces:**
- Consumes: CPU-tested lifecycle samples and existing `EglLampDrawCommand`/SSD primitive commands.
- Produces: GLSL uniforms for visual source/anchor/direction/shape/bump/stage channels, one warped group for client/subsurface/SSD commands, and unchanged external overlay ordering.

- [ ] **Step 1: Add RED renderer tests**

Add `lamp_commands_include_frozen_ssd_primitives_in_the_same_visual_group`;
construct a decorated retained scene, call the existing Lamp command builder,
and assert the client/subsurface and every SSD primitive command carry the same
window/root identity and participate in one Lamp sample.
Add `lamp_mesh_topology_upload_count_is_independent_of_progress_frames`;
rebuild commands once, render samples at 0.1, 0.5, and 0.9, and assert the
geometry key and topology-upload counter are unchanged while uniform updates
are recorded for each sample.
Add `lamp_mesh_reaches_preferred_cell_size_before_global_budget`; pass a
1920-pixel-wide group to the grid planner and assert its selected cell width is
within 30–36 physical pixels while its vertex count is below
`MAX_LAMP_VERTICES`.
Add `lamp_mesh_coarsens_deterministically_under_global_vertex_budget`; fill
the shared vertex budget with the same concurrent window set twice and assert
both calls return identical subdivisions, command ranges, and total vertices.

- [ ] **Step 2: Run the RED renderer tests**

```bash
rtk cargo test --locked lamp_mesh
rtk cargo test --locked lamp_commands
rtk cargo test --locked egl_renderer
```

Expected: failures for the old 48-pixel/32-cap mesh and any titlebar/client grouping gap.

- [ ] **Step 3: Update the shader inputs and equations**

Replace `u_full_window_rect`/`u_pull_constant`-only behavior with uniforms carrying the visual source rectangle, anchor, direction, shape factor, bump distance, and the three sampled channels. Keep the existing texture, output-size, framebuffer-origin, progress/opacity, and sampler flow. Mirror the CPU finite clamps, InOutCubic curve, axis rotation, staged neck/squash, and exact endpoint mapping. Keep the shader source Typhon-native; do not copy reference GPL implementation text.

- [ ] **Step 4: Make the renderer use one group transform**

Ensure all lifecycle surface commands and retained SSD primitives for one window are emitted into the same Lamp command set and use the same sample. Keep Dock and cursor drawing after the Lamp overlay. Preserve LegacyScene and EffectGraph ordering and current resolved-source interfaces.

- [ ] **Step 5: Raise bounded mesh fidelity**

Set the target cell size to a documented 30–36 pixel value and raise the per-axis cap enough for a 1920-pixel group to achieve it. Preserve `MAX_LAMP_VERTICES`; when the preferred grid does not fit, reduce subdivisions deterministically before admitting the command. Keep `lamp_geometry_key` independent of progress so animation frames update uniforms only.

- [ ] **Step 6: Run renderer tests GREEN**

```bash
rtk cargo test --locked lamp_mesh
rtk cargo test --locked lamp_commands
rtk cargo test --locked egl_renderer
rtk cargo test --locked decoration
```

Expected: PASS, including required decoration resource retention and static topology reuse.

- [ ] **Step 7: Commit shader and renderer integration**

```bash
rtk git add src/egl_renderer.rs src/egl_renderer/program.rs src/compositor/render.rs
rtk git commit -m "feat: render Lamp v2 with dense directional mesh"
```

### Task 6: Cover lifecycle visibility eligibility without changing pacing

**Files:**
- Modify: `src/compositor/state/lifecycle_animation.rs`.
- Modify: `src/window_lifecycle_animation.rs` tests if shared pure sampling belongs there.

**Interfaces:**
- Consumes: frozen visual group, staged opacity, and existing render evidence/fallback state.
- Produces: deterministic coverage for endpoint no-visual settlement and visible progress-frame eligibility.

- [ ] **Step 1: Add RED lifecycle tests**

Add `invisible_restore_start_may_settle_no_visual_change`; construct a restore
sample at `p = 1.0` with endpoint opacity zero and assert the existing no-visual
settlement path may retire that cycle safely.
Add `leaving_invisible_endpoint_creates_lifecycle_damage`; sample the same
restore at `p = 0.99` and assert the lifecycle footprint is non-empty and the
sample is eligible for a presentable frame.
Add `visible_lamp_progress_cannot_repeatedly_settle_no_visual_change`; submit
two samples with distinct visible progress values and assert the second one
remains pending until its render evidence/pageflip settlement is processed.

- [ ] **Step 2: Run the RED lifecycle tests**

```bash
rtk cargo test --locked lifecycle_animation
```

Expected: the endpoint case may already pass; the test must document that as legitimate, while visible-progress assertions expose any remaining false no-visual path.

- [ ] **Step 3: Implement only the sampling/evidence correction required by the tests**

Use the visual-group footprint and staged opacity when deciding whether a Lamp sample is visible and damageable. Preserve fallback/evidence semantics, transition IDs, suppression, and physical ledger behavior. Do not alter native-output scheduling or pacing code.

- [ ] **Step 4: Run lifecycle regressions GREEN**

```bash
rtk cargo test --locked lifecycle_animation
rtk cargo test --locked stale
rtk cargo test --locked physical
```

Expected: PASS for stale ACK, physical endpoint settlement, suppression, disable-mid-flight, reversal IDs, teardown, and the new visibility contracts.

- [ ] **Step 5: Commit lifecycle eligibility coverage**

```bash
rtk git add src/window_lifecycle_animation.rs src/compositor/state/lifecycle_animation.rs
rtk git commit -m "test: qualify visible lamp lifecycle progress"
```

### Task 7: Full verification and handoff

**Files:**
- No new production files; only test/doc adjustments discovered by verification.

- [ ] **Step 1: Run focused requested filters**

```bash
rtk cargo test --locked lamp
rtk cargo test --locked lifecycle_animation
rtk cargo test --locked decoration
rtk cargo test --locked egl_renderer
rtk cargo test --locked native_output
```

- [ ] **Step 2: Run fresh repository verification in the existing target directory**

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 3: Run any existing source-layout/presentation qualification commands**

Discover them with `rtk rg -n "qualification|source-layout|presentation" Makefile justfile .github scripts docs` and run only commands already present in the checkout.

- [ ] **Step 4: Inspect the final diff and worktree boundary**

```bash
rtk git status --short
rtk git diff --stat
rtk git diff --check
```

Verify unrelated pre-existing KMS, pacing, worker, native-output, capture, and presentation changes remain in the worktree and were not staged by Lamp commits.

- [ ] **Step 5: Report native acceptance honestly**

Only report the 1920x1080@165 Hz Blur-disabled Wayland/XWayland visual checks if they were actually run. Otherwise state that native acceptance was not executed. Always retain the known Lamp + Blur / `ResolvedOwnedEffects` issue in the remaining-known-issue section.
