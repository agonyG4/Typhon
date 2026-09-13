# Typhon dmem Hardening and Application Scopes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden native foreground dmem transitions and isolate normal Typhon-launched applications in transient systemd user scopes without changing `ChildSupervisor` ownership.

**Architecture:** Keep the existing injected-root dmem resolver, cached-target controller, and latest-wins worker. Add transactional per-region mutation with pending cleanup and generation gates, then add a testable `application_scope` module that wraps only `ProcessKind::Application` argv in an internal Typhon helper. The helper performs bounded user-bus registration and cgroup verification before `exec`.

**Tech Stack:** Rust 2024, existing `zbus` async-io dependency, `libc` pipes/fcntl/open, systemd user manager `StartTransientUnit`, cgroup v2 procfs, existing `ChildSupervisor` and native doctor protocol.

## Global Constraints

- Use the existing local repository checkout and target directory for all Cargo builds/tests.
- Do not add KDE, Plasma, KCGroups, Gamescope, GameMode, NVML, privileged helpers, root daemons, or external `systemd-run` processes.
- Do not perform systemd, D-Bus, procfs, or cgroupfs work on compositor/input/render hot paths.
- Preserve structural argv/environment/stdio/working-directory/process-group behavior and `ChildSupervisor` PID/reap ownership.
- `OBLIVION_ONE_APP_SCOPES=auto|on|off`, default `auto`; all scope failures fall back to direct application `exec`.
- Use `/sys/fs/cgroup/dmem.capacity` as the sole managed-region authority; write one region/value operation per fresh `dmem.low` open.
- Preserve cached resolved cgroups for revert; never re-resolve old PIDs during cleanup.
- Keep unrelated working-tree changes unstaged and commit only task-owned files.

---

### Task 1: Establish dmem transactional test seams

**Files:**
- Modify: `src/native/dmem_foreground.rs`
- Create: `src/native/dmem_foreground_tests.rs`

**Interfaces:**
- Preserve `DmemLowWriter`, `ForegroundController`, `TransitionResult`, and public dmem snapshots.
- Add a controller reconciliation path that accepts a generation/stopping predicate and can report a stale transition without publishing a current target.
- Keep rollback errors bounded while preserving the first mutation error.

- [ ] **Step 1: Move the existing inline unit tests to the dedicated test module.**

Replace the inline `#[cfg(test)] mod tests { ... }` block with:

```rust
#[cfg(test)]
#[path = "dmem_foreground_tests.rs"]
mod tests;
```

Copy the existing deterministic fixtures and tests unchanged into
`src/native/dmem_foreground_tests.rs`, retaining `use super::*;`. This keeps
behavior stable while bringing the production file under the documented
source-size limit.

- [ ] **Step 2: Run the existing focused dmem tests and verify the extraction is green.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: the moved tests pass before any behavior change.

- [ ] **Step 3: Add a failing partial-apply regression test.**

Add a writer that fails on a configured write number and assert that applying
`region-a` succeeds, `region-b` fails, `region-a` is then written as zero, and
`controller.current_path()` remains `None`.

- [ ] **Step 4: Add a failing cleanup-continuation regression test.**

Configure the writer to fail the first zero write and assert that zero writes
for the remaining managed regions are still attempted and the first cleanup
error is returned.

- [ ] **Step 5: Run the two new tests to confirm they fail for the current non-transactional implementation.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: the tests fail because `apply_low_entries` returns without tracking
written regions and the cleanup helper stops reporting after its first error.

- [ ] **Step 6: Commit only the test extraction and red tests.**

```bash
rtk git add src/native/dmem_foreground.rs src/native/dmem_foreground_tests.rs
rtk git commit -m "test: cover dmem transactional cleanup failures"
```

### Task 2: Implement dmem rollback, stale generations, and PID identity guards

**Files:**
- Modify: `src/native/dmem_foreground.rs`
- Modify: `src/native/dmem_foreground_tests.rs`

