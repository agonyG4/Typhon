# Typhon Post-Closure Correctness and Native Qualification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Verify and close the remaining WindowVisual lifecycle, scroll protocol, source-layout, fullscreen, damage, and native-qualification gaps without rewriting the existing compositor architecture.

**Architecture:** Preserve the shared `WindowVisualGroup` ordering used by CPU and GLES. Make orphan handling explicit at scene-build boundaries, keep scroll semantics version-aware at the Wayland dispatch boundary, and use deterministic framebuffer/state tests for claims that cannot be qualified in the current native environment. Separate static evidence, code ownership, and native evidence in the final English report.

**Tech Stack:** Rust, Smithay/Wayland protocol tests, CPU/GLES scene renderers, libinput normalization, Cargo, RTK, bounded compositor diagnostics, and repository source-layout checks.

## Global Constraints

- Preserve all unrelated dirty files and untracked files.
- Do not use reset, restore, checkout, clean, stash, or history rewriting.
- Use RTK for shell commands and reuse Cargo's existing build cache.
- Follow TDD: write each new regression test, run it red, implement the smallest fix, then run it green.
- Do not add application-specific workarounds or globally disable partial repaint or Direct Scanout.
- Do not claim Firefox, Kitty, rollback, or other native behavior fixed without reproduction evidence.
- Stage only corrective-closure-owned hunks where practical.

### Task 1: Establish current evidence and ownership boundaries

**Files:**
- Read: recent closure commits `c05c406`, `84a4e62`, `d9326a7`, `13baca1`, `506af97`
- Read: `src/compositor/render.rs`, `src/egl_renderer.rs`, `src/compositor/decoration/`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/window_interaction.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/state/xdg_lifecycle.rs`, `src/compositor/input.rs`, `src/compositor/state/input_dispatch.rs`, `src/native_output/input/`, `src/native_output/output/damage.rs`, `src/native_output/runtime/`, `src/xwayland/`, `src/wayland/registry_state.rs`
- Create: `docs/superpowers/specs/REPORT-2026-08-17-windowvisual-post-closure-qualification.md`

- [ ] Record HEAD, recent log, status, diff stat, and changed-file list before source edits.
- [ ] Use the codebase graph for structural discovery, read exact source for all operated symbols, and check graph coverage for every relied-on path.
- [ ] Classify recent closure files as closure-new, pre-existing dirty, or mixed ownership; record that classification in the report.
- [ ] Run the source-layout checker and focused baseline tests before changing production code; record failures as baseline or closure-induced.

### Task 2: Make WindowVisual ordering lifecycle-safe

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/compositor/server.rs` and/or `src/compositor/state/window_decoration.rs` only if the owning-root evidence requires it
- Test: `src/compositor/render.rs` and relevant compositor state/render tests

- [ ] Write failing tests for orphan decorations, stale generation/unmap, stale XWayland ownership, and no orphan appearing above an unrelated window.
- [ ] Write failing CPU/GLES-compatible ordering tests for two and three overlapping SSD windows, SSD/CSD permutations, XDG/XWayland roots, backing ownership, rear activation/raise, popup, and subsurface/titlebar overlap.
- [ ] Confirm `WindowVisualGroup` order derives from authoritative `window_stacking`, not incidental surface or decoration iteration.
- [ ] Implement explicit orphan discard plus bounded diagnostic; never append an invalid decoration as a top-level visual.
- [ ] Make popup/subsurface semantics explicit and identical in CPU and GLES.
- [ ] Run focused scene, decoration, popup, subsurface, XDG, XWayland, and GLES tests green.

### Task 3: Close high-resolution scroll semantics and source layout

**Files:**
- Modify: `src/compositor/input.rs`, `src/compositor/state/input_dispatch.rs`, `src/native_output/input/routing.rs`
- Modify: `src/compositor/tests/support/registry_state.rs`, `src/compositor/tests/support/input_client.rs`, and scroll protocol tests
- Split if needed: `src/wayland/registry_state.rs` into focused existing/new modules without changing the source-layout limit
- Test: focused pointer/scroll protocol suites

