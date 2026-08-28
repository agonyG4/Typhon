# Typhon Surface and Presentation Locality v2 Closure Report

Date: 2026-08-27

## Outcome

The current dirty checkout now has an indexed ordered `RenderableSurface` authority, exact frame and cursor presentation lineage, O(1)-average steady-state content lookup, O(K)-average presentation capture and settlement, and XWayland content/topology separation.

The final locked test suite passed. `clippy` and the source-layout checker remain red because of pre-existing or unrelated dirty-checkout findings; their exact results are recorded below. No TTY, DRM, KMS, or 165 Hz hardware qualification was executed, and this report makes no hardware-performance claim.

Unrelated checkout changes were preserved. No reset, clean, stash, discard, or unrelated rewrite was performed.

## Pre-change source audit

Before editing, the following source areas were audited:

- Compositor state and entry points: `src/compositor/mod.rs`, `server.rs`, `state/surface_commits.rs`, `surfaces.rs`, `active_scene.rs`, `scene_order.rs`, `subsurfaces.rs`, `windows.rs`, `window_resize.rs`, `xwayland_windows.rs`, `surface_commit_cursor.rs`, and `input_resources.rs`.
- Native frame and presentation flow: `src/native_output/runtime/frame.rs`, `bootstrap.rs`, `presentation_cycle.rs`, `presentation_cursor.rs`, `cursor_cycle.rs`, `cycle/pageflip.rs`, and `plane_cycle.rs`.
- Native output and presentation ownership: `src/native_output/output/cursor.rs`, `cursor_buffer.rs`, `damage.rs`, `presentation/transaction.rs`, `presentation/ledger.rs`, and `presentation/plane.rs`.
- Worker and scanout ownership: `src/native_output/kms_worker/cursor_sidecar.rs`, `scanout/output_swapchain.rs`, `scanout/atomic_egl_gbm.rs`, `scanout/atomic_egl_gbm/direct.rs`, and the related worker/direct transition paths.

The audit reconfirmed the supplied residuals:

1. Ordinary Wayland buffer commits performed several independent global vector searches.
2. Damage-only commits used a global mutable search.
3. Incremental active-scene publication used another global search.
4. Global presentation capture walked all renderables and all stored client cursor surfaces, with linear duplicate detection.
5. Settlement searched global renderables and cursor surfaces for every token entry.
6. `ResolvedNativeFrameScene::surface_ids()` already represented the final active-workspace/fullscreen-resolved authority.
7. Atomic EGL/GBM and initial modeset capture occurred before final-scene ownership was bound.
8. Direct Scanout already used candidate-local capture.
9. Ordinary XWayland buffer publication retained and reinserted content, reordered the global stack, and reapplied visual assignment even for steady-state content.
10. Removing the global cursor scan without replacement would have lost client-cursor settlement.

## Exact hot-path scans found

The pre-change hot-path scans were:

- `commit_surface_buffer`: repeated `.iter().find(...)` operations for existence, buffer identity, mapping state, mutable update, damage publication, and active-scene publication.
- `commit_surface_damage_only`: `.iter_mut().find(...)` for the mapped surface.
- `refresh_active_scene_surface`: global `.iter().find(...)` for incremental publication.
- `capture_surface_damage_presentation`: global renderable traversal plus all client cursor surfaces and linear duplicate checks.
- `commit_surface_damage_presented`: per-entry global renderable and cursor collection scans.
- `commit_xwayland_surface_buffer`: global retain, append, committed-stack reorder, and unconditional root visual reassignment for ordinary content.

The remaining vector operations were re-audited after the change. They are confined to real topology, tree, geometry, or input paths, or to the retained compatibility/test-only global capture wrapper. No normal native frame calls the legacy global capture API.

## Global RenderableSurface index architecture

The authoritative ordered render stack remains:

```text
Vec<RenderableSurface>
```

`CompositorState` now owns a sidecar:

```text
HashMap<u32, usize>   // SurfaceId -> ordered render-vector position
```

The sidecar is maintained through narrow helpers in `state/surfaces.rs`:

- `renderable_surface_index`
- `renderable_surface`
- `renderable_surface_mut`
- `append_renderable_surface`
- `replace_renderable_surface`
- `remove_renderable_surface`
- `retain_renderable_surfaces`
- `rebuild_renderable_surface_index`

Same-ID replacement and same-position content updates do not rebuild the map. Append updates the map in O(1) average time. Removal, retain, drain/reorder, and assignment paths rebuild after the topology mutation.

Debug and test builds validate that:

