# F13 Presentation Feedback Frame Samples Implementation Plan

**Goal:** Ensure `wp_presentation_feedback.presented` is emitted only for the exact client Content Updates sampled by the physically presented frame.

**Architecture:** Preserve the existing `(surface_id, surface_presentation_generation, commit_sequence)` feedback lineage and add a separate immutable `SurfacePresentationCommitKey` for physical sample membership. Native composited frames derive keys from `ResolvedNativeFrameScene` after fullscreen culling, then add the frozen software or hardware client cursor key when delivery is real; direct scanout derives the exact candidate key. Frame-batch capture partitions pending feedback by keyed membership, while frame-eligibility scheduling uses a fullscreen-aware/cursor-aware policy snapshot and remains separate from physical completion.

**Tech Stack:** Rust 2024, Wayland protocol resources, native KMS/GL frame paths, existing compositor and native-output test harnesses, `rtk` command proxy.

## Global Constraints

- Preserve `PendingPresentationFeedback` lineage fields and `surface_presentation_generation`.
- Keep `SurfaceDamagePresentation` as damage identity only; use it only as a membership input where useful.
- Keep callbacks, FIFO, Commit Timing, buffer release, direct-scanout policy, and NoVisualChange semantics independent of physical presentation membership.
- Use keyed linear partitioning, not per-feedback scene reconstruction.
- Keep all compilation artifacts in the checkout's existing `target` directory.
- Preserve unrelated dirty worktree changes and make one focused commit for this task.

### Task 1: Capture the current RED behavior

**Files:**
- Test: `src/compositor/state/frame_tests.rs` or the existing realistic native-output presentation test module selected after fixture inspection.

- [ ] **Step 1: Add one failing regression for the current bug.** Build a background/fullscreen pending-feedback setup whose batch capture uses current active-scene visibility, then assert that a fullscreen frame currently owns the culled background feedback and completes it as presented. Include a same-buffer/new-commit assertion only if the chosen fixture already exposes commit identity without additional setup.
- [ ] **Step 2: Run the narrow test with `rtk cargo test ... -- --nocapture` and record the expected RED failure or current incorrect assertion.** Do not change production code before this result.

### Task 2: Add the exact physical sample identity and keyed batch capture

**Files:**
- Modify: `src/compositor/mod.rs` or the smallest existing compositor identity module for `SurfacePresentationCommitKey`.
- Modify: `src/compositor/state/surfaces.rs`, `src/compositor/server.rs`, `src/compositor/server_frames.rs`.
- Modify: `src/compositor/state/frame_callbacks.rs`, `src/compositor/state/frames.rs`, `src/compositor/frame_batch.rs`.
- Test: compositor frame tests covering matching, unmatched-current retention, supersession, generation reuse, retry, late pageflip, buffer reuse, and NoVisualChange.

- [ ] **Step 1: Write unit tests for key equality and keyed feedback partitioning.** Use distinct commit sequences with the same buffer identity and distinct generations with the same numeric surface ID; assert only exact triples are captured, unmatched current feedback stays in the pending/deferred pool, and superseded feedback is discarded by existing lifecycle rules.
- [ ] **Step 2: Run those tests and confirm RED.** The current implementation must either capture all active-scene feedback or lack the new API; record the actual failure.
- [ ] **Step 3: Implement `SurfacePresentationCommitKey` and a narrow helper that resolves a renderable exact commit to the current presentation generation.** Do not infer from `wl_buffer`; reject missing/stale generation or non-active exact commits.
- [ ] **Step 4: Add `take_frame_batch_for_render_with_presentation_samples(frame_id, samples)` or an equivalent bound-at-capture API.** Build a `HashSet<SurfacePresentationCommitKey>`, linearly partition pending feedback into owned exact matches and still-current deferred feedback, and keep callbacks/FIFO/Commit Timing/release ownership unchanged.
- [ ] **Step 5: Update restore/requeue and terminal paths.** A failed attempt requeues owned feedback once; NoVisualChange and safe abandonment settle only feedback owned by that batch; current unmatched feedback remains pending; direct completion reuses the same exact-key comparison semantics.
- [ ] **Step 6: Run the focused frame-batch tests and confirm GREEN.** Check ownership counts and exactly-once terminal outcomes.

