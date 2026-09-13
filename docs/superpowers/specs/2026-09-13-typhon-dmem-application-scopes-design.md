# Typhon dmem Hardening and Application Scopes

## Goal

Make native foreground `dmem.low` protection reliable for normal applications
launched by Typhon, while preserving the existing asynchronous compositor
integration and making those applications eligible for safe foreground
protection through systemd user application scopes.

## Confirmed blockers

The existing foreground dmem implementation already has the correct focus
authority, injected proc/cgroup roots, cached old-target identity, latest-wins
mailbox, bounded retries, and runtime lifecycle hooks. The audit found four
correctness gaps:

1. Multi-region capacity application returns on an intermediate write error
   without a complete policy-level rollback contract.
2. Generation checks surround `reconcile()` but do not gate each capacity
   mutation, so a stale target can continue writing after a newer focus is
   visible.
3. PID start time is read only once, so a numeric PID reused during resolution
   is not rejected by an identity revalidation.
4. Shutdown reconciliation can submit the focused window again after shutdown
   admission while output is still permitted.

Normal Typhon application launches are classified as `ProcessKind::Application`
but currently pass the final command directly to `ChildSupervisor`, leaving the
child in the compositor cgroup. The dmem resolver correctly rejects that
cgroup, so application scope isolation is required for the feature to work on
normal launches.

## Architecture

### Application scope helper

Add a small library module, `src/application_scope.rs`, with:

- `ApplicationScopePolicy` parsing for `OBLIVION_ONE_APP_SCOPES`, defaulting to
  `auto` and normalizing unknown values to `auto`.
- Structural argv wrapping that produces
  `Typhon --internal-app-scope-exec -- <real argv>` without shell quoting.
- A synchronous helper entry point used by the Typhon binary. It creates a
  bounded user-bus/systemd operation in the helper process, not in the
  compositor parent.
- A testable registration/verification seam. The production registrar uses
  the existing `zbus` dependency to call the current user's
  `org.freedesktop.systemd1.Manager.StartTransientUnit` method.
- A bounded unit name of the form `app-typhon-<pid>-<counter>.scope`.
- Minimal transient scope properties: `PIDs=[helper pid]`, `Slice=app.slice`,
  and a bounded fixed description.
- Bounded polling of `/proc/self/cgroup`, requiring the final path to contain
  `app.slice/<expected unit>` before the helper claims success.
- Direct `exec` of the target argv after either successful setup or bounded
  fallback. There is no persistent wrapper process.

The native launch module remains the owner of source classification. One
central helper wraps only launch requests whose `ProcessOptions.kind` is
`Application` and whose source is startup, binding application, or shell
control. External shell, session commands, spotlight/Alt-Tab helpers,
XWayland, infrastructure, and other session-critical processes are not
wrapped.

The parent uses a bounded status pipe only for diagnostics. The helper reports
scoped or fallback status before setting the status descriptor close-on-exec;
the parent drains a bounded set of status readers when the doctor snapshot is
requested. Failure to create or use the status channel never prevents launch.

### dmem controller and worker

Retain `ForegroundController` and the condition-variable latest-wins worker.
Extend the controller with a cached pending-cleanup identity separate from the
authoritative current target. A failed or stale partial apply records the
target path for cleanup, attempts every already-touched region, and never
publishes it as current. Later reconciliation retries pending cleanup before
starting another capacity transition.

Each capacity write is preceded by a generation/stopping check. If a newer
generation appears after any write, the stale transition stops, cleans every
region that may have been touched, and returns control to the worker's newest
desired state. Cleanup continues after individual write failures and preserves
the first meaningful error.

Process resolution reads UID/start time, cgroup, and target validity, checks
the target's writable `dmem.low`, then reads start time again immediately
before returning. A mismatch is treated as a stale PID identity and causes no
dmem mutation. Old targets are reverted only through their cached resolved
cgroup path.

The explicit shutdown request submits `None` before KMS worker admission and
teardown work can block. Runtime reconciliation also treats any non-running
shutdown state as ineligible for a foreground target. `Drop` remains the final
join and best-effort cleanup guard.

### Source ownership

Move the large inline dmem unit-test module into
`src/native/dmem_foreground_tests.rs`, leaving the production module below the
documented 1,500-line limit. The production module continues to own policy,
proc/cgroup parsing, writer semantics, controller transitions, worker state,
and snapshots; the extracted file owns deterministic unit fixtures and
regressions.

## Error and fallback behavior

- `off`: do not wrap and do not contact systemd.
- `auto`: try the user scope; on unavailable user bus, denied/unsupported
  manager, timeout, or failed migration proof, report bounded fallback status
  and execute the real argv normally.
- `on`: use the same non-fatal fallback path, but mark diagnostics degraded and
  expose the bounded reason at warning severity.
- A malformed internal helper argv is rejected before any target exec.
- A disappearing old cgroup during zeroing is successful terminal cleanup.
- Partial apply and cleanup failures are reported without suppressing attempts
  against remaining managed regions.

## Testing

Add deterministic tests for transactional dmem writes, cleanup continuation,
stale generation between regions, PID start-time reuse, shutdown target
clearing, application launch classification, policy behavior, structural argv
preservation, systemd registration/verification/fallback decisions, bounded
unit names, and doctor snapshots. Tests use injected proc/cgroup roots,
controllable writers, fake scope registrars, and fake migration observations;
no host systemd, GPU, or real cgroup hierarchy is required.

Run focused dmem and application-scope tests before the full repository
verification. Build and test in the repository checkout so Cargo reuses the
existing local target directory. Manual systemd/cgroup qualification is
reported separately and cannot make the optional fallback path unavailable.
