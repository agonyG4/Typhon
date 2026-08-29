# Typhon NoVisualChange Lineage and Decoration-Aware Order Damage Correction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore logical NoVisualChange damage-baseline settlement and include SSD bounds in existing regional visual-order repair while strengthening the integrated topology/buffer-age oracle.

**Architecture:** Reintroduce one `SurfaceDamageSettlement::{Presented, NoVisualChange}` disposition over the existing exact `SurfaceDamagePresentation` token. Both dispositions update logical journal accounting, while only the existing physical pageflip path updates output presentation history. Extend `NativeSceneSurfaceSnapshot` with the visual-root identity produced by `visual_stack_groups`, and have changed-span order damage include old/current bounds for all affected group members and matching decorations.

**Tech Stack:** Rust, existing compositor frame batches and surface journals, native scene snapshots, `PartialRepaintPlanner`, deterministic unit tests, `rtk` verification commands.

## Global Constraints

- Preserve the current checkout and unrelated pointer-lock changes; do not reset, clean, stash, discard, or rewrite them.
- Do not change O1 callback admission policy, SHM release timing, DMA-BUF release authority, KMS scheduling, READY admission, Direct Scanout policy, resize, pointer-lock, tracing, VRR, tearing, color, or scene-graph ownership.
- NoVisualChange may settle only a production terminal whose output-damage authority has proven Empty; it must not advance physical output history.
- Rejected or abandoned non-empty rendered frames must retain their exact token and damage until a later physical presentation.
- Use Typhon's existing `visual_stack_groups`/`WindowVisualGroup` authority for visual-root ownership; do not infer ownership from numeric IDs or create a parallel stack.
- Run every required command through `rtk` and keep tests deterministic without wall-clock sleeps.

---

### Task 1: Add RED tests for logical settlement and SSD order coverage

**Files:**
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `src/native_output/tests/output.rs`
- Modify: `src/native_output/runtime/presentation_cycle_tests.rs` only if the compatibility invariant needs a focused test seam

**Interfaces:**
- Consumes the existing `SurfaceDamagePresentation`, frame-batch terminal methods, `NativeSceneSnapshot`, and regional damage function.
- Produces failing tests that require logical baseline advancement, live NoVisualChange metrics, physical-state non-advancement, rejected non-empty retention, SSD-aware reorder damage, and unchanged-order no-op behavior.

- [ ] **Step 1: Change the existing frame-batch token-drop tests into RED assertions.** Require a batch-owned token and a batchless token to advance `presented_surface_commits`, increment `surface_damage_settlement_no_visual_change`, and leave `surface_damage_settlement_presented` unchanged.
- [ ] **Step 2: Change the 128-entry journal test into the required bounded-lineage oracle.** After each Empty settlement assert the baseline equals that commit; after commit 128 assert `DamageSince(baseline)` is Empty; after commit 129 Partial(7,9,3,5) assert `DamageSince(baseline)` is Known with that partial damage.
- [ ] **Step 3: Add a RED physical-authority test using the existing integrated oracle or production state boundaries.** Capture serial, presented state, planner history, and pageflip/presentation counters before repeated logical settlement and assert they remain unchanged afterward.
- [ ] **Step 4: Add a RED rejected-non-empty test.** Capture a Partial token, render a candidate, reject/abandon it without physical presentation, assert the baseline and damage remain unsettled, then present a retry and assert the Partial is repaired.
- [ ] **Step 5: Add decorated-window order tests to `output.rs`.** Use two overlapping client snapshots and decorations extending outside client bounds. Assert reordered SSD-only pixels are damaged regionally, unchanged order/decoration state adds no order damage, and unrelated output pixels remain clean.
- [ ] **Step 6: Run the RED tests unchanged.** Use focused `rtk cargo test` filters for frame tests and native output damage tests. Record expected failures: logical baseline remains absent/history becomes lost, SSD-only reorder pixels are outside damage, and the rejected token is not settled by the retry proof.
- [ ] **Step 7: Commit only the RED tests and plan if the repository’s established test-commit convention permits it.** Keep production files unchanged in this commit.

