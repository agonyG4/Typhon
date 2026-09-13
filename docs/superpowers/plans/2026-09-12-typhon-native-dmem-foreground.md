# Typhon Native Foreground dmem Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add optional native foreground application protection using Linux cgroup v2 `dmem.low`, driven by Typhon's focused desktop-window state.

**Architecture:** A new library module owns dmem UAPI parsing, safe proc/cgroup resolution, per-region writes, diagnostics, and a latest-wins condition-variable worker. The compositor exposes only an in-memory focused `(WindowId, pid)` accessor, while `NativeRuntime` submits that identity after protocol/XWayland dispatch and owns suspend/resume/shutdown reconciliation.

**Tech Stack:** Rust 2024, libc `open(2)`/`write(2)` for defensive cgroupfs writes, `std::sync::{Mutex, Condvar, Arc}`, cgroup v2 proc files, existing Typhon native runtime and doctor control protocol.

## Global Constraints

- Use `/sys/fs/cgroup/dmem.capacity` as the only capacity authority and represent capacities as `u64`.
- Each `dmem.low` write contains one region/value pair and uses a fresh offset-zero open; never modify `dmem.max`, `dmem.min`, `dmem.current`, `dmem.peak`, or ancestor `dmem.low`.
- Do not add KDE, Qt, KCGroups, Gamescope, Hyprland, Plasma, CPU scheduling, XRes, NVML, or hierarchy-management dependencies.
- No proc/cgroup I/O is allowed in pointer handling, keyboard handling, `set_desktop_focus()`, Wayland/XWM dispatch, rendering, or frame scheduling.
- `off` starts no worker; unsupported `auto`/`on` never prevents compositor bootstrap.
- The worker has one bounded latest-wins desired state, cancellable generations, bounded retry/backoff, deterministic join, and best-effort revert on exit.
- Preserve all unrelated dirty working-tree changes; only stage files belonging to this feature.
- Compile and test in `/home/agony/GitHub/Typhon` so Cargo uses the existing local target directory.

## Files and Responsibilities

- Create `src/native/dmem_foreground.rs`: policy, injected paths, capacity/cgroup parsing, target validation, safe writer, worker state machine, diagnostics, and focused unit tests.
- Modify `src/native/mod.rs`: expose the native dmem module to the binary runtime.
- Modify `src/compositor/server_control.rs`: add the cheap in-memory focused normal-window identity accessor.
- Modify `src/native_output/runtime/mod.rs`: store the dmem owner and include it in runtime construction/drop ownership.
- Modify `src/native_output/runtime/bootstrap.rs`: construct the diagnostic-only or worker-backed dmem owner without making capability optionality fatal.
- Modify `src/native_output/runtime/cycle.rs`: reconcile after all focus/metadata-affecting dispatch, submit `None` for inactive sessions, and resubmit after resume.
- Modify `src/native_output/runtime/cycle_dispatch.rs`: add the bounded `dmem_foreground.state` doctor check.

## Task 1: Policy and deterministic parser foundations

**Files:**
- Create: `src/native/dmem_foreground.rs`
- Modify: `src/native/mod.rs`

**Interfaces:**
- Produce `pub enum DmemForegroundPolicy { Auto, On, Off }` with `parse(&str)`, `from_env()`, and `as_str()`.
- Produce `pub struct DmemPaths { pub proc_root: PathBuf, pub cgroup_root: PathBuf }` with `production()`.
- Produce `fn parse_capacity(input: &str) -> Result<BTreeMap<String, u64>, DmemParseError>`.

- [ ] **Step 1: Write failing capacity and policy tests.**

```rust
#[test]
fn capacity_parser_accepts_multiple_regions_in_sorted_order() {
    let capacity = parse_capacity("region-b 67108864\nregion-a 8589934592\n").unwrap();
    assert_eq!(capacity.into_iter().collect::<Vec<_>>(), vec![
        ("region-a".to_string(), 8589934592),
        ("region-b".to_string(), 67108864),
    ]);
}

#[test]
fn capacity_parser_rejects_malformed_duplicate_and_overflow_entries() {
    assert!(parse_capacity("region-a\n").is_err());
    assert!(parse_capacity("region-a 1\nregion-a 2\n").is_err());
    assert!(parse_capacity("region-a 18446744073709551616\n").is_err());
}

#[test]
fn policy_parser_defaults_and_normalizes_unknown_values_to_auto() {
    assert_eq!(DmemForegroundPolicy::parse("auto"), DmemForegroundPolicy::Auto);
    assert_eq!(DmemForegroundPolicy::parse("ON"), DmemForegroundPolicy::On);
    assert_eq!(DmemForegroundPolicy::parse("off"), DmemForegroundPolicy::Off);
    assert_eq!(DmemForegroundPolicy::parse("unknown"), DmemForegroundPolicy::Auto);
}
```

