# Typhon Native Output Damage Authority v1.1.1

## Compatibility NoVisualChange terminal closure

Date: 2026-08-27

Repository: `/home/agony/GitHub/Typhon`

This closure addresses the final compatibility/non-Atomic renderer terminal residual after Native Output Damage Authority v1.1. It does not redesign damage authority, output repair, frame lineage, scheduling, buffering, Direct Scanout, cursor scheduling, XWayland, or renderer behavior.

## Pre-change compatibility terminal flow

The compatibility path already captured the final resolved scene's exact surface damage lineage before renderer invocation:

```text
final ResolvedNativeFrameScene
        |
        +-- exact SurfaceDamagePresentation
        +-- prepared compositor frame batch
        |
        `-- compatibility EGL renderer
                  |
                  `-- NativePaintOutcome::Skipped(NoLogicalDamage)
```

Before this closure, the skipped-render arm performed these terminal actions:

```text
frame scheduler immediate completion
complete_no_visual_change_frame_batch(batch)
frame_completed = true
queued_redraw_requested = false
cursor damage baseline updates
```

It did not retire `last_rendered_scene_generation`. The pre-render no-primary branch and Atomic skipped branch did retire that logical scheduling baseline. Consequently, a compatibility renderer skip could leave a scene generation looking changed on the next cycle even though the current cycle had already completed its logical work.

The same old arm also assigned the software cursor baseline twice. The duplicate assignment was removed as part of this exact branch closure.

## Root cause of sticky compatibility scene generation

`last_rendered_scene_generation` is a logical render/coalescing baseline, not physical presentation evidence. The compatibility renderer's `Skipped` arm had been added around the older batch-completion behavior and never adopted the logical retirement already used by the pre-render and Atomic terminals. The omission was therefore a branch-ownership asymmetry, not a damage-journal or physical-presentation problem.

## Files changed

Task-owned source and documentation changes are:

- `src/native_output/runtime/presentation_cycle.rs`: added `complete_compatibility_no_visual_change()` and routed the actual compatibility `NativePaintOutcome::Skipped` branch through it.
- `src/native_output/runtime/presentation_cycle_tests.rs`: added a deterministic regression using the actual `NativePaintOutcome::Skipped` terminal value.
- `src/native_output/runtime/mod.rs`: registered the focused runtime test module.
- `src/compositor/state/frame_tests.rs`: renamed the direct state-machine 128-entry test to describe its bounded helper/journal scope accurately.
- `REPORT-2026-08-27-typhon-native-output-damage-authority-v1-1.md`: made the minimal factual correction to the stale identity-to-footprint repair statement.
- `docs/superpowers/plans/2026-08-27-typhon-native-output-damage-authority-v1-1-1-plan.md`: implementation plan.
- This report.

Unrelated dirty-checkout changes were preserved.

## Compatibility renderer Skip closure

The new production helper is called only from the existing compatibility renderer skip arm. It accepts the actual `NativePaintOutcome` and returns without terminal work for a rendered outcome. For `NativePaintOutcome::Skipped`, it owns exactly:

1. immediate scheduler completion;
2. completion of the already-prepared `complete_no_visual_change_frame_batch()`;
3. retirement of `last_rendered_scene_generation` to the current scene generation;
4. clearing queued redraw state;
5. one update of each logically accounted client/software cursor baseline.

It accepts no physical output-history authority and never calls presented-frame completion.

The regression is `compatibility_renderer_skip_retires_logical_generation_and_terminally_owns_batch`. It creates a prepared compatibility batch, supplies an actual `NativePaintOutcome::Skipped`, and proves:

- baseline `G-1` becomes `G`;
- the unchanged next state is not `scene_changed`;
- a later generation is `scene_changed` again;
- the prepared batch is removed exactly once;
- queued redraw is cleared;
- cursor baselines equal the current logically accounted values;
- no prepared/orphan batch remains.

The RED run occurred before the helper existed and failed to compile with the expected missing production helper. After the minimal implementation, the same test passed.

## Logical generation before and after

```text
Before compatibility Skip:
    last_logical_scene_generation = G-1
    scene_generation              = G
    scene_changed                 = true

After compatibility Skip:
    last_logical_scene_generation = G
    physical presented state      = unchanged

Following unchanged cycle:
    logical_scene_changed(G, G)   = false

Later real mutation:
    logical_scene_changed(G, G+1) = true
```

## Surface lineage settlement proof

