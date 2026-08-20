# Typhon KMS Commit Worker Timing and Throughput Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Determine whether Typhon's KMS commit worker has an evidence-backed timing or throughput defect, correct only the demonstrated cause, and qualify the worker against synchronous KMS without weakening ownership or deadline semantics.

**Architecture:** Keep the bounded `KmsCommitJob`/eventfd/Condvar worker and immutable transaction ownership. First separate worker-specific timing telemetry from main-loop telemetry and run an alternating Triple/Worker 2×2 experiment. Any production timing change must be preceded by a deterministic failing model test and must use pageflip/runtime feedback as the authoritative observation source.

**Tech Stack:** Rust, Cargo, DRM atomic KMS, existing `TimingSummary`/bounded samples, Typhon control snapshots, `/home/agony/Typhon-perf/harness`, native Wayland, vkmark FIFO.

## Global Constraints

* Preserve unrelated dirty worktree changes and never reset, stash, clean Cargo, delete `target/`, or disable the KMS worker as a benchmark fix.
* Reuse the existing target/cache and benchmark harness.
* Do not add per-frame text logging, busy waits, GPU clock forcing, privilege requirements, arbitrary vendor constants, or queue-capacity expansion without evidence.
* Preserve `KmsCommitJob`, bounded admission, `WorkerPacingReservation`, validation, cursor sidecars, generations, pageflip tokens, quiesce/shutdown, Direct Scanout, and async/presentation semantics.
* Use the existing graph for structural discovery, check coverage for relied-on paths, and read flagged source ranges directly.

---

### Task 1: Establish committed-tree integrity

**Files:**
- Verify: `src/native_output/runtime/metrics.rs`
- Verify: `src/native_output/kms_worker/mod.rs`

- [x] Inspect `HEAD` versions and the dirty diff.
- [x] Compile detached clean `HEAD` against the existing target cache.
- [x] Commit only the missing `WorkerMetricsSnapshot` re-export when confirmed.
- [x] Recompile detached clean `HEAD` after the corrective commit.

Expected result: clean committed tree compiles independently; corrective commit is isolated as `2d3ff99`.

---

### Task 2: Map worker timing and result ownership

**Files:**
- Read: `src/native_output/kms_worker/timing.rs`
- Read: `src/native_output/kms_worker/thread.rs`
- Read: `src/native_output/kms_worker/queue.rs`
- Read: `src/native_output/kms_worker/payload.rs`
- Read: `src/native_output/kms_worker/presentation_executor.rs`
- Read: `src/native_output/runtime/kms_worker.rs`
- Read: `src/native_output/runtime/presentation.rs`
- Read: `src/native_output/runtime/presentation_worker.rs`
- Read: `src/native_output/runtime/presentation_ready.rs`

- [ ] Record the exact production call path for `submit_at`, `observe_submission`, worker wait, ioctl, worker eventfd, main-thread acknowledgement, and pageflip settlement.
- [ ] Separate main-loop wake samples from worker-submit wake samples in the evidence notes.
- [ ] Identify whether `observe_submit_delta_ns` has any production caller; if it is orphaned, defer policy changes until the feedback semantics are specified.
- [ ] Record TEST_ONLY and real-submit accounting paths before changing validation.

Expected result: one written timing diagram and an explicit list of current metric-name/data-source mismatches.

---

### Task 3: Add worker-specific aggregate observability