- index length equals vector length;
- every vector ID maps to exactly its vector position;
- every map entry points to the matching vector item; and
- duplicate IDs cannot pass the invariant.

The invariant is not run on every release-build content commit. Bounded test counters record indexed lookups, content-path lookups, and rebuilds without per-frame strings or unbounded history.

The global index is deliberately separate from `ActiveSceneView.surface_indices`. The former indexes authoritative global render-stack storage; the latter indexes the cloned active-scene view.

## Production index mutation audit

All production mutations of `renderable_surfaces` were searched and classified.

| Mutation class | Current handling |
| --- | --- |
| Content replacement | Indexed in-place update in `commit_surface_buffer`, `commit_surface_damage_only`, and ordinary mapped XWayland content commits. |
| Initial insertion | `append_renderable_surface`; the append updates the sidecar directly. |
| Removal/unmap/teardown | `retain_renderable_surfaces` or indexed `remove_renderable_surface`; a real removal rebuilds the sidecar. |
| Bulk topology reorder | `scene_order.rs` drains/reconstructs the ordered stack and rebuilds. |
| Window stacking/raise | `windows.rs` drains/reconstructs and rebuilds. |
| Subsurface/tree reorder | `subsurfaces.rs` drains/reconstructs and rebuilds. |
| Minimize/restore | Minimize removes visible surfaces and rebuilds; restore appends the minimized surfaces through the helper. |
| XWayland attachment/replacement | Same-placement mapped content replaces in place; initial/changed-placement/family paths use helper-backed removal, insertion, reorder, and rebuild. |
| Visual geometry mutation | Updates the indexed item or performs the existing tree/geometry refresh; it does not use content-path vector reconstruction. |
| Test-only direct mutation | The deliberate test vector swap is followed by `rebuild_renderable_surface_index`; test setup insertions use the helper. |

The only direct production vector writes left are inside the centralized helper implementations or real drain/reconstruct topology code. The source audit also covered client-lifecycle teardown, role adoption, popup/tree raise, and minimized XWayland paths.

## Wayland content locality

Before this closure, one ordinary Wayland commit could perform multiple O(N) global searches. It now obtains one indexed position, derives the existing-surface decisions from that item, and performs the mutable update at that position.

The indexed state supplies:

- whether the surface was already renderable;
- buffer identity change;
- mapping and geometry/viewport/scale/transform state; and
- the mutable content update position.

Initial mapping remains conservative Full damage. Same-size A -> B -> C buffer rotation does not become logical Full damage merely because `BufferId` changes. Buffer identity remains authoritative for resource lifetime, import, synchronization, release, and Direct Scanout identity.

Content-only commits do not rebuild the global index or reorder the stack. Real placement/mapping/topology changes still take their existing cold path. Minimized commit cleanup was also changed from a global retain pass to indexed removal, with a rebuild only if an actually visible stale entry is removed.

`publish_surface_generation` and `refresh_active_scene_surface` now use the global index. Tree refresh remains a bounded-by-tree visual cold path because it must discover all affected descendants when tree inputs change.

## Damage-only locality

`commit_surface_damage_only` now resolves the current renderable exactly once through the global index and updates it in place. Damage normalization, SHM copy, resize conservatism, journal publication, and incremental active-scene publication remain intact. The path does not rebuild the global index or perform stack reconstruction for ordinary damage-only content.

## XWayland content/topology separation

For an already mapped, non-minimized XWayland surface whose placement remains unchanged, the buffer commit now:

1. obtains the existing indexed position;
2. preserves the existing `render_placement` and `visual_clip`;
3. replaces the `RenderableSurface` at that position; and
4. records Full XWayland content damage.

It does not retain the entire vector, append at the end, reorder the committed stack, or reassign the root visual tree.

The existing conservative XWayland Full content damage was intentionally preserved. No unsupported X11 partial-damage authority was introduced.

Topology work remains for initial mapping, unmap, minimized/restore transitions, changed placement, changed visual inputs, X11 restack/family operations, and other actual topology changes. Root visual assignment is reapplied only when the in-place conditions are not sufficient or pending visual content/geometry says the visual inputs changed.

Deterministic regression:

`mapped_xwayland_content_commits_do_not_touch_global_topology` performs 1,000 same-geometry commits with 1,000 unrelated renderables. It verifies unchanged stack order, 1,000 in-place replacements, zero XWayland topology reorders, zero full visual reassignments, zero index rebuilds, and 1,000 content indexed lookups.

## Exact composited frame lineage

The composited ownership sequence is now:

```text
final ResolvedNativeFrameScene
    -> scene identity validation
    -> exact scene surface_ids()
    -> exact cursor sample, if actually delivered and rendered
    -> capture keyed SurfaceDamagePresentation
    -> render
    -> frame/batch owns token
    -> submit
    -> matching confirmed pageflip
    -> settle frozen token
```

Atomic EGL/GBM captures after constructing and validating the final `ResolvedNativeFrameScene`. The initial atomic modeset uses the already-created `initial_resolved_scene`. The compatibility/native path captures from the final scene used at its render boundary and attaches the token to the prepared frame batch.

`RenderedOutputFrame` owns the atomic token. A compatibility frame batch owns the non-atomic token. Failed preparation, failed render, skipped output, rejected KMS submission, supersession, safe abandonment, and output loss drop or abandon the token without advancing presented surface state. Only the matching confirmed pageflip consumes it.

The final scene surface set is authoritative after active workspace selection, fullscreen culling, and popup/subsurface expansion. Inactive workspace and fullscreen-culled surfaces are not captured merely because they exist globally.

## Software client cursor lineage

When the frozen primary delivery is software and the renderer receives `server.client_cursor_render_state()`, the exact `(surface_id, commit_sequence)` is added to the primary frame sample set. The identity is obtained at render time and remains owned by the frame token.

Hidden delivery, theme/compositor cursor delivery, and software delivery without a client `wl_surface` add no client cursor sample. Cursor sample counters distinguish software and hardware delivery for deterministic tests.

## Hardware client cursor lineage

The existing `NativeCursorSourceKey::Client(NativeCursorImageKey)` is now used as the frozen source. `NativeCursorImageKey` carries the client surface ID, buffer ID, and commit sequence. Hardware lineage is not inferred from later compositor focus.

When a client cursor is bundled into a primary atomic transaction, its exact source commit is included in the primary frame token. Worker extraction may clear the source key from the returned framebuffer owner, but the primary `RenderedOutputFrame` has already retained the exact presentation token.

## Cursor-only PlaneDelta lineage

A cursor-only hardware update now captures the exact client commit from the frozen cursor source key and stores it on the cursor `PlaneDelta` `OutputTransaction`. The transaction owns the token independently of any primary frame.

The matching cursor pageflip reads only that transaction’s token and settles it. A cursor-only pageflip does not require a primary frame. Theme and hidden cursor assignments have no client source key and therefore receive no client surface token. Superseded cursor transactions are terminalized without settlement, so their old tokens cannot become presented.

## Direct Scanout preservation

Direct Scanout continues to capture the candidate surface locally. The candidate is the primary sample, with an additional exact client cursor sample only when the same transaction actually presents a client hardware cursor. Unrelated surfaces are not captured. The existing direct lease continues to own and settle the token on its confirmed pageflip.

No Direct Scanout admission or default policy was changed.

## Presentation capture and settlement architecture

Filtered capture uses a temporary `HashSet<u32>` for duplicate suppression and keyed lookups into `surface_presentation_generations` and `surface_damage_journals`. It never scans `renderable_surfaces` or all client cursor surfaces. Exact cursor commits are looked up by their recorded commit sequence in the bounded journal; the bounded journal lookup is O(1) with respect to global compositor size.

Settlement processes only token entries:

1. verify the frozen generation;
2. look up the journal by surface ID;
3. reject a regressing sampled counter;
4. advance only that surface’s presented counter;
5. update the normal renderable through the global index, or the cursor through `client_cursor_surfaces.get_mut(surface_id)`; and
6. preserve `HistoryLost -> Full`.

The resulting capture and settlement costs are O(K) average for K sampled surfaces, independent of unrelated compositor population M. The old global capture method remains only as a compatibility/test-only API and is instrumented as a global scan. No normal native presentation call site uses it.

## Deterministic observability and evidence

Bounded `SurfaceLocalityMetrics` counters cover indexed lookups, content-path lookups, index rebuilds, sampled entries, journal lookups, settlement entries, global presentation scans, XWayland replacements/reorders/visual reassignments, and software/hardware cursor samples.

The locality tests assert operation counts rather than wall-clock thresholds:

