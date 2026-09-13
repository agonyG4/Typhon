# Typhon Native Foreground Device-Memory Protection

## Goal

When a normal desktop window is Typhon's focused desktop window, apply the
kernel-published device-memory capacities to that application's cgroup's
`dmem.low` entries. Revert the previous protected cgroup when focus changes,
when focus becomes empty, when the native session is inactive, and during
shutdown.

This is a Typhon-native foreground policy. It does not launch or depend on
KDE, Plasma, Qt, KCGroups, Gamescope, Hyprland, XRes, CPU scheduling, or
`dmemcg-booster` service names. `dmemcg-booster` may prepare the hierarchy,
but Typhon only manages the focused leaf application's `dmem.low` entries.

## Architecture

### Native dmem subsystem

Create `src/native/dmem_foreground.rs` and expose it through
`src/native/mod.rs`. The module owns:

- `DmemForegroundPolicy` parsing for `OBLIVION_ONE_DMEM_FOREGROUND`, with
  `auto` as the default and unknown values normalized to `auto` with a
  diagnostic.
- Kernel capacity parsing from an injected cgroup root, using a sorted
  `BTreeMap<String, u64>`. Every valid region must occur exactly once, each
  count must parse as `u64`, and an empty set is unavailable.
- `/proc/<pid>/status`, `/proc/<pid>/stat`, and `/proc/<pid>/cgroup` parsing
  behind an injected proc root for deterministic tests.
- Safe cgroup target resolution relative to an injected cgroup root. It
  rejects zero or missing PIDs, UID mismatches, missing or duplicate `0::`
  entries, `(deleted)` entries, root/ancestor/self targets, unsafe path
  components, and targets without an openable writable `dmem.low`.
- Fresh close-on-exec, no-follow opens for every individual `dmem.low` write.
  A single write contains exactly one `region value` line. Reverts write the
  cached managed regions individually with `0`.
- A worker-owned resolved cgroup identity containing the normalized relative
  path and optional `/proc/<pid>/stat` start time. Reverts use this cached
  identity and never re-resolve an old PID.
- A bounded latest-wins mailbox: one desired target plus a monotonically
  increasing generation guarded by a mutex and condition variable. New
  generations wake the worker and supersede retry waits.
- Bounded retry/backoff for target preparation/controller-not-ready failures.
  Retries stop when a newer generation arrives. Missing old cgroups during
  revert are successful terminal cleanup.
- Shared bounded diagnostics and counters for doctor output. Worker panics or
  notification failures mark the subsystem unavailable and attempt cleanup.

The worker is not registered with the native reactor because it has no
completion payload that must be serviced by the compositor. Its condition
variable provides wakeup and cancellation, while the runtime only performs a
cheap mailbox submission.

### Compositor identity boundary

Add a read-only method in `src/compositor/server_control.rs` that returns
`Option<(WindowId, u32)>` for the current focused normal desktop application.
It reads only `focused_window_id`, the existing window table, backend/kind,
and `metadata.pid`. XDG windows use the PID captured from Wayland peer
credentials. XWayland windows use the existing `_NET_WM_PID` metadata and are
subject to the worker's same-UID and containment validation. No compositor
method performs proc or cgroup I/O.

Normal means an XDG managed window or a managed X11 window with a normal
X11 role. Layer-shell, override-redirect, notifications, and other auxiliary
windows do not produce a target. This accessor intentionally follows
`focused_window_id`, not keyboard-focus history or game heuristics.

### Native runtime ownership

Add one `DmemForeground` field to `NativeRuntime`. During native bootstrap,
construct it without propagating capability or worker errors into compositor
startup. `off` constructs a diagnostic-only disabled state and starts no
worker. `auto` and `on` probe root `cgroup.controllers` and nonempty
`dmem.capacity`; unavailable capability leaves the runtime healthy but
diagnosed.

After the cycle's Wayland dispatch, control dispatch, and XWayland scene
dispatch have completed, call the dmem submit method. The submit method
compares the desired `(WindowId, pid)` with the last submitted value and does
nothing for an identical value. It does not request redraw, schedule a frame,
or touch the filesystem. This point captures PID metadata that arrives after
initial XWayland focus, including PID changes on the same window.

When the session begins suspension, submit `None`. During suspended/inactive
cycles submit `None` again only if the desired state changed. After successful
resume, the normal post-dispatch reconciliation recomputes focus and submits
the current application. `DmemForeground` explicitly shuts down in
`NativeRuntime::Drop`, requests `None`, joins the worker, and relies on the
worker's cached target cleanup as a best-effort early-teardown safety path.

## Transition behavior

The worker has one authoritative current protected cgroup:

- `None -> A`: resolve A and apply all capacity entries.
- `A -> B` with different resolved cgroups: revert A's cached managed entries,
  then resolve and apply B.
- `A -> B` with the same resolved cgroup: resolve B for identity validation,
  record a same-cgroup no-op, and perform no writes.
- `A -> None`: revert A.
- Any failed new-target transition leaves no authoritative current target after
  the old target has been reverted. Partially applied new entries are reverted
  before recording the failure.

The worker checks its generation before and between retry/write steps. A
sequence such as A, B, C may discard B and converge on C. A retry never
blocks newer focus state for the full retry window.

## Diagnostics and doctor

Add `dmem_foreground.state` to the native `astreactl doctor` checks. Its
summary/detail is bounded and includes policy, kernel capability, worker
availability, discovered-region count, desired window/PID when known, current
protected cgroup and protected capacity, and the last failure/reason.

Severity is `OK` for `off`, unsupported `auto`, and healthy active operation;
`Warning` for runtime degradation after capability, and for unavailable or
degraded `on`. Unsupported optional dmem does not make compositor bootstrap
fail. Counters include requested transitions, applied transitions, coalesced
or stale requests, same-cgroup no-ops, retries, resolution failures, write
failures, and successful reverts.

## Testing

Unit tests in the native subsystem use temporary injected proc/cgroup roots
and a recording writer. They cover capacity and cgroup parsing, malformed or
unsafe paths, UID/root/self/ancestor validation, writable-target discovery,
all transition cases, same-cgroup no-ops, PID metadata arrival/change, stale
mailbox coalescing, superseded retries, per-region writes and reverts,
disappearing cgroups, suspend/resume/shutdown cleanup, contained worker
failure, policy parsing, and doctor severity/state matrices.

The implementation must pass the repository's required formatting, check,
clippy, focused-test, full-test, and diff checks. Manual dmem qualification
is reported separately when the host exposes `/sys/fs/cgroup/dmem.capacity`.