### Task 2: Restore one logical NoVisualChange settlement authority

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/server.rs`

**Interfaces:**
- Produces `SurfaceDamageSettlement::{Presented, NoVisualChange}` and one keyed settlement implementation used by both terminals.
- `commit_surface_damage_presented(token)` remains the only physical-pageflip caller; `commit_surface_damage_no_visual_change(token)` is logical-only and updates the same journal baseline.

- [ ] **Step 1: Add the explicit settlement enum and a server wrapper.** Keep `SurfaceDamagePresentation` as the ownership token and leave existing method visibility patterns intact.
- [ ] **Step 2: Parameterize the existing `settle_surface_damage` helper by the disposition.** Increment the Presented or NoVisualChange metric accordingly; preserve generation validation, monotonic commit protection, keyed journal/index lookup, and `HistoryLost` evidence.
- [ ] **Step 3: Settle a batch-owned token as NoVisualChange after callback/release cleanup.** Do not add any native scene history, serial, planner, pageflip, feedback, or KMS mutation to this compositor helper.
- [ ] **Step 4: Settle a batchless token as NoVisualChange in `settle_no_visual_change_work`.** Keep the no-work path batchless and keep callback/SHM/DMA-BUF ownership behavior unchanged.
- [ ] **Step 5: Run the focused frame/journal tests.** Verify the 128-entry baseline remains current and the NoVisualChange metric equals the number of logically settled token entries.
- [ ] **Step 6: Run the focused rejected-frame test.** Verify non-empty rejected work does not call the logical terminal and the next physical presentation settles it exactly once.
- [ ] **Step 7: Commit the logical settlement correction.**

### Task 3: Add authoritative visual-root metadata and SSD-aware order repair

**Files:**
- Modify: `src/native_output/output/damage.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/tests/output.rs`

**Interfaces:**
- `NativeSceneSnapshot::from_surfaces` retains its existing callers; add a narrow popup-aware constructor for production resolution if needed.
- `NativeSceneSurfaceSnapshot` carries an immutable visual-root surface ID derived from `visual_stack_groups`.
- `push_order_transition_damage` remains the common-prefix/common-suffix algorithm but repairs affected visual groups and their matching `DecorationSceneSnapshot` bounds.

- [ ] **Step 1: Add the visual-root field and include it in scene identity.** Populate root IDs by enumerating `visual_stack_groups(surfaces, popup_surface_ids)` so popups stay separate from parent SSD.
- [ ] **Step 2: Route `ResolvedNativeFrameScene::from_server` through the popup-aware snapshot construction.** Preserve exact surface IDs, decoration IDs, visibility, and external-overlay semantics.
- [ ] **Step 3: Extend changed-span order repair.** Collect roots from old/current span members, then push old/current client/subsurface bounds for all members of those roots and old/current decoration bounds with matching `root_surface_id`. Leave unchanged ordered IDs untouched.
- [ ] **Step 4: Add bounded metrics only if the current native metrics boundary already supports them.** Do not add synchronous per-frame logs or unrelated observability redesign.
- [ ] **Step 5: Run decorated reorder, popup reorder, subsurface reorder, and true-global-invalidation tests.** Verify regional output for bounded transitions and `FullOutput` only for existing visibility/external-overlay reasons.
- [ ] **Step 6: Commit the SSD-aware damage correction.**

### Task 4: Prove compatibility scene identity and strengthen the integrated oracle

**Files:**
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/tests/output.rs`
- Modify: `src/native_output/tests/integrated_swapchain_oracle.rs` only when the existing production-like oracle needs the client/output identity assertion

**Interfaces:**
- Compatibility capture records the exact resolved scene signature and ordered sampled IDs before protocol-only frame-batch mutation, then asserts the re-resolved paint scene matches.
- The existing topology/output-buffer-age oracle performs actual rejected-slot rendering, leaves planner/presented state unchanged, and presents a later retry against the true predecessor.

- [ ] **Step 1: Add the compatibility identity invariant around the existing re-resolution.** Assert the second resolved scene’s identity signature and surface IDs equal the token-capture scene; keep the current batch and O1 flow unchanged.
- [ ] **Step 2: Extend the existing topology oracle’s visuals with overlapping SSD decoration snapshots and a reference painter that follows `window_visual_stack_order` ownership.** Include decoration pixels outside clients.
- [ ] **Step 3: Add explicit client buffer A/B/C/A labels and output slot 0/1/2/0 ages 1/2/3 to the existing sequence.** Keep logical Empty independent of client buffer identity.
- [ ] **Step 4: Replace the fake rejected-candidate stage.** Render the candidate into its selected slot, assert it differs from presented state where appropriate, do not call planner presented-transition commit or update `presented_scene`, then run a separate retry through the normal oracle-present helper.
- [ ] **Step 5: Run the integrated topology/buffer-age oracle and client/output swapchain oracle.** Require pixel equality with the full reference after every physically presented frame and assert rejected candidates do not advance physical history.
- [ ] **Step 6: Commit the integrated oracle and compatibility invariant.**

### Task 5: Focused review, full verification, and closure report

**Files:**
- Create: `REPORT-2026-08-29-typhon-no-visual-decoration-damage-correction.md`
- Modify: only task-owned files from Tasks 1–4 if verification exposes a direct regression

**Interfaces:**
- Consumes source scans, focused test output, full verification output, and git history.
- Produces the exact final report requested by the user, including local/public differences, root causes, before/after flows, RED failures, oracle results, complexity/ownership evidence, remaining FullOutput reasons, and preserved non-regressions.

- [ ] **Step 1: Re-search all requested callers and symbols.** Prove NoVisualChange never advances physical authorities, rejected non-empty frames retain damage, order repair includes SSD, unchanged order is a no-op, Atomic lineage is unchanged, and compatibility scene identity is asserted.
- [ ] **Step 2: Run focused suites.** Cover surface journal, NoVisualChange, frame batches, native damage, decorations/visual groups, partial repaint/buffer age, integrated swapchain, Atomic, Direct Scanout, O1 callbacks, and SHM materialization.
- [ ] **Step 3: Run `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, `rtk git diff --check`, and `rtk git status --short`. Record exact exit status and classify unrelated existing failures.
- [ ] **Step 4: Review the diff for scope discipline.** Confirm no O1, SHM, DMA-BUF, KMS worker, Direct Scanout policy, pointer-lock, resize, trace, VRR, tearing, color, or scene-graph work entered the corrective diff.
- [ ] **Step 5: Write the English closure report and explicitly state native KMS qualification status.** Do not claim native qualification unless it was run on a real DRM/KMS TTY.
- [ ] **Step 6: Run final verification after the report and commit the final corrective closure.**
