# Effect GPU Graph Gap Attribution Implementation Plan

> **For agentic workers:** Execute this plan inline. Repository instructions prohibit subagents.

**Goal:** Attribute the largest contiguous interval outside timed effect passes using existing absolute GPU timestamp results.

**Architecture:** Preserve `(start_ns, end_ns)` through span resolution. Keep first and last pass endpoints, the largest inter-pass gap, and timeline validity in each scope's `GraphAggregate`; derive pre/post candidates only when TOTAL resolves. Emit one set of aggregate scalar fields on the existing timing record and document their limits.

**Tech Stack:** Rust, existing EGL timestamp query pool, internal deterministic unit tests, Markdown qualification documentation.

## Global Constraints

- Keep production implementation in `src/egl_renderer/effects/gpu_timing.rs` and update `docs/EFFECTS_QUALIFICATION.md`.
- Issue zero new GPU queries; retain `TIMING_SPAN_POOL_CAPACITY = 2048`, `TIMING_QUERY_OBJECT_CAPACITY = 4096`, and four `query_counter(... TIMESTAMP)` call sites.
- Do not modify render executor behavior, shaders, Replay behavior, EGL priority, or presentation behavior.
- Keep attribution state O(1) per active graph scope and keyed by exact `scope_id`.
- Run Cargo output and caches only under `/mnt/Aether/Desktop/GitHub`.

---

### Task 1: Add failing synthetic gap tests

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs` (test module only)

**Interfaces:**
- Consume existing `TimingState`, `GraphTimingScope`, `pass_metadata`, `capture_metadata`, and `poll_front` test helpers.
- Assert the new record fields `graph_gap_attribution_available` and `max_graph_gap` once implemented.

- [x] Add synthetic cases for inter-gap winning an equal post gap, pre winner, post winner, largest of multiple inter gaps, and earliest equal inter gap.
- [x] Add a capture-boundary case with Replay mode and non-zero checkpoint count.
- [x] Add same-frame separate-scope, dropped-pass, invalid interval, cross-pass overlap, first/last outside TOTAL, and no-valid-pass cases.
- [x] Extend the canonical formatter uniqueness regression to assert all new keys once and stable unavailable values.
- [x] Run the focused test selector and confirm it fails on missing behavior before implementing production code.

### Task 2: Implement bounded scope attribution and telemetry

**Files:**
- Modify: `src/egl_renderer/effects/gpu_timing.rs`

**Interfaces:**
- `poll_front` passes the validated absolute timestamps into `finish_pass` and `finish_total` while retaining the existing duration calculation.
- `TimingSpanMetadata` creates immutable `TimedPassBoundary` identities.
- `GraphAggregate` holds first/last endpoints, the largest inter gap, and a validity flag; `GpuTimingRecord` carries finalized attribution.

- [x] Track successful pass intervals in resolve order and invalidate attribution on dropped, invalid, unidentified, or out-of-order pass spans.
- [x] Select pre, inter, and post candidates chronologically with strict `>` replacement; preserve the earliest equal inter gap.
- [x] Require a valid TOTAL, at least one valid pass, no dropped pass, and pass endpoints within TOTAL for availability.
- [x] Append each required scalar field once to `event=effect_gpu_timing`; unavailable state uses zeros and `none`.
- [x] Preserve existing duration/category/max-pass/Replay aggregation, exact scope ownership, slot reuse, and disjoint cleanup.

### Task 3: Document timeline evidence and verify invariants

**Files:**
- Modify: `docs/EFFECTS_QUALIFICATION.md`

- [x] Explain the total remainder and largest contiguous gap, including the factual adjacent-pass boundary contract and possible timeline contents without assigning a cause.
- [x] Verify synthetic gap accounting against `graph_unattributed_ns` without adding redundant telemetry.
- [x] Inspect source to confirm unchanged pool capacities, timestamp query call-site count, and untouched executor/render paths.

### Task 4: Run requested verification and native qualification

**Files:** None

- [x] Print and verify the effective Aether Cargo target/cache directories before the first Cargo command.
- [x] Run `rtk cargo test --locked gpu_timing` and `rtk cargo test --locked effect_gpu_timing`.
- [x] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, and `rtk cargo test --locked` (check passed; fmt/clippy and the full suite report unrelated pre-existing repository issues).
- [x] Attempt the authorized 1920x1080@165 qualification workload and record the concrete environment blocker: this shell is a Hyprland Wayland session rather than the required native Typhon TTY compositor session.
- [x] Commit the completed logical changes without including unrelated pre-existing edits.
