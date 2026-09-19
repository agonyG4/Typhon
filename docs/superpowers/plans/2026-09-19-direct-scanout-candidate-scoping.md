# Direct Scanout Candidate-Scoped Presentation Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Scope temporary Geometry and Opacity Direct Scanout blockers to the candidate WindowGroup and prove immutable XWayland opacity frame history across backing replacement.

**Architecture:** Keep `PresentationEngine` as the only active-track authority keyed by `SceneNodeId`. Resolve the Direct Scanout covering application group first, adapt its root surface to the owning WindowGroup SceneNodeId, and query property-specific track maps. Keep canonical opacity and physically presented presentation snapshots as separate candidate-root authorities; keep `NativeSceneHistory` promotion unchanged and test its stored snapshot behavior.

**Tech Stack:** Rust, Cargo, existing compositor state tests, `PresentationEngine`, `NativeSceneHistory`, `rtk` command proxy, Codebase Memory MCP.

## Global Constraints

- Compile and test in `/home/agony/GitHub/Typhon`; do not create a build outside this checkout.
- Do not use subagents.
- Preserve unrelated dirty work; stage only files changed for this task in each commit.
- Do not modify renderer/effect semantics, transaction/revision semantics, Clip, LifecycleAnimation/Lamp, or pageflip promotion logic.
- Use `PresentationEngine::has_geometry_track(SceneNodeId)` and `has_opacity_track(SceneNodeId)`; do not add root-keyed track storage or a generalized dynamic-property query.
- Keep scheduler/general `has_pending_visible(...)` behavior unchanged.
- Keep source-layout debt at or below the starting state; do not raise limits.

---

### Task 1: Add property-specific Presentation Engine query coverage

**Files:**
- Modify: `src/presentation_animation/transactions_tests.rs`
- Modify: `src/presentation_animation/engine.rs`

**Interfaces:**
- Consumes: Existing `PresentationEngine::commit`, `PresentationGeometryMutation`, and `PresentationOpacityMutation` test APIs.
- Produces: Public `PresentationEngine::has_geometry_track(SceneNodeId) -> bool`, symmetric with `has_opacity_track` and implemented by `geometry_tracks.contains_key(&scene_node_id)`.

- [ ] **Step 1: Write the failing test**

Add a focused test beside the existing transaction tests:

```rust
#[test]
fn property_specific_track_queries_do_not_cross_properties() {
    let mut engine = PresentationEngine::enabled();
    let scene_node_id = node(120);
    let curve = AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear);

    engine
        .commit(PresentationTransactionRequest::geometry(
            AnimationTime::from_nanos(0),
            vec![PresentationGeometryMutation::new(
                scene_node_id,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(1.0, 0.0, 10.0, 10.0),
                curve,
            )],
        ))
        .expect("geometry transaction");
    assert!(engine.has_geometry_track(scene_node_id));
    assert!(!engine.has_opacity_track(scene_node_id));

    engine.cancel_geometry(scene_node_id);
    engine
        .commit(PresentationTransactionRequest::opacity(
            AnimationTime::from_nanos(0),
            vec![PresentationOpacityMutation::new(
                scene_node_id,
                PresentationOpacity::OPAQUE,
                PresentationOpacity::new(0.5).expect("opacity"),
                curve,
            )],
        ))
        .expect("opacity transaction");
    assert!(!engine.has_geometry_track(scene_node_id));
    assert!(engine.has_opacity_track(scene_node_id));
}
```

- [ ] **Step 2: Run the test to verify it fails for the missing API**

Run: `rtk cargo test --locked --all-targets property_specific_track_queries_do_not_cross_properties`

Expected: compile failure because `PresentationEngine::has_geometry_track` does not exist yet. Do not change production code before observing this failure.

- [ ] **Step 3: Write the minimal implementation**

Add this method immediately before `has_opacity_track` in `engine.rs`:

```rust
pub fn has_geometry_track(&self, scene_node_id: SceneNodeId) -> bool {
    self.geometry_tracks.contains_key(&scene_node_id)
}
```

- [ ] **Step 4: Run the focused engine tests**

Run: `rtk cargo test --locked --all-targets property_specific_track_queries_do_not_cross_properties`

Expected: the new test passes with exit code 0.

- [ ] **Step 5: Commit**

```bash
git add src/presentation_animation/engine.rs src/presentation_animation/transactions_tests.rs
git commit -m "feat(presentation): expose geometry track presence"
```

### Task 2: Add failing candidate-scoped Direct Scanout regressions

**Files:**
- Modify: `src/compositor/state/desktop_window_tests.rs`

**Interfaces:**
- Consumes: Existing `install_x11_scanout_surface`, `x11_shm_surface`, WindowGroup SceneNode lookup, and Direct Scanout analysis helpers.
- Produces: Isolated regressions proving candidate and unrelated Geometry/Opacity tracks, no-candidate behavior, and XWayland backing replacement expectations.

- [ ] **Step 1: Split the accumulated Opacity regression**

