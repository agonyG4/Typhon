# Modern XWayland Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline execution) to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Build a disabled-by-default, generation-safe Modern XWayland foundation owned by `NativeRuntime`, without implementing an XWM or enabling X11 fallback for normal applications.

**Architecture:** `ChildSupervisor` owns stable process identities and verified process groups; `XwaylandService` owns a long-lived authenticated display lease and one generation-specific launch at a time. The native reactor preserves exact XWayland tokens and flags, while `OwnCompositorServer` authorizes a private client by exact `ClientId` and exposes only commit-latched `xwayland-shell-v1` association events.

**Tech Stack:** Rust 2024, `std::process::Command`, Linux `epoll`/Unix sockets/signalfd, `wayland-server` 0.31, staging `wayland-protocols` 0.32, existing NativeRuntime reactor and compositor state.

## Global Constraints

- Keep `TYPHON_XWAYLAND=off` as the default and keep normal app launches Wayland-only.
- Production XWayland spawning must use `Command` directly; no shell command strings or `/bin/sh -c`.
- Every session-owned XWayland child uses a dedicated verified process group.
- All child descriptors are owned typed FDs until the spawn boundary.
- Every start gets a new nonzero generation; stale tokens, exits, readiness, and client notifications are ignored.
- `RunningBase` requires both validated `displayfd` and the exact private-client `xwayland-shell-v1` bind.
- Do not add XWM, X11 window management, `WL_SURFACE_ID`, clipboard, DnD, RandR, scaling, renderer, KMS, scheduler, or scanout branches.
- New modules must pass `./bin/check-source-layout` without an allowlist exception.
- Follow TDD: each behavior begins with a failing test, then minimal implementation, then refactor while green.
- Commit each phase with the exact requested message where specified.

---

## Task 1: Process identity and explicit process groups

**Files:**
- Modify: `src/process.rs`
- Test: `src/process.rs` unit tests under `#[cfg(test)]`

**Interfaces:**
- Produce `ManagedProcessId`, `SpawnedProcess`, `ProcessGroupPolicy::Inherit|Dedicated`, and `ProcessKind::Xwayland`.
- Change `ChildExit` to include `id`, `pid`, `pgid`, `kind`, `status`, and `restarted` identity while retaining compatibility accessors for existing PID callers.
- Make `ChildSupervisor::spawn` return `SpawnedProcess`; add lookup by `ManagedProcessId` and preserve a PID helper for existing callers.

- [ ] **Step 1: Write failing identity and process-group tests.** Add tests that spawn a dedicated Linux child, assert `pgid == pid`, assert two sequential identities differ even when the old PID is absent, and assert `ProcessGroupPolicy::Dedicated` rejects `session_owned == false`.
- [ ] **Step 2: Run the focused tests and verify failure.** Run `cargo test --locked process::tests::dedicated_process_has_own_group -- --nocapture`; expect missing policy/identity APIs.
- [ ] **Step 3: Implement stable identity allocation and group policy.** Add a monotonic nonzero counter to `ChildSupervisor`, store entries by `ManagedProcessId` with PID/PGID indexes, validate PID/PGID conversions, and use Unix `CommandExt::process_group(0)` for dedicated children with a documented `// SAFETY:` comment only where a libc fallback is necessary.
- [ ] **Step 4: Run process tests and the full unit target.** Run `cargo test --locked process::tests -- --nocapture` and `cargo test --locked --lib`; both must pass.
- [ ] **Step 5: Commit the process identity slice.**
  ```bash
  git add src/process.rs
  git commit -m "fix(process): own session process groups and fatal cleanup"
  ```

## Task 2: Typed inherited FDs and bounded cleanup

**Files:**
- Modify: `src/process.rs`
- Test: `src/process.rs` Linux tests

**Interfaces:**
- Produce `ChildFdMapping`, `SpawnCommand`, deterministic mapping validation, `begin_emergency_cleanup`, `kill_session_owned_now`, `BootstrapChildGuard`, and nonblocking final `Drop` cleanup.

