# Implementation Plan: Client-Provided Wayland Cursor Overlay

## Overview

Implement the approved dedicated client-cursor overlay from protocol selection
through buffer lifecycle, CPU and EGL rendering, native and nested scheduling,
and regression validation. Each behavior slice starts with a focused failing
test and leaves cursor surfaces isolated from the ordinary scene.

## Architecture Decisions

- Keep exact Wayland pointer ownership in compositor-only state and expose a
  borrowed renderer snapshot containing only a `RenderableSurface` and logical
  coordinates.
- Store committed content per permanent cursor-role surface; active selection
  is independent from retained surface content.
- Reuse generic pending commit, explicit-sync, frame callback, presentation,
  and deferred buffer-release machinery.
- Keep the EGL window scene cache cursor-free and build client cursor commands
  only in the overlay path.
- Keep CPU reusable bases cursor-free and composite the client cursor last.

## Task List

### Phase 1: Protocol State Foundation

## Task 1: Add active cursor state and renderer snapshot

**Description:** Add permanent cursor-role state, exact pointer ownership,
hotspot storage, client-cursor render snapshot capture, and cursor-specific
generation causes.

**Acceptance criteria:**

- [ ] Valid exact-pointer requests expose the selected surface and hotspot.
- [ ] Invalid client or stale serial requests cannot replace active state.
- [ ] Snapshot coordinates equal rounded pointer position minus hotspot.

**Verification:**

- [ ] Focused protocol tests fail before implementation and pass afterward.
- [ ] Existing cursor ownership and exact-serial tests pass unchanged.

**Dependencies:** None

**Files likely touched:**

- `src/compositor/mod.rs`
- `src/compositor/server.rs`
- `src/compositor/tests/input_output.rs`
- `src/compositor/tests/support.rs`

**Estimated scope:** Medium

## Task 2: Implement ownership cleanup and permanent role isolation

**Description:** Apply explicit hidden-cursor semantics to null selection,
focus loss, pointer destruction, surface destruction, client disconnect, and
locked-pointer transitions without permitting cursor-role surfaces into scene
or input structures.

**Acceptance criteria:**

- [ ] Null selection and null attachment never restore the built-in cursor.
- [ ] Exact owner cleanup damages stale imagery without affecting other clients.
- [ ] Cursor-role surfaces cannot map, stack, focus, or hit test.

**Verification:**

- [ ] Focused ownership, cleanup, mapping, and pointer-lock tests pass.
- [ ] Existing Sober reveal-order tests pass unchanged.

**Dependencies:** Task 1

**Files likely touched:**

- `src/compositor/mod.rs`
- `src/compositor/protocols/core.rs`
- `src/compositor/tests/input_output.rs`
- `src/compositor/tests/pointer_constraints.rs`
- `src/compositor/tests/support.rs`

**Estimated scope:** Medium

### Checkpoint: Protocol State

- [ ] Focused cursor and pointer-lock tests pass.
- [ ] `cargo check --workspace` succeeds.

### Phase 2: Commit And Lifecycle

## Task 3: Route cursor buffers through generic commit lifecycle

**Description:** Convert cursor attaches and damage-only commits into dedicated
`RenderableSurface` entries while preserving viewport, scale, SHM, dmabuf,
acquire-fence, frame callback, presentation, and deferred release behavior.

**Acceptance criteria:**

- [ ] Cursor content updates without entering `renderable_surfaces()`.
- [ ] Null attach removes visible content while retaining active ownership.
- [ ] Active and inactive callback/release behavior cannot stall indefinitely.

**Verification:**

- [ ] Focused SHM, dmabuf, explicit-sync, frame callback, and null-attach tests pass.
- [ ] Existing generic surface lifecycle tests pass unchanged.

**Dependencies:** Tasks 1-2

**Files likely touched:**

- `src/compositor/mod.rs`
- `src/compositor/state_data.rs`
- `src/compositor/protocols/core.rs`
- `src/compositor/tests/input_output.rs`
- `src/compositor/tests/support.rs`

**Estimated scope:** Medium

### Checkpoint: Compositor Lifecycle

- [ ] Cursor protocol and lifecycle tests pass.
- [ ] `cargo check --workspace` succeeds.

### Phase 3: CPU Rendering

## Task 4: Composite client cursor in CPU renderer

**Description:** Extend CPU compose requests with the cursor snapshot, restore
the cursor-free reusable base, then sample, scale, clip, and alpha blend client
cursor pixels as the final layer.

**Acceptance criteria:**

- [ ] Client cursor renders above windows and shell with correct hotspot/alpha.
- [ ] Clipping is safe on all edges and movement/removal leaves no trails.
- [ ] Built-in and client cursors are never drawn together.

**Verification:**

- [ ] Deterministic CPU pixel tests fail before implementation and pass afterward.
- [ ] Existing frame reuse and damage tests pass.

**Dependencies:** Task 3

**Files likely touched:**

- `src/compositor/render.rs`
- `src/compositor/mod.rs`

**Estimated scope:** Medium

### Phase 4: EGL Rendering And Damage

## Task 5: Add generic client cursor damage tracking

