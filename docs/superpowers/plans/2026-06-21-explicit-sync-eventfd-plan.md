# Implementation Plan: Event-Driven Explicit Sync Acquire Points

## Overview

Replace native refresh-driven acquire polling with exact-commit syncobj eventfd
watches, while retaining a bounded absolute-deadline fallback for unsupported
DRM implementations. Preserve current buffer release and pageflip completion
ordering.

## Architecture Decisions

- The compositor owns commit state and exact commit IDs; native code owns
  eventfds, epoll registrations, backend generation, fallback timing, and watch
  metrics.
- Native syncobj imports use a duplicate of the active DRM file description.
- The reactor uses slot-plus-generation tokens so fd and slot reuse cannot
  transfer readiness identity.
- Acquire readiness and commit promotion are separate transitions. A ready
  commit is not promoted while an older pageflip remains pending.
- Eventfd and fallback watches are separate sets; only fallback entries are
  scanned on their dedicated deadline.

## Phase 1: Foundations

### Task 1: Add the audited syncobj eventfd UAPI boundary

**Description:** Add typed request construction, ioctl registration, capability
classification, and active-DRM device construction to `syncobj.rs`.

**Acceptance criteria:**

- [ ] Layout, point width, flags, padding, fd validation, and errno tests fail
      before implementation and pass afterward.
- [ ] Signal notification never uses `WAIT_AVAILABLE`.
- [ ] Active-native device duplication preserves owned-fd lifetime.

**Verification:** `cargo test syncobj::tests`

**Dependencies:** None

**Files likely touched:** `src/syncobj.rs`

**Estimated scope:** Medium

### Task 2: Make reactor registrations dynamic and generation-safe

**Description:** Introduce `ReactorToken`, dynamic add/remove, explicit acquire
ready events, stale classification, and token overflow handling.

**Acceptance criteria:**

- [ ] Removed and reused slots reject stale queued tokens.
- [ ] Numeric fd reuse cannot reuse token identity.
- [ ] Existing fixed-source wake behavior remains unchanged.

**Verification:** `cargo test native::event_loop::tests`

**Dependencies:** None

**Files likely touched:** `src/native/event_loop.rs`

**Estimated scope:** Medium

### Checkpoint: Foundations

- [ ] Foundation tests pass.
- [ ] `cargo check --workspace` passes.

## Phase 2: Exact Commit Lifecycle

### Task 3: Add acquire commit IDs and lifecycle transitions

**Description:** Represent pending commits by exact ID and readiness state;
provide native watch request/cancel/ready APIs and polling-mode compatibility.

**Acceptance criteria:**

- [ ] Exact ready transitions are at-most-once and reject mismatches.
- [ ] A newer surface commit supersedes the older blocked commit.
- [ ] Cancellation never signals release or buffer completion.

**Verification:** targeted compositor explicit-sync lifecycle tests

**Dependencies:** Task 1

**Files likely touched:**

- `src/compositor/explicit_sync.rs`
- `src/compositor/mod.rs`
- `src/compositor/server.rs`
- `src/compositor/protocols/syncobj.rs`
- `src/compositor/tests/protocol_buffers.rs`

**Estimated scope:** Medium

### Task 4: Cover destruction and disconnect cancellation

**Description:** Connect surface, buffer, sync-surface, timeline, client, and
server teardown paths to exact pending-commit cancellation.

**Acceptance criteria:**

- [ ] Every required destruction scope removes blocked commits.
- [ ] Duplicate destruction/cancellation is harmless.
- [ ] Presentation, frame callback, and release behavior is not fabricated.

**Verification:** targeted compositor destruction tests

**Dependencies:** Task 3

**Files likely touched:**

- `src/compositor/mod.rs`
- `src/compositor/protocols/core.rs`
- `src/compositor/protocols/buffers.rs`
- `src/compositor/protocols/syncobj.rs`
- `src/compositor/tests/protocol_buffers.rs`

**Estimated scope:** Medium

### Checkpoint: Commit Lifecycle

- [ ] Compositor tests pass with native-watch mode and polling mode.
- [ ] Release lifecycle regressions pass.

## Phase 3: Native Watch Registry

### Task 5: Implement eventfd-backed watch registry

**Description:** Add the dedicated registry with injected notifier support,
owned eventfds, exact indexes, race-proof registration, event draining,
defensive validation, cancellation, metrics, and shutdown.

**Acceptance criteria:**

- [ ] Setup races and already-signaled points cannot remain blocked.
- [ ] Duplicate, stale, backend-mismatched, and fd-reused events cannot release
      another commit.
- [ ] Registry shutdown leaves zero watches and registrations.

**Verification:** `cargo test native::explicit_sync::tests`

**Dependencies:** Tasks 1-3

**Files likely touched:**

- `src/native/explicit_sync.rs`
- `src/native/mod.rs`
- `src/native/event_loop.rs`

**Estimated scope:** Medium

### Task 6: Implement bounded fallback state machine

**Description:** Add immediate checks, absolute retry deadlines, bounded
backoff, disarming, and separate fallback metrics.