- [ ] **Step 1: Write failing FD and cleanup tests.** Cover mapped target visibility, no inheritance of unrelated CLOEXEC FDs, source/target aliasing, failed-spawn RAII closure, child-plus-grandchild termination for orderly and emergency cleanup, unrelated application survival, quiesce preventing critical restart, and bootstrap guard cleanup.
- [ ] **Step 2: Run the tests and verify the failures are feature failures.** Run `cargo test --locked process::tests::mapped_inherited_fd -- --nocapture` and `cargo test --locked process::tests::emergency_cleanup -- --nocapture`.
- [ ] **Step 3: Implement `SpawnCommand`.** Store `Command` plus owned `ChildFdMapping` sources; reject duplicate targets and invalid descriptors; in `pre_exec`, duplicate sources to fixed targets with `dup2`, clear `FD_CLOEXEC` only on targets, close unrelated temporary descriptors, and preserve aliases by duplicating all sources before closing any source target.
- [ ] **Step 4: Implement cleanup semantics.** Quiesce restart before signaling; signal stored PGIDs for session-owned entries and direct PIDs only for session-owned entries without a group; reap with `try_wait` until the bounded grace deadline; make `Drop` issue nonblocking SIGKILL only as a last safety net and never call `wait`.
- [ ] **Step 5: Verify and commit.** Run `cargo test --locked process::tests -- --nocapture`, `cargo fmt --check`, and `cargo clippy --locked --all-targets -- -D warnings`; commit with the Part A message if not already committed as one atomic process commit.

## Task 3: Focused XWayland module boundary and mode/generation types

**Files:**
- Delete: `src/xwayland.rs`
- Create: `src/xwayland/mod.rs`, `src/xwayland/config.rs`, `src/xwayland/generation.rs`, `src/xwayland/metrics.rs`, `src/xwayland/tests.rs`
- Modify: `src/lib.rs` exports/tests

**Interfaces:**
- Publicly expose only `XwaylandGeneration`, `XwaylandMode`, `XwaylandStateKind`, `XwaylandAppEnvironment`, and `XwaylandService` runtime-facing methods.
- Keep launch-plan construction private; retain a compatibility test only where the existing public API requires an intentional migration.

- [ ] **Step 1: Write failing parsing and generation tests.** Assert absent/unknown mode is `Off`, `base` is `BaseLazy`, `eager` is `BaseEager`, binary override is honored, unknown mode emits a diagnostic, and every allocated generation is nonzero and distinct.
- [ ] **Step 2: Run focused tests and verify failure.** Run `cargo test --locked xwayland::tests::mode -- --nocapture`.
- [ ] **Step 3: Replace the stub with focused modules.** Define private state/resource modules and public re-exports in `mod.rs`; implement `NonZeroU64` generation allocation and bounded config parsing without reading host `DISPLAY`.
- [ ] **Step 4: Run compile and source-layout checks.** Run `cargo test --locked xwayland`, `cargo check --locked --all-targets`, and `./bin/check-source-layout`.
- [ ] **Step 5: Commit.**
  ```bash
  git add src/xwayland.rs src/xwayland src/lib.rs
  git commit -m "refactor(xwayland): establish isolated service modules"
  ```

## Task 4: Authenticated display lease

**Files:**
- Create: `src/xwayland/display.rs`, `src/xwayland/auth.rs`
- Modify: `src/xwayland/config.rs`, `src/xwayland/mod.rs`
- Test: `src/xwayland/tests.rs`

**Interfaces:**
- Produce `DisplayLease` with display number/string, lock identity, filesystem listener, abstract listener, Xauthority path/cookie owner, and exact-lease cleanup.
- Produce a parseable local Xauthority record writer with no external `xauth` dependency.

- [ ] **Step 1: Write failing lease tests.** Cover unique allocation, live lock skip, safe stale lock recovery, symlink rejection, rollback after partial failure, both socket forms accepting connections, mode `0700` runtime directory, mode `0600` auth file, nonempty 128-bit cookie, exact artifact removal, and long-lived artifacts surviving generation teardown.
- [ ] **Step 2: Run the display tests and verify failure.** Run `cargo test --locked xwayland::tests::display_lease -- --nocapture`.
- [ ] **Step 3: Implement safe lock/socket allocation.** Scan `0..=63` or test-configured bounds; create lock with `O_CREAT|O_EXCL|O_CLOEXEC|O_NOFOLLOW`, record the current PID, verify `/tmp/.X11-unix` is a non-symlink directory, bind/listen filesystem and Linux abstract sockets before launch, and roll back all already-created artifacts on error.
- [ ] **Step 4: Implement Xauthority.** Create `$XDG_RUNTIME_DIR/typhon/xwayland/` with `0700`, generate at least 16 random cookie bytes using the OS source, write the MIT-MAGIC-COOKIE-1 binary record for local connections with `0600`, and retain cleanup ownership in the lease.
- [ ] **Step 5: Verify and commit.** Run `cargo test --locked xwayland -- --nocapture`, `git diff --check`, and `./bin/check-source-layout`; commit `feat(xwayland): add authenticated display lease`.

