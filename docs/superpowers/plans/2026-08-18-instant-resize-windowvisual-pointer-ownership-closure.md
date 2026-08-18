# Instant Resize and WindowVisual Pointer Ownership Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make processed interactive resize geometry visible in the same input dispatch, pipeline at most three protocol-pressure XDG configures with exact ownership, and route SSD pointer ownership through one ordered scene hit test.

**Architecture:** Remove `PendingInteractiveResizeUpdate` as a visual cache and call the existing `queue_resize_root_window_to()` authority directly from resize interaction updates. Keep committed client content and visual target separate. Refactor `ResizeConfigureFlow` to count only unACKed plus newest ACKed/uncaptured pressure, retain captured snapshots independently, and prioritize the final configure. Add `PointerSceneHit` as the only ordinary pointer scene authority, interleaving existing renderable surfaces and current SSD input regions.

**Tech Stack:** Rust, Smithay Wayland protocol objects, existing Typhon `CompositorState`, XDG/XWayland backend queues, existing WindowVisual/render-generation/damage/presentation-history tests.

## Global Constraints

- Preserve all dirty working-tree closure work; never use `git reset`, `git restore`, `git checkout`, `git clean`, or `git stash`.
- The configure hard bound is `3` and must never grow with pointer frequency.
- Captured commits remain owned until apply/release but do not consume protocol pressure capacity.
- Visual resize uses `ToplevelVisualGeometry` and never stretches stale client buffers by default.
- SSD decorations are compositor-owned input regions; do not fabricate a Wayland surface or disable pointer-enter focus globally.
- Grabs, constraints, popup grabs, layer ordering, CSD, XWayland behavior, triple buffering, buffer age, explicit sync, presentation history, and fullscreen scene authority remain enabled.
- Use `rtk` for all shell commands and `apply_patch` for file edits.

---

### Task 1: Reproduce the SSD fall-through and same-dispatch resize latency

**Files:**
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/state/window_interaction_tests.rs`
- Modify: `src/compositor/state/task_05_8_tests.rs`
- Test support: `src/compositor/tests/support/window_ops.rs` only if an existing fixture helper must expose two overlapping managed windows

**Interfaces:**
- Consumes: `CompositorState::decoration_hit_at`, `CompositorState::pointer_target_at`, `CompositorState::update_window_interaction_by_id`, `ToplevelVisualGeometry`.
- Produces: named red tests `pointer_scene_fallthrough_reproduces_before_unified_hit_routing` and `interactive_resize_visual_geometry_is_applied_before_dispatch_returns` that later implementation tasks must turn green.

- [ ] **Step 1: Write the failing overlapping-window pointer test**

Create two managed SSD-capable test windows with B below A, place B's client under A's titlebar, focus A, and move the pointer from A's client coordinate to A's titlebar coordinate. Assert the current pre-fix behavior records a lower client target/focus opportunity while the independent decoration query identifies A. The test must fail after the intended behavior assertion is added, not merely pass because it only checks `decoration_hit_at`.

- [ ] **Step 2: Run the pointer test and record the expected red failure**

Run:

```bash
rtk cargo test --locked pointer_scene_fallthrough_reproduces_before_unified_hit_routing -- --exact --nocapture
```

Expected: FAIL because ordinary pointer routing resolves B or because no unified scene-hit value exists; this establishes the confirmed symptom.

- [ ] **Step 3: Write the same-dispatch visual resize test**

Use an existing `WindowInteraction` fixture with a root surface and bottom-right or top-left edges. Call:

```rust
assert!(state.update_window_interaction_by_id(interaction.id, target_x, target_y));
assert_eq!(
    state.current_visual_root_window_geometry(interaction.root_surface_id),
    Some(expected_visual_geometry),
);
```

Do not call `prepare_frame()` or `apply_pending_interactive_resize_update()` in the test.

- [ ] **Step 4: Run the resize test and record the expected red failure**

Run:

```bash
rtk cargo test --locked interactive_resize_visual_geometry_is_applied_before_dispatch_returns -- --exact --nocapture
```

Expected: FAIL because the current geometry remains unchanged until `prepare_frame()` consumes `PendingInteractiveResizeUpdate`.

- [ ] **Step 5: Commit only the reproduction tests**

```bash
rtk git add src/compositor/state/window_decoration_tests.rs src/compositor/state/window_interaction_tests.rs src/compositor/state/task_05_8_tests.rs
rtk git commit -m "test(input): reproduce SSD fall-through and deferred resize"
```

### Task 2: Add the ordered `PointerSceneHit` model and route decoration ownership

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/tests/input_output/window_interaction.rs` for protocol enter/leave assertions when the existing fixture is required