Replace `visible_opacity_track_blocks_scanout_but_hidden_unrelated_track_does_not` with two independent tests. The candidate test installs the fullscreen XWayland candidate, commits an Opacity mutation on its WindowGroup SceneNodeId, and asserts `PresentationOpacity`. The unrelated test installs the same candidate without a candidate track, adds a 16×16 XDG window at `SurfacePlacement::absolute_root_at(2_000, 0)`, commits only the off-output window's Opacity track, and asserts:

```rust
assert!(state.direct_scanout_scene_candidate().is_ok());
assert!(!state
    .direct_scanout_scene_blockers()
    .reasons()
    .contains(&DirectScanoutSceneRejection::PresentationOpacity));
```

- [ ] **Step 2: Add the Geometry candidate and unrelated-track tests**

Use the same candidate/off-output setup and commit:

```rust
PresentationTransactionRequest::geometry(
    AnimationTime::from_nanos(0),
    vec![PresentationGeometryMutation::new(
        scene_node_id,
        PresentationRect::new(0.0, 0.0, 100.0, 100.0).expect("geometry start"),
        PresentationRect::new(1.0, 0.0, 100.0, 100.0).expect("geometry target"),
        AnimationCurve::easing(Duration::from_millis(10), EasingCurve::Linear),
    )],
)
```

The candidate case asserts `AnimationTransform`; the unrelated off-output case asserts candidate eligibility and absence of `AnimationTransform`.

- [ ] **Step 3: Add the no-covering-candidate regression**

Create only an off-output WindowGroup with an active Geometry or Opacity track and no output-covering group. Assert `NoOutputCoveringApplication` is present and the unrelated `AnimationTransform`/`PresentationOpacity` reason is absent. Keep this as a separate test so a missing candidate cannot inherit an ActiveScene-wide animation reason.

- [ ] **Step 4: Add the XWayland root-replacement candidate regression**

Create a fullscreen XWayland root A, retain its X11 handle and WindowGroup SceneNodeId, install an active Opacity and Geometry track on that SceneNodeId, retire/attach root B through the existing XWayland replacement path, append a valid fullscreen root-B scanout surface, rebuild the active scene, and assert both `PresentationOpacity` and `AnimationTransform` remain in the Direct Scanout blockers. Do not query the engine by root ID in the test; assert the stable SceneNode owner is what preserves both tracks.

- [ ] **Step 5: Run the new regressions to verify the unrelated cases fail**

Run: `rtk cargo test --locked --all-targets candidate_presentation_track`

Expected: candidate tests may pass under the old global flow, but the unrelated off-output and no-candidate tests fail because the old ActiveScene helpers manufacture blockers. The XWayland test must also expose any setup issue before production changes.

### Task 3: Scope Direct Scanout active-track checks to the covering WindowGroup

