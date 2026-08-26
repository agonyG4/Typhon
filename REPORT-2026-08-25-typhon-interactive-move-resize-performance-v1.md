# Typhon Interactive Move/Resize Performance v1

## A. Executive result

This closure changes the expensive interactive paths so work follows the changed scene and repaired output area rather than raw pointer frequency, total visible surfaces, or the product of damage rectangles and draw commands.

Implemented closures:

- stable-geometry content damage remains localized even when content generation or commit sequence changes;
- EGL content/resource identity is separated from scene command topology;
- scene and overlay GPU geometry use independent retained VBOs and upload only when their geometry is rebuilt;
- partial repairs reject non-intersecting commands before texture binding or drawing, with conservative proven-opaque occlusion;
- move/resize geometry uses affected-root output membership and avoids duplicate ActiveScene root refreshes;
- XDG position-only moves no longer enqueue discarded backend configure commands, while X11 moves retain configure behavior;
- pointer-owner locality is used for nearby points without weakening exact local coordinates or captured interaction routing;
- floating move updates keep latest desired pointer state and flush the terminal target exactly.

The architecture targets cursor lag, choppy Chromium/Electron move/resize, repeated scene/VBO work, and populated-workspace hover amplification. Real KMS/NVIDIA qualification was not executed in this environment; matched host measurements remain the final gate for numerical CPU/GPU claims.

## B. Root-cause evidence

| Finding | Current status | Source location | Evidence | Fix/skip reason |
| --- | --- | --- | --- | --- |
| F1 | changed | `src/compositor/state/window_resize.rs`, `state/output_membership.rs` | geometry path previously reached all-surface reconciliation | affected-root reconciliation is now used for ordinary root geometry changes; global path remains for topology changes |
| F2 | changed | `src/compositor/state/subsurfaces.rs`, `state/active_scene.rs` | placement and visual-assignment paths could refresh the same root twice | visual-assignment refresh is treated as the single root refresh and duplicate work is counted |
| F3 | changed | `src/egl_renderer.rs` | scissored replay previously iterated all commands for every scissor with no command culling | output-space bounds reject commands before texture bind/draw |
| F4 | changed | `src/egl_renderer.rs` | batch replay uploaded the complete vertex vector repeatedly | retained scene/overlay VBOs upload only while their geometry-dirty flag is set |
| F5 | changed | `src/compositor/state/hit_testing.rs` | exact-coordinate cache did not provide same-owner spatial locality | owner-local fast path computes fresh exact coordinates and validates frontmost ownership |
| F6 | changed | `src/compositor/state/hit_testing.rs`, `state/surfaces.rs` | content publication advanced render generation used by the old hit cache | pointer locality is keyed to pointer topology generation, not content-only render generation |
| F7 | changed | `src/native_output/output/damage.rs` | commit/generation changes could promote stable small damage to surface bounds | stable known damage remains local; identity-only retries without damage use a conservative current-footprint fallback |
| F8 | changed | `src/egl_renderer.rs` | commit sequence, buffer ID, and generation were part of command-topology identity | derived geometry/sampling facts remain topology inputs; content identity remains in presentation/damage tracking |
| F9 | changed | `src/compositor/state/surfaces.rs` | root publication required an overly broad tree refresh | visible root content publication uses the root-surface refresh authority; child updates remain incremental |
| F10 | changed | `src/compositor/state/window_interaction.rs` | XDG position-only move queued backend work later discarded | backend position/configure path is now explicitly X11-only |
| F11 | already fixed / preserved | `src/compositor/state/window_interaction.rs`, `state_data.rs` | bounded resize configure/capture flow and metrics were already present | no policy change was made without host evidence; existing ACK/capture/backpressure behavior is retained |
| F12 | already fixed / preserved | `src/compositor/state/active_scene.rs` | active scene selection already filters workspace/special-workspace visibility | locality work uses the existing active-scene authority and does not alter ordering |
| F13 | changed conservatively | `src/compositor/state_data.rs`, `src/compositor/surface.rs`, `src/egl_renderer.rs` | opaque state was coupled to input-region semantics and could be misread | committed opaque state is separate; ordered add/subtract regions become conservative normalized rectangles, while default/null and unknown coverage remain non-opaque |