- [ ] **Step 2: Run the focused tests and verify they fail because the module/parser is absent.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: compilation failure naming the missing dmem module/parser symbols.

- [ ] **Step 3: Add the module, policy enum, capacity parser, and `#[cfg(test)]` module.** Parse exactly two whitespace fields per line, reject empty region names, reject duplicate keys through `BTreeMap::insert`, parse counts with `u64::from_str`, and return an empty map for empty input.

- [ ] **Step 4: Re-run the focused parser tests.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: parser and policy tests pass.

- [ ] **Step 5: Commit only the parser/module files.**

```bash
rtk git add src/native/mod.rs src/native/dmem_foreground.rs
rtk git commit -m "feat: add dmem foreground policy and capacity parsing"
```

## Task 2: Proc/cgroup parsing, containment validation, and write semantics

**Files:**
- Modify: `src/native/dmem_foreground.rs`

**Interfaces:**
- Produce `fn parse_cgroup_v2_path(input: &str) -> Result<RelativeCgroupPath, DmemResolveError>`.
- Produce `fn parse_process_uid(input: &str) -> Result<u32, DmemResolveError>` and `fn parse_process_start_time(input: &str) -> Result<u64, DmemResolveError>`.
- Produce `fn validate_cgroup_target(target: &RelativeCgroupPath, own: &RelativeCgroupPath) -> Result<(), DmemResolveError>`.
- Produce `fn apply_low_entries(writer: &mut impl DmemLowWriter, regions: &BTreeMap<String, u64>)` and `fn revert_low_entries(writer: &mut impl DmemLowWriter, regions: &BTreeSet<String>)`.

- [ ] **Step 1: Write failing parser, validation, and recording-writer tests.**

```rust
#[test]
fn cgroup_parser_accepts_unified_entry_and_ignores_other_controllers() {
    let path = parse_cgroup_v2_path("2:cpu,cpuacct:/ignored\n0::/user.slice/app.slice/app.scope\n").unwrap();
    assert_eq!(path.as_str(), "user.slice/app.slice/app.scope");
}

#[test]
fn cgroup_parser_rejects_missing_duplicate_deleted_and_unsafe_entries() {
    assert!(parse_cgroup_v2_path("2:cpu:/x\n").is_err());
    assert!(parse_cgroup_v2_path("0::/x\n0::/y\n").is_err());
    assert!(parse_cgroup_v2_path("0::/x (deleted)\n").is_err());
    assert!(parse_cgroup_v2_path("0::/x/../y\n").is_err());
    assert!(parse_cgroup_v2_path("0::/.\n").is_err());
}

#[test]
fn target_validation_rejects_root_own_and_ancestor_targets() {
    let own = RelativeCgroupPath::parse("user.slice/typhon.scope").unwrap();
    assert!(validate_cgroup_target(&RelativeCgroupPath::root(), &own).is_err());
    assert!(validate_cgroup_target(&own, &own).is_err());
    assert!(validate_cgroup_target(&RelativeCgroupPath::parse("user.slice").unwrap(), &own).is_err());
}

#[test]
fn dmem_low_writes_are_one_fresh_region_value_operation_each() {
    let mut writer = RecordingDmemWriter::default();
    let regions = BTreeMap::from([
        ("region-a".to_string(), 8_u64),
        ("region-b".to_string(), 4_u64),
    ]);
    apply_low_entries(&mut writer, &regions).unwrap();
    assert_eq!(writer.writes, vec![
        "region-a 8\n".to_string(),
        "region-b 4\n".to_string(),
    ]);
}
```

- [ ] **Step 2: Run the tests and verify failures cover missing parsing/validation/write behavior.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: focused tests fail before implementation.

- [ ] **Step 3: Implement strict `0::` parsing and safe normalized paths.** Require one unified entry, strip only the leading root slash, reject empty/root paths, `.`/`..`, prefixes, NULs, and `(deleted)`, and reject a target equal to or above Typhon's own cgroup. Parse the effective UID from the `Uid:` line and the `/proc/<pid>/stat` start time after the final `)` in the comm field.