**Interfaces:**
- Add `TransitionResult::Stale` or an equivalent private outcome used by the worker.
- Add cached pending cleanup separate from `current` so a failed partial target is never reported current but can be retried safely.
- Add `ProcCgroupResolver` start-time revalidation before returning `ResolvedCgroup`.

- [ ] **Step 1: Implement best-effort zeroing that attempts every requested region.**

Update `revert_low_entries` to retain the first `DmemError` while continuing
through all regions. Treat `NotFound` as successful terminal cleanup only for
reverts. Add a helper that converts a failed capacity write into a rollback of
every region touched, including the region whose write returned the error.

- [ ] **Step 2: Implement controller pending cleanup and transactional apply.**

When a target apply fails or becomes stale, cache its resolved cgroup in a
pending-cleanup field, zero the touched regions, leave `current` as `None`,
and return the first meaningful error. Before any future desired-target apply,
finish pending cleanup; do not write a new target while cleanup remains
unproven. Keep ordinary current-target cleanup cached until all managed regions
are successfully zeroed.

- [ ] **Step 3: Add generation checks before each capacity write.**

Implement `reconcile_with(desired, should_continue)` and call the predicate
before every capacity mutation. On a false result, stop starting new writes,
zero all touched regions, and return the stale outcome. Use the existing public
`reconcile` method with an always-true predicate for deterministic controller
tests.

- [ ] **Step 4: Add the stale-between-regions test and run it.**

Use a controllable writer that publishes a newer generation after the first
capacity write. Assert that no later stale capacity write occurs, the first
entry is zeroed, and the controller has no current target.

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: the stale test passes after the controller implementation and the
partial-write tests are green.

- [ ] **Step 5: Add the PID reuse red test.**

Add a resolver metadata helper test that supplies equal numeric PID/UID data,
an initial stat start time, a valid cgroup, and a changed final stat start
time. Assert resolution fails with a stale identity error and no writer call
can be made from that resolution.

- [ ] **Step 6: Implement start-time revalidation.**

Read initial `/proc/<pid>/stat`, then read status/cgroup and validate target,
check writable `dmem.low`, and finally read `/proc/<pid>/stat` again. Require
the final start time to equal the initial value before returning. Treat
disappearance or mismatch as a transient/stale resolution failure.

- [ ] **Step 7: Add actionable same-Typhon-cgroup diagnostics.**

Map `Typhon cgroup` and `ancestor of Typhon cgroup` resolution failures to the
bounded message:

```text
focused application shares Typhon's cgroup; application scope isolation is unavailable or was bypassed
```

Keep the safety rejection unchanged.

- [ ] **Step 8: Run focused dmem tests and commit the hardening.**

Run: `rtk cargo test native::dmem_foreground --locked`

```bash
rtk git add src/native/dmem_foreground.rs src/native/dmem_foreground_tests.rs
rtk git commit -m "fix: harden foreground dmem transitions and identity checks"
```

### Task 3: Add the internal application-scope policy and helper

**Files:**
- Create: `src/application_scope.rs`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produce `ApplicationScopePolicy::{Auto, On, Off}` with `parse`, `from_env`, and `as_str`.
- Produce `wrap_application_argv(real_argv, executable) -> Result<Vec<OsString>, ScopeError>` with bounded structural argv.
- Produce `run_internal_scope_exec(args: &[OsString]) -> Result<Infallible, ScopeError>` or an equivalent final-exec function.
- Produce `ApplicationScopeSnapshot` with policy, support state, attempted/scoped/fallback counters, pending status count, and bounded last failure plus doctor severity/detail methods.

- [ ] **Step 1: Add failing policy, argv, unit-name, and decision tests.**

Cover default/unknown/on/off parsing, empty target rejection, arguments with
spaces/metacharacters remaining separate, bounded valid names, successful fake
registration plus migration verification, registration failure, timeout,
migration-not-observed fallback, and direct-exec selection without executing
the test runner.

- [ ] **Step 2: Run the application-scope tests and verify the expected red state.**

Run: `rtk cargo test application_scope --locked`

Expected: compilation/test failures for the absent module and helper seams.

