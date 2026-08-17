# Typhon WindowVisual and Input Corrective Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon’s normal windows one renderer/input/damage ownership unit while sealing fullscreen interaction, keyboard ownership, focus policy, XWayland geometry, and high-resolution wheel semantics.

**Architecture:** Extend the existing `DecorationSceneSnapshot` work into a renderer-independent `WindowVisualSnapshot`/ordered visual-group model keyed by `WindowId` and root surface. CPU and GLES consume the same grouped order, while native damage consumes old/current complete visual bounds. Input and lifecycle changes use explicit eligibility, ownership, and focus-reason decisions rather than application-specific exceptions.

**Tech Stack:** Rust 2024, Smithay-generated Wayland bindings, existing CPU/GLES renderers, native DRM/KMS output, existing deterministic compositor test harness, RTK for shell inspection.

## Global Constraints

- Preserve the dirty working tree and never reset, restore, clean, stash, or overwrite unrelated changes.
- Use `render::surface_origins()` and the authoritative placement model for all global render, hit-test, XWayland, and damage geometry.
- Keep normal-window visual ownership grouped; do not add a parallel decoration z-order or global SSD post-pass.
- Do not force full-output repaint, disable buffer age, globally disable Direct Scanout, or add unbounded diagnostics.
- Do not add Firefox/Kitty/application-specific workarounds; trace unresolved symptoms instead.
- Do not parse themes, SVG, or fonts in the frame loop; retain cached immutable visual plans and assets.
- Protocol behavior must follow the repository’s generated Wayland protocol versions and negotiated client versions.
- Every production change gets a focused regression test written and observed failing before implementation.

---

### Task 1: Capture closure evidence and establish WindowVisual ownership

**Files:**
- Inspect/modify: `src/compositor/render.rs`, `src/compositor/server.rs`, `src/compositor/state/window_decoration.rs`, `src/compositor/state/windows.rs`
- Test: `src/compositor/render.rs` or the existing renderer-independent test module
- Docs: this plan and `docs/superpowers/specs/2026-08-17-window-visual-input-corrective-closure-design.md`

**Interfaces:**
- Consume existing `DecorationSceneSnapshot`, `DecorationRenderInstance`, XWayland scene data, and authoritative window order.
- Produce a stable `WindowVisualSnapshot`/group representation containing `WindowId`, root surface identity, resolved origin, client membership/bounds, complete visual bounds, optional SSD/backing, and visual signature.

- [ ] Record HEAD, dirty status, diff stat/name list, current design/plan, and focused baseline test results in the journal.
- [ ] Add a renderer-independent two-window overlap regression that fails when all normal decorations are emitted after all surfaces.
- [ ] Implement the smallest shared ordered visual model that can represent SSD, CSD, XDG, XWayland backing, popups/subsurfaces, and explicit layer-shell boundaries.
- [ ] Make CPU and GLES command construction consume that order and retire global normal-window decoration post-passes.
- [ ] Add render/input topmost-owner parity coverage and three-window/mixed CSD-XDG-XWayland/layer-shell cases.

### Task 2: Seal fullscreen move/resize eligibility and cancellation

**Files:**
- Modify: `src/compositor/state/window_interaction.rs`, `src/compositor/state/windows.rs`, `src/compositor/state/fullscreen.rs`, relevant XDG/X11 request routing
- Test: existing window interaction/fullscreen test modules

- [ ] Add failing tests for every generic move/resize source against `ToplevelMode::Fullscreen`.
- [ ] Centralize the eligibility decision before IDs, resize-flow state, render placement, focus, or interaction state are allocated.
- [ ] Cancel active move/resize through normal end/cancel ownership before fullscreen geometry becomes stable.
- [ ] Add rejection/no-mutation assertions and 100-cycle fullscreen/maximize geometry drift tests.
- [ ] Preserve true-fullscreen no-SSD and Direct Scanout eligibility.

### Task 3: Reconcile client-visible keyboard ownership

**Files:**
- Modify: `src/native_output/input/state.rs`, `src/native_output/input/routing.rs`, `src/compositor/state/input_dispatch.rs`
- Test: native input state tests and compositor keyboard tests

- [ ] Add a failing forwarded-Alt-down → shortcut/Alt-Tab → physical-release test, including left/right Alt and focus changes.
- [ ] Refactor the deferred/forwarded modifier ledger only as needed to guarantee exactly-once release or explicit lifecycle retirement.
- [ ] Move release reconciliation before shortcut early returns while preserving target-client ordering and no duplicate/no synthetic releases.
- [ ] Generalize regression coverage to Super/Ctrl/Shift, inhibition, client destruction, and VT/session clearing.