- [ ] **Step 4: Implement the descriptor-safe writer.** Open each target `dmem.low` with `O_WRONLY | O_CLOEXEC | O_NOFOLLOW`, create a fresh `File` for each region, and write exactly one `region value\n` record. Map target disappearance during revert to success, but return other errors so transitions can report degraded state. Use a recording writer for tests.

- [ ] **Step 5: Re-run the parser/validation/write tests and keep them green.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: all Task 2 tests pass, including two separate writes for two capacity regions and zero-value writes for every managed region.

- [ ] **Step 6: Commit the cgroup parser/validation/writer.**

```bash
rtk git add src/native/dmem_foreground.rs
rtk git commit -m "feat: validate dmem application cgroups safely"
```

## Task 3: Latest-wins worker and transition state machine

**Files:**
- Modify: `src/native/dmem_foreground.rs`

**Interfaces:**
- Produce `pub struct DmemForeground` with `start(policy, paths, session_uid, typhon_pid)`, `submit(Option<ForegroundTarget>)`, `snapshot()`, and `shutdown()`.
- Produce `pub struct ForegroundTarget { pub window_id: WindowId, pub pid: u32 }`.
- Produce `pub struct DmemForegroundSnapshot` with bounded diagnostic fields and counter values.
- The worker owns `Option<ResolvedCgroup>` and its managed region set; runtime code never owns a resolved PID/cgroup.

- [ ] **Step 1: Write failing transition and mailbox tests using an injected resolver/writer.**

```rust
#[test]
fn transition_none_to_a_applies_capacity_and_a_to_none_reverts_it() {
    let mut machine = TestMachine::new();
    machine.reconcile(Some(target(1, 100))).unwrap();
    assert_eq!(machine.writes(), &["region-a 8\n"]);
    machine.reconcile(None).unwrap();
    assert_eq!(machine.writes(), &["region-a 8\n", "region-a 0\n"]);
}

#[test]
fn transition_between_windows_in_one_cgroup_does_not_write() {
    let mut machine = TestMachine::new();
    machine.set_cgroup(100, "apps/shared");
    machine.set_cgroup(101, "apps/shared");
    machine.reconcile(Some(target(1, 100))).unwrap();
    machine.reconcile(Some(target(2, 101))).unwrap();
    assert_eq!(machine.write_count(), 1);
}

#[test]
fn failed_new_target_does_not_leave_old_target_protected() {
    let mut machine = TestMachine::new();
    machine.reconcile(Some(target(1, 100))).unwrap();
    machine.fail_pid(101);
    assert!(machine.reconcile(Some(target(2, 101))).is_err());
    assert_eq!(machine.current(), None);
    assert_eq!(machine.last_write(), "region-a 0\n");
}

#[test]
fn newer_desired_generation_supersedes_a_retry() {
    let mailbox = TestMailbox::new();
    mailbox.submit(Some(target(1, 100)));
    mailbox.submit(Some(target(2, 101)));
    assert_eq!(mailbox.take().unwrap().target, Some(target(2, 101)));
    assert_eq!(mailbox.coalesced_count(), 1);
}
```

- [ ] **Step 2: Run the state-machine tests and verify they fail before the worker exists.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: missing `DmemForeground`/state-machine implementation failures.

- [ ] **Step 3: Implement the single-slot mailbox.** Guard desired target, generation, stopping, and counters with one mutex; notify a condition variable on every replacement. The worker checks generation before resolving, before each write, and after each bounded backoff wait.

- [ ] **Step 4: Implement resolution and transition execution inside the worker.** Validate nonzero PID, process existence, same effective UID, cgroup v2 path, safe target containment, non-root/non-self/non-ancestor identity, and writable `dmem.low`. Revert the cached old cgroup before resolving a different new target. Treat disappearing old cgroups as successful revert. If a new target fails after partial writes, revert all entries written to it and publish no current target.

- [ ] **Step 5: Add bounded retry/backoff for missing target dmem files, permission/controller-not-ready failures, and transient process/cgroup preparation.** Use a small fixed schedule such as 10ms, 25ms, 50ms, 100ms, and 200ms, for no more than five attempts; `Condvar::wait_timeout` must return early when the generation changes or shutdown is requested.

- [ ] **Step 6: Add worker panic containment and deterministic shutdown.** Catch the worker execution boundary, mark diagnostics unavailable/degraded, best-effort revert the worker-owned current cgroup, stop accepting mailbox work, and join. `DmemForeground::Drop` must invoke the same shutdown path.