The compatibility path continues to capture exact lineage from the resolved scene before preparing the batch. It stores that token with `set_prepared_frame_surface_damage()`. The new helper completes that same prepared batch with `complete_no_visual_change_frame_batch()`; it does not recapture current global state.

The compositor state tests continue to prove that an exact frozen surface token settles the sampled commit as NoVisualChange, leaves physical presentation counters untouched, and does not consume a later commit. The 128-entry helper/journal test is now named `repeated_surface_only_no_visual_settlement_keeps_lineage_bounded`, accurately distinguishing journal-capacity coverage from end-to-end runtime decision coverage.

The existing newer-commit tests preserve the required ordering:

```text
surface commit N       -> captured by the prepared batch
surface commit N+1     -> occurs after capture
compatibility Skip     -> settles N only
N+1                    -> remains pending
```

## Physical presentation non-advancement proof

The compatibility helper does not receive or mutate:

- `NativeSceneHistory::presented`;
- output presentation serials;
- `last_presented_serial`;
- pageflip sequence or confirmed pageflip state;
- wp_presentation presented timestamps;
- `PartialRepaintPlanner` presented history.

The batch completion remains `complete_no_visual_change_frame_batch()`, not presented-frame completion. Existing transaction and integrated-oracle tests continue to assert zero physical presentation advancement for NoVisualChange.

## Production no-primary runtime wiring

The existing v1.1 production no-primary path remains asymmetric only in its input condition, not in its terminal ownership:

```text
resolve final scene and NativeOutputDamage
        |
        +-- authoritative Empty + scene_changed
        |       |
        |       +-- capture exact resolved-scene lineage
        |       +-- finish_no_primary_work()
        |       `-- retire_logical_scene_generation()
        |
        `-- no protocol work: surface-only settlement, no batch
```

That path still captures only when a logical scene generation changed, settles exact scene/cursor lineage through `finish_no_primary_work()`, retires the logical baseline, and leaves no batch when no protocol/release work is owned. Its existing v1.1 tests and the logical-retirement tests remain green. No new production no-primary abstraction was added because the current helper already owns the correct output-decision boundary and the full native runtime requires real output setup not available in this test environment.

## Terminal matrix

| Terminal | Logical generation | Surface damage | Protocol/release ownership | Physical presentation |
| --- | --- | --- | --- | --- |
| Pre-render No Primary Work | Retired when scene work was changed | Exact NoVisualChange settlement | Batch only when required | Unchanged |
| Atomic EGL `NoLogicalDamage` | Retired | NoVisualChange settlement | Dedicated Atomic NoVisualChange transaction | Unchanged |
| Compatibility EGL `NoLogicalDamage` | Retired by the new helper | Exact prepared batch NoVisualChange settlement | Prepared batch terminally completed | Unchanged |
| Direct identical content | Existing identical content epoch remains authoritative | No new content claim | Protocol work completed as appropriate | No invented presentation |

## Direct candidate-key invariant confirmation

`DirectScanoutCandidateKey` remains unchanged. It includes the surface identity, buffer identity, content epoch, output generation, cursor content key, and color epoch. The content epoch advances with published visual buffer content, so an identical candidate key remains valid evidence that no newer Direct visual content commit exists.

`identical_direct_candidate_key_proves_the_visual_content_epoch_is_unchanged` remains green. No presentation lineage was added to duplicate Direct candidates, and Direct Scanout policy/admission was not changed.

## XWayland release ownership preservation

No release ownership code was changed in v1.1.1. The v1.1 behavior remains:

- SHM client memory is represented by Typhon's internal snapshot before release;
- DMABUF release ownership follows the existing explicit resource lifetime;
- protocol-only callback work with pending releases can drain through one safe terminal batch.

The isolated regression `mapped_xwayland_frame_callback_completes_after_output_sample_before_present` remains green.

## Stale v1.1 report correction

The final `Preserved behavior and non-goals` sentence in the v1.1 report was stale documentation carried forward from the pre-v1 closure. The identity-to-footprint fallback had already been removed by Native Output Damage Authority v1. The current source and regressions prove that generation-only or commit-sequence-only changes with authoritative Empty remain Empty. The v1.1 report now states that fact explicitly; unrelated historical evidence was not rewritten.

## Focused tests

Fresh focused results after the implementation:

