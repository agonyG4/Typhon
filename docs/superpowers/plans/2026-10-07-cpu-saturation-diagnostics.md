# Typhon CPU Saturation Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` for inline execution. Repository instructions require main, prohibit branches and subagents, and put build output on Aether.

**Goal:** Add a repeatable external probe and runbook for diagnosing Typhon responsiveness under CPU saturation without changing compositor scheduling or cycle behavior.

**Architecture:** A standard-library Python tool samples Linux procfs, cgroup v2 and sysfs outside Typhon's frame path. A runbook pairs those samples with Typhon's existing opt-in traces and a timed, ordinary-priority CPU workload.

**Tech Stack:** Python 3 standard library, Linux procfs/cgroup v2/sysfs, Markdown.

## Global Constraints

- Work on the current `main` checkout; do not create a branch or worktree.
- Preserve existing unrelated working-tree modifications.
- Do not build in the checkout or on the system SSD; any future Cargo build must set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` after verifying the path.
- Do not run a CPU stress workload in the current pressured desktop session.
- Do not change native scheduling, compositor work budgets, event ownership, KMS ownership or presentation behavior without target evidence.
- Keep the probe opt-in, standard-library-only, and require output under `/mnt/Aether/Desktop/GitHub`.

---

### Task 1: Add the external Linux sampling tool

**Files:**
- Create: `scripts/typhon_cpu_saturation_probe.py`
- Test: `tests/tooling/test_typhon_cpu_saturation_probe.py`

**Interfaces:**
- CLI: `--pid PID --duration SECONDS --interval SECONDS --output PATH`; reject paths outside `/mnt/Aether/Desktop/GitHub`.
- Probe emits JSON Lines: one metadata record, time-stamped system/cgroup samples, per-TID scheduler samples, and a final percentile summary.
- Testable pure helpers parse schedstat, PSI and cgroup CPU counters and compute percentiles.

- [x] Parse target TIDs and `schedstat` runtime/wait/timeslice counters; report nice, scheduler class and affinity.
- [x] Sample global CPU, memory and I/O PSI; visible cgroup `cpu.max`, `cpu.stat` and `cpu.weight`; available CPU frequency and thermal sensors.
- [x] Compute per-interval CPU percentage and runnable-wait time per second; include per-thread-instance p50/p95/p99 summaries without treating missing counters as zero.
- [x] Require explicit PID and output path, validate duration/interval, detect PID reuse, and stop cleanly if the target exits.
- [x] Unit-test fixture parsing and percentile behavior with the standard library.

### Task 2: Document the qualification procedure

**Files:**
- Create: `docs/cpu-saturation-qualification.md`

**Interfaces:**
- Commands invoke the probe from Task 1 and place output under `/mnt/Aether/Desktop/GitHub`.
- Runbook records the exact existing trace switches, metric semantics, workload stages, repetition counts and known qualification gaps.

- [x] Document three 300-second idle, interaction, and animation/video trials plus three 120-second CPU-load trials for the 1920x1080@165 Hz NVIDIA target.
- [x] Document a normal-priority CPU-only `stress-ng` workload behind `timeout --kill-after`; keep it independent of the probe.
- [x] Explain schedstat runnable wait versus `epoll_wait` blocked duration, pointer transition timing versus physical presentation, and cgroup/swap/GPU/thermal confounders.
- [x] Record that this turn has no Typhon hardware baseline and recommends keeping `nice` and `rr` disabled.

The target-machine trials remain unrun because Typhon was not running, memory
pressure was elevated, and `stress-ng` was unavailable in the investigated
session. Treat hardware qualification as a follow-up, not as completed evidence.

### Task 3: Validate and review

**Files:**
- Review: `scripts/typhon_cpu_saturation_probe.py`
- Review: `tests/tooling/test_typhon_cpu_saturation_probe.py`
- Review: `docs/cpu-saturation-qualification.md`

- [x] Run `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests/tooling -p 'test_typhon_cpu_saturation_probe.py'`.
- [x] Run `python3 -B -c 'import ast, pathlib; ast.parse(pathlib.Path("scripts/typhon_cpu_saturation_probe.py").read_text())'` for syntax validation without bytecode files.
- [x] Inspect the staged diff and confirm no Rust runtime or existing user-modified file changed.
- [x] Commit the diagnostic tool, tests and runbook as one logical change.

## Handoff

Execute inline. Do not dispatch subagents or run a saturation workload on the
current desktop. A real before/after target trial remains a qualification item
for a session with Typhon running and adequate memory headroom.