- [ ] **Step 7: Re-run the focused worker tests.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: transitions, same-cgroup no-op, stale coalescing, PID changes, disappearing cgroups, retry supersession, failure containment, and shutdown tests pass.

- [ ] **Step 8: Commit the worker/state machine.**

```bash
rtk git add src/native/dmem_foreground.rs
rtk git commit -m "feat: add latest-wins dmem foreground worker"
```

## Task 4: Compositor identity accessor and runtime bootstrap ownership

**Files:**
- Modify: `src/compositor/server_control.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/bootstrap.rs`

**Interfaces:**
- Produce `pub fn OwnCompositorServer::foreground_dmem_target(&self) -> Option<(WindowId, u32)>`.
- Store `dmem_foreground: DmemForeground` in `NativeRuntime`.

- [ ] **Step 1: Write failing accessor tests for normal XDG/X11 windows, auxiliary X11 windows, missing PID, and missing focus.** Assert that the method reads only existing compositor state and returns the focused window ID with its current PID.

- [ ] **Step 2: Run the accessor tests and verify they fail because the method is absent.**

Run: `rtk cargo test compositor::server_control --locked`

Expected: missing method or test failure.

- [ ] **Step 3: Implement the accessor.** Use `focused_window_id`, `window()`, `metadata.pid`, and `is_workspace_managed()`. Do not access procfs/cgroupfs and do not use `last_application_keyboard_focus`.

- [ ] **Step 4: Write failing bootstrap tests for `off`, unsupported `auto`, and unsupported `on`.** Verify `off` has no worker, while `auto`/`on` preserve an unavailable diagnostic rather than returning a bootstrap error.

- [ ] **Step 5: Run the bootstrap/policy tests and verify the new constructor behavior is missing.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: unavailable-construction assertions fail before runtime ownership is wired.

- [ ] **Step 6: Construct `DmemForeground` during native bootstrap after the event loop/process identity prerequisites exist.** Probe root `cgroup.controllers` for `dmem`, parse nonempty `dmem.capacity`, and attempt worker startup only for `auto`/`on`; convert all capability/worker errors into status diagnostics. Do not register a reactor token and do not propagate optional feature errors through `NativeResult`.

- [ ] **Step 7: Initialize and store the field in `NativeRuntime`, including every constructor path and early-local teardown path.**

- [ ] **Step 8: Re-run accessor and bootstrap tests.**

Run: `rtk cargo test compositor::server_control native::dmem_foreground --locked`

Expected: focused identity and non-fatal capability tests pass.

- [ ] **Step 9: Commit the compositor boundary and bootstrap ownership.**

```bash
rtk git add src/compositor/server_control.rs src/native_output/runtime/mod.rs src/native_output/runtime/bootstrap.rs
rtk git commit -m "feat: connect focused windows to dmem runtime ownership"
```

## Task 5: Post-dispatch reconciliation and session lifecycle

**Files:**
- Modify: `src/native_output/runtime/cycle.rs`

**Interfaces:**
- Produce a private `NativeRuntime::reconcile_dmem_foreground(&mut self)` that selects `None` when `!self.session.permits_output()` and otherwise converts `server.foreground_dmem_target()` into `ForegroundTarget`.

- [ ] **Step 1: Write failing runtime model tests for repeated identity, PID arrival/change, suspend, inactive session, resume, and shutdown.**

```rust
#[test]
fn same_window_pid_arrival_is_a_new_submission_without_focus_generation_change() {
    let mut model = RuntimeDmemModel::default();
    model.submit(None);
    model.submit(Some(target(7, 100)));
    model.submit(Some(target(7, 101)));
    assert_eq!(model.submissions(), &[None, Some(target(7, 100)), Some(target(7, 101))]);
}

#[test]
fn inactive_session_submits_none_and_resume_recomputes_focus() {
    let mut model = RuntimeDmemModel::default();
    model.set_focus(Some(target(4, 100)));
    model.reconcile_active();
    model.reconcile_inactive();
    model.reconcile_active();
    assert_eq!(model.submissions(), &[
        Some(target(4, 100)),
        None,
        Some(target(4, 100)),
    ]);
}
```

- [ ] **Step 2: Run the runtime model tests and verify they fail before reconciliation exists.**

Run: `rtk cargo test native_output::runtime --locked`

Expected: missing reconciliation/model behavior.

- [ ] **Step 3: Call reconciliation after Wayland/control/XWayland scene dispatch and before presentation planning.** The call must be a cheap state read plus mailbox comparison; it must not request redraw or alter deadlines.

