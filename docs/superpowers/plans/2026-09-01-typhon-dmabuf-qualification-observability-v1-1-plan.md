# Typhon DMA-BUF Qualification Observability v1.1 Implementation Plan

> **For agentic workers:** Execute this plan inline in the current session. Do not dispatch subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make composited timestamp-unavailable failures explicitly unpairable while preserving exact timestamp classification and all existing release behavior.

**Architecture:** Keep the existing `DmabufGpuReleaseOrigin`, bounded 256-entry transaction ledger, readable sync-file query, and kernel-derived pageflip pairing unchanged. Adjust only the origin-specific metric accounting in `DmabufGpuReleaseObservability::note_timestamp_unavailable`, with a regression test for a composited failure that arrives after its ledger entry is already gone.

**Tech Stack:** Rust, Cargo unit tests, `BoundedSamples`, `rtk` command proxy.

## Global Constraints

- Preserve the accepted `Composited GPU release lease -> exact OutputTransactionId -> exact sync-file signal timestamp vs kernel-derived pageflip timestamp` architecture.
- Preserve `DmabufGpuReleaseOrigin`, the bounded 256-entry correlation ledger, exact transaction pairing, NoVisual exclusion, DeferredRetry exclusion, GPU protocol-release ownership, Direct/KMS safety, current-token revalidation, retry debt, O1, SHM, damage, and KMS scheduling.
- Do not add fields to `NativeRenderFence` or alter GPU release timing, protocol completion, Direct Scanout, KMS worker behavior, retry scheduling, O1, SHM, damage, buffer age, input/pointer code, or native qualification behavior.
- Treat `signal_timestamp_ns < registered_at_ns` as a valid pre-signaled completion: increment `already_signaled_before_registration`, record zero registry wait, and leave `timestamp_order_anomalies` unchanged.
- Keep `gpu_release_registry_wait_p50_us`, `gpu_release_registry_wait_p95_us`, and `gpu_release_registry_wait_p99_us` defined as remaining wait from async-watch registration, including zero samples for pre-signaled fences.
- For `Composited { transaction_id }` timestamp lookup failure, remove the exact correlation if present and increment `correlations_unpairable_signal_timestamp`; NoVisual and DeferredRetry only increment `signal_timestamp_unavailable`.
- Continue protocol release exactly once after a readable completion FD even when timestamp metadata is unavailable.
- Run focused tests and the exact requested `rtk` verification suite; do not run native KMS qualification, ydotool, or desktop screenshots.

---

### Task 1: Add the missing origin-level unpairable metric regression

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs:1245-1261` test module

**Interfaces:**
- Consumes: `DmabufGpuReleaseObservability::note_timestamp_unavailable` and `DmabufGpuReleaseOrigin::Composited`.
- Produces: A failing test that defines `correlations_unpairable_signal_timestamp` as a count of composited timestamp failures even when no ledger entry remains.

- [ ] **Step 1: Write the failing test**

Add this test immediately after `unavailable_composited_timestamp_removes_unpairable_correlation`:

```rust
#[test]
fn unavailable_composited_timestamp_counts_without_a_pending_correlation() {
    let mut observability = DmabufGpuReleaseObservability::default();
    let transaction_id = transaction_id(603);

    observability.note_timestamp_unavailable(DmabufGpuReleaseOrigin::Composited {
        transaction_id,
    });

    let summary = observability.summary();
    assert_eq!(summary.signal_timestamp_unavailable, 1);
    assert_eq!(summary.correlations_unpairable_signal_timestamp, 1);
    assert_eq!(summary.correlation_pending, 0);
    assert_eq!(summary.composited_correlations_paired, 0);
    assert_eq!(summary.release_before_pageflip_leases, 0);
    assert_eq!(summary.release_after_pageflip_leases, 0);
    assert_eq!(summary.release_same_timestamp_leases, 0);
}
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `rtk cargo test unavailable_composited_timestamp_counts_without_a_pending_correlation -- --exact`

Expected: FAIL because the current implementation increments the unpairable counter only when `correlations.remove(&transaction_id)` returns `Some`.

### Task 2: Make timestamp-unavailable accounting origin-correct

**Files:**
- Modify: `src/native_output/runtime/dmabuf_release.rs:262-273`

**Interfaces:**
- Consumes: the existing `note_timestamp_unavailable(origin: DmabufGpuReleaseOrigin)` call from `service_ready_with_timestamp`.
- Produces: Origin-correct bounded metrics with no change to watch removal, protocol completion, physical timestamp comparison, or registry capacity.

- [ ] **Step 1: Implement the minimal change**

Change the existing method to remove the transaction independently of the counter increment:

```rust
fn note_timestamp_unavailable(&mut self, origin: DmabufGpuReleaseOrigin) {
    self.summary.signal_timestamp_unavailable =
        self.summary.signal_timestamp_unavailable.saturating_add(1);
    if let DmabufGpuReleaseOrigin::Composited { transaction_id } = origin {
        self.correlations.remove(&transaction_id);
        self.summary.correlations_unpairable_signal_timestamp = self
            .summary
            .correlations_unpairable_signal_timestamp
            .saturating_add(1);
    }
}
```

- [ ] **Step 2: Run the focused tests to verify they pass**

Run: `rtk cargo test dmabuf_release -- --exact` (if the test filter does not select module tests, run `rtk cargo test unavailable_composited_timestamp -- --exact` and the full focused module command used by the repository).

Expected: PASS for the new test and the existing unavailable-timestamp tests, with no physical before/after/same classification added.

- [ ] **Step 3: Refactor only if needed while keeping tests green**

Keep the one origin match and saturating counters. Do not rename metrics or introduce a secondary timestamp field.

### Task 3: Verify the complete observability closure and commit

**Files:**
- Verify: `src/native_output/runtime/dmabuf_release.rs`
- Verify: `docs/superpowers/specs/2026-09-01-typhon-dmabuf-qualification-observability-v1-1-design.md`
- Verify: `docs/superpowers/plans/2026-09-01-typhon-dmabuf-qualification-observability-v1-1-plan.md`

**Interfaces:**
- Consumes: the existing RED 1–5 tests and all repository checks requested in the user brief.
- Produces: A clean, committed observability-only patch with no native KMS qualification run.

- [ ] **Step 1: Run focused DMA-BUF tests**

Run: `rtk cargo test dmabuf_release`

Expected: all DMA-BUF release unit tests pass, including pre-signaled zero-wait, mixed percentile, physical-classification independence, unavailable-timestamp eviction, repeated-failure capacity, duplicate admission, and exactly-once release tests.

- [ ] **Step 2: Run the required repository verification**

Run each command fresh:

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
git diff --check
git status --short
```

Expected: each command exits successfully; no native DRM/KMS test is run.

- [ ] **Step 3: Inspect the final diff**

Run: `rtk git diff -- src/native_output/runtime/dmabuf_release.rs docs/superpowers/specs/2026-09-01-typhon-dmabuf-qualification-observability-v1-1-design.md docs/superpowers/plans/2026-09-01-typhon-dmabuf-qualification-observability-v1-1-plan.md`

Expected: only the origin-counter test, the minimal counter-accounting change, and the two workflow documents are present; no accepted runtime architecture is modified.

- [ ] **Step 4: Commit the implementation**

```bash
rtk git add src/native_output/runtime/dmabuf_release.rs docs/superpowers/plans/2026-09-01-typhon-dmabuf-qualification-observability-v1-1-plan.md
rtk git commit -m "fix: account unpairable DMA-BUF timestamps by origin"
```