**Files:**
- Modify: `src/native_output/kms_worker/queue.rs` or the existing worker timing owner
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/control_snapshots.rs`
- Test: focused worker timing/metrics tests in the existing worker test modules

- [ ] Write failing tests proving worker wake, ioctl, queue residency, signed submit-earliness, target-hit/miss, and late-before/after-ioctl aggregates remain distinct from main-loop wake metrics.
- [ ] Run the focused tests and confirm the expected failures.
- [ ] Implement bounded aggregates without per-frame strings or render-path locks.
- [ ] Expose current safety margin, worker-specific percentiles, TEST_ONLY/real submit counts and durations, and worker acknowledgement delay only if measured at the existing ownership boundary.
- [ ] Run focused tests and verify a snapshot read does not mutate scheduling state.

Expected result: `astreactl performance --json` can distinguish worker timing from main-loop timing with signed earliness preserved.

---

### Task 4: Run the alternating pre-change 2×2 experiment

**Files:**
- Read: `/home/agony/Typhon-perf/harness`
- Create outside production source: `/home/agony/Typhon-perf/artifacts/<timestamp>/`
- Report: `/home/agony/Typhon-perf/REPORT-2026-08-15-kms-worker-timing-closure.md`

- [ ] Build the current release from the corrected committed tree using the existing target cache.
- [ ] Run five measured runs plus one warmup for all cells, alternating order: Triple Auto/Worker On, Triple Off/Worker On, Triple Auto/Worker Off, Triple Off/Worker Off.
- [ ] Keep fullscreen FIFO, native Wayland, identical scene list, environment, binary, refresh, kernel, driver, and instrumentation policy.
- [ ] Record GPU graphics/memory clocks, utilization, power, temperature, P-state if available, CPU governor/frequency, CPU usage, task-clock, context switches, external frametime, and control snapshots.
- [ ] Preserve raw artifacts and calculate cell means, medians, variance, worker penalty by equivalent Triple mode, and interaction effect.

Expected result: a controlled classification into worker-wide, Triple interaction, vanished regression, or GPU cadence/power-state correlation.

---

### Task 5: Convert the demonstrated weakness into a failing deterministic model test

**Files:**
- Modify: `src/native_output/kms_worker/timing.rs`
- Test: `src/native_output/kms_worker/tests.rs` or a focused timing test module
- Possibly test: `src/native/presentation_deadline.rs`, `src/native/scheduler.rs`

- [ ] Select one root-cause hypothesis from the 2×2 and worker-specific measurements.
- [ ] Write the smallest failing test for that hypothesis: margin convergence, target selection, mode transition, pageflip feedback, acknowledgement delay, TEST_ONLY overhead, or another measured cause.
- [ ] Run the test and confirm it fails for the expected reason.
- [ ] Do not modify production policy until this failure is reproducible deterministically.

Expected result: the test demonstrates the exact unsafe/inefficient state without relying on vkmark timing.

---

### Task 6: Implement the minimum evidence-backed timing correction

**Files:**
- Modify only the files implicated by Task 5.
- Add or update focused tests beside the changed timing owner.

- [ ] Implement one correction at a time, preserving immutable job identity and bounded Condvar waiting.
- [ ] If adding feedback, use pageflip/runtime truth and ignore stale generation/transaction observations.
- [ ] If using mode timing, validate zero/malformed clock, interlace/doublescan/vscan, and refresh transitions.
- [ ] If testing best-effort RT scheduling, keep it non-fatal, minimum-priority, reset-on-fork, and optional; remove it if it does not measurably improve tails.
- [ ] Run the failing model test to green, then all focused worker/lifecycle tests.

Expected result: the measured cause is corrected without a new worker ownership or scheduling race.

---

### Task 7: Final 2×2 and live qualification

**Files:**
- Report: `/home/agony/Typhon-perf/REPORT-2026-08-15-kms-worker-timing-closure.md`
- Modify only production documentation if needed.

- [ ] Repeat the alternating 2×2 benchmark with the same protocol and machine state.
- [ ] Run native Wayland `vkcube`, fullscreen vkmark FIFO, Kitty idle, and Kitty rapid scroll.
- [ ] Compare worker-specific wake/ioctl/residency/earliness/target-hit/miss, commit-to-present, CPU, GPU power state, and external throughput.
- [ ] Confirm no worker identity error, pageflip assertion, broken Wayland connection, visual corruption, cursor regression, starvation, or shutdown failure.
- [ ] Calculate before/after worker penalties under equivalent Triple mode and classify any remaining tradeoff.

Expected result: Worker On is within the evidence-backed acceptance threshold or the report explicitly states why a larger difference remains and why the worker architecture is still justified.

---

### Task 8: Final review, validation, and commits

**Files:**
- Review all changed production files and the external report.

- [ ] Re-read KWin commit-thread/pipeline/connector/realtime references and the pinned Hyprland/Aquamarine path.
- [ ] Search the diff for timing sign/overflow, stale mode/generation, one-refresh errors, deadlocks, logging, locks, busy waits, RT privilege requirements, and Direct Scanout/cursor regressions.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, focused worker/pacing/render-ahead/presentation tests, `rtk cargo test --locked`, `rtk run "./bin/check-source-layout"`, and `rtk git diff --check`.
- [ ] Keep unrelated dirty files unstaged.
- [ ] Commit reviewable timing/telemetry changes and report exact final branch, HEAD, commits, and status.

Expected result: final report contains the integrity correction, pre/post 2×2 matrices, demonstrated cause, KWin/Hyprland comparison, tests, power observations, and an explicit worker verdict.