**Interfaces:**
- Produces: `PointerSceneHit`, `CompositorState::pointer_scene_hit_at(x, y)`, and `CompositorState::focus_desktop_window_at_pointer_scene_hit(&PointerSceneHit)`.
- Consumes: current `renderable_surfaces` order, `current_visual_root_window_geometry`, `DecorationLayout::hit_test`, `pointer_target_allowed_by_popup_grab`, and existing focus/grab guards.

- [ ] **Step 1: Add focused red tests for decoration ownership**

Add tests for:

```rust
assert!(matches!(state.pointer_scene_hit_at(a_titlebar_x, a_titlebar_y),
    PointerSceneHit::Decoration { window_id, .. } if window_id == a_id));
assert_eq!(state.focused_window_id, Some(a_id));
assert_eq!(state.focus_generation, focus_generation_before);
```

Also cover button, invisible resize margin, A client → A decoration → A client pointer leave/enter, popup above decoration, layer surface above decoration, and CSD without decoration. The pre-fix failure is either a B `Client` hit or absence of the enum/method.

- [ ] **Step 2: Run the focused input tests and verify red**

```bash
rtk cargo test --locked window_decoration -- --nocapture
rtk cargo test --locked pointer_scene_hit -- --nocapture
```

Expected: the new ownership tests fail while existing client/surface tests remain attributable to the new assertions.

- [ ] **Step 3: Implement one top-to-bottom scene walk**

Define the enum in `hit_testing.rs`. In `pointer_scene_hit_at`, walk `renderable_surfaces` in reverse order. For each renderable, use the existing surface input-region test first. For a child/popup, return `Client`. For a window root, evaluate the current SSD layout via a helper extracted from `decoration_hit_at`; return `Decoration` for interactive frame regions, otherwise return the root `Client` when its input region contains the point. Continue through non-input shadows and transparent extensions.

Make `pointer_target_at` project only `PointerSceneHit::Client`. Make `decoration_hit_at` project only `PointerSceneHit::Decoration` so existing decoration callers share ordering.

- [ ] **Step 4: Implement focus and client pointer routing for decorations**

Route `PointerSceneHit::Decoration` to the owner `WindowId` with the existing pointer-enter guards. Clear client pointer focus while over SSD; do not fabricate a `wl_surface`. On return to a client hit, use normal enter/motion dispatch. Keep locked/confined pointers, implicit grabs, active interactions, popup grabs, drag-and-drop, and layer ordering ahead of ordinary scene resolution.

- [ ] **Step 5: Make decoration button/resize clicks use the scene owner**

Extend `handle_decoration_button` to start resize from a `DecorationHit::Resize` on the left button and ensure generic decoration clicks never fall through to `pointer_target_at`. Preserve exact A `WindowId` for titlebar move, button capture, and resize-edge capture.

- [ ] **Step 6: Run the focused tests green and commit**

```bash
rtk cargo test --locked window_decoration -- --nocapture
rtk cargo test --locked pointer_scene_hit -- --nocapture
rtk cargo test --locked input_output::window_interaction -- --nocapture
```

Expected: new and existing decoration, popup, layer, CSD, grab, and pointer-enter tests pass.