- `renderable_surface_index_survives_content_and_topology_mutations` checks append, same-ID replacement, removal, retain-style rebuild, and ordered-vector reorder invariant behavior.
- `filtered_surface_damage_capture_is_keyed_and_deduplicated` uses 1,000 global journals and requests `[7, 7, 8, 9]`; it observes exactly three samples, three journal lookups, and zero global scans.
- `presentation_capture_follows_the_final_resolved_surface_set` uses 1,000 global surfaces but captures only six supplied final-scene IDs.
- `presentation_capture_and_settlement_scale_with_sample_count` uses 1,000 global journals and four samples; both capture and settlement counters increase by exactly four, with zero global scans.
- `old_frame_completion_advances_only_its_sampled_damage_commit`, `exact_surface_commit_capture_does_not_consume_a_newer_commit`, `older_pageflip_cannot_regress_presented_surface_commit`, and `exact_cursor_commit_settles_only_the_frozen_cursor_content` cover frozen commit ownership and non-regression.
- `stale_surface_generation_cannot_advance_reused_surface_identity` covers numeric ID reuse protection.

## Integrated client/output swapchain oracle

`src/native_output/tests/integrated_swapchain_oracle.rs` adds a deterministic model covering:

- client A -> B -> C -> A-style rotation;
- output slot 0 -> 1 -> 2 -> 0 reuse;
- partial damage and authoritative Empty;
- buffer age including age 3+;
- a rejected output attempt;
- full-reference pixel comparison for each accepted presentation; and
- a newer client commit surviving an older frozen frame pageflip.

Both oracle tests pass:

- `client_and_output_swapchains_match_full_reference_across_rejection_and_aging`
- `frozen_old_surface_commit_does_not_consume_newer_client_commit`

The oracle is deterministic test evidence, not a hardware measurement.

## Focused tests

The final focused runs included:

- ordered surface publication/index tests: 14 passed;
- repeated Wayland content locality tests: 2 passed;
- mapped XWayland topology locality: 1 passed;
- integrated swapchain oracle: 2 passed;
- presentation transaction tests: 55 passed;
- KMS-worker rejection ownership tests: 4 passed; and
- scanout tests: 65 passed.

The final full run was:

```text
TMPDIR=/tmp rtk cargo test --locked
cargo test: 3018 passed, 5 ignored, 40 filtered out (30 suites, 42.15s)
```

## Verification results

Passed:

- `rtk cargo fmt --all -- --check`
- `rtk cargo check --locked` — zero errors, seven existing warnings in unrelated input/runtime/debug code.
- `rtk git diff --check`
- `TMPDIR=/tmp rtk cargo test --locked`

The final `rtk cargo clippy --locked --all-targets --all-features -- -D warnings` did not pass. It reported 22 errors and one warning. The reported locations are existing dirty-checkout lint findings in `src/compositor/protocols/workspace.rs`, `src/compositor/surface.rs` (the manual `SurfaceOpaqueRegion` default at line 155 was pre-existing code), `src/compositor/workspace_protocol.rs`, `src/compositor/state/fullscreen.rs`, dirty tiling files, `src/compositor/state/window_interaction_tests.rs`, and dirty WM layout files. No task-owned Clippy failure was found; unrelated lint cleanup was deliberately not performed.

The final source-layout check did not pass. Exact over-limit files reported by `bin/check-source-layout` were:

```text
src/compositor/tests/support/frame_buffer_client.rs: 2102 > 2000
src/compositor/tests/windows.rs: 2087 > 2000
src/compositor/state/desktop_windows.rs: 1516 > 1500
src/compositor/state/window_interaction_tests.rs: 2104 > 2000
src/compositor/state/window_interaction.rs: 1606 > 1500
src/compositor/state/windows.rs: 1696 > 1500
src/compositor/state/surfaces.rs: 1711 > 1500
src/compositor/toplevel_publication.rs: 1501 > 1500
src/compositor/mod.rs: 953 > 800
src/compositor/server.rs: 1714 > 1500
src/compositor/state_data.rs: 1688 > 1500
src/native_output/runtime/bootstrap.rs: 1551 > 1500
src/native_output/runtime/presentation_cycle.rs: 1600 > 1500
src/native_output/input/routing.rs: 1559 > 1500
src/native_output/tests/output.rs: 2034 > 2000
src/native_output/tests/input.rs: 2046 > 2000
src/xwayland/xwm/events.rs: 1551 > 1500
src/xwayland/tests.rs: 1502 > 1500
src/xwayland/service.rs: 1532 > 1500
```

These include known unrelated history and several files already participating in the dirty checkout. They were not split or rewritten as unrelated cleanup.

During implementation, the required RED index test initially failed to compile because the new index APIs did not yet exist; it was then made green by the implementation. An initial oracle run exposed a test-model slot-pixel expectation and an initial surface locality run exposed test setup/import issues; those task-owned test issues were corrected. A pre-final full-suite invocation also exposed two failures:

- `native::event_loop::tests::non_xwayland_closed_registration_still_surfaces_ebadf`: `unwrap_err()` received `Ok(true)` at `src/native/event_loop.rs:1275:42`.
- `xwayland::trace::tests::production_lifecycle_names_survive_geometry_noise`: assertion left `6`, right `5`, at `src/xwayland/trace.rs:279:9`.

Neither file was modified by this task, and the final full-suite rerun passed all tests.

## Correctness and ownership review 1

The first adversarial review explicitly challenged:

- SurfaceId destruction and numeric reuse;
- destruction between render and pageflip;
- a newer surface commit after render;
- render preparation/execution failure;
- KMS submission failure;
- superseded and worker-queued transactions;
- Direct Scanout transitions;
- software cursor and hardware cursor bundled in a primary;
- hardware cursor-only PlaneDelta;
- cursor supersession;
- theme and hidden cursor delivery;
- XWayland minimize/restore, restack, and resize;
- popup/subsurface trees;
- workspace switches; and
- fullscreen culling.

The review found and fixed the minimized Wayland global-retain residual. The remaining cases are protected as follows:

- generation is checked before settlement, so reused numeric IDs cannot consume an old token;
- destruction removes generation/journal authority and settlement safely skips stale entries;
- newer commits remain pending because tokens carry their sampled counters and settlement rejects regressions;
- failed, dropped, superseded, or unsubmitted transactions drop their owned token;
- cursor-only transactions have independent transaction ownership;
- worker queue ownership is retained through the ledger/sidecar path;
- bundled hardware cursor identity is frozen before worker extraction;
- theme/hidden cursor paths have no client source key;
- final resolved scene IDs exclude inactive and culled surfaces; and
- popup/subsurface surfaces are sampled from the expanded final scene rather than by root approximation.

## Locality and accidental-scan review 2

The second adversarial review searched production code for:

```text
renderable_surfaces.iter().find(...)
renderable_surfaces.iter_mut().find(...)
renderable_surfaces.iter().any(...)
renderable_surfaces.retain(...)
renderable_surfaces.drain(...)
```

Remaining occurrences were classified as follows:

- ordinary Wayland content: removed from the steady-state path and replaced with indexed lookup;
- damage-only content: indexed lookup;
- ordinary mapped XWayland content: in-place replacement when placement is unchanged;
- incremental active-scene publication: indexed lookup for the single-surface path;
- presentation capture: exact ID iteration only; the old global method is compatibility/test-only;
- pageflip settlement: keyed generation/journal lookup, indexed renderable update, or keyed cursor update;
- cursor presentation: frozen source-key lookup, never current-focus lookup; and
- topology/tree/geometry/input paths: remaining scans are real cold-path work and were not incorrectly flattened into the content authority.

Every production capture/settlement call site was checked. Normal atomic, initial-modeset, compatibility, Direct Scanout, primary pageflip, worker sidecar, and cursor-only pageflip paths now carry or consume the matching local token. The only remaining production reference to the global capture wrapper is the compositor compatibility API and a test runtime command; no normal native frame silently falls back to it.

## Preserved NativeOutput repair residual

The existing conservative NativeOutput repair behavior was intentionally preserved: when content identity changes while current logical damage is authoritative Empty, the current surface footprint may still be repainted to repair an output slot after an unpresented/rejected attempt.

The rejected-content-only repair regression remains green. This closure does not attempt to distinguish authoritative client Empty from output-repair-required Empty. That separation is explicitly deferred to the next closure; removing the heuristic here could expose stale pixels.

## Non-goals and remaining uncertainty

This change did not modify O1 admission/controller/predictor behavior, scheduler target selection, KMS-worker policy/defaults, triple-buffer policy/defaults, Direct Scanout admission/defaults, VRR, tearing, commit timing/FIFO policy, workspace semantics, Dwindle semantics, or the ActiveScene cloning model.

No real TTY/DRM/KMS/165 Hz qualification was executed. No claim is made that this source change alone resolves an observed approximately 30 FPS symptom. Hardware pageflip behavior, driver cursor-plane behavior, and real output buffer-age behavior remain to be qualified on native hardware.

The architectural end state is:

```text
content commit       -> work for its surface
resolved frame       -> exact primary sampled surfaces
cursor transaction   -> exact client cursor content sampled
confirmed pageflip   -> settles only that frozen lineage
topology mutation    -> topology work
global surface count -> not a steady-state content/presentation tax
```
