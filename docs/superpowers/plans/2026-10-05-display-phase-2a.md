# Display Phase 2A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use inline execution in this session. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add safe Atomic KMS runtime mode changes with temporary application, server rollback, durable Keep, and startup restore.

**Architecture:** Keep one applied configuration in `NativeRuntime`; retain exact native connector modes under the semantic generation; prepare one scanout and Atomic candidate and use that candidate for both validation and submit. A bounded asynchronous request coordinator gates presentation work until KMS ownership is safe. Its server-owned transaction/deadline controls the same reconfiguration path for apply and rollback. Private persistence and startup selection use a restart-stable exact timing fingerprint.

**Tech Stack:** Rust 2024, `drm-ffi`/`drm-sys`, serde JSON, Typhon native event loop and eventfd workers, CMake/CTest for Eclipse Settings.

## Global Constraints

- Work on `main`; do not create a branch or worktree.
- Leave `/home/agony/GitHub/Typhon/msg.txt` untouched and unstaged.
- Put every Cargo target directory under `/mnt/Aether/Desktop/GitHub`; reuse `/mnt/Aether/Desktop/GitHub/Typhon-target`.
- Put every Eclipse CMake build directory under `/mnt/Aether/Desktop/GitHub`; reuse `/mnt/Aether/Desktop/GitHub/Eclipse-build`.
- Before any compile, print and verify the effective output directory.
- Keep the Typhon Display transaction separate from `OutputTransactionLedger`.
- Advertise only mode selection on a qualified Atomic path; keep scale and transform unsupported.
- Keep `Settings/qml/pages/system/Display.qml` read-only.
- Use injected time and KMS submission in deterministic tests; do not use a 15-second wall-clock test.
- Commit each completed logical change without staging unrelated files.

---

## File map

- `src/native_output/output/target.rs` retains every exact connector mode, creates the existing public projection, and resolves current-generation IDs.
- `src/native_output/output/configuration.rs` owns the semantic generation, applied mode/configuration model, transaction IDs/states, monotonic rollback deadline, and pure transition decisions.
- `src/native/kms/backend.rs` and `src/native/kms/` prepare, test, submit, and adopt one runtime Atomic modeset candidate.
- `src/native_output/scanout/` prepares an unattached replacement scanout matching candidate dimensions while retaining the current backend until KMS adoption.
- `src/native_output/runtime/` coordinates the pending request, safe boundary, successful publication, deadline wake, persistence completion, and complete control replies.
- `src/native_output/input/state.rs`, `src/native_output/input/routing.rs`, and `src/native_output/output/cursor.rs` update output bounds without reopening devices or losing cursor image/position.
- `src/native/scheduler.rs`, `src/native_output/presentation/kms_timing.rs`, `src/native/presentation_deadline.rs`, render-journal/adaptive-buffering owners, and runtime transition helpers rebase state tied to the prior mode.
- `src/private_config.rs` and `src/keyboard_persistence.rs` provide the existing private-file and bounded worker patterns for a dedicated output persistence operation.
- `src/native_output/output/` and `src/native/kms/` deterministic test modules cover state and request identity without physical DRM.
- `docs/control-plane.md` documents wire and transaction behavior; `docs/astreactl.md` and relevant output-control text are updated if they currently describe mode mutation as absent.
- Eclipse changes are limited to `Settings/README.md`, `Settings/docs/ARCHITECTURE.md`, and backend code only if the current typed transaction decoder cannot represent the final snapshot semantics. QML remains unchanged.

## Contracts between tasks

```rust
struct NativeOutputModeEntry {
    id: u32,
    mode: drm_sys::drm_mode_modeinfo,
}

struct NativeOutputModeInventory {
    generation: OutputConfigurationGeneration,
    entries: Vec<NativeOutputModeEntry>,
}

struct NativeAppliedOutputConfiguration {
    target: KmsTarget,
    scale_milli: u32,
    transform: OutputTransformSnapshot,
}

struct PendingOutputConfigurationRequest {
    control_token: ReactorToken,
    request_id: u64,
    configure: OutputConfigureRequest,
}
```