## Task 5: Direct XWayland launch resources and service state machine

**Files:**
- Create: `src/xwayland/launch.rs`, `src/xwayland/service.rs`, `src/xwayland/association.rs`, `src/xwayland/protocol.rs`
- Modify: `src/xwayland/mod.rs`, `src/process.rs` interfaces as required
- Test: `src/xwayland/tests.rs`

**Interfaces:**
- Produce `XwaylandState`, `ArmedState`, `StartingState`, `RunningBaseState`, `BackoffState`, `FailedState`, `XwaylandReactorRegistration`, and the required `XwaylandService` methods.
- Define one `ChildFdTarget` table for `WAYLAND_SOCKET`, WM, displayfd, filesystem listener, and abstract listener.
- Produce launch commands with `-rootless -terminate -nolisten tcp -listenfd ... -displayfd ... -wm ... -auth ...`, sanitized environment, dedicated process group, and no supervisor restart factory.

- [ ] **Step 1: Write failing service tests.** Cover off/no lease, base armed/no process, eager sharing the same start path, one start for simultaneous listeners, fresh generation resources, valid displayfd alone, authorized shell bind alone, both readiness orders, malformed/oversized/wrong/zero/negative readiness, startup timeout, stale generation events, clean exit rearming, abnormal backoff, crash budget, failed state, and cleanup with no resource growth.
- [ ] **Step 2: Run the tests and confirm failure.** Run `cargo test --locked xwayland::tests::service -- --nocapture`.
- [ ] **Step 3: Implement launch resources.** Build private `UnixStream::pair()` objects, a WM pair, displayfd pipe, duplicated listener sources, startup deadline, and typed spawn mappings. Set only `WAYLAND_SOCKET` to the deterministic private child target, remove `WAYLAND_DISPLAY`, `DISPLAY`, and `XAUTHORITY`, and pass the lease auth path as an argument.
- [ ] **Step 4: Implement readiness transaction.** Read bounded ASCII displayfd until newline/EOF, validate exactly the lease display, set only the display-ready bit, accept shell-ready only for the active generation/client, and transition to `RunningBase` only when both bits are set.
- [ ] **Step 5: Implement exits, deadlines, and backoff.** Treat clean `-terminate` exit as rearm without budget consumption; schedule 250 ms, 1 s, and 4 s abnormal backoffs; count three crashes in ten minutes; enter `Failed` after budget exhaustion while retaining but not rearming listeners; ignore and count all stale events.
- [ ] **Step 6: Verify and commit.** Run `cargo test --locked xwayland -- --nocapture`, `cargo check --locked --all-targets`, and commit `feat(xwayland): add lazy generation-bound launcher`.

## Task 6: Reactor event retention and NativeRuntime ownership

**Files:**
- Modify: `src/native/event_loop.rs`, `src/native_output/mod.rs`, `src/native_output/runtime/mod.rs`, `src/native_output/runtime/bootstrap.rs`, `src/native_output/runtime/cycle.rs`, `src/native_output/runtime/metrics.rs`, `src/native_output/runtime/session_io.rs`, `src/native_output/runtime/shutdown_cycle.rs`
- Test: `src/native/event_loop.rs`, `src/native_output/runtime/*` existing test modules

**Interfaces:**
- Produce `NativeEventSource::XwaylandListen|XwaylandDisplayReady`, `XwaylandReadyEvent { token, flags }`, and `NativeWakeup::xwayland_events`.
- Add `xwayland: XwaylandService` to `NativeRuntime`, route exact events and `ChildExit` values before generic launch tracking, include service deadlines, and call service cleanup in orderly/fatal paths.

