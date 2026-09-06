# Buffering timing implementation plan

**Goal:** Remove temporary allocations from buffering prediction, worker budget
calculation, and fixed-size pipeline validation while retaining exact results.

**Architecture:** Retain existing bounded journals and ownership models. Replace
temporary vectors with stack scratch storage and optional arrays.

**Tech stack:** Rust standard library, Cargo, rtk.

**Execution:** Inline in the existing checkout, as requested; no subagents.

## Steps

- [ ] Add allocation regressions in `tests/frame_timing_allocations.rs`; run
  `rtk cargo test --test frame_timing_allocations --locked` and verify failure.
- [ ] In `src/native/adaptive_buffering.rs` and
  `src/native_output/kms_worker/timing.rs`, copy deque slices into
  `[0; SAMPLE_CAPACITY]`, slice to the populated length, and use
  `select_nth_unstable(rank - 1)` with the existing nearest-rank formula.
- [ ] Compare against a fully sorted oracle for every history length through
  capacity and several ring rotations; verify allocation regressions pass.
- [ ] In `src/native_output/presentation/pipeline.rs`, use four optional slot
  entries and three optional targets. Flatten targets before adjacent comparisons
  so absent owners cannot hide aliasing or target-order violations.
- [ ] Run pipeline, swapchain, adaptive-buffering, and worker regressions; run
  formatting, the full suite, and Clippy. Record results and limitations.
- [ ] Review the diff, stage only these files and tests/docs, and commit.