```bash
rtk git add src/compositor/state/hit_testing.rs src/compositor/state/window_decoration.rs src/compositor/state/windows.rs src/compositor/state/input_resources.rs src/compositor/state/input_dispatch.rs src/compositor/state/window_decoration_tests.rs src/compositor/tests/input_output/window_interaction.rs
rtk git commit -m "fix(input): close WindowVisual decoration ownership"
```

### Task 3: Apply visual resize immediately and remove the deferred visual cache

**Files:**
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/compositor/state/window_interaction_tests.rs`
- Modify: `src/native_output/tests/input.rs` where the existing prepare-frame assertion encodes the old delay

**Interfaces:**
- Consumes: `queue_resize_root_window_to`, `ToplevelVisualGeometry`, `send_window_interaction_pointer_motion`.
- Produces: same-dispatch visual geometry mutation; no `PendingInteractiveResizeUpdate` visual authority; same-dispatch `flush_pending_resize_configure` invocation from server input wrappers.

- [ ] **Step 1: Update the red test expectations without adding production code**

Change the new resize test to assert no pending update exists and that `render_target_size` is still cleared for stale-content policy. Change existing tests that explicitly call `apply_pending_interactive_resize_update()` to assert the direct interaction update already installed the preview.

- [ ] **Step 2: Run the resize tests red**

```bash
rtk cargo test --locked interactive_resize_visual_geometry_is_applied_before_dispatch_returns -- --exact --nocapture
```

Expected: FAIL because production still stores the target in `pending_interactive_resize_update`.

- [ ] **Step 3: Replace the cache with the existing resize authority**

In `update_window_interaction_by_id`, construct the clamped geometry and call `queue_resize_root_window_to` immediately. Preserve raw/coalescing diagnostics and return the actual applied result. Remove the `PendingInteractiveResizeUpdate` struct/field and delete its frame-preparation, cleanup, and grabbed-pointer coordinate branches. Remove `apply_pending_interactive_resize_update()` and its `prepare_frame()` call. Keep `prepare_frame()` flushing client configures as a safety net.

- [ ] **Step 4: Ensure pointer and decoration state use the new geometry**

In the active interaction branch of `send_pointer_motion`, update the interaction before `update_pointer_position` so decoration hover and cursor hit regions use the current `ToplevelVisualGeometry`. Keep interaction-owned client motion dispatch and existing cursor-generation behavior.

- [ ] **Step 5: Flush configures in the same server input dispatch**

After `state.send_pointer_motion`, `state.send_pointer_motion_sample`, and the public `update_window_interaction` entry point, call `state.flush_pending_resize_configure()` before `display.flush_clients()`. Do not add blocking waits or render work to the pointer path.

- [ ] **Step 6: Run focused resize and rendering tests**

```bash
rtk cargo test --locked interactive_resize_visual_geometry_is_applied_before_dispatch_returns -- --exact --nocapture
rtk cargo test --locked window_interaction -- --nocapture
rtk cargo test --locked native_output::tests::input -- --nocapture
```

Expected: same-dispatch geometry, immediate SSD layout/hit regions, stale buffer non-stretching, X11 preview, and existing render-generation assertions pass.

- [ ] **Step 7: Commit the visual path**

```bash
rtk git add src/compositor/state/window_interaction.rs src/compositor/state/window_resize.rs src/compositor/mod.rs src/compositor/state/frames.rs src/compositor/state/input_dispatch.rs src/compositor/state/surface_commits.rs src/compositor/server.rs src/compositor/server_toplevel.rs src/compositor/state/window_interaction_tests.rs src/native_output/tests/input.rs
rtk git commit -m "fix(resize): apply interactive visual geometry immediately"
```

### Task 4: Refactor `ResizeConfigureFlow` to a bounded sliding window

**Files:**
- Modify: `src/compositor/interaction.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/resize.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/task_05_8_tests.rs`
- Modify: `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Produces: `MAX_IN_FLIGHT_RESIZE_CONFIGURES == 3`, protocol-pressure count excluding `captured`, final-priority send selection, and bounded diagnostics.
- Consumes: existing `SentResizeConfigure`, `ResizeCommitSnapshot`, XDG serial state, and `flush_pending_resize_configure`.

