# Typhon Native Frame Pacing v3.2.1 — Predictive O1 Observability Final Closure

Date: 2026-09-04

## Outcome

The v3.2.1 closure is implemented in source commit `9bdad3270145fd45e484a1ffc9afc6069ebec130`.
The accepted v3/v3.1 physical architecture and the v3.2 bounded Predictive O1
ledger remain unchanged. The remaining normal READY wait double count is
removed, and predictive terminal events now settle only the exact frame ID
owned by the caller.

## Root causes and fixes

`normal_ready_wait_count` was being incremented at both the READY waiting
transition and the later submit observation for `ReactiveDouble`. The submit
path still finalizes READY-wait timing, but no longer owns the count. The
counter is now owned exclusively by `note_ready_frame` at
`src/native_output/pacing.rs:2806`.

The former terminal resolver selected the first available identity from
`worker_reservation`, `ready`, or `active`. That priority order could settle a
different live Predictive O1 entry when frames overlapped. The resolver is now
replaced by exact-ID terminalization at
`src/native_output/pacing.rs:2517`; `None` and unknown IDs are no-ops.
Worker cancellation already carries the exact `pacing_frame_id`. Worker
overtake, safe-abandonment, and stale deferred-binding callers now pass their
own exact frame identity:

- `src/native_output/runtime/cycle/pageflip.rs:222` — queued worker overtake;
- `src/native_output/runtime/cycle/pageflip.rs:1052` — stale deferred binding;
- `src/native_output/runtime/kms_worker/rejection.rs:55` — worker rejection;
- `src/native_output/runtime/presentation_cycle.rs:1344` — stale deferred binding.

Identity and generation abandonment are represented as explicit terminal kinds,
so the exact ledger entry is removed before the corresponding legacy and O1
counters are incremented. The fixed four-entry capacity remains at
`src/native_output/pacing.rs:1713`. Duplicate settlement therefore cannot
inflate terminal counts or leave a live ledger remainder.

## Adversarial evidence

The focused pacing suite has 49 passing tests. The v3.2.1 regression coverage
proves:

- reactive READY wait followed by submit counts exactly one wait;
- repeated submit observation does not recount it;
- normal immediate submit remains at zero READY waits;
- three normal READY waits count as three, not six;
- normal READY accounting remains isolated from Predictive O1 accounting;
- a READY terminal settles the newer READY frame while an older worker entry
  remains live;
- a worker terminal settles the older worker frame while a newer READY entry
  remains live;
- an unknown exact terminal ID mutates neither the live entry nor its counters;
- identity/generation abandonment, duplicate submit, pageflip presentation,
  and shutdown drain each close the exact lifecycle they own.

The authoritative ledger equation remains:

```text
predictive_o1_created
= predictive_o1_presented
 + predictive_o1_abandoned_identity
 + predictive_o1_abandoned_generation
 + predictive_o1_other_safe_abandonment
 + predictive_o1_failed
 + predictive_o1_current_at_shutdown
```

No O1 admission threshold, deferred binding selection, physical claim,
predictor, fast-client attribution, DMA-BUF ownership,
OutputTransaction behavior, KMS timing or worker policy, scheduler, wake
ownership, ReactiveDouble immediate-submit behavior, Direct Scanout policy, or
SafeDisable behavior was changed.

## Static verification

All commands ran in `/home/agony/GitHub/Typhon` and reused its existing
`target/` directory.

```text
rtk cargo fmt --check: PASS
rtk cargo check: PASS
rtk cargo clippy --lib --all-features -- -D warnings: PASS
rtk cargo clippy --all-targets --all-features -- -D warnings: PASS
rtk cargo test native_output::pacing --no-fail-fast: PASS (49 passed)
rtk git diff --check: PASS before report creation
rtk cargo build --release: PASS (0 errors, 1 warning)
```

The release warning is the pre-existing unused `KEY_CAPSLOCK` constant in the
unrelated user-edited `src/native_output/input/events.rs:25`. The release
binary was built from the current checkout, whose separate keyboard/input
changes were preserved and excluded from the frame-pacing commit.

The fresh repository-wide test run had one unrelated flaky integration failure:

```text
rtk cargo test: failed once in tests/sigchld.rs
  one_child_exit_wakes_the_sigchld_signalfd_once
  main suite: 2069 passed, 0 failed, 2 ignored
  other reported suites: passed except that one sigchld test
isolated rerun: 1 passed
```

The exact test was rerun once, as requested; it passed. The full suite therefore
did not produce a clean single-run result in this environment, but no failure
was in the touched frame-pacing files.

Post-commit release identity:

```text
commit: 9bdad3270145fd45e484a1ffc9afc6069ebec130
sha256(target/release/oblivion-one):
c5444113cd1600e1291da168ac34037c263bd68a731e3572676a6eb2aa981186
```

## Native qualification

The approved native launcher was run with 1920x1080@165, Atomic KMS,
native-egl-gbm, eager XWayland, worker-auto, triple-auto, and Direct Scanout
off. It reached:

```text
/dev/dri/card1
DP-1
1920x1080@165Hz (exact)
direct DRM
explicit Atomic EGL/GLES GBM
```

It stopped before rendering at the pre-render Atomic TEST_ONLY commit:

```text
Permission denied (os error 13)
```

No workload ran, so hardware observations for Predictive O1 cadence, wake,
DMA-BUF, OutputTransaction, protocol, or SafeDisable cannot be inferred from
this attempt. The launcher also reported that user `agony` is not in the
`input` group. No ydotool, desktop screenshot, Eclipse modification, or
machine-configuration change was used.

## Acceptance status

Source-level v3.2.1 closure is verified and committed. Native hardware
acceptance remains inconclusive until the seat/DRM permissions allow the
pre-render Atomic TEST_ONLY commit. The next native run should confirm
nonzero submitted/presented Predictive O1 counts, exact terminal
reconciliation, no normal-wait inflation, and the already accepted cadence,
wake, DMA-BUF, transaction, protocol, and SafeDisable invariants.

## Final worktree note

After the source and report commits, the shared checkout still contained
separate unstaged keyboard/input and KMS cursor edits. A final validation
snapshot against that current worktree reported:

```text
rtk cargo fmt --check: BLOCKED by rustfmt layout in
  src/native_output/tests/input_shortcut_inhibition.rs
rtk cargo check: PASS
rtk cargo clippy --lib --all-features -- -D warnings: PASS
rtk cargo clippy --all-targets --all-features -- -D warnings: BLOCKED by 8
  compile errors in the unstaged src/native/kms cursor edits
rtk cargo test native_output::pacing --no-fail-fast: BLOCKED before execution
  by 7 compile errors in the same unstaged src/native/kms cursor edits
rtk git diff --check: PASS
```

The committed frame-pacing files were not changed by those edits. The earlier
post-source focused run (49 passing tests) and all-target clippy pass remain
the valid verification of this change set before the unrelated KMS edits were
present in the shared worktree.