- [ ] Write failing tests proving zero v120 emits no `axis_value120`, non-zero v120 dispatches to modern clients, legacy clients receive correct accumulated detents, continuous scrolling stays continuous, horizontal matches vertical, and source/stop/frame/timestamp grouping remains valid.
- [ ] Reproduce the `scroll_v120_i32(0.0) -> Some(0)` path before fixing it.
- [ ] Fix the semantic boundary so zero values cannot become protocol events; preserve version-specific modern/legacy behavior without duplicate representations.
- [ ] Run `bash bin/check-source-layout` through RTK, split only closure-introduced oversized responsibilities, and rerun the checker.
- [ ] Verify sensitivity behavior for factors `0.5`, `1.0`, `1.5`, and `2.0` only if an existing normalization boundary supports it; do not add a magic Hyprland multiplier.

### Task 4: Revalidate fullscreen, modifiers, and complete damage

**Files:**
- Modify only if a failing test proves a gap: `src/compositor/state/window_interaction.rs`, `src/compositor/state/fullscreen.rs`, `src/compositor/state/xdg_lifecycle.rs`, `src/native_output/input/state.rs`, `src/native_output/output/damage.rs`, `src/native_output/runtime/`
- Test: `src/compositor/state/window_interaction_tests.rs`, native input tests, damage/output tests, framebuffer regression tests

- [ ] Add/retain stress tests for fullscreen move/resize rejection and 100 floating↔fullscreen cycles with exact geometry restoration.
- [ ] Add/retain 100 maximize↔restore cycles with no titlebar-height or one-pixel drift.
- [ ] Test left/right Alt, repeated Alt-Tab, focus changes, client destruction, inhibition, Ctrl, Shift, Super, and session reset for exactly-once modifier release ownership.
- [ ] Test old/current complete visual bounds for move, resize, minimize, destroy, fullscreen transitions, CSD↔SSD, theme/title/hover/pressed/focus changes, XWayland backing, and client content.
- [ ] Prove 30+ move and resize positions produce pixel-identical partial and clean full-reference framebuffers under reusable-buffer conditions; do not solve failures with full-output repaint.

### Task 5: Native and trace qualification

**Files:**
- Read/modify only if evidence demands it: `src/xwayland/`, `src/native_output/runtime/`, diagnostics/test support
- Update: `docs/superpowers/specs/REPORT-2026-08-17-windowvisual-post-closure-qualification.md`

- [ ] Detect available Astrea/Typhon, Wayland, DRM, Firefox/Zen, Kitty, GTK, Qt, XWayland, and Hyprland tools without claiming unavailable runs.
- [ ] If a native session is available, trace Firefox tear-off, Kitty drag selection, stationary-pointer rollback, scroll comparison, Direct Scanout, MacTahoe geometry, and the full acceptance matrix.
- [ ] If unavailable, record exact blockers and leave those behaviors unclaimed; do not add app-specific workarounds.
- [ ] Correlate bounded frame history with logical/render generations, visual signatures, damage, buffer age, swapchain slots, framebuffer IDs, transactions, and pageflip/presentation tokens before any rollback conclusion.

### Task 6: Final verification, audit, and corrective commits

**Files:**
- Update: `docs/superpowers/specs/REPORT-2026-08-17-windowvisual-post-closure-qualification.md`
- Stage/commit: only corrective-closure-owned files and hunks

- [ ] Run focused suites continuously, then `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo test --locked`, `cargo clippy --locked --all-targets -- -D warnings`, source-layout, diff-check, and final status through RTK.
- [ ] Compare any global Clippy/source-layout failure against the baseline and distinguish pre-existing from newly introduced failures.
- [ ] Manually answer the final WindowVisual, orphan, popup/subsurface, zero-v120, source-layout, fullscreen, modifier, damage, native evidence, MacTahoe, Direct Scanout, and dirty-tree questions in the report.
- [ ] Create corrective commits at actual dependency boundaries and record their hashes plus final status.