`NativeOutputModeInventory::resolve(generation, id)` rejects a generation
mismatch before returning the exact native struct. A prepared KMS candidate
owns one mode blob, geometry, framebuffer assignment, cursor state, and one
validated request; its `test_only` and `commit` methods submit that same request
state. Runtime publication consumes the candidate only after successful real
submission.

---

### Task 1: Preserve exact modes and startup fingerprints

**Files:**
- Modify: `src/native_output/output/target.rs`
- Modify: `src/native_output/output/configuration.rs`
- Modify: `src/native_output/runtime/bootstrap.rs`
- Test: `src/native_output/output/target.rs` tests and a focused output-configuration test module

**Consumes:** Existing `OutputConfigurationGeneration`, `KmsTarget`, `OutputModeSnapshot`, current native timing identity, and `NativeModePreference`.

**Produces:** Generation-qualified `NativeOutputModeInventory`, a single applied configuration type, exact-mode resolution, persisted timing fingerprint/resolver, and startup selection precedence `explicit non-auto environment > resolvable persisted mode > normal selection`.

- [ ] Add tests proving IDs retain connector inventory identity through sorted/truncated public projection, every advertised ID resolves one exact `drm_mode_modeinfo`, a stale generation rejects before resolution, and a truncated public list does not expose hidden IDs as selectable.
- [ ] Add persistence-model tests proving preferred/type metadata is excluded from the timing fingerprint, different porches/clock/flags do not collide, connector identity qualifies resolution, missing modes fall back, and explicit environment preference wins.
- [ ] Implement the internal inventory by enumerating the full connector `modes` vector and preserving raw structs; leave existing public snapshot fields and truncation unchanged.
- [ ] Put exact applied mode, mode ID, output identity, scale `1000`, and normal transform behind one runtime configuration value. Ensure label, dimensions, and refresh are derived from that value rather than an independent mode copy.
- [ ] Add a strict version-1 persisted schema with `deny_unknown_fields`, bounded byte/field validation, connector name, optional physical-size evidence, and exact stable timing fields. Keep opaque IDs and DRM object IDs out of the schema.
- [ ] Load and resolve this record before initial target, scanout, and KMS construction; on invalid/unavailable hardware emit one bounded diagnostic and keep normal startup behavior.
- [ ] Run focused Typhon tests with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test output::target` and commit this task.

### Task 2: Prepared Atomic runtime modeset candidate

**Files:**
- Modify: `src/native/kms/backend.rs`
- Modify: `src/native/kms/` request/submission modules as needed
- Test: Atomic backend test module using the existing injected submitter pattern

**Consumes:** Exact `drm_mode_modeinfo`, selected pipeline identity, `AtomicPlaneGeometry`, candidate framebuffer, candidate cursor visual, and the existing submitter abstraction.

**Produces:** A prepared candidate lifecycle `prepare -> test_only -> commit -> adopt_into_backend` that owns one mode blob and one request description.

- [ ] Write fake-submitter tests asserting runtime TEST_ONLY and real submissions contain `ALLOW_MODESET`, the same connector/CRTC/plane, mode blob, geometry, primary framebuffer, and cursor state.
- [ ] Assert TEST_ONLY rejection and real-submit rejection leave current `DrmAtomicBackend` mode, geometry, and active mode blob unchanged; assert dropping an unadopted candidate destroys its blob once.
- [ ] Assert successful real submit adopts the new mode/blob/geometry, keeps the adopted blob alive, and subsequent recovery submits the adopted mode and geometry.
- [ ] Implement the candidate so it creates its own mode blob, constructs the atomic request once, and submits clones of that validated request state. Keep existing initial modeset and recovery paths unchanged.
- [ ] Add an adoption method that is callable only after the real submission succeeds; replace the active backend fields atomically and let the prior blob drop only after that point.
- [ ] Run focused KMS tests with Aether `CARGO_TARGET_DIR` and commit this task.

### Task 3: Dimension-correct candidate scanout and safe boundary

**Files:**
- Modify: `src/native_output/scanout/mod.rs` and relevant backend files (`egl_gbm.rs`, `gbm_cpu.rs`, or compatibility backend)
- Modify: `src/native_output/runtime/session_io.rs` and shared runtime transition owner
- Test: scanout reconfiguration and runtime safe-boundary model tests

**Consumes:** Candidate dimensions, existing scanout kind/renderer/effect registry, the task 2 Atomic candidate, current pageflip/worker/arbiter/presentation ownership.

**Produces:** A replacement scanout that can be rendered before KMS changes; a proven safe-boundary gate; shared semantic post-synchronous-modeset retirement/reset behavior.

- [ ] Add deterministic gate cases for main-thread pageflip ownership, worker in-flight ownership, queued-next work, atomic arbiter ownership, output presentation ledger ownership, cursor-plane pending work, deferred worker events, and outstanding explicit-sync obligations. Each case must remain blocked until its existing terminal/retirement path proves it safe.
- [ ] Add candidate preparation failure tests that preserve the active backend, and success tests that preserve backend kind, renderer/effect generation, and full-frame contents at the candidate dimensions.
- [ ] Refactor the common session-recovery post-modeset transition into semantic helpers for stale Direct Scanout abandonment, old scanout retirement, timing reconfiguration, validation invalidation, journal/adaptive reset, proven-deadline reset, redraw, scheduler rearm, cursor requalification, and generation handling.
- [ ] Add a pending-configuration gate that prevents new frame submissions and new worker `queue_next` work while still pumping control, pageflip, worker, cursor, and synchronization completion events.
- [ ] Prepare a dimension-correct unattached replacement pool using the existing backend's canonical construction path; render a complete compositor frame and retain its framebuffer before building the Atomic candidate.
- [ ] Keep the old scanout alive through real commit. On successful synchronous modeset, prove KMS switched away, retire old primary ownership exactly once, terminalize prior Direct Scanout through existing release helpers, and publish the new compositor-owned `presented_planes` baseline.
- [ ] Test failure at candidate creation/render/test/real-submit leaves the old runtime and KMS state authoritative. Test direct primary release occurs once only after successful submit.
- [ ] Run focused scanout/runtime tests and commit this task.

### Task 4: Asynchronous request and Display transaction state machine

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/runtime/` output mutation coordinator and cycle scheduler owner
- Modify: `src/native_output/output/configuration.rs`
- Modify: `src/native_output/runtime/wake_plan.rs`
- Test: output mutation coordinator and wake planner tests

