# Implementation Plan: Interactive Resize Buffer Identity

## Overview

Introduce stable Wayland buffer identity, make dmabuf cache ownership safe,
accept acknowledged resize commits using actual client geometry, and preserve
partial repaint, explicit sync, KMS, callback, and release behavior.

## Architecture Decisions

- `BufferId` is allocated at `wl_buffer` creation and carried through every
  committed/renderable buffer representation.
- Dmabuf imports use `BufferId` plus layout metadata; raw fds are diagnostics
  and EGL attributes only.
- A testable cache lifecycle policy owns identity and retention decisions; the
  EGL renderer owns context-bound destruction.
- Resize acceptance is serial-gated but size-tolerant and uses actual committed
  logical/window geometry for anchored placement.
- Buffer identity changes invalidate scene/resource and partial-repaint state.

## Phase 1: Reproduce And Identity Foundation

### Task 1: Add failing buffer identity and cache policy tests

**Acceptance criteria:** Tests demonstrate raw-fd aliasing, same-buffer reuse,
metadata isolation, exact-once eviction, import failure, generation reset, and
bounded swapchain behavior.

**Verification:** targeted render-backend and EGL renderer tests fail for the
missing stable identity behavior.

**Dependencies:** None

### Task 2: Add stable `BufferId` allocation and propagation

**Acceptance criteria:** Every SHM/dmabuf `wl_buffer` gets a nonzero unique ID;
clones preserve it; committed/renderable surfaces expose it.

**Verification:** buffer allocation/propagation tests pass.

**Dependencies:** Task 1

### Checkpoint: Identity

- [ ] Focused identity tests pass.
- [ ] Existing SHM/dmabuf protocol tests pass.

## Phase 2: Renderer Cache Lifecycle

### Task 3: Replace fd-based keys and add lifecycle planning

**Acceptance criteria:** Same live ID reuses one resource; different IDs with
identical fds never alias; layout changes never alias; dead and excess entries
are evicted exactly once.

**Verification:** focused cache tests pass.

**Dependencies:** Task 2

### Task 4: Integrate context-safe destruction, metrics, and diagnostics

**Acceptance criteria:** Import failure publishes nothing; renderer teardown
drains one owner per resource; current/peak/eviction metrics are observable;
default logs remain quiet.

**Verification:** EGL renderer unit tests and `cargo check --workspace` pass.

**Dependencies:** Task 3

### Checkpoint: Cache

- [ ] Renderer cache focused tests pass.
- [ ] Explicit-sync and dmabuf release tests pass.

## Phase 3: Resize Transaction Progression

### Task 5: Add failing serial and client-geometry resize tests

**Acceptance criteria:** Tests cover cell-aligned and exact sizes,
commit-before-ACK, stale ACK, configure bursts, actual-size anchoring,
geometry-only state, intermediate bufferless commit, viewport, SHM/dmabuf
parity, and preview cleanup.

**Verification:** focused compositor window tests fail only for missing rules.

**Dependencies:** None

### Task 6: Implement serial-safe, actual-geometry acceptance

**Acceptance criteria:** A valid post-ACK commit clears pending state without
exact size equality; older ACKs cannot replace newer state; left/top anchors
use actual dimensions; irrelevant bufferless commits defer.

**Verification:** focused resize tests pass.

**Dependencies:** Task 5

### Checkpoint: Resize

- [ ] Window, viewport, popup, maximize/fullscreen, and minimize tests pass.
- [ ] No exact-size gate remains in interactive resize completion.

## Phase 4: Rendering Regression And Documentation

### Task 7: Add buffer replacement invalidation and Kitty-style regression

**Acceptance criteria:** Buffer IDs participate in visual identity; replacement
forces safe damage/history handling; fake A/B/C/D swapchain sequence selects
new resources in order and leaves no resize/cache/release leaks.

**Verification:** focused integration regression passes.

**Dependencies:** Tasks 3 and 6

### Task 8: Update architecture documentation and run full validation

**Acceptance criteria:** Stable identity and resize semantics are documented;
all requested format/check/clippy/test/release/diff commands pass or a
base-commit unrelated failure is reproduced and recorded.

**Verification:** the complete command matrix from TASK 05.1.

**Dependencies:** Task 7

## Risks And Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Destroying an image while still selected | stale texture or GL error | transfer single ownership and evict only weak-expired inactive entries |
| ACK ordering accepts the wrong configure | misplaced window | monotonic serial comparison and explicit acknowledged transaction tests |
| Geometry-only commit ambiguity | pending resize leaks or early completion | commit-associated geometry-change flag and bufferless defer test |
| Buffer replacement bypasses partial damage | stale pixels | include `BufferId` in signatures and invalidate repaint history |
| Hardware-only behavior differs | unverified Kitty result | portable fake regression plus explicit real-TTY validation report |

## Final Checkpoint

- [ ] Focused and workspace tests pass.
- [ ] Cache/resource counts remain bounded in deterministic stress tests.
- [ ] Manual legacy/atomic application matrix is reported accurately as run or
      untested when no real TTY/hardware session is available.