- [ ] **Step 3: Implement policy parsing, safe unit names, and structural wrapping.**

Use only the current executable path, numeric PID, and an internal monotonic
counter in `app-typhon-<pid>-<counter>.scope`. Reject empty argv before
wrapping and prepend a fixed internal command marker plus `--` delimiter.

- [ ] **Step 4: Implement the fake-testable bounded setup state machine.**

Define a registrar trait with `register(unit, pid)`. Run registration with a
bounded `recv_timeout`; after registration, poll an injected migration
observer for a fixed deadline. Return scoped or fallback outcomes and bounded
reasons without ever returning an unrecoverable application-launch error.

- [ ] **Step 5: Implement the production user-systemd registrar with existing zbus.**

Connect to the current user's session bus inside the helper thread/process,
call `org.freedesktop.systemd1.Manager.StartTransientUnit` with:

```text
PIDs=[helper pid]
Slice="app.slice"
Description="Typhon application scope"
```

Use no auxiliary units and do not invoke `systemd-run`. Verify
`/proc/self/cgroup` ends in `app.slice/<expected unit>` before claiming
success.

- [ ] **Step 6: Implement status reporting and direct/real exec.**

Use a bounded parent-owned status pipe for diagnostics. The helper writes
scoped/fallback status, makes the status descriptor close-on-exec, constructs
the real target `Command` from structural argv, and calls Unix `exec`. On any
scope failure it reports the bounded reason and executes the real target
normally.

- [ ] **Step 7: Add the hidden Typhon CLI command.**

Recognize `__internal-app-scope-exec` before normal CLI parsing in `main.rs`,
validate the `--` delimiter and target argv, call the helper, and return its
failure code only if the final real `exec` itself fails.

- [ ] **Step 8: Run the focused tests and commit the helper.**

Run: `rtk cargo test application_scope --locked`

```bash
rtk git add src/application_scope.rs src/lib.rs src/main.rs
rtk git commit -m "feat: add internal user application scope executor"
```

### Task 4: Wrap only normal application launch requests

**Files:**
- Modify: `src/native_output/launch.rs`
- Modify: `src/process.rs` only if needed for status-fd mapping support
- Modify: `src/launch_env.rs` only if command reconstruction requires an existing environment helper

**Interfaces:**
- Centralize native launch wrapping in one function called by both the normal launch path and pending shell-control launch path.
- Preserve `NativeLaunchSource`, `ProcessOptions`, command argv boundaries, environment setup, cursor/XWayland setup, and `ChildSupervisor` calls.

- [ ] **Step 1: Add failing launch-selection tests.**

Assert startup, binding application, and shell-control requests produce a
wrapped helper argv when policy allows; external shell, spotlight, Alt-Tab,
binding session commands, and XWayland/session services remain direct. Assert
policy off produces the original argv and arguments remain separate.

- [ ] **Step 2: Run the launch tests and verify they fail before wrapping is wired.**

Run: `rtk cargo test native_output::launch --locked`

Expected: the new selection assertions fail because current commands start at
the real executable.

- [ ] **Step 3: Centralize eligibility on `ProcessKind::Application`.**

Build the final private-D-Bus/application argv first, then prepend the helper
only for normal application requests. Keep the existing external shell
restart factory and all session-critical launch classes unchanged.

- [ ] **Step 4: Map the diagnostic write end without changing stdio.**

Use the existing `SpawnCommand` fd-mapping API for wrapped application
launches, reserve one fixed non-stdio descriptor for the status channel, keep
the parent read end nonblocking, and preserve all existing command environment,
working directory, and stdio behavior.

- [ ] **Step 5: Add auto fallback/unavailable tests through the fake helper seam.**

Prove a failed registrar, timeout, or missing migration observation selects
the direct real argv and does not turn a valid launch into a spawn error.

- [ ] **Step 6: Run focused launch/process tests and commit the integration.**

Run: `rtk cargo test native_output::launch process --locked`