**Consumes:** Task 1 exact resolver, task 3 safe boundary/candidate engine, typed `outputs.configure`, confirm, and revert request shapes.

**Produces:** Immediate validation and exact no-op response; exactly one asynchronous request; one nonzero stale-safe server transaction with 15-second monotonic deadline; authoritative transaction snapshots and deadline-driven rollback.

- [ ] Test exact generation validation before native mode lookup, unknown output/mode rejection, scale/transform equality with authoritative values, exact-current no-op, one pending mutation maximum, and second mutation rejection.
- [ ] Test a pending request suppresses newly scheduled presentation work yet waits for every ownership holder above to complete; test it progresses as soon as ownership is safe instead of requiring a KMS-idle instant at dispatch.
- [ ] Add pure injected-time state-machine cases for temporary apply, `pending_confirmation`, remaining milliseconds, expiry, explicit revert, stale confirm/revert, rollback success, and terminal `rollback_failed`.
- [ ] Extend `NativeDeadlineOwner` indexing/metrics and wake-plan inputs with `OutputConfiguration`; test earliest-deadline choice and no second timer source.
- [ ] Change the configure arm in `dispatch_control_command` to retain control token/request ID and enqueue one bounded request; queue its full response only after the result is known. Keep status/outputs control available during the pending operation.
- [ ] Implement apply/rollback completion so both call the same exact-mode output reconfiguration engine. Advance semantic generation once after apply and once after successful rollback; do not advance on persistence-only confirmation or per frame.
- [ ] Make `control_output_list_snapshot` include the active pending-confirmation or rollback-failed transaction with ID, output ID, applied generation, authoritative remaining time, and optional error. Keep all configure/confirm/revert success responses complete.
- [ ] Test an ordinary outputs refresh recovers the transaction and simulated Settings disconnect has no effect on its ownership.
- [ ] Run focused output/wake/control tests and commit this task.