- [ ] **Step 1: Write failing reactor tests.** Assert exact token and epoll flag retention, stale token rejection after unregister/reuse, normal displayfd HUP/RDHUP delivery, and deadline participation in earliest native deadline selection.
- [ ] **Step 2: Run focused reactor/runtime tests and verify failure.** Run `cargo test --locked native::event_loop -- --nocapture`.
- [ ] **Step 3: Implement event retention without collapsing stale protection.** Add source bits and an event vector containing both token and flags; route displayfd error flags to the service while retaining fatal behavior for unrelated sources.
- [ ] **Step 4: Integrate runtime ownership.** Bootstrap service after event loop and supervisor creation, register/unregister service tokens, dispatch service events before app launch, route process exits once, include deadlines, and wrap `run()` errors with explicit emergency cleanup before drop.
- [ ] **Step 5: Verify and commit.** Run `cargo test --locked native::event_loop`, native runtime tests, `cargo fmt --check`, and commit `feat(native): integrate XWayland foundation into reactor`.

## Task 7: Private Wayland client identity and protocol global

**Files:**
- Modify: `src/compositor/server.rs`, `src/compositor/state_data.rs`, `src/compositor/state/client_lifecycle.rs`, `src/compositor/protocols/globals.rs`, `src/compositor/protocols.rs`, `src/compositor/protocols/versions.rs`, `src/compositor/plan.rs`
- Create: `src/compositor/protocols/xwayland_shell.rs`
- Test: `src/compositor/tests/` protocol/lifecycle support and `src/xwayland/tests.rs`

**Interfaces:**
- Produce `XwaylandClientIdentity { client_id, generation }`, server insertion/lookup/revocation APIs, one version-1 `xwayland_shell_v1` global, and a readiness event from an authorized exact-client bind.

- [ ] **Step 1: Write failing authorization tests.** Cover normal-client invisibility, forged same-UID denial, exact private-client visibility/bind, one readiness event per generation, and old-generation revocation after restart.
- [ ] **Step 2: Run protocol tests and verify failure.** Run `cargo test --locked compositor::tests::lifecycle -- --nocapture` with the new test filter.
- [ ] **Step 3: Implement private client insertion.** Accept the compositor side of the private socket, insert it through the existing `DisplayHandle` path, record exact `ClientId` and generation, and use normal disconnect cleanup.
- [ ] **Step 4: Implement global visibility and dispatch.** Use staging `wayland_protocols` if its generated module exists; otherwise vendor only the exact upstream XML with its license and document the dependency gap. Hide the global from all other clients, defensively reject unauthorized binds, and notify the service on the exact bind.
- [ ] **Step 5: Verify and commit.** Run targeted compositor protocol tests, `cargo check --locked --all-targets`, and commit `feat(xwayland): add private shell association protocol` after the association behavior in Task 8 is green.

## Task 8: Surface role, commit-latched association registry, and environment

**Files:**
- Modify: `src/compositor/state/roles.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/surface_commits.rs`, `src/compositor/state/client_lifecycle.rs`, `src/launch_env.rs`, `src/xwayland/association.rs`, `src/xwayland/mod.rs`
- Test: `src/compositor/tests/protocol_error.rs`, `src/compositor/tests/lifecycle.rs`, `src/xwayland/tests.rs`, `src/launch_env.rs`

**Interfaces:**
- Produce `SurfaceRole::Xwayland`, `PermanentSurfaceRole::Xwayland`, `LiveRoleInstance::Xwayland`, `XwaylandSurfaceState`, `AssociationRegistry`, and normalized `XwaylandAssociationEvent` values.
- Extend `XwaylandAppEnvironment` and `X11Bridge::IsolatedXWayland` with `xauthority`; add explicit diagnostic/test environment application without changing default launch behavior.