- [ ] **Step 1: Add red ledger tests**

Add exact tests for:

```rust
flow.mark_sent(a, 10, 1);
flow.mark_sent(b, 11, 2);
flow.mark_sent(c, 12, 3);
assert_eq!(flow.protocol_pressure_count(), 3);
assert!(flow.queue(d));
assert!(flow.take_sendable().is_none());
```

Then capture A and assert a new target is sendable while `captured_count() == 1`. Add a final-priority test proving a final target can replace stale unsent/intermediate pressure without exceeding 3.

- [ ] **Step 2: Run ledger tests red**

```bash
rtk cargo test --locked task_05_8 -- --nocapture
```

Expected: FAIL because current `take_sendable()` requires zero total in-flight state and captured commits count against capacity.

- [ ] **Step 3: Implement pressure accounting and latest-wins queueing**

Change `in_flight_configure_count()` or introduce `protocol_pressure_count()` so captured snapshots are excluded. Make `take_sendable()` return the final target first, then the latest queued target, while enforcing the hard bound. If final is pending at capacity, retire/supersede the oldest unACKed intermediate entry before taking it. Keep `queued_latest` at one entry.

- [ ] **Step 4: Allow multiple sends per flush**

Change `flush_pending_resize_configure()` to repeatedly choose sendable flows until every flow is full or empty. `mark_sent()` must retain all required serial/sequence/interaction/geometry/timestamp ownership and update bounded peak metrics.

- [ ] **Step 5: Update metrics and tests**

Track current/peak protocol pressure separately from retained capture count, coalesced targets, final-priority sends, oldest outstanding age, and latest target lag. Keep verbose logs behind the existing debug controls.

- [ ] **Step 6: Run green focused tests and commit**

```bash
rtk cargo test --locked task_05_8 -- --nocapture
rtk cargo test --locked window_interaction -- --nocapture
rtk cargo test --locked resize -- --nocapture
```

```bash
rtk git add src/compositor/interaction.rs src/compositor/state/window_resize.rs src/compositor/state/resize.rs src/compositor/mod.rs src/compositor/state/task_05_8_tests.rs src/compositor/state/window_interaction_tests.rs
rtk git commit -m "refactor(resize): pipeline bounded XDG configures"
```

### Task 5: Strengthen ACK, supersede, capture, and final reconciliation semantics

**Files:**
- Modify: `src/compositor/interaction.rs`
- Modify: `src/compositor/state/resize.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/task_05_8_tests.rs`
- Modify: `src/compositor/state/window_interaction_tests.rs`

**Interfaces:**
- Produces: serial-correct `ack`, `capture`, `complete_applied`, and final `resizing=false` behavior.
- Consumes: bounded flow from Task 4 and `active_toplevel_resizes`/`toplevel_visual_geometries`.

- [ ] **Step 1: Add red ownership tests**

Cover these exact sequences:

```text
A, B, C sent; ACK C -> A/B retired; C eligible
A sent; ACK A; B sent; ACK B; one commit -> B captured
A ACKed/captured; B/C sent; A applies -> visual target remains C
many intermediate targets; release at F -> final F, resizing=false
```

Also add slow-client 1,000-update and fast-client 1,000-update tests asserting pressure `<= 3`, queued unsent targets `<= 1`, and visual geometry equal to the latest processed target.

- [ ] **Step 2: Run ownership tests red**

```bash
rtk cargo test --locked resize_flow -- --nocapture
rtk cargo test --locked intermediate -- --nocapture
rtk cargo test --locked final -- --nocapture
```

Expected: FAIL on ACK replacement, captured-commit pressure, or stale visual rollback under the current single-ACK/single-in-flight semantics.

- [ ] **Step 3: Implement newer-ACK supersession**

When ACKing a newer serial, retire older outstanding serials and replace the uncaptured ACK explicitly. Reject ACKs older than the current uncaptured ACK as stale. Preserve captured snapshots and their commit sequence until apply/release.