## C. Architecture implemented

- Content/presentation: commit sequence, content generation, resource identity, and presentation-relative damage remain separate from spatial command topology. Rejected/retried content with no known damage retains a conservative current-footprint fallback.
- Render topology: command cache identity contains output geometry, surface placement, buffer dimensions/scale/transform, clipping, order, popup/decorative geometry, and derived sampling facts, but not ordinary content identity alone.
- Pointer topology: pointer generation invalidates ownership for input-region, map/unmap, stacking, popup, workspace, placement, and decoration changes. A stable owner is validated against the current frontmost visual groups before exact local coordinates are delivered.
- Output membership: ordinary geometry changes reconcile only the affected root tree. Global reconciliation remains available for output topology/hotplug/scale/transform work.
- Damage history: scene transition damage is relative to the supplied previous scene and keeps full fallbacks for footprint changes, history loss, visibility/order changes, and uncertain identity-only retries.
- Opacity/occlusion: `wl_surface.opaque_region` is committed independently of `input_region`. Add/subtract operations are normalized into conservative surface-local rectangles, transformed through the render target and clip aperture, and unknown/default coverage remains non-opaque.
- Interactive geometry: raw move samples replace a pending desired target, visual application is gated by the output presentation interval, and user-final termination flushes the latest exact target. Resize configure policy and captured commit flow remain unchanged.

## D. Before/after deterministic counters

The pre-change checkout was already dirty and was not used as a clean executable baseline. Therefore no fabricated before numbers are reported. The source-operation comparison and post-change counters are:

| Area | Before operation | After operation / evidence |
| --- | --- | --- |
| Full-bounds damage | stable content identity changes could escalate the footprint | stable known partial content damage stays local; the regression test asserts a 5x6 repair rather than a 400x300 surface repair |
| Scene topology rebuilds | content/resource identity was coupled to command topology | content-only key tests reuse geometry; topology facts still invalidate the key |
| Scene VBO uploads | complete geometry upload occurred in each replay batch | `scene_vbo_uploads` and `scene_vbo_upload_bytes` are retained per frame and upload only after scene rebuild |
| Commands/draw calls | all commands were visited and submitted per scissor | `commands_considered`, `commands_executed`, `commands_rejected_outside_damage`, `texture_binds`, and `draw_calls` are counted; out-of-repair commands are rejected before submission |
| Occlusion | no conservative renderer occlusion | `commands_rejected_occluded` is counted when a lower command is fully covered by the normalized proven opaque rectangle set; holes remain visible |
| Pointer scans/locality | cache authority included content render generation and had no same-owner locality | the decoration locality regression test observes one full scan followed by one owner-local hit with exact changed coordinates |
| Membership | geometry could invoke all-surface reconciliation | `broad_membership_reconciliations`, `affected_root_membership_reconciliations`, and `membership_surfaces_inspected` distinguish the paths |
| ActiveScene refresh | placement path could refresh the root redundantly | `active_root_scene_refreshes` and `prevented_duplicate_root_refreshes` expose the single-refresh path |
| Interaction applies | raw move input could directly drive expensive visual mutation | `raw_pointer_move_updates`, `pending_move_updates_replaced`, `move_updates_applied`, and `move_updates_skipped_unchanged` expose desired/applied separation; the terminal flush remains exact |
| Resize configure flow | existing bounded flow was not changed | existing `configures_requested`, `configures_sent`, ACK, capture, capacity-block, and replacement counters remain available for host qualification |

## E. Tests

Verification completed with the repository build/cache and `rtk` workflow:

- `TMPDIR=/tmp rtk cargo fmt --check` — passed.
- `TMPDIR=/tmp rtk cargo check --locked` — passed with six pre-existing warnings and no errors.
- `TMPDIR=/tmp rtk cargo test --locked -- --test-threads=1` — **2945 passed, 5 ignored, 40 filtered out, 30 suites**.
- `rtk git diff --check` — passed.

The full suite includes focused coverage for stable content damage, rejected content-only retry fallback, content/topology cache reuse, output-space command bounds, safe opaque-region semantics, pointer decoration locality and overlapping-window ownership, XDG/X11 move behavior, latest-target terminal move flush, and existing interaction/resize correctness.

## F. Real-host measurements

No native KMS/NVIDIA session, matched Hyprland session, `pidstat`, `perf`, or NVIDIA telemetry was available during this implementation. No CPU, GPU, frame-rate, or 1.5x comparison number is claimed.

Qualification remains pending for S1–S12 from the task matrix, especially heavy Electron/Chromium hover, populated-workspace move, Chromium resize, hardware/software cursor comparison, XWayland, and multi-output boundary behavior. On the target host, use a release build and matched scenarios with:

```bash
pidstat -p <TYPHON_PID> 1
perf stat -p <TYPHON_PID> \
  -e task-clock,cycles,instructions,branches,branch-misses,context-switches,cpu-migrations,page-faults
perf record -F 999 -g -p <TYPHON_PID>
```

Collect the retained native EGL fields (`current_damage_pixels`, `repair_damage_pixels`, `commands_considered`, `commands_executed`, `draw_calls`, `scene_vbo_uploads`, `overlay_vbo_uploads`, and `damage_history_depth`) alongside CPU and available GPU telemetry.

## G. Review pass 1 findings

- Corrected stable content-only damage classification so commit/generation identity alone does not turn known partial damage into full bounds.
- Retained a conservative footprint fallback for identity-only rejected/retried content when no damage journal is available.
- Separated opaque-region state from input-region state and made default/null semantics non-opaque.
- Added derived buffer dimensions/scale/transform to topology identity while removing content-only identity fields from command-cache reuse.
- Preserved X11 configure behavior while eliminating the XDG position-only backend command.
- Added frontmost-owner validation to the pointer locality path after overlapping-window regression coverage exposed stale-owner risk.
- Preserved explicit-sync, buffer lifetime, workspace filtering, and existing resize backpressure paths; the full suite passed after these corrections.

## H. Review pass 2 findings

- Confirmed scene and overlay geometry have separate dirty/upload ownership; scissor replay no longer uploads stable scene vertices.
- Confirmed command bounds are tested before texture binding and draw submission, while background and partially intersecting commands remain eligible.
- Confirmed fully covered lower commands are only rejected when a proven normalized opaque rectangle set covers them; subtract-created holes and unknown/translucent regions remain visible.
- Confirmed ordinary move/resize geometry uses affected-root membership rather than broad reconciliation and that visual-assignment refresh prevents a second root refresh.
- Confirmed latest move samples replace pending targets and terminal user completion flushes the final target; no arbitrary sleep or fixed 165 Hz constant was introduced.
- Confirmed no clean-baseline hardware numbers were invented and existing dirty user changes remain unstaged unless task-owned.

## I. Remaining risks

1. Real KMS/NVIDIA qualification is still required to measure whether the implementation removes the reported order-of-magnitude CPU/GPU behavior on the target machine.
2. Move application is bounded using the output refresh interval available in compositor state; it is not yet proven against an actual presentation-callback opportunity stream under GPU saturation. The terminal flush and existing resize protocol path are covered deterministically.
3. Opacity propagation is intentionally conservative: malformed/unknown regions and any transform/aperture case that cannot be represented safely remain non-opaque. Hardware traces are still needed to quantify the resulting occlusion rate.
4. Aggregate membership no-op and preferred-output counters remain available through the existing compliance/resize structures, but hardware traces are needed to quantify cross-output and idle-inhibition behavior.
