# Typhon Regional Damage Micro-Closure

Date: 2026-08-28

Repository: `/home/agony/GitHub/Typhon`

## Result

The current checkout already contains the narrow corrective implementation in
the commits after `238da48`:

- `efa9ec0` restores logical `NoVisualChange` surface-damage settlement;
- `048a007` adds decoration-aware regional order damage;
- `fcbd17c` strengthens the topology/output-buffer-age pixel oracle;
- `f1fb4a4`, `184c624`, and `9e4f171` add metric, visual-root, and test-helper
  follow-ups.

This report records fresh verification. No additional production change was
required after the source and test audit; this report is the only new file in
this validation commit.

## Root causes

The NoVisualChange regression was ownership loss. Both the batch-owned terminal
and the batchless terminal dropped the exact `SurfaceDamagePresentation` token.
The journal baseline therefore stayed behind even though output damage had been
proven empty. After 128 empty commits exceeded a capacity-64 journal, a later
partial commit could be reported as `HistoryLost`.

The order-damage regression was an incomplete visual footprint. The changed
middle-span algorithm considered only `NativeSceneSurfaceSnapshot` client
bounds. `DecorationSceneSnapshot` is a separate visual object and its bounds
can extend beyond the root client surface, so a pure reorder could expose stale
titlebar, border, or shadow pixels without any decoration mutation.

## Before and after ownership flow

Before:

```text
exact SurfaceDamagePresentation
        |
        +-- Presented -> keyed logical settlement
        `-- NoVisualChange -> token dropped
```

After:

```text
exact SurfaceDamagePresentation
        |
        +-- SurfaceDamageSettlement::Presented
        |       `-- logical baseline + physical pageflip path settles output
        `-- SurfaceDamageSettlement::NoVisualChange
                `-- logical baseline only; no physical presentation claim
```

`SurfaceDamageSettlement` feeds one keyed `settle_surface_damage` operation.
The NoVisualChange path advances the monotonic logical journal baseline and its
live settlement metric. It does not touch `NativeSceneHistory`, output serials,
slot presentation state, pageflip state, presentation feedback timestamps,
KMS transaction state, or `PartialRepaintPlanner` presented history.

NoVisualChange remains reachable from the existing proven terminal paths:
pre-render no-primary work, compatibility `NativePaintOutcome::Skipped`, and
Atomic `NoLogicalDamage`. A rendered-but-rejected non-empty frame remains on
the retry/failure path.

## Before and after regional order damage

Before, common-prefix/common-suffix order damage pushed old/current bounds only
for the changed surface IDs.

After, the same changed-span algorithm collects the visual roots of old and
current changed members from `visual_stack_groups(surfaces, popup_surface_ids)`
and then damages:

```text
changed span
    -> affected visual roots
    -> old/current client and subsurface bounds
    -> old/current matching DecorationSceneSnapshot bounds
    -> coalesced regional output damage
```

Popup roots stay independent because the root identity is produced by Typhon's
existing `VisualStackGroup` authority with the exact popup IDs. Unchanged
ordered IDs return before adding order-specific decoration damage. Decoration
geometry/signature changes continue through
`from_decoration_bounds_changes()`.

## RED tests and corrected failure modes

The task commits contain the RED tests written before their corresponding
fixes. Their pre-fix failures were:

1. `repeated_surface_only_no_visual_settlement_keeps_lineage_bounded` could
   not advance the baseline; the later partial became history-lost after the
   journal rolled over. It now settles 128 empty entries, records metric 128,
   and preserves `DamageSince::Known(Partial(7, 9, 3, 5))` for commit 129.
2. `no_visual_change_batch_settles_owned_surface_damage_without_presentation`
   saw no logical settled commit because the batch token was dropped. It now
   settles the exact sampled counter with `Presented == 0` and live
   `NoVisualChange == 1` for that test.
3. `rejected_non_empty_frame_does_not_settle_damage_before_retry` protects
   against using a rendered-but-unpresented frame as a logical terminal. The
   rejected partial remains pending; only the later presented retry settles it.
4. `decorated_window_reorder_repairs_ssd_only_pixels_regionally` previously
   had no SSD-only footprint in the order transition. It now includes the
   overlapping SSD pixel `(190, 95)` without escalating to `FullOutput`.
5. `unchanged_decorated_window_order_does_not_damage_ssd` proves that matching
   order, geometry, and decoration state do not add order-specific SSD damage.
6. `topology_transitions_match_full_reference_with_rotating_output_ages`
   exercises the regional topology sequence and compares every presented slot
   to a complete reference render.

The compatibility invariant is also explicit: after token capture and
protocol-only batch mutation, the second resolved scene must have the exact
ordered surface IDs and `scene_identity_signature()` before it is painted.

## Integrated pixel-oracle sequence

The existing topology/output-buffer-age oracle presents, in one deterministic
test:

- popup map, move, reorder, and unmap;
- subsurface map, move, sibling map, reorder, and unmap;
- overlapping SSD decorations, including pixels outside client bounds;
- client buffer A -> B -> C -> A, including authoritative Empty and Partial
  content transitions;