- [ ] **Step 4: Preserve visual target over stale commits**

In `complete_applied_resize_transaction` and related commit paths, compare snapshot interaction ID with active preview ownership. Apply committed client content metadata for stale snapshots, but never replace an active newer `ToplevelVisualGeometry` or its placement/size.

- [ ] **Step 5: Prioritize and reconcile the final configure**

Queue final geometry from `current_visual_root_window_geometry`, clear only unsent intermediate targets, send `resizing=false` with bounded pressure priority, and remove preview override only when the matching final snapshot applies. Assert no one-pixel placement drift for all edge/corner variants.

- [ ] **Step 6: Run green ownership/X11 tests and commit**

```bash
rtk cargo test --locked resize_flow -- --nocapture
rtk cargo test --locked xwayland -- --nocapture
rtk cargo test --locked window_interaction -- --nocapture
```

```bash
rtk git add src/compositor/interaction.rs src/compositor/state/resize.rs src/compositor/state/window_resize.rs src/compositor/state/surface_commits.rs src/compositor/state/subsurfaces.rs src/compositor/state/task_05_8_tests.rs src/compositor/state/window_interaction_tests.rs
rtk git commit -m "fix(resize): preserve pipelined configure ownership"
```

### Task 6: Add bounded resize latency instrumentation

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/support_types.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/resize.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Test: `src/compositor/state/task_05_8_tests.rs`

**Interfaces:**
- Produces: bounded resize latency samples and p50/p95 derivations for the required timestamp pairs.
- Consumes: existing resize debug controls, presentation/pageflip timing, render-generation snapshots, and `ResizeFlowMetrics`.

- [ ] **Step 1: Add red metric-shape tests**

Assert histories have a fixed maximum, timestamps preserve nondecreasing stage order for a synthetic sample, and p50/p95 functions return deterministic values for 1, 2, and 100 samples.

- [ ] **Step 2: Run metric tests red**

```bash
rtk cargo test --locked resize_latency -- --nocapture
```

Expected: FAIL because the timestamp history and percentile derivation do not yet exist.

- [ ] **Step 3: Implement the minimal bounded instrumentation**

Record input, interaction, visual application, configure queue/send, frame resolution/submit, ACK, capture, apply, and pageflip timestamps only when the existing debug/performance control is enabled. Use a fixed-capacity ring/history and avoid logging/rasterization/allocation-heavy work in pointer dispatch.

- [ ] **Step 4: Run metric tests and focused compile checks**

```bash
rtk cargo test --locked resize_latency -- --nocapture
rtk cargo check --locked --all-targets
```

- [ ] **Step 5: Commit diagnostics**

```bash
rtk git add src/compositor/mod.rs src/compositor/state/support_types.rs src/compositor/state/window_interaction.rs src/compositor/state/window_resize.rs src/compositor/state/resize.rs src/native_output/runtime/cycle.rs src/native_output/runtime/metrics.rs src/native_output/runtime/presentation.rs src/native_output/runtime/cycle/pageflip.rs src/compositor/state/task_05_8_tests.rs
rtk git commit -m "feat(resize): record bounded interaction latency"
```

### Task 7: Protect damage, render-ahead, and buffer-age correctness

**Files:**
- Modify: only if required by failing tests: `src/compositor/state/window_resize.rs`, `src/compositor/render.rs`, `src/native_output/output/damage.rs`, `src/native_output/runtime/scene_history.rs`
- Test: `src/native_output/tests/output.rs`
- Test: `src/native_output/tests/output_retry.rs`
- Test: `src/native_output/tests/fullscreen_frame_scene.rs`
- Test: `src/egl_renderer/damage_tests.rs`

**Interfaces:**
- Consumes: immediate WindowVisual geometry changes and existing `ResolvedNativeFrameScene`, `NativeFrameSceneSnapshot`, `NativeSceneHistory`, `PresentedTransitionDamage`, `PartialRepaintPlanner`, and buffer-age oracle.
- Produces: regression proof that every rapid resize transition damages old UNION new complete visual bounds and preserves actual presented transitions.