### Task 5: Publish coordinated runtime authority after commit

**Files:**
- Modify: `src/native_output/runtime/` output reconfiguration owner
- Modify: `src/native_output/input/state.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/output/cursor.rs`
- Modify: `src/native/scheduler.rs`
- Modify: `src/native_output/presentation/kms_timing.rs`
- Modify: `src/native/presentation_deadline.rs`
- Modify: render journal and adaptive buffering owners as identified in the transition helper
- Test: relevant module tests plus runtime transition model tests

**Consumes:** A successfully committed candidate and task 3 retirement transition.

**Produces:** Coherent target, server, scanout, input, cursor, scheduler, timing, cache, and redraw state for the new configuration.

- [ ] Add method tests for `NativeInputState` dimension update, pointer clamp, unrelated key/button preservation, and pointer-constraint reconciliation.
- [ ] Add a `NativeInputBackend` dimension update method and test libinput absolute-coordinate scaling changes without closing/reopening devices.
- [ ] Add cursor output identity reconfiguration that preserves image and logical position, clamps position, and invalidates dimension-keyed capability/proof caches; test old proof rejection.
- [ ] Reconfigure `NativeFrameScheduler` to the new exact refresh interval and rebase `KmsPresentationTimingModel`, `PresentationDeadlinePlanner`, scheduled targets, server commit-timing targets, adaptive buffering, journal, and proven deadline-miss state.
- [ ] After candidate real commit succeeds, update target mode ID/native struct/size/label/refresh, bound Wayland output size and refresh, input state/backend, cursor identity, timing, and scanout as one runtime publication step.
- [ ] Invalidate Direct Scanout qualification, direct validation cache, VRR/presentation confirmation tied to old mode, and dimension-dependent damage; force conservative full repaint.
- [ ] Add deterministic runtime transition cases for `1920x1080@165 -> 1920x1080@60 -> revert` and `1920x1080 -> 1280x720`, asserting server/input/libinput/cursor/scheduler/timing/damage state changes only after commit.
- [ ] Run focused module and runtime tests and commit this task.

### Task 6: Persistence worker, timeout race, and durable Keep

**Files:**
- Modify: `src/private_config.rs` only if its existing bounded/private/atomic contract needs a focused operation
- Add: `src/native_output/output/persistence.rs` for schema/path/fingerprint worker request types
- Modify: `src/native_output/runtime/` persistence worker ownership and event dispatch
- Test: output persistence schema, worker, and transaction race tests

**Consumes:** Versioned persisted fingerprint from task 1, transaction state/deadline from task 4, and existing `PrivateConfigFile` plus eventfd worker patterns.

**Produces:** One bounded asynchronous persistence operation; Keep disarms rollback only after successful atomic private write; expiry cannot be confirmed by late completion.