**Files:**
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/state/active_scene.rs`

**Interfaces:**
- Consumes: `PresentationCoverageAnalysis.covering_application_group`, `presentation_scene_node_id_for_root`, `PresentationEngine::has_geometry_track`, and `has_opacity_track`.
- Produces: Candidate-specific temporary blocker decisions with existing rejection enums and unchanged blocker ordering for checks that remain valid before/after candidate resolution.

- [ ] **Step 1: Remove the old global property-specific checks**

Delete the pre-candidate calls to `presentation_animation_has_pending_visible_geometry()` and `presentation_animation_has_pending_visible_opacity()` from `direct_scanout_scene_analysis`, leaving lifecycle pending-visible handling in its current position.

- [ ] **Step 2: Add candidate-scoped property checks after covering-group resolution**

Immediately after:

```rust
let root_surface_id = covering_group.root_surface_id;
```

add the repository-style equivalent of:

```rust
if let Some(scene_node_id) = self.presentation_scene_node_id_for_root(root_surface_id) {
    if self.presentation_animator.has_geometry_track(scene_node_id) {
        blockers.push(DirectScanoutSceneRejection::AnimationTransform);
    }
    if self.presentation_animator.has_opacity_track(scene_node_id) {
        blockers.push(DirectScanoutSceneRejection::PresentationOpacity);
    }
}
```

Do not use `has_track`, add root-keyed storage, or move canonical/physical checks. The existing `covering_application_group == None` early return must now occur without a temporary candidate track blocker.

- [ ] **Step 3: Retire the misleading ActiveScene helpers after caller search**

Run:

```bash
rtk rg -n "presentation_animation_has_pending_visible_(geometry|opacity)" src
```

If only definitions remain, remove both methods from `active_scene.rs`. Leave `presentation_animation_has_pending_visible()` and its scheduler callers intact. If a legitimate diagnostic/test caller remains, retain it and document why in the code review rather than reusing it for Direct Scanout.

- [ ] **Step 4: Run the Direct Scanout regressions to verify green**

Run:

```bash
rtk cargo test --locked --all-targets candidate_presentation_track
rtk cargo test --locked --all-targets canonical_opacity_blocks_an_opaque_direct_scanout_candidate
rtk cargo test --locked --all-targets direct_scanout_opacity_recovers_only_after_opaque_frame_is_physically_presented
```

Expected: candidate Geometry/Opacity tracks block, unrelated off-output tracks do not, no-candidate analysis reports no temporary track blocker, canonical opacity still blocks, and physical Opacity recovery still waits for physical publication.

- [ ] **Step 5: Commit**

```bash
git add src/compositor/state/direct_scanout.rs src/compositor/state/active_scene.rs src/compositor/state/desktop_window_tests.rs
git commit -m "fix(scanout): scope geometry and opacity tracks to candidate"
```

### Task 4: Prove immutable NativeSceneHistory opacity evidence

**Files:**
- Modify: `src/native_output/runtime/frame_scene_identity_tests.rs`

**Interfaces:**
- Consumes: `NativeSceneHistory::{replace_ready, queue_submission, promote_pageflip}` and immutable `PresentationFrameSnapshot` opacity fields.
- Produces: A focused test proving old submitted frame A retains root A/O1/T/R after frame B root replacement is queued and only changes to root B/O2/T/R when token B is promoted.

- [ ] **Step 1: Write the failing history regression**

Add a helper that starts from the existing `frame_snapshot` and replaces its `presentation` with a `PresentationFrameSnapshot` containing:

```rust
PresentationGroupOpacity::with_scene_node(
    scene_node_id,
    root_surface_id,
    opacity,
    Some(PresentationOpacityTransitionEvidence {
        transaction_id: transaction,
        revision_id: revision,
        mathematically_settled: false,
    }),
)
```

Add a test that uses one stable `scene_node_id`, exact nonzero `PresentationTransactionId` T and `PresentationRevisionId` R, frame A `(root A, O1)`, frame B `(root B, O2)`, and an initial frame ID 0. Queue A with token A, queue B with token B, promote A, and assert the presented snapshot has exactly root A/O1/T/R. Promote B and assert exactly root B/O2/T/R. This test should pass against current immutable history behavior; its role is to lock the regression so any future pageflip reconstruction breaks the explicit assertions.

- [ ] **Step 2: Run the focused history test**

Run: `rtk cargo test --locked --all-targets submitted_opacity_history_preserves_old_backing_evidence`

Expected: PASS with both exact snapshot assertions and no production history changes.

- [ ] **Step 3: Commit**

```bash
git add src/native_output/runtime/frame_scene_identity_tests.rs
git commit -m "test(presentation): preserve submitted opacity evidence across backing replacement"
```

### Task 5: Preserve static and physical Geometry authorities and verify full scope

**Files:**
- Modify: `src/compositor/state/desktop_window_tests.rs` only if an existing assertion needs isolation or a focused Geometry physical regression is missing.
- Modify: `docs/superpowers/specs/2026-09-19-direct-scanout-candidate-scoping-design.md` only for factual corrections discovered during verification.

**Interfaces:**
- Consumes: Existing Direct Scanout candidate analysis and presentation publication APIs.
- Produces: Fresh evidence that static canonical opacity, physically presented Opacity, and physically presented nonidentity Geometry remain candidate-scoped; scheduler behavior and source-layout boundaries remain unchanged.

- [ ] **Step 1: Run focused physical Geometry and scheduler tests**

Run:

```bash
rtk cargo test --locked --all-targets AnimationTransform
rtk cargo test --locked --all-targets presented_presentation
rtk cargo test --locked --all-targets pending_visible
```

Use exact existing test names if a filter yields zero tests. Do not change scheduler helpers or lifecycle checks.

- [ ] **Step 2: Inspect the final diff for forbidden scope expansion**

Run:

```bash
rtk git diff HEAD~3 -- src/presentation_animation src/compositor/state/direct_scanout.rs src/compositor/state/active_scene.rs src/native_output/runtime/scene_history.rs src/native_output/runtime/frame_scene_identity_tests.rs
```

Confirm no renderer/effect, transaction semantics, pageflip promotion, Clip, Lifecycle/Lamp, or root-keyed track changes appear.

- [ ] **Step 3: Run formatting and full verification**

Run each command fresh in this checkout:

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

Record exact exit codes and distinguish any pre-existing `egl_renderer` or source-layout failures from task failures. Do not claim success from stale or partial output.

- [ ] **Step 4: Commit any narrowly required cleanup**

If verification requires a scoped cleanup, stage only its exact files and use a message matching the change, such as:

```bash
git commit -m "refactor(scene): remove obsolete global presentation blocker helpers"
```

Otherwise leave the implementation commits as-is.

- [ ] **Step 5: Final status and handoff**

Run:

```bash
rtk git rev-parse HEAD
rtk git status --short
rtk git log --oneline --decorate -8
```

Report starting/ending HEAD, preserved dirty overlap decision, graph status/generation, old/new blocker flow, all focused results, full Cargo results, and source-layout result. State explicitly that Clip, Lifecycle/Lamp migration, and broader multi-output behavior are not implemented.