- [ ] **Step 1: Write failing protocol/registry tests.** Cover role assignment/conflict, zero serial, exact high/low reconstruction, pending invisibility before commit, one committed event, already-associated rejection, destroy-before/after-commit semantics, surface/disconnect cleanup, generation reuse without collision, and environment removal of host `DISPLAY`, `XAUTHORITY`, and stale bridge variables.
- [ ] **Step 2: Run tests and verify failure.** Run `cargo test --locked xwayland -- --nocapture` and the focused compositor protocol-error tests.
- [ ] **Step 3: Implement role and protocol state.** Add permanent/live XWayland role variants and wire role-error handling through existing role lifecycle helpers; store pending/committed serial state per surface and retire stale-generation pending state.
- [ ] **Step 4: Implement `AssociationRegistry`.** Index by generation+nonzero serial and surface ID, reject duplicate committed association, emit normalized committed/removed events, and implement `surface_for_serial`, `serial_for_surface`, `remove_surface`, `clear_generation`, and `take_events`.
- [ ] **Step 5: Implement diagnostic environment sanitization and metrics.** Reuse `X11Bridge`, export only service-owned `DISPLAY`/`XAUTHORITY` for explicit opt-in callers, keep Wayland-only routing otherwise, and log structured non-secret state transitions, generations, display number, readiness failures, backoff, stale events, unauthorized binds, associations, and cleanup results.
- [ ] **Step 6: Verify and commit.** Run all targeted suites and commit `feat(xwayland): publish isolated diagnostic environment`.

## Task 9: Full validation and optional integration test

**Files:**
- Modify: `src/xwayland/tests.rs` or add an ignored integration test beside existing native tests only if the installed binary is discoverable
- Modify: documentation only if the final API needs a short compatibility note

- [ ] **Step 1: Run repeated targeted suites.**
  ```bash
  cargo test --locked process::tests -- --nocapture
  cargo test --locked xwayland -- --nocapture
  cargo test --locked native::event_loop -- --nocapture
  cargo test --locked compositor::tests -- --nocapture
  ```
- [ ] **Step 2: Run the complete validation gate.**
  ```bash
  cargo fmt --check
  cargo check --locked --all-targets
  cargo clippy --locked --all-targets -- -D warnings
  cargo test --locked
  ./bin/check-source-layout
  git diff --check
  ```
- [ ] **Step 3: Add and run only an ignored opt-in real-Xwayland test when `Xwayland` is installed.** Allocate a lease, trigger lazy startup, observe displayfd and private-client identity, verify exact global visibility, terminate the process group, and assert lock/socket/auth cleanup. The normal suite must not depend on it.
- [ ] **Step 4: Review scope and final history.** Confirm no `WL_SURFACE_ID`, XWM, clipboard/DnD, RandR, scaling, renderer/KMS branches, host-display fallback, or default X11 routing exists; record `git log --oneline` and final validation output.

## Checkpoints

### Checkpoint A: After Tasks 1-2

- Process tests pass on Linux.
- Dedicated groups clean descendants without killing normal applications.
- No mapped FD leaks remain after failed spawn.

### Checkpoint B: After Tasks 3-5

- Focused XWayland modules compile and source-layout gate passes.
- Display lease artifacts are authenticated and RAII-owned.
- Service remains disabled by default and has generation-safe readiness/backoff.

### Checkpoint C: After Tasks 6-8

- Native runtime owns service lifecycle and exact reactor events.
- Private client is the only shell-global authority.
- Association semantics are fully commit-latched and generation-indexed.

### Checkpoint D: Final

- All validation commands pass.
- Wayland-native behavior remains green.
- Product status is explicitly: Modern XWayland foundation implemented; XWM and X11 window management not implemented yet.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Existing supervisor callers assume PID return values | High | Add typed identity return plus explicit PID compatibility helpers and update callers incrementally. |
| `wayland-protocols` generated staging module shape differs | High | Inspect pinned crate before coding; use its generated server module or vendor only the exact upstream XML with license. |
| `wayland-server` private client insertion needs existing display state wiring | High | Reuse `OwnCompositorServer` insertion and disconnect paths; keep identity map separate from UID/PID authorization. |
| `/tmp` display artifacts are concurrently manipulated | High | `O_NOFOLLOW`, exact lease ownership, live-PID/socket checks, rollback, and never unlink paths not created by this lease. |
| Reactor HUP handling currently treats all source errors as fatal | Medium | Special-case only XWayland displayfd HUP/RDHUP and preserve existing fatal behavior elsewhere. |
| NativeRuntime bootstrap is large and tightly coupled | Medium | Add the service at the narrow bootstrap/event/shutdown seams and do not alter render/KMS code. |