- compatibility terminal: 1 passed;
- NoVisualChange: 12 passed;
- compositor frame-consumption/frame-batch tests: 48 passed;
- surface-damage tests: 13 passed;
- commit-timing/logical-retirement tests: 3 passed;
- Direct Scanout stage tests: 18 passed;
- compatibility pacing tests: 30 passed;
- integrated client/output swapchain oracle: 7 passed;
- dedicated Atomic NoVisualChange transaction: 1 passed;
- isolated XWayland callback/release regression: 1 passed.

## Verification

Passed:

- `rtk cargo fmt --all -- --check`;
- `rtk cargo check --locked` — 0 errors, 7 existing dead-code warnings;
- `rtk git diff --check`.

The full command `TMPDIR=/tmp rtk cargo test --locked` was run three times. Each run had one different suite-only failure while the remaining tests passed:

1. Pointer cursor: `compositor::tests::input_output::pointer_cursor::locked_unlock_does_not_reveal_committed_hint_before_followup_warp` — 1,915 passed; isolated rerun passed.
2. Process signal: `one_child_exit_wakes_the_sigchld_signalfd_once` — 1,916 passed; isolated rerun passed.
3. XWayland reactor: `native_output::runtime::xwayland_reactor_tests::xwayland_reactor_x11_window_reaches_window_ready_without_direct_fd_polling` — 1,916 passed; isolated rerun passed.

These failures were non-reproducible in isolation, occurred in unrelated code, and were not modified. No task-owned full-suite failure remained.

Strict Clippy was run with:

```text
rtk cargo clippy --locked --all-targets --all-features -- -D warnings
```

It reported 22 errors and 1 warning in existing unrelated code, including `src/compositor/protocols/workspace.rs`, `src/compositor/surface.rs`, `src/compositor/workspace_protocol.rs`, compositor tiled-layout/resize/fullscreen/XWayland modules, `src/native/adaptive_buffering.rs`, and `src/wm/layout/*`. No task-owned Clippy diagnostic was reported.

The source-layout check was run with `rtk run "bin/check-source-layout"`. It reported the known 18 oversized files, including the already-over-limit `src/native_output/runtime/presentation_cycle.rs` (1,691 lines against a 1,500-line limit). The file was already over the limit before this closure; no unrelated split or rewrite was performed.

## Review pass 1: correctness and ownership

The following cases were manually challenged against the changed flow:

- newer scene generation after an older compatibility skip;
- failed compatibility render, which still restores the prepared batch and does not retire logical work;
- compatibility skip after exact lineage capture;
- duplicate completion attempts after the prepared batch is removed;
- physical history and pageflip state remaining outside the helper;
- software and hardware cursor ownership remaining on their existing paths;
- Atomic NoLogicalDamage retaining its dedicated terminal;
- Direct identical-candidate suppression retaining its existing invariant;
- XWayland callback-plus-release ownership remaining terminal and safe.

The task-owned issue found was the missing compatibility logical retirement. It was fixed by the shared production helper. The duplicate software-baseline assignment was also removed.

## Review pass 2: locality and accidental scans

The production search was repeated for all requested terminal symbols and for compatibility completion calls. The compatibility renderer has one terminal arm, and it now routes through `complete_compatibility_no_visual_change()`. No global damage recapture, physical presented completion, renderer refactor, cursor refactor, or unrelated scheduler change was introduced.

The no-primary, Atomic, Direct Scanout, cursor-only, and pageflip paths remain separate authorities. The v1.1 global surface-index and exact sampling locality changes remain untouched by this narrow closure.

## Preserved non-goals

This closure did not change:

- O1 policy/controller/predictor;
- KMS-worker policy/default;
- triple-buffer policy/default;
- Direct Scanout admission/default;
- VRR or tearing;
- scheduler target selection or commit timing policy;
- workspace, Dwindle, XWayland protocol semantics, or renderer behavior;
- conservative output-attempt repair behavior.

## Hardware qualification and remaining uncertainty

No real TTY/DRM/KMS/165 Hz qualification was executed. The deterministic tests establish logical terminal ownership, prepared-batch removal, cursor-baseline ownership, exact lineage delegation, and physical-state separation; they do not substitute for native hardware measurement.

After this source closure, further optimization should wait for the requested real qualification sequence:

```text
TTY -> DRM/KMS -> 165 Hz -> normal daily use -> KMS worker A/B -> O1 A/B -> real telemetry
```

No claim is made that this source change alone fixes an observed approximately 30 FPS symptom.
