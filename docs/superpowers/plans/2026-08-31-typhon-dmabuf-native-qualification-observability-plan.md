# Typhon DMA-BUF Native Qualification Observability Plan

## Scope

Add bounded, timestamp-authoritative observability to the existing DMA-BUF GPU
release registry. This plan does not change release ownership, current-token
revalidation, Direct/KMS safety, retry debt, O1 admission, SHM lifetime,
regional damage, buffer age, or KMS scheduling.

## Design

1. Extend each release watch with an origin, exact output transaction identity
   for normal composited frames, obligation count, and monotonic registration
   timestamp.
2. Keep a fixed-capacity transaction correlation ledger in the release
   registry. It accepts exact sync-file signal timestamps and the existing
   kernel-derived `presented_at_ns` timestamp in either arrival order.
3. Reuse `BoundedSamples` for fence wait, GPU-before-pageflip lead, and
   pageflip-before-GPU lag percentile summaries.
4. Query sync-file metadata only after a release FD is readable and before the
   FD is dropped. Timestamp-query failure increments observability counters but
   never prevents an already-proven protocol release.
5. Add only a metrics hook at the Atomic composited pageflip terminal and one
   bounded shutdown timing summary. No runtime ownership or presentation state
   transition is changed.

## Registration-timing correction

The async-watch registration timestamp is not a fence-creation timestamp. A
sync-file signal observed before watch registration is therefore a valid
pre-signaled completion: count it separately and record a zero remaining
registry wait. Exact GPU-vs-pageflip classification continues to compare only
the sync-file signal timestamp with the kernel-derived pageflip timestamp.

If an exact signal timestamp cannot be queried for a composited watch, remove
that transaction from the correlation ledger as unpairable. Preserve the safe
protocol completion and keep NoVisual/DeferredRetry uncorrelated.

## Test-first sequence

- RED: correlation ordering, before/after/equal classification, unavailable
  timestamps, origin exclusion, percentile semantics, inversion handling,
  capacity, transaction isolation, and exactly-once registry completion.
- RED v1.1: pre-signaled zero-wait accounting, registration-wait percentile
  inclusion, physical-classification independence, unpairable correlation
  removal, repeated unavailable-timestamp boundedness, and successful-arm
  metric semantics.
- GREEN: implement the ledger and watch metadata.
- GREEN: wire normal composited registration to `OutputTransactionId`, the
  readable-FD timestamp query, and the exact DRM pageflip timestamp.
- GREEN: add shutdown summary output and run focused tests.

## Verification

Run the focused DMA-BUF release tests, then `rtk cargo fmt --check`, `rtk cargo
check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`,
`rtk cargo test`, `git diff --check`, and `git status --short`. A native DRM/KMS
qualification is attempted only if a suitable TTY/device environment is
actually available; otherwise the report explicitly separates static/unit
evidence from unavailable native evidence.
