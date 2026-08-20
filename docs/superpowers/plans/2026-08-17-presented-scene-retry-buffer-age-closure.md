# Typhon Presented-Scene Retry and Buffer-Age Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the residual native repaint correctness gap where a rejected frame is retried at the same logical generation, while preserving presented-scene ownership, partial repaint, buffer age, render-ahead, and direct-scanout behavior.

**Architecture:** Keep `NativeSceneHistory` as the owner of ready, submitted, and pageflip-confirmed snapshots. Derive scene damage from the visual transition between the presented snapshot (or the existing buffer-age predecessor) and the exact current frame snapshot. Keep `last_rendered_scene_generation` available only for logical scheduling, diagnostics, and coalescing decisions.

**Tech Stack:** Rust, Typhon CPU/GLES native output paths, GBM/EGL/KMS output ownership, deterministic scene/framebuffer fixtures, Cargo, and RTK.

## Global Constraints

- Preserve unrelated tracked and untracked dirty worktree content.
- Use RTK for every shell command and `apply_patch` for source edits.
- Follow TDD: add the focused regression before changing production behavior and observe it fail.
- Compare visual snapshots, not only logical generation counters; do not create a second scene model.
- Do not turn retries into full-output repaint, disable buffer age, disable render-ahead/triple buffering, disable KMS workers, or disable Direct Scanout.
- Keep logical scene bounds un-clipped until the final output-damage conversion.
- Treat pageflip confirmation as the only asynchronous presentation promotion authority.
- Report native behavior only when a real Typhon DRM/KMS session is available.

### Task 1: Reproduce the same-generation retry defect

**Files:**
- Modify: `src/native_output/tests/output.rs`
- Read: `src/native_output/runtime/scene_history.rs`, `src/native_output/runtime/presentation.rs`, `src/native_output/runtime/presentation_worker.rs`

- [ ] Record the current HEAD, dirty-tree classification, and relevant focused-test baseline.
- [ ] Add the smallest deterministic model for `present A → render B → reject B → retry B`.
- [ ] Make the test fail because the retry is incorrectly treated as having no scene damage when it is relative to presented A.
- [ ] Assert that rejected B does not alter the presented snapshot.

### Task 2: Make visual snapshot transition the damage authority

**Files:**
- Modify: `src/native_output/output/damage.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation.rs` only where damage inputs are wired
- Modify: `src/native_output/tests/output.rs` call sites and transition tests

- [ ] Extend the existing compact scene snapshot with the content/commit identity required to distinguish client-content changes from unchanged geometry.
- [ ] Remove the logical `scene_changed` boolean from the native damage decision.
- [ ] Compute old/new surface bounds, current damage, content transitions, and decoration transitions from the presented/current snapshots, then clip at the output boundary.
- [ ] Keep `scene_changed` only where it controls logical scheduling or diagnostics, if still needed.
- [ ] Re-run the red test and verify it is green without a full-output fallback.

### Task 3: Add pixel-level retry regressions

**Files:**
- Modify: `src/native_output/tests/output.rs`

- [ ] Compare partial retry output to a clean full-reference render for oversized SSD A=2200 and B=1400.
- [ ] Repeat for CSD geometry-only changes without decorations.
- [ ] Add unchanged-geometry client/content-generation changes.
- [ ] Add decoration-only changes, preview-resize geometry, and left-edge preview geometry.
- [ ] Sample stale traffic-light positions and titlebar edges, not only damage rectangles.
- [ ] Cover the required width matrix (2200, 2050, 1900, 1700, 1400, 1000, 800), shrinking and expanding, with inside/right/left/both/offscreen placement where the fixture supports it.

### Task 4: Prove buffer-age 1/2/3 behavior

**Files:**
- Read and modify the existing repaint/buffer-age test module discovered under `src/egl_renderer/` or `src/native_output/`
- Modify: `src/native_output/tests/output.rs`

- [ ] Add explicit age-1 rejection/retry coverage with a buffer containing presented A.
- [ ] Add an age-2 history with valid intervening presentation and reuse the planner’s slot/history semantics.
- [ ] Add an age-3 history with an older valid buffer predecessor.
- [ ] For every age, compare partial output pixel-for-pixel to the full-reference current scene using the oversized/offscreen fixture.
- [ ] Verify no test or production path disables buffer age or replaces the bounded repair with full repaint.

### Task 5: Audit frame identity and rejection ownership

**Files:**
- Read/modify: `src/native_output/runtime/scene_history.rs`
- Read/modify: `src/native_output/runtime/presentation.rs`
- Read/modify: `src/native_output/runtime/presentation_worker.rs`
- Read/modify: `src/native_output/runtime/cycle/pageflip.rs`
- Read/modify: `src/native_output/runtime/kms_worker/rejection.rs`
- Read/modify: `src/native_output/runtime/session_io.rs`

- [ ] Prove the snapshot is captured from the exact state used to render its frame, before unrelated logical state can advance it.
- [ ] Prove a pageflip for B promotes B’s snapshot rather than mutable current state C.
- [ ] Prove a rejected/discarded B leaves presented A unchanged and retires B’s token.
- [ ] Cover multiple rejected frames B/C followed by presented D.
- [ ] Preserve the synchronous backend exception only where its presentation semantics are genuinely immediate.

### Task 6: Verify renderer and client-mode parity

**Files:**
- Read/modify relevant CPU/GLES native output tests
- Read/modify relevant XWayland and fullscreen/direct-scanout tests if a missing regression is exposed

- [ ] Verify CPU and GLES use the same presented/current snapshot transition semantics.
- [ ] Run managed XWayland + SSD retry coverage where the existing fixture permits it.
- [ ] Re-run CSD, fullscreen, and Direct Scanout eligibility tests without changing their recently closed rules.
- [ ] Keep diagnostics bounded and disabled by default; include retry/reject/presented identity only if existing hooks support it without architectural expansion.

### Task 7: Verify, qualify, and report

**Files:**
- Create: `docs/superpowers/specs/REPORT-2026-08-17-presented-scene-retry-buffer-age-closure.md`

- [ ] Run focused scene-history, native damage, buffer-age, decoration, CPU/GLES, CSD, XWayland, fullscreen, and direct-scanout suites.
- [ ] Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, `bash bin/check-source-layout`, and `git diff --check` through RTK.
- [ ] Separate new failures from baseline/environment failures and record exact evidence.
- [ ] Attempt native qualification only if a real Typhon DRM/KMS session is available; otherwise record the blocker without claiming native closure.
- [ ] Answer every final self-review item with source or test evidence and record final `git status --short`.
