# Typhon Surface and Presentation Locality v2 Implementation Plan

> **Execution note:** This plan is executed inline in the current checkout;
> no subagents or worktrees are used. The supplied task brief is the approved
> design authority.

## 1. Establish RED coverage

- Add focused tests for the global index invariant across append, replacement,
  removal, reorder, tree/window operations, minimize/restore, XWayland, and
  teardown.
- Add deterministic locality tests for Wayland, damage-only, XWayland, and
  keyed presentation capture/settlement with unrelated population.
- Add lineage tests for final scene filtering, fullscreen/popup membership,
  old/new commits, stale generations, destruction, cursor delivery, cursor
  supersession, Direct Scanout, and the client/output swapchain oracle.
- Run the smallest relevant `rtk cargo test` commands and record the expected
  failures before production implementation.

## 2. Implement indexed surface authority

- Add the global index and bounded observability counters.
- Centralize append, in-place replacement, and index rebuild operations.
- Update every real topology mutation to leave the index valid.
- Convert ordinary Wayland buffer/damage-only and incremental active-scene
  publication to indexed access.

## 3. Separate XWayland content from topology

- Audit ordinary mapped commits for unchanged identity, placement, root,
  stack, and visual geometry.
- Replace same-topology content publication in place.
- Retain topology work only for actual map/unmap, attachment, placement,
  restack, family/tree, minimize/restore, or visual-input changes.
- Preserve conservative XWayland Full damage.

## 4. Bind exact presentation lineage

- Add filtered keyed primary capture after final resolved-scene creation.
- Attach tokens to compatibility frame batches and explicit rendered frames.
- Freeze software and hardware client-cursor identity at render/transaction
  preparation time.
- Give cursor-only plane transactions independent token ownership and settle
  them only on their own confirmed pageflip.
- Keep Direct Scanout candidate-local.

## 5. Verify and report

- Run focused tests, formatting, locked check, full locked tests, Clippy,
  diff-check, and source-layout validation when available through `rtk`.
- Perform an explicit ownership/correctness adversarial review and a separate
  locality/accidental-scan review.
- Write the English closure report with source audit, complexity evidence,
  verification outcomes, known blockers, and the explicit hardware-
  qualification statement.