**Description:** Track clipped client cursor bounds and content identity
independently from the fixed built-in cursor so state, motion, hotspot, scale,
and content transitions return old plus new damage.

**Acceptance criteria:**

- [ ] Movement and hotspot changes damage previous and current rectangles.
- [ ] Hide/removal damages the previous rectangle; content damages current bounds.
- [ ] Bounds use actual scaled client surface dimensions and clip safely.

**Verification:**

- [ ] Focused EGL damage unit tests fail before implementation and pass afterward.

**Dependencies:** Task 3

**Files likely touched:**

- `src/egl_renderer/damage.rs`
- `src/egl_renderer.rs`

**Estimated scope:** Small

## Task 6: Import and draw client cursor in EGL overlay path

**Description:** Reuse surface resource upload/import machinery, retain active
cursor resources independently of ordinary scene liveness, and append client
cursor commands after shell commands without changing the scene cache key.

**Acceptance criteria:**

- [ ] SHM and dmabuf cursor resources update and remain live when needed.
- [ ] Client cursor is the final draw command after shell overlay.
- [ ] Cursor-only motion does not rebuild ordinary scene commands.

**Verification:**

- [ ] Focused resource, ordering, and cache-stat tests pass.
- [ ] Existing EGL surface upload/import tests pass.

**Dependencies:** Tasks 3 and 5

**Files likely touched:**

- `src/egl_renderer.rs`
- `src/egl_renderer/damage.rs`
- `src/egl_renderer/geometry.rs`

**Estimated scope:** Medium

### Checkpoint: Renderers

- [ ] CPU and EGL focused tests pass.
- [ ] `cargo check --workspace` succeeds.

### Phase 5: Backend Integration And Scheduling

## Task 7: Propagate snapshots through nested paths

**Description:** Pass the client cursor independently through nested output and
renderer requests, trigger redraw from cursor render generations, and keep the
host/built-in cursor suppressed while client ownership is active.

**Acceptance criteria:**

- [ ] Nested GPU and CPU requests use identical snapshot semantics.
- [ ] Cursor movement and commits request redraw without duplicate host cursor.
- [ ] Output scale is applied exactly once.

**Verification:**

- [ ] Nested renderer/request tests pass.
- [ ] Nested backend compiles for GPU and CPU feature combinations.

**Dependencies:** Tasks 4 and 6

**Files likely touched:**

- `src/nested_renderer.rs`
- `src/nested_output.rs`
- `src/compositor/server.rs`

**Estimated scope:** Medium

## Task 8: Propagate snapshots through native paths

**Description:** Pass the snapshot through native EGL and CPU requests, include
cursor generations in repaint scheduling and output damage decisions, and
preserve the hardware built-in cursor motion fast path.

**Acceptance criteria:**

- [ ] Native EGL and CPU render the same client cursor overlay semantics.
- [ ] Client cursor motion schedules repaint while hardware cursor-only motion keeps its fast path.
- [ ] Output changes and lock transitions remove stale cursor pixels.

**Verification:**

- [ ] Native request, repaint, and damage tests pass.
- [ ] Existing scheduler and pointer-lock tests pass unchanged.

**Dependencies:** Tasks 4 and 6

**Files likely touched:**

- `src/native_output.rs`
- `src/compositor/server.rs`

**Estimated scope:** Medium

### Phase 6: Validation And Handoff

## Task 9: Run full quality gates and document validation

**Description:** Format, lint, run focused and full tests, build release,
inspect diffs, and separate environmental or pre-existing failures from new
failures.

**Acceptance criteria:**

- [ ] All automated acceptance commands complete or have evidenced pre-existing failures.
- [ ] No unrelated or generated artifacts enter the change.
- [ ] Final report distinguishes automated from hardware/manual validation.

**Verification:**

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] Focused cursor and pointer-lock tests
- [ ] `cargo test --workspace`
- [ ] `cargo build --workspace --release`
- [ ] `git diff --check`

**Dependencies:** Tasks 1-8

**Files likely touched:** None beyond fixes required by validation

**Estimated scope:** Small

### Checkpoint: Complete

- [ ] All approved protocol, rendering, lifecycle, and regression criteria pass.
- [ ] Implementation commits are logically separated and reviewable.
- [ ] Manual Zen/Sober validation is claimed only if actually performed.

## Risks And Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Cursor explicit-sync commits bypass the dedicated sink when fences signal later | High | Dispatch ready commits by permanent cursor role and test delayed acquire signaling. |
| Retained inactive dmabuf content is released too early | High | Keep existing active buffer ownership keyed by cursor surface until replacement/null/destroy. |
| CPU reusable frames retain old cursor pixels | High | Keep reusable base cursor-free and restore it before each cursor overlay. |
| Cursor generation invalidates EGL window cache | Medium | Exclude cursor state from scene signatures and test `scene_rebuilt == false` on motion. |
| Output scale or negative hotspot is applied inconsistently | Medium | Snapshot logical coordinates; convert and clip in one renderer boundary with edge tests. |
| Unlock exposes a transient compositor cursor | High | Preserve pending reveal ordering and run existing Sober sequence tests unchanged. |

## Open Questions

None. The approved design fixes the state, lifecycle, ordering, and fallback
semantics needed for implementation.