- [ ] **Step 4: Call reconciliation when suspension begins so the worker receives `None` even though `focused_window_id` may still contain an application during quiesce.** Keep suspended-cycle reconciliation idempotent.

- [ ] **Step 5: Call reconciliation after `finish_native_session_recovery()` so a successful resume re-applies current focus, including metadata that arrived while inactive.**

- [ ] **Step 6: Ensure the existing `NativeRuntime::Drop` path explicitly shuts down dmem before the worker field is dropped, preserving best-effort revert without delaying unrelated compositor teardown indefinitely.**

- [ ] **Step 7: Re-run runtime/focused dmem tests.**

Run: `rtk cargo test native_output::runtime native::dmem_foreground --locked`

Expected: no filesystem work is reachable from compositor/input/render functions, and lifecycle tests pass.

- [ ] **Step 8: Commit runtime reconciliation/lifecycle wiring.**

```bash
rtk git add src/native_output/runtime/cycle.rs
rtk git commit -m "feat: reconcile dmem protection with focus and session state"
```

## Task 6: Doctor diagnostics and observability

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native/dmem_foreground.rs`

**Interfaces:**
- Produce `DmemForegroundSnapshot::doctor_severity() -> DoctorSeverity` and `DmemForegroundSnapshot::detail() -> String` with bounded output.

- [ ] **Step 1: Write failing policy/doctor matrix tests.** Cover `off => Ok`, unsupported `auto => Ok`, capability-available healthy active => `Ok`, post-capability runtime failure => `Warning`, and unavailable/degraded `on => Warning`.

- [ ] **Step 2: Run the doctor tests and verify the severity method/check is absent.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: missing severity/check failures.

- [ ] **Step 3: Implement bounded shared diagnostics.** Include policy, capability, worker, region count, desired window/PID, current protected cgroup, total capacity, last failure, and counters; truncate cgroup/reason strings to a fixed small limit.

- [ ] **Step 4: Add `dmem_foreground.state` to the `ControlCommand::Doctor` vector.** Use the subsystem severity and detail without making optional unsupported `auto` unhealthy.

- [ ] **Step 5: Add focused tests that deserialize the doctor check and verify the ID, severity, and bounded detail.**

- [ ] **Step 6: Re-run focused doctor tests.**

Run: `rtk cargo test native::dmem_foreground native_output::runtime::cycle_dispatch --locked`

Expected: policy/doctor tests pass.

- [ ] **Step 7: Commit diagnostics.**

```bash
rtk git add src/native/dmem_foreground.rs src/native_output/runtime/cycle_dispatch.rs
rtk git commit -m "feat: expose dmem foreground diagnostics"
```

## Task 7: Verification and host qualification

**Files:**
- No additional source files unless a verification failure identifies a feature-related defect.

- [ ] **Step 1: Inspect the feature diff and status without staging unrelated work.**

Run: `rtk git status --short; rtk git diff --check; rtk git diff HEAD~6..HEAD --stat`

Expected: only the dmem design, plan, and implementation commits are in the feature history; the pre-existing dirty files remain unstaged.

- [ ] **Step 2: Run the focused dmem tests separately.**

Run: `rtk cargo test native::dmem_foreground --locked`

Expected: all dmem parser, validation, write, worker, policy, lifecycle, and doctor tests pass.

- [ ] **Step 3: Run repository verification in the same checkout.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Expected: all commands pass. If a failure is pre-existing, reproduce it against the baseline before classifying it and record the exact evidence.

- [ ] **Step 4: Run any existing source-layout or architecture verification command discovered during the audit.** Do not invent a missing command.

- [ ] **Step 5: Qualify host dmem support read-only.**

```bash
rtk run -c 'cat /sys/fs/cgroup/cgroup.controllers'
rtk run -c 'cat /sys/fs/cgroup/dmem.capacity'
rtk run -c 'systemctl status dmemcg-booster-system.service'
rtk run -c 'systemctl --user status dmemcg-booster-user.service'
```

If dmem is unavailable, report that manual before/after evidence is not possible. If available, inspect a focused application cgroup and capture before/after `dmem.low` values, same-cgroup no-op behavior, XWayland late PID behavior, suspend/resume, and clean Typhon shutdown cleanup without changing the implementation's service-name dependencies.

- [ ] **Step 6: Inspect the final feature-only diff, stage only feature files, and create the final focused commit if any verification fixes were needed.**

```bash
rtk git diff --check
rtk git status --short
```