**Acceptance criteria:**

- [ ] No fallback deadline exists without fallback entries.
- [ ] Timeout alone never marks readiness.
- [ ] Supported eventfd watches are not scanned by fallback retries.

**Verification:** fallback-focused registry tests

**Dependencies:** Task 5

**Files likely touched:** `src/native/explicit_sync.rs`

**Estimated scope:** Small

### Checkpoint: Registry

- [ ] Registry and reactor suites pass.
- [ ] `cargo check --workspace` passes.

## Phase 4: Runtime Integration

### Task 7: Bind native explicit sync to the active DRM backend

**Description:** Replace the pre-discovered device before GPU globals are
advertised and establish the backend generation used by every watch.

**Acceptance criteria:**

- [ ] Native imports and eventfd ioctls use the active DRM file description.
- [ ] Backend teardown cancels watches before DRM ownership drops.
- [ ] Non-native device discovery remains functional.

**Verification:** native setup unit checks and `cargo check --workspace`

**Dependencies:** Tasks 1 and 5

**Files likely touched:**

- `src/compositor/server.rs`
- `src/native_output.rs`

**Estimated scope:** Small

### Task 8: Route watches through the native event cycle

**Description:** Process compositor watch changes after Wayland/input dispatch,
apply acquire readiness before scheduling, defer promotion behind outstanding
pageflips, and arm the earliest scheduler/fallback deadline.

**Acceptance criteria:**

- [ ] Supported readiness no longer relies on refresh polling.
- [ ] Readiness queues work immediately without submitting a second pageflip.
- [ ] Existing Wayland/input/pageflip ordering remains intact.

**Verification:** native scheduler/reactor integration tests

**Dependencies:** Tasks 3, 5-7

**Files likely touched:**

- `src/native_output.rs`
- `src/native/scheduler.rs`
- `src/compositor/server.rs`

**Estimated scope:** Medium

### Task 9: Add structured performance diagnostics

**Description:** Expose registry counters and latency samples through existing
native performance logging without normal per-frame info logs.

**Acceptance criteria:**

- [ ] Required watch, fallback, stale, cancellation, failure, and latency
      counters are observable.
- [ ] Identity logs include internal token and backend generation context.
- [ ] Leak assertion count remains zero in tests and shutdown.

**Verification:** diagnostic field unit tests and trace inspection

**Dependencies:** Tasks 5 and 8

**Files likely touched:**

- `src/native/explicit_sync.rs`
- `src/native_output.rs`

**Estimated scope:** Small

### Checkpoint: Runtime

- [ ] Native focused tests pass.
- [ ] No supported-path global acquire scan remains.
- [ ] `cargo check --workspace` passes.

## Phase 5: Documentation And Verification

### Task 10: Update native documentation

**Description:** Document cycle ordering, supported and fallback behavior,
diagnostics, lifecycle, hardware restrictions, and known limitations.

**Acceptance criteria:**

- [ ] Architecture and native-session docs match delivered behavior.
- [ ] Known issues no longer describe refresh polling as the normal path.
- [ ] Explicit-sync research records actual implementation boundaries.

**Verification:** documentation diff review and `git diff --check`

**Dependencies:** Tasks 7-9

**Files likely touched:**

- `docs/ARCHITECTURE.md`
- `docs/NATIVE_SESSION.md`
- `docs/KNOWN_ISSUES.md`
- `docs/research/native-explicit-sync-eventfd.md`

**Estimated scope:** Small

### Task 11: Run complete quality gates

**Description:** Run focused and workspace verification, reproduce any
suspected baseline failure on untouched `f22aa20`, and record unavailable
hardware scenarios honestly.

**Acceptance criteria:**

- [ ] Format, check, clippy, tests, release build, and diff checks complete.
- [ ] Any baseline exception is reproduced on untouched HEAD.
- [ ] Hardware validation is reported only if actually performed.

**Verification:**

- `cargo fmt --check`
- `cargo check --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo build --release --workspace`
- `git diff --check`

**Dependencies:** Tasks 1-10

**Files likely touched:** None

**Estimated scope:** Medium

## Risks And Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Future callbacks attach to an older pending pageflip | High | Separate acquire-ready from commit promotion and defer promotion while a flip is pending |
| DRM file mismatch between import and notifier | High | Replace native syncobj device with a duplicate of the active DRM file description |
| Queued epoll events alias reused fds or slots | High | Slot-plus-generation tokens and exact registry lookup |
| Driver returns ambiguous ioctl errors | Medium | Preserve errno, classify narrowly, keep commits blocked, and expose diagnostics |
| Destruction path misses pending state | High | Central exact cancellation API plus focused tests for every resource scope |
| Fallback recreates idle polling | High | One absolute deadline only while fallback entries exist; disarm at zero |
| Real DRM behavior cannot run in CI | Medium | Inject notifier/readiness behavior and keep hardware tests conditional |

## Open Questions

None. Hardware availability is a verification constraint, not an implementation
decision.
