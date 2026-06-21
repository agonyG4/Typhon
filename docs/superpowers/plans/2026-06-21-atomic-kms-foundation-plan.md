# Implementation Plan: Atomic KMS Foundation

## Phase 1: Typed model

- [ ] Add KMS policy/effective-backend types and startup fallback decisions.
- [ ] Add typed object/property IDs, discovery snapshots, and required/optional
      property validation.
- [ ] Add primary-plane selection and checked 16.16 geometry.
- [ ] Add deterministic request and commit-state models.

Checkpoint: focused pure tests pass and existing code still builds.

## Phase 2: DRM boundary and startup

- [ ] Add atomic capability, property/plane enumeration, mode blob, and raw
      atomic ioctl wrappers.
- [ ] Capture restoration state and construct initial/restore/disable requests.
- [ ] Integrate `TEST_ONLY|ALLOW_MODESET`, real initial atomic commit, and
      startup-only legacy fallback.

Checkpoint: initial-state tests and native KMS selection tests pass.

## Phase 3: Runtime presentation

- [ ] Route EGL/GBM and CPU GBM flips through the selected KMS backend.
- [ ] Preserve one-pending token/buffer state and retry ready buffers on ioctl
      rejection.
- [ ] Keep DRM event parsing, scheduler completion, and presentation metadata
      shared.
- [ ] Restore or safely disable before scanout resources are dropped.

Checkpoint: atomic state, buffer ownership, pageflip, cursor, scheduler, and
explicit-sync tests pass.

## Phase 4: Diagnostics and validation

- [ ] Add probe, selection, initial commit, runtime rejection/completion, and
      cleanup diagnostics.
- [ ] Document policy, lifecycle, cursor limitation, and non-goals.
- [ ] Run focused tests and all repository quality gates.
- [ ] Perform safe hardware takeover validation when the session permits it;
      otherwise report it unavailable without claiming runtime success.

## Risks

- Driver property sets differ: required properties fail closed; optional
  properties are diagnostics only.
- Atomic event user data is absent from the existing convenience wrapper: keep
  the raw ioctl boundary isolated and test serialization/token preservation.
- Restoration may reference resources owned by another client: test exact
  restore first and use one atomic safe-disable fallback.
- Legacy cursor/atomic primary-plane mixing is driver-sensitive: do not touch
  cursor-plane properties and retain the existing software fallback.
