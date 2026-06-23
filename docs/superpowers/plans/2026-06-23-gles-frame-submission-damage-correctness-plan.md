# Implementation Plan: GLES Frame Submission And Damage Correctness

## Architecture Decisions

- Make skipped versus rendered a typed boundary before GL and EGL work.
- Keep surface commits journaled independently from renderer-presented state.
- Commit scene and buffer-age state only after successful swap.
- Keep one ready-frame contract for nested, legacy KMS, and Atomic KMS callers.
- Leave partial repaint opt-in until software and real-hardware validation pass.

## Task List

1. Add failing planner/submission tests proving empty logical damage is a typed
   skip and cannot execute draw, swap, history, GBM, or KMS transitions.
2. Add `Empty` surface damage semantics, normalized union/clipping tests, and a
   bounded surface commit journal with history-loss behavior.
3. Split pending surface/buffer damage and add commit-time scale/viewport
   conversion tests with conservative full fallback.
4. Route normal, damage-only, cursor, synchronized-tree, and explicit-sync
   publication through commit journaling without changing transaction timing.
5. Add presented element snapshots and old/new/resource/commit-relative output
   damage tests, including failure/retry behavior.
6. Refactor repaint planning and GL execution into explicit skipped/rendered
   outcomes; make partial repaint opt-in and preserve diagnostic precedence.
7. Integrate nested and native EGL callers so swap success commits history and
   native GBM/KMS readiness occurs only after successful swap.
8. Make SHM resources upload all unseen commit damage while retaining stable
   dmabuf `BufferId` cache behavior.
9. Add deterministic three-buffer reference-oracle and end-to-end mock legacy /
   Atomic ready-sequence regression tests.
10. Add bounded diagnostics and update architecture, native-session, and known-
    issues documentation with the actual validation status.
11. Run focused tests, formatter, workspace check, clippy, full tests, release
    build, and diff checks; record unavailable TTY/hardware validation honestly.

## Checkpoints

- After tasks 1-3: damage and frame-outcome unit tests demonstrate red/green.
- After tasks 4-6: transaction, scene, planner, and retry suites pass.
- After tasks 7-9: native/nested contract and swapchain oracle pass.
- After tasks 10-11: workspace validation and documentation are complete.

## Risks And Mitigations

- Renderer resource preparation currently mutates caches before swap. Keep
  resource upload success separate from presented-scene acceptance and force
  conservative damage when resource identity changes.
- Existing native scheduling has an early empty-damage optimization. Retain it,
  but make the renderer outcome authoritative so every caller is safe.
- Surface generation is shared with resize and synchronized-tree publication.
  Add commit counters alongside generation instead of replacing those contracts.
- Hardware validation requires a real TTY and DRM device. Never infer those
  results from mocks or the software oracle.

## Open Questions

None. TASK 04.1 supplies and approves the behavior, precedence, scope boundary,
and acceptance matrix.