### Task 3: Wire concrete native frame membership

**Files:**
- Modify: `src/native_output/runtime/frame.rs` if a reusable sample-derivation helper belongs beside `ResolvedNativeFrameScene`.
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`.
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`.
- Modify: `src/native_output/runtime/presentation_cycle.rs`.
- Modify: `src/native_output/scanout/output_swapchain.rs` and compatibility helpers only where they can emit physical presentation.
- Test: native output frame/pageflip/direct/cursor tests.

- [ ] **Step 1: Add RED tests for post-culling ordinary membership and cursor delivery.** Assert a culled background is absent, an allowed fullscreen overlay is present, a child/popup uses its own exact `commit_sequence`, software cursor includes the client cursor commit, hardware cursor includes the frozen submitted source, and Hidden excludes it.
- [ ] **Step 2: Run the native-focused tests and record RED.** At least one test must exercise realistic frame submission/completion infrastructure, not only a pure helper.
- [ ] **Step 3: In composited native capture, derive ordinary keys from the final `ResolvedNativeFrameScene.surfaces` and add only the exact cursor key matching `PresentedCursorDelivery::Software` or `Hardware`.** Freeze hardware cursor source from the submitted frame owner; never query current cursor state at pageflip.
- [ ] **Step 4: In direct scanout, bind the candidate's exact `(generation, commit_sequence)` and the physically associated cursor-plane source key.** Preserve current direct eligibility and only change feedback ownership.
- [ ] **Step 5: In compatibility/headless paths, pass the actual render-list keys or an explicitly empty sample set for NoVisualChange.** Do not let the legacy active-scene set stand in for physical membership.
- [ ] **Step 6: Run native/direct/cursor tests and confirm GREEN.** Verify late pageflip after a newer commit settles only the frozen submitted key.

### Task 4: Separate scheduling eligibility from physical membership

**Files:**
- Modify: `src/compositor/state/frame_callbacks.rs`, `src/compositor/state/frames.rs`, and the existing fullscreen/cursor policy module with the smallest shared snapshot hook.
- Modify: state-transition callers only where existing eligibility refresh is insufficient.
- Test: fullscreen, cursor visibility, workspace/minimize/destroy, and FIFO/Commit Timing interaction tests.

- [ ] **Step 1: Add RED tests for culled feedback driving endless frame work and hidden/theme cursor feedback being treated as eligible.** Also assert callbacks and FIFO/Commit Timing behavior remain unchanged.
- [ ] **Step 2: Implement a presentation-specific frame-eligibility predicate using existing fullscreen composition-plan state and `client_cursor_render_state()` semantics.** A culled ordinary root and hidden/replaced client cursor move to deferred work; exact sample membership remains independent.
- [ ] **Step 3: Refresh eligibility on fullscreen enter/exit/replacement, overlay changes, cursor delivery changes, workspace switch, minimize/unminimize, and destroy through the narrowest existing shared refresh points.** Avoid per-feedback scene reconstruction and global frame rebuilds.
- [ ] **Step 4: Run the scheduling and protocol-interaction tests and confirm GREEN.** Confirm a current culled feedback survives without continuously requesting frames, then presents after it is actually sampled.

### Task 5: Full audit and verification

**Files:**
- Inspect all production routes in `src/compositor`, `src/native_output/runtime`, `src/native_output/scanout`, and pageflip/worker completion modules.

- [ ] **Step 1: Search every production `presented()`/`discarded()` route and every frame-batch caller.** Classify each route as exact sample + physical completion, discard lifecycle, retry, or safe abandonment.
- [ ] **Step 2: Run the focused F13 matrix (fullscreen, supersession, overlay, subsurface, cursors, direct scanout, buffer reuse, late pageflip, retry, NoVisualChange, generation reuse, FIFO/Commit Timing).** Record counts and outcomes.
- [ ] **Step 3: Run the source-layout checker observationally and preserve unrelated historical violations.
- [ ] **Step 4: Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `git diff --check`.
- [ ] **Step 5: Review the diff against the pre-existing dirty worktree, stage only F13 files plus this plan, and create `fix(compositor): bind feedback to presented content`.
