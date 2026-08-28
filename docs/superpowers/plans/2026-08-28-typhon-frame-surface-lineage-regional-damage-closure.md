# Typhon Frame Surface Lineage and Regional Damage Closure

> **Execution note:** This approved plan is executed inline in the current
> checkout. No subagents or worktrees are used.

## 1. Add RED coverage

- Add native snapshot tests proving popup and subsurface map/unmap/move and
  middle-span reorder damage are regional while visibility/global invalidation
  remains full output.
- Add evidence tests proving authoritative empty content remains empty and
  journal history loss repairs the current footprint.
- Add frame-batch tests proving exact scene sampling excludes unrelated
  surfaces, physical presentation settles only sampled surfaces, and a
  non-presented frame does not consume damage.
- Add READY lineage and index-consistency tests where the current test seams do
  not already cover them.
- Add the deterministic overlapping topology pixel oracle and integrate client
  buffer rotation with output triple-buffer age and reference rendering.
- Run the focused tests unchanged and record their expected failures.

## 2. Implement evidence and regional scene damage

- Preserve `HistoryLost` through compositor settlement and expose explicit
  native snapshot evidence.
- Regionalize popup/subsurface lifecycle and ordered middle-span transitions.
- Retain explicit conservative FullOutput handling for visibility and external
  overlays, with bounded reason labels/counters where the existing metrics
  boundary supports them.

## 3. Close exact-frame ownership

- Capture the damage token from the exact resolved scene used by the paint call.
- Ensure READY, admission, kernel ownership, and physical pageflip retain the
  same token; only physical presentation settles it.
- Remove any remaining normal-path global capture/settlement dependency while
  preserving Direct Scanout's exact candidate identity.
- Replace any remaining steady-state global vector lookup with the existing
  global index; leave topology rebuilds as cold-path work.

## 4. Verify and commit

- Run focused RED/GREEN tests after each invariant, then formatting, locked
  check, locked Clippy, locked full tests, diff check, and status.
- Review every requested caller and the O1/SHM/DMA-BUF/Direct Scanout paths for
  unchanged ownership and timing semantics.
- Run native qualification only if the DRM/KMS TTY and input environment is
  genuinely available; otherwise report it as not run.
- Commit the implementation and verification-ready result with focused commit
  history and report exact outputs, remaining conservative follow-ups, and
  native qualification status.