- [ ] Test strict unknown-field rejection, version rejection, bounded input, private path resolution under `XDG_CONFIG_HOME` and `$HOME/.config`, and atomic write semantics using a temporary test directory.
- [ ] Add a dedicated output worker with one operation in flight and native eventfd completion; use the secure `PrivateConfigFile` facility, not a new file-security scheme.
- [ ] Test Keep remains `pending_confirmation` with deadline armed while write is pending; successful write clears the transaction; failed write returns `output_persist_failed` and keeps rollback armed.
- [ ] Test expiry before persistence completion starts rollback; late worker success cannot confirm; if it persisted the temporary record, serialize compensation to the exact previous configuration before accepting another mutation. A compensation failure stays explicit and blocks another Display mutation.
- [ ] Ensure Keep's control response is queued only after persistence outcome and always contains the complete authoritative output-list snapshot or structured rejection.
- [ ] Run focused persistence/transaction tests and commit this task.

### Task 7: Qualification gates, documentation, and final verification

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs` output capability projection
- Modify: `docs/control-plane.md` and any directly relevant `docs/astreactl.md` output-control description
- Modify: `Settings/README.md` and `Settings/docs/ARCHITECTURE.md` only where existing truth needs updating
- Do not modify: `Settings/qml/pages/system/Display.qml`
- Test: Typhon crate test suite; Eclipse existing Display and Settings suites

**Consumes:** Complete runtime path, transaction/wire semantics, persistence/startup behavior, and tests from prior tasks.

**Produces:** Honest Atomic-only capability advertisement, current documentation, verified deterministic suites, and commits on `main`.

- [ ] Advertise mode selection only when the active path has Atomic KMS, a full exact inventory, a qualified dimension-changing scanout path, TEST_ONLY, and rollback. Keep Legacy and unsupported scanout false; keep scale/transform/topology/position/enable/VRR false.
- [ ] Update output-control docs to remove “mode reconfiguration does not exist” only after all required runtime and rollback owners are wired. Document exact opaque runtime IDs, stable persisted fingerprint, deadline, stale-generation rejection, startup precedence, identity/EDID limitation, unsupported mutations, and unqualified hardware status.
- [ ] Review Eclipse current backend and tests; change only mismatched final transaction semantics. Run existing read-only structural/display guards and leave `Display.qml` untouched.
- [ ] Run Typhon tests only after printing `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`; run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test`.
- [ ] Before Eclipse configure/build/test, print and verify `/mnt/Aether/Desktop/GitHub/Eclipse-build`; configure with `cmake -S /home/agony/GitHub/Eclipse -B /mnt/Aether/Desktop/GitHub/Eclipse-build -DCMAKE_BUILD_TYPE=Debug`, build that exact tree, and run `ctest --test-dir /mnt/Aether/Desktop/GitHub/Eclipse-build --output-on-failure`.
- [ ] Perform the final adversarial search from the task: no persisted opaque IDs, reconstructed DRM modes, different TEST_ONLY/real candidate, premature scanout destruction/direct release, stale completion authority, stale input/cursor/scheduler/cache state, per-frame generation increment, ledger conflation, timer thread, rollback disarm before durable write, or QML mutation controls.
- [ ] If Atomic KMS hardware is present and deterministic tests are green, run the authorized live refresh/resolution apply/revert/keep and client-disconnect qualification matrix. Record it separately from deterministic results. If no hardware is available, state that clearly and do not claim live qualification.
- [ ] Confirm only intended files are staged, leave the user's untracked `msg.txt` untouched, and commit the final documentation/capability/test change.

## Plan self-review

- Every requirement group is assigned to a task: exact native inventory and startup resolution (1), candidate identity/adoption (2), scanout ownership and synchronous transition (3), async protocol and timeout transaction (4), runtime authorities and presentation reset (5), Keep persistence and expiry race (6), capability honesty/docs/frontend guards/test runs/hardware distinction (7).
- The implementation uses no second timer source, no QML timer authority, no parallel applied-output source, and no `OutputTransactionLedger` reuse.
- The plan's public-facing output mode list remains bounded; mode truncation never creates IDs or exposes an unadvertised native entry.
- The single-output/connector-name identity limit is explicit because Typhon does not own EDID identity yet.