- [ ] **Step 1: Add/extend a rapid A-B-C-D resize transition test**

Assert render-ahead journal order and framebuffer-reference damage for ages 1, 2, and 3 under immediate visual mutations; assert no full repaint/triple-buffer/buffer-age disablement.

- [ ] **Step 2: Run the test red if immediate mutation exposes a gap**

```bash
rtk cargo test --locked native_output::tests::output -- --nocapture
rtk cargo test --locked egl_renderer::damage -- --nocapture
```

Expected: either PASS with no patch needed or a targeted failure identifying the missing old/new visual damage transition.

- [ ] **Step 3: Apply only the minimal damage-domain correction**

Use existing complete WindowVisual bounds and presentation-domain transition bookkeeping. Do not introduce a full repaint or alter buffer-age policy.

- [ ] **Step 4: Run all render-ahead/fullscreen tests and commit if code changed**

```bash
rtk cargo test --locked native_output::tests::fullscreen_frame_scene -- --nocapture
rtk cargo test --locked output_retry -- --nocapture
rtk git diff --check
```

Commit only task-owned render changes:

```bash
rtk git add src/compositor/state/window_resize.rs src/compositor/render.rs src/native_output/output/damage.rs src/native_output/runtime/scene_history.rs src/native_output/tests/output.rs src/native_output/tests/output_retry.rs src/native_output/tests/fullscreen_frame_scene.rs src/egl_renderer/damage_tests.rs
rtk git commit -m "test(render): preserve resize presentation damage"
```

### Task 8: Run native qualification and write the closure report

**Files:**
- Create: `REPORT-2026-08-18-instant-resize-windowvisual-input-closure.md`
- Modify: none unless the report needs exact generated test names or command results

**Interfaces:**
- Consumes: all focused/full test output, baseline status, bounded metrics, and native launcher observations.
- Produces: evidence-backed final report with no “Hyprland-equivalent” claim unless native observation supports it.

- [ ] **Step 1: Run the requested focused suites**

```bash
rtk cargo test --locked ResizeConfigureFlow -- --nocapture
rtk cargo test --locked window_interaction -- --nocapture
rtk cargo test --locked window_decoration -- --nocapture
rtk cargo test --locked input_output::window_interaction -- --nocapture
rtk cargo test --locked xwayland -- --nocapture
rtk cargo test --locked native_output::tests::fullscreen_frame_scene -- --nocapture
rtk cargo test --locked output_retry -- --nocapture
```

- [ ] **Step 2: Run the full validation commands and record exact exit/output**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo test --locked
rtk cargo clippy --locked --all-targets -- -D warnings
rtk bash bin/check-source-layout
rtk git diff --check
rtk git status --short
```

Classify failures against a freshly captured pre-change baseline; do not call a failure pre-existing without matching baseline evidence.

- [ ] **Step 3: Run the supplied native launcher on the 165 Hz output**

Use:

```bash
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell ASTREA_COMPOSITOR_BACKEND=typhon TYPHON_XWAYLAND=eager ./bin/start-oblivion-one-tty
```

Exercise all eight resize directions, direction reversal, offscreen/small/large windows, Kitty, Firefox/Zen, GTK, Qt, and a practical XWayland client. Exercise A client → A titlebar → A button → A client with B underneath and verify focus/pointer traces. Record whether delay is compositor geometry or client content convergence.

- [ ] **Step 4: Create and review the final report**

Include baseline HEAD/status, root causes, reference comparisons, old/new data flow, bound and ledger semantics, pointer scene architecture, overlap/popup/layer/CSD/XWayland results, p50/p95 metrics, configure throughput, 165 Hz qualification, subjective before/after result, ghosting results, full validation, commits, blockers, and final `rtk git status --short` output.

- [ ] **Step 5: Final verification before completion claim**

Re-run:

```bash
rtk git diff --check
rtk git status --short
```

Then compare every requirement in the design and plan against code/test/report evidence before reporting completion.