### Task 4: Unify complete WindowVisual damage and bounded diagnostics

**Files:**
- Modify: `src/compositor/render.rs`, `src/native_output/output/damage.rs`, `src/native_output/runtime/frame.rs`, `src/native_output/runtime/presentation.rs`, `src/native_output/runtime/presentation_worker.rs`, renderer frame paths
- Test: native output damage/frame tests and deterministic partial-vs-full reference tests

- [ ] Add failing appearance/disappearance/move/resize/signature-change tests using complete old/current visual bounds and buffer ages 1–3.
- [ ] Replace parallel decoration/surface damage ownership with one logical WindowVisual diff consumed equivalently by CPU and GLES.
- [ ] Verify partial framebuffer pixels equal clean full-reference renders across 30–100 moves and all eight resize directions.
- [ ] Add opt-in bounded frame-history and pointer/grab/XDG-focus traces with no default per-frame spam.

### Task 5: Make focus policy explicit and qualify pointer grabs

**Files:**
- Modify: `src/compositor/state/xdg_lifecycle.rs`, `src/compositor/state/surface_focus.rs`, `src/compositor/state/window_interaction.rs`, `src/compositor/input.rs`
- Test: XDG lifecycle, focus, pointer grab, and input integration tests

- [ ] Add a failing map-during-client-grab test showing unconditional low-level focus is unsafe.
- [ ] Route map activation through explicit `WindowFocusReason` policy; preserve first-window and authorized activation behavior.
- [ ] Add deterministic Kitty-equivalent press/grab/motion/release and destruction-during-grab tests, proving decoration capture cannot leak into client content.
- [ ] Add bounded XDG/focus trace fields for map/unmap, focus transition, active grab, configure/ack context.

### Task 6: Make MacTahoe geometry borderless and unify XWayland extents

**Files:**
- Modify: `resources/decorations/MacTahoe-Dark/theme.json`, built-in fallback metrics, `src/compositor/decoration/*`, XWayland frame/extents code
- Test: decoration geometry/theme/XWayland tests

- [ ] Add failing assertions for zero visible side/bottom borders, 26 logical px titlebar, and independent 6 px resize affordance.
- [ ] Set bundled and fallback MacTahoe visible border metrics to zero without removing invisible resize targets or real SVG/font assets.
- [ ] Derive XWayland frame extents from the same decoration extents used by render and hit testing.
- [ ] Cover normal/no-decoration/override-redirect/maximized/fullscreen/theme-reload/titlebar-height cases.

### Task 7: Preserve high-resolution wheel semantics

**Files:**
- Modify: `src/compositor/input.rs`, native input normalization/dispatch, Wayland pointer dispatch and generated protocol compatibility points
- Test: compositor pointer-axis and Wayland protocol-version tests

- [ ] Add failing high-resolution horizontal/vertical wheel tests that demonstrate v120 loss at the current boundary.
- [ ] Extend `PointerAxisComponent` with a validated `value120` field and preserve it through native normalization, frame/source/stop grouping, and dispatch.
- [ ] Emit modern events only for negotiated modern pointer versions; preserve accumulated legacy discrete semantics for older clients; keep continuous input continuous.
- [ ] Add zero/invalid, fractional accumulation, sign, source, stop, frame, and neutral-factor coverage. Do not tune subjective speed before fidelity passes.

### Task 8: Direct Scanout, verification, native qualification, and report

**Files:**
- Modify: Direct Scanout qualification code only where the grouped visual model requires it; `docs/ARCHITECTURE.md` only for durable contracts
- Add: `docs/superpowers/specs/2026-08-17-window-visual-input-corrective-closure-design.md`, `REPORT-2026-08-17-window-visual-input-corrective-closure.md`

- [ ] Add/verify true-fullscreen versus decorated fullscreen Direct Scanout regressions.
- [ ] Re-run focused suites after every task, then `cargo fmt --check`, `cargo check --locked --all-targets`, locked tests, strict clippy, source-layout, and `git diff --check`.
- [ ] Attempt native Astrea/Typhon qualification only if the real session is available; otherwise record the exact blocker and claim only deterministic results.
- [ ] Self-review all closure invariants, preserve unrelated dirty files, stage focused logical units, commit only closed units, and report hashes plus final status.