```bash
rtk git add src/native_output/launch.rs src/process.rs src/launch_env.rs
rtk git commit -m "feat: isolate Typhon applications in user scopes"
```

### Task 5: Wire diagnostics, shutdown gating, and runtime lifecycle

**Files:**
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/shutdown_cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/bootstrap.rs` only if app-scope state needs explicit runtime initialization

**Interfaces:**
- Preserve the cheap post-dispatch dmem mailbox submission.
- Add `app_scopes.state` beside `dmem_foreground.state` with bounded snapshot detail.

- [ ] **Step 1: Add failing shutdown-ordering tests.**

Test the runtime model so a shutdown request first records desired dmem target
`None`, and a later cycle cannot re-submit focused-window protection while
`session.permits_output()` remains true.

- [ ] **Step 2: Run the shutdown tests and confirm the current re-submission bug.**

Run: `rtk cargo test native_output::runtime --locked`

Expected: the new ordering test fails because current reconciliation checks
only output permission.

- [ ] **Step 3: Clear dmem at shutdown admission before lengthy KMS work.**

Submit `None` as soon as the first shutdown request is admitted, then keep the
existing worker quiescence/teardown sequence. Make
`reconcile_dmem_foreground` require both `session.permits_output()` and
`shutdown.is_running()`.

- [ ] **Step 4: Keep suspend/resume and Drop semantics intact.**

Retain suspend `None`, resume recomputation, and final `Drop` shutdown/join.
Add or update deterministic tests for suspend, resume, late XWayland PID,
same-window PID changes, same-cgroup no-write, disappearing old cgroup, and
explicit shutdown cleanup.

- [ ] **Step 5: Add application-scope doctor output.**

Drain bounded status readers when taking the snapshot, expose policy,
availability/support, scoped/fallback counts, pending count, and last reason,
and add `app_scopes.state` with warning severity for degraded forced policy.

- [ ] **Step 6: Run focused dmem, scope, launch, and runtime tests.**

Run:

```bash
rtk cargo test native::dmem_foreground application_scope native_output::launch native_output::runtime --locked
```

- [ ] **Step 7: Commit runtime and doctor integration.**

```bash
rtk git add src/native_output/runtime/cycle.rs src/native_output/runtime/shutdown_cycle.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/mod.rs src/native_output/runtime/bootstrap.rs
rtk git commit -m "fix: gate dmem protection during shutdown and expose scope health"
```

### Task 6: Source-layout, full verification, and qualification

**Files:**
- Modify documentation only if verification discovers a required architecture note or qualification result.

- [ ] **Step 1: Check source layout using only an existing repository checker.**

Discover an existing `check-source-layout` command/script with `rtk find` or
`rtk rg`; run it if present. Do not create a historical checker solely from
the documentation reference.

- [ ] **Step 2: Review task-owned status and diff.**

Run:

```bash
rtk git status --short
rtk git diff --check
rtk git diff main~5..HEAD --stat
```

Confirm unrelated working-tree changes remain untouched and only task-owned
files are staged in the feature commits.

- [ ] **Step 3: Run the required repository verification in the same checkout.**

Run each command separately:

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Record exit codes and complete failure summaries. If a full-suite failure
appears unrelated, check the pre-task baseline at `0b3f9451f9118ff89ce480f4c66f35dd13c5616d`
in a temporary same-folder comparison without changing the user's worktree,
then report evidence rather than assuming.

- [ ] **Step 4: Perform manual host qualification when available.**

Inspect `cgroup.controllers`, `dmem.capacity`, and the two optional service
names. Launch an application through Typhon, compare Typhon/application PID
cgroups, inspect every managed `dmem.low`, focus another scoped app, test
same-cgroup focus, quit the app, and request shutdown. If user-manager scope
creation is unavailable because the session hierarchy is unsuitable, record
the concrete reason and ensure `astreactl doctor` explains it.

- [ ] **Step 5: Commit any task-owned qualification documentation.**

Use a focused report only when manual qualification was actually performed;
otherwise report the environmental limitation in the final response without
inventing evidence.