- output slots reused as 0 -> 1 -> 2 -> 0 with explicit ages 1, 2, and 3;
- a candidate rendered into an output slot, rejected without planner presented
  history or presented-scene advancement, followed by a separate retry and
  actual presentation.

Every physical presentation is compared pixel-for-pixel with the complete
reference scene. The separate client/output oracle additionally asserts that a
NoVisualChange logical settlement advances surface accounting while preserving
the physical serial and presented state.

## Files changed by the corrective implementation

The task-owned implementation/test files in the committed closure are:

- `src/compositor/mod.rs`
- `src/compositor/server.rs`
- `src/compositor/state/frame_callbacks.rs`
- `src/compositor/state/frame_tests.rs`
- `src/compositor/state/frames.rs`
- `src/compositor/state/surfaces.rs`
- `src/native_output/output/damage.rs`
- `src/native_output/runtime/frame.rs`
- `src/native_output/runtime/presentation_cycle.rs`
- `src/native_output/tests/output.rs`

The shared render ownership implementation in `src/compositor/render.rs` was
reused; it was not duplicated or rewritten. Existing pointer-lock changes in
the checkout were preserved and are outside this closure.

## Fresh focused verification

All commands below ran through `rtk` and passed:

```text
rtk cargo test --locked no_visual_change                 18 passed
rtk cargo test --locked decorated_window                  2 passed
rtk cargo test --locked topology_transitions              1 passed
rtk cargo test --locked integrated_swapchain              8 passed
rtk cargo test --locked native_output::tests::output::   54 passed
rtk cargo test --locked compositor::state::frame_tests::  48 passed
rtk cargo test --locked presentation_transactions        58 passed
rtk cargo test --locked output_retry                      3 passed
rtk cargo test --locked partial_repaint                  33 passed
rtk cargo test --locked surface_damage                   18 passed
rtk cargo test --locked frame_batch                      12 passed
rtk cargo test --locked decoration                       51 passed
rtk cargo test --locked atomic_egl                        1 passed
rtk cargo test --locked direct_scanout                   34 passed
rtk cargo test --locked frame_callback                   12 passed
rtk cargo test --locked shm                              20 passed
rtk cargo test --locked buffer_age                        7 passed
rtk cargo test --locked presentation                    206 passed
```

The task-specific focused suites were also covered by the narrower filters
above; overlapping filters intentionally count some tests more than once.

## Full verification

Fresh commands and results:

```text
rtk cargo fmt --check                                      PASS
rtk cargo check                                           PASS
rtk cargo clippy --all-targets --all-features -- -D warnings PASS
rtk cargo test                                             PASS: 3089 passed, 5 ignored
rtk git diff --check                                        PASS
rtk git status --short                                      clean before this report
```

The first full-suite attempt had one unrelated, transient KMS test failure:
`native::kms::tests::explicit_atomic_flip_closes_kernel_written_out_fence_on_ioctl_failure`
observed `1` instead of `-1`. Its isolated rerun passed, and the fresh full
rerun passed 3,089 tests. No task-owned test failed.

## Adversarial ownership evidence

1. 128 authoritative Empty NoVisualChange commits advance the logical
   baseline on every iteration, so they cannot cause `HistoryLost` solely
   because no pageflip occurred.
2. `complete_no_visual_change_frame_batch` and the batchless
   `settle_no_visual_change_work` call only the logical keyed settlement. The
   physical authorities remain in the pageflip/output paths. The integrated
   no-visual test observes unchanged serial and presented state.
3. A rejected Partial frame restores/retains its batch/token and leaves the
   logical baseline unchanged; only a later physical presentation settles it.
4. Changed-span order repair includes old/current matching SSD bounds, so
   titlebar/border/shadow pixels outside client bounds are repaired regionally.
5. Root ownership comes from `visual_stack_groups`/`VisualStackGroup`, not
   numeric-ID heuristics or a parallel stack.
6. Unchanged ordered IDs return before order-specific SSD collection.
7. Popup and subsurface map/move/reorder/unmap transitions remain regional in
   the pixel oracle; visibility and external-overlay changes retain their
   conservative global invalidation.
8. The rejected oracle renders into the selected slot, does not commit
   planner presented history or update the presented scene, then presents a
   separate retry.
9. Atomic exact frame lineage remains on its existing resolved-scene and
   pageflip paths. Compatibility asserts exact scene identity before paint.
10. No corrective change enters O1 callback admission, SHM materialization or
    release timing, DMA-BUF release ownership, KMS scheduling, READY admission,
    or Direct Scanout policy/ownership.

## Remaining conservative `FullOutput` reasons

Regional damage still escalates conservatively when the sampled visibility
signature changes or external-overlay membership changes. The output-buffer
repair planner can also request full repair when buffer age/history is invalid
or insufficient. Those are separate visibility/output-buffer authorities and
are not reinterpreted as logical Empty.

No real DRM/KMS TTY or 165 Hz qualification was run, so this report makes no
native hardware qualification claim.
