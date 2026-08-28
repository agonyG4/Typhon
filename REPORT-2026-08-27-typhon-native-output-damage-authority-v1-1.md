# Typhon Native Output Damage Authority v1.1 Closure Report

Date: 2026-08-27

Repository: `/home/agony/GitHub/Typhon`

Scope: NoVisualChange runtime ownership, logical scene-generation retirement, compatibility frame-batch ownership, exact surface-damage ownership, Atomic no-visual transaction normalization, and preservation of the existing Surface Presentation Locality v2 architecture.

The dirty checkout was treated as authoritative. Unrelated modifications were preserved. No reset, clean, stash, discard, or broad unrelated refactor was performed.

## Outcome

`NoVisualChange` is now a terminal logical outcome with separate ownership for:

- logical scene scheduling/coalescing state;
- exact surface-damage lineage;
- protocol callbacks, presentation feedback, and buffer releases;
- physical output presentation.

A scene-only no-visual result settles its exact surface token without fabricating a compositor frame batch. Protocol or release work creates or reuses exactly one batch and completes it through the existing terminal NoVisualChange state machine. Compatibility batch creation is delayed until the resolved output decision proves that rendering is required.

No physical presentation authority is advanced by NoVisualChange.

## Pre-change source audit

The v1.1 audit covered the requested compositor, native-output runtime, scanout, presentation, cursor, KMS-worker, and damage files. The codebase-memory graph was verified for project `home-agony-GitHub-Typhon`, generation `2026-08-27T20:20:40Z`; the only recorded parse-partial range was `src/native_output/runtime/presentation_cycle.rs` line 118, which was read directly from source.

The relevant pre-change runtime flow was:

1. `refreshed_published_state()` compared `scene_render_generation()` with `last_rendered_scene_generation` and separately reported `has_unowned_frame_work()`.
2. The compatibility branch called `capture_frame_callbacks_for_render()` before final output damage resolution. That helper always created a `legacy_prepared_frame_batch` when none existed, including an empty batch.
3. The no-primary path captured surface lineage only when `pending_frame_work` was true. This made logical surface accounting depend on protocol work.
4. The no-primary path completed protocol work through `complete_no_visual_change_frame_batch()` when a batch existed, but did not itself own the surface-only path.
5. `frame_completed` was derived from `pending_frame_work`, so a scene generation that was terminally accounted with no callback could remain represented as dirty to the logical scheduler.
6. Atomic EGL/GBM `NoLogicalDamage` used the generic dropped-transaction helper even though the ledger had a dedicated NoVisualChange transition. Direct Scanout already used the dedicated transition.
7. `last_rendered_scene_generation` was a logical scheduling/coalescing baseline; NativeSceneHistory, output swapchain presentation serials, pageflip sequence, planner presented history, and protocol presented timestamps remained separate physical authorities.

The v1/v2 audit also confirmed that the current checkout already contains the intended global RenderableSurface index and exact filtered presentation lineage from the earlier locality closures. This v1.1 closure did not replace or weaken those designs.

## Exact hot-path scans found

The ordinary Wayland content paths, damage-only path, incremental ActiveScene publication, filtered presentation capture, and pageflip settlement do not use a global `renderable_surfaces.iter().find()` or `iter_mut().find()` in the current source.

The remaining vector traversals are bounded cold/topology operations or debug invariants:

- `src/compositor/state/surfaces.rs`: retain/removal and debug index validation;
- `src/compositor/state/subsurfaces.rs`: tree raise/reorder through `drain(..)`;
- `src/compositor/state/windows.rs`: minimize and window raise/reorder through `drain(..)`;
- `src/compositor/state/scene_order.rs`: real committed-stack reorder;
- `src/compositor/state/surface_commits.rs`: unmap/family teardown and role-adoption paths;
- `src/compositor/state/xwayland_windows.rs`: initial insertion, topology-changing replacement, and family reorder paths.

The global `capture_surface_damage_presentation()` API remains for the legacy compositor test/support helper `mark_render_damage_presented()`. No normal native frame uses it. Native bootstrap, composited frames, Direct Scanout, and cursor transactions use keyed filtered capture.

## Global RenderableSurface index architecture

`Vec<RenderableSurface>` remains the authoritative ordered render stack. The sidecar `HashMap<u32, usize>` maps `SurfaceId` to the authoritative vector position. It is distinct from `ActiveSceneView.surface_indices`, which maps a cloned active-scene view.

The narrow access boundary is in [surfaces.rs](/home/agony/GitHub/Typhon/src/compositor/state/surfaces.rs:4):

- `renderable_surface_index()`;
- `renderable_surface()`;
- `renderable_surface_mut()`;
- `append_renderable_surface()`;
- `replace_renderable_surface()`;
- `remove_renderable_surface()`;
- `retain_renderable_surfaces()`;
- `rebuild_renderable_surface_index()`.

Append and same-ID replacement are constant-time. Removal, retain, and real bulk reorder rebuild the sidecar in O(N), which is permitted for topology work. The debug/test invariant proves equal lengths, unique IDs, exact vector positions, and no stale map entries. It is not executed on release-build content commits.

## Production index mutation sites audited

All production mutation sites were classified as follows:

- initial insertion: `append_renderable_surface()`;
- same-ID content replacement: `replace_renderable_surface()`;
- direct removal: `remove_renderable_surface()`;
- unmap/removal and XWayland family withdrawal: `retain_renderable_surfaces()`;
- committed-stack reorder and layer ordering: `reorder_renderable_surfaces_by_committed_stack()`;
- window raise/restack: `reorder_renderable_surfaces_by_window_stack()` and the window drain paths;
- subsurface/tree raise: the subsurface drain path followed by an index rebuild;
- minimize/restore: window drain/append paths followed by an index rebuild;
- XWayland initial map or topology-changing replacement: append/retain plus committed-stack reorder;
- test-only direct mutation: focused compositor state tests, followed by explicit invariant checks where required.

No ordinary content-only path directly rewrites the ordered vector or scatters sidecar map writes.

## Wayland content-path complexity

The authoritative content path in [surface_commits.rs](/home/agony/GitHub/Typhon/src/compositor/state/surface_commits.rs:6) resolves the existing renderable once through `content_renderable_surface_index()`. That result supplies surface existence, buffer identity, visual mapping state, and the mutable vector position. Content decisions are derived before one indexed mutable update.

Same-size buffer rotation remains a resource/import/lifetime/synchronization identity change, not an automatic logical Full-damage promotion. First map and real geometry/viewport/scale/transform changes retain conservative behavior.

Before the locality closure, repeated property-specific global searches made steady-state content work scale with the global renderable population. The current path is O(1) average for the global surface lookup and does not rebuild the global index for content-only updates.

## Damage-only path complexity

`commit_surface_damage_only()` obtains the current buffer from its keyed current-buffer map, resolves the renderable through the same global index, and mutates the indexed entry. The old global vector search is absent from the steady-state path.

The path preserves SHM pixel updates, Partial/Empty authority, geometry promotion, and conservative Full behavior when visual mapping changes. It is O(1) average for the authoritative renderable lookup and does not rebuild the global renderable index for content-only damage.

## Incremental ActiveScene publication

`refresh_active_scene_surface()` in [active_scene.rs](/home/agony/GitHub/Typhon/src/compositor/state/active_scene.rs:157) uses the ActiveScene view index to locate the cloned view entry and the global RenderableSurface index to clone the authoritative source. It falls back to a complete ActiveScene rebuild only when membership or selection requires it.

The global and ActiveScene indices remain separate responsibilities; the ActiveScene ownership/cloning model was not redesigned.

## XWayland content/topology behavior

For an already mapped, non-minimized XWayland surface whose placement and stack membership are unchanged, [xwayland_windows.rs](/home/agony/GitHub/Typhon/src/compositor/state/xwayland_windows.rs:321) preserves the existing render placement and visual clip, replaces the RenderableSurface at its existing indexed position, records conservative Full XWayland content damage, and avoids global retain/remove/reinsert, stack reorder, and root visual-tree reassignment.

Topology work remains for initial map, unmap, minimized/restore transitions, placement or resize changes, X11 restack/family changes, and other real visual-assignment changes. XWayland Full content damage remains conservative; no unsupported X11 partial-damage authority was introduced.

The conditional visual-assignment decision is in [xwayland_windows.rs](/home/agony/GitHub/Typhon/src/compositor/state/xwayland_windows.rs:384). Same-placement content replacement increments the bounded replacement counter and does not increment topology-reorder or full-visual-reassignment counters.

## Exact composited frame-lineage architecture

The compatibility path now resolves `ResolvedNativeFrameScene` and `NativeOutputDamage` before acquiring a compatibility frame batch. The sequence is:

```text
resolve final scene and output damage
        |
        +-- NoVisualChange: capture exact scene lineage when scene generation changed
        |                  settle surface-only or batch-owned terminal work
        |
        `-- visual render: capture exact scene lineage, then create/use one batch
                           attach token, render, submit, and wait for matching pageflip
```

The no-primary branch in [presentation_cycle.rs](/home/agony/GitHub/Typhon/src/native_output/runtime/presentation_cycle.rs:980) samples only the final resolved scene surface IDs. It captures the exact software client-cursor commit only when software cursor content is part of the primary composition.

The ordinary non-Atomic render path captures its exact token at the final resolved-scene boundary and only then calls `capture_frame_callbacks_for_render()` before rendering. A render preparation/execution failure restores or discards protocol ownership without settling the surface token.

Atomic composited capture occurs inside `render_frame()` after final scene-signature validation. The initial Atomic modeset uses its already-created `initial_resolved_scene` before rendering and settles the exact token only after initial presentation is established.

## NoVisualChange ownership and logical generation retirement

The state-level owner is [frames.rs](/home/agony/GitHub/Typhon/src/compositor/state/frames.rs:119), exposed through [server_frames.rs](/home/agony/GitHub/Typhon/src/compositor/server_frames.rs:342):

- surface token plus no protocol/release work: `commit_surface_damage_no_visual_change(token)` with no dummy batch;
- protocol or release work: create/use one prepared batch, attach the token if present, and call `complete_no_visual_change_frame_batch()`;
- no token and no work: return false and create nothing.

`has_unowned_frame_work()` includes pending SHM and dmabuf releases because releases are legitimate batch-owned protocol work. The callback-only predicate remains compatible with the existing XWayland callback contract; when a protocol-only tick encounters pending releases, it drains them through the NoVisualChange batch owner.

The logical baseline helpers are in [commit_timing.rs](/home/agony/GitHub/Typhon/src/native_output/runtime/commit_timing.rs:26). After a terminal no-primary NoVisualChange result, `last_rendered_scene_generation` is retired to the current logical scene generation. The cycle result is based on `scene_changed || protocol_work`, so a real scene transition is completed even without callbacks while an idle tick remains incomplete.

The same logical retirement is applied after Atomic `NoLogicalDamage`/skipped render handling. A later scene mutation produces `scene_changed == true` again.

## Software client-cursor lineage

Software cursor content is sampled as part of the final primary composition only when the frozen delivery mode is `PresentedCursorDelivery::Software` and `client_cursor_render_state()` supplies a client wl_surface. The sampled surface ID and commit sequence are frozen before rendering and carried by the primary surface-damage token.

Hidden delivery and theme/compositor cursors produce no client-surface sample. The pageflip path does not look up current focus later.

## Hardware client-cursor lineage

Hardware cursor lineage continues to use the existing frozen `NativeCursorSourceKey::Client(NativeCursorImageKey)`, including surface ID, buffer ID, and commit sequence. Bundled primary transactions add the exact frozen cursor commit to the transaction-owned sample set. A later cursor commit cannot be consumed by the older primary pageflip.

Independent cursor-only PlaneDelta transactions retain their exact cursor token through the frozen cursor/sidecar/transaction ownership path. Matching cursor pageflip settlement commits only that cursor token and does not require a primary frame. Theme cursors carry `NativeCursorSourceKey::Theme` and therefore no client token.

Superseded cursor transactions do not settle their old token.

## Direct Scanout preservation

Direct Scanout remains local: the primary sample is the candidate surface only, plus an exact frozen client cursor plane sample if the same KMS transaction presents one. The current identical-candidate lightweight NoVisualChange branches remain unchanged in policy and admission behavior.

The invariant is that an identical `DirectScanoutCandidateKey` contains the same content epoch and therefore proves there is no newer visual content commit requiring a new damage token. The deterministic regression `identical_direct_candidate_key_proves_the_visual_content_epoch_is_unchanged` covers this.

## Presentation settlement architecture

Physical pageflip settlement remains transaction-local. A presented transaction obtains its frozen `SurfaceDamagePresentation` from the transaction or owned frame batch and settles it only after the matching confirmed pageflip. Surface settlement verifies the current `SurfacePresentationKey.generation`, advances only the sampled commit, obtains the journal by keyed lookup, and updates either the global renderable through its SurfaceId index or the client cursor map directly.

NoVisualChange does not call the physical presentation completion path. It does not advance NativeSceneHistory presented state, output swapchain presentation serials, last-presented serials, confirmed pageflip sequence, wp_presentation presented timestamps, or PartialRepaintPlanner presented state.

Atomic no-visual output transaction handling now uses the dedicated `settle_no_visual_change_output_transaction()` transition in [atomic_egl_gbm.rs](/home/agony/GitHub/Typhon/src/native_output/scanout/atomic_egl_gbm.rs:967). The ledger accepts this only from Built/Ready and records one terminal NoVisualChange drop. A second settlement, submission, or presentation is rejected.

## Deterministic operation-count evidence

The bounded locality metrics and tests provide deterministic evidence rather than timing thresholds:

- 1,000 repeated Wayland content commits: 1,001 indexed content lookups and zero global index rebuilds;
- 1,000 repeated Wayland damage-only commits: 1,001 indexed content lookups and zero global index rebuilds;
- 1,000 unchanged-geometry XWayland content commits surrounded by 1,000 unrelated surfaces: 1,000 in-place replacements, zero topology reorders, zero full visual reassignments, zero index rebuilds;
- filtered presentation capture with 1,000 unrelated surfaces and four requested samples: four sampled entries, four journal lookups, four settlement entries, four settlement journal lookups, and zero global presentation scans;
- the 128-entry Empty sequence settles each exact surface token without a prepared or submitted batch, leaves physical state untouched, and preserves a later Partial damage result;
- compatibility no-work, callback-work, presentation-feedback-work, and release-work cases each verify the correct batch ownership boundary.

The required relationship is therefore operation-count proportional to sampled or owned work, not global compositor population.

## Integrated client/output swapchain oracle

The existing integrated oracle in [integrated_swapchain_oracle.rs](/home/agony/GitHub/Typhon/src/native_output/tests/integrated_swapchain_oracle.rs:1) remains green: 7 tests passed.

It covers client A/B/C/A rotation, output slot rotation, partial damage, authoritative Empty, buffer-age repair, rejected output attempts, confirmed-only surface settlement, and newer client commits surviving older pageflips. The conservative rejected-output repair behavior was preserved.

## RED tests and focused tests

The required RED-first sequence was performed. Before the state ownership helper existed, the new frame tests failed to compile because `CompositorState::settle_no_visual_change_work` was absent. After production implementation, the tests passed. A first 128-entry test used a 2x2 surface with an out-of-bounds partial rectangle; that test-only fixture was corrected to a 100x80 surface, with no production behavior change.

Focused final results include:

- commit-timing/logical-retirement tests: 3 passed;
- compositor frame-consumption tests: 48 passed;
- compositor compatibility surface-frame tests: 49 passed;
- presentation transactions: 58 passed;
- Direct Scanout stage tests: 18 passed;
- output damage: 43 passed;
- output retry: 3 passed;
- integrated swapchain oracle: 7 passed;
- scanout: 66 passed;
- plane: 59 passed;
- pageflip: 67 passed;
- partial repaint: 33 passed;
- no-visual-change: 12 passed;
- surface-damage: 13 passed;
- buffer-age: 2 passed;
- isolated XWayland callback ownership regression: 1 passed.

## Full verification

- `rtk cargo fmt --all -- --check`: passed.
- `rtk cargo check --locked`: passed with 0 errors and 7 existing dead-code warnings in native input, cursor arbitration, resource-efficiency, and pointer-debug code.
- `TMPDIR=/tmp rtk cargo test --locked`: final run passed, 3,046 passed, 5 ignored, 40 filtered out across 30 suites.
- `rtk git diff --check`: passed.
- `rtk run "bin/check-source-layout"`: failed on 18 existing oversized files. Exact reported paths were `src/compositor/tests/support/frame_buffer_client.rs` (2119/2000), `src/compositor/tests/windows.rs` (2087/2000), `src/compositor/state/desktop_windows.rs` (1516/1500), `src/compositor/state/window_interaction_tests.rs` (2104/2000), `src/compositor/state/window_interaction.rs` (1606/1500), `src/compositor/state/windows.rs` (1696/1500), `src/compositor/state/surfaces.rs` (1815/1500), `src/compositor/toplevel_publication.rs` (1501/1500), `src/compositor/mod.rs` (962/800), `src/compositor/server.rs` (1719/1500), `src/compositor/state_data.rs` (1688/1500), `src/native_output/runtime/bootstrap.rs` (1551/1500), `src/native_output/runtime/presentation_cycle.rs` (1650/1500), `src/native_output/input/routing.rs` (1559/1500), `src/native_output/tests/input.rs` (2046/2000), `src/xwayland/xwm/events.rs` (1551/1500), `src/xwayland/tests.rs` (1502/1500), and `src/xwayland/service.rs` (1532/1500). These files already contain broader dirty-checkout work and were not split or rewritten as unrelated cleanup.
- `rtk cargo clippy --locked --all-targets --all-features -- -D warnings`: failed with 22 errors and 1 warning after the task-owned test lint was fixed. Remaining diagnostics are in unrelated pre-existing workspace/layout, protocol, adaptive-buffering, resize, and test code; no remaining diagnostic points to the v1.1 production ownership changes.

The first full-suite attempt exposed one task-owned interaction: adding releases to frame-batch work made an existing XWayland callback-only test fail because it had two pending buffer releases. Root-cause instrumentation showed `prepare=false`, `interactive=false`, `callbacks=true`, `feedback=0`, `color=0`, `SHM releases=2`, and `dmabuf releases=0`. The callback-only classification was preserved, and protocol-only completion was extended to drain pending releases through a terminal NoVisualChange batch. The isolated test and the final full suite then passed.

## Adversarial review pass 1: correctness and ownership

Reviewed:

- no visual change with no callback, callback, presentation feedback, and buffer releases;
- compatibility and Atomic branches;
- no prepared batch and existing prepared-batch reuse;
- scene-generation retirement versus surface-damage settlement;
- physical presentation state;
- later scene mutation after logical retirement;
- render preparation/execution failure;
- KMS submission rejection and worker failure paths;
- queued/superseded transactions;
- Direct Scanout transition;
- software cursor, bundled hardware cursor, independent hardware cursor PlaneDelta, cursor supersession, and theme cursor;
- XWayland minimize/restore/restack/resize and unchanged content;
- popup/subsurface scene resolution, workspace selection, and fullscreen culling.

The review found no double settlement or fake physical presentation. It did find the callback-plus-release classification issue described above; it was fixed at the protocol-only release ownership boundary, then retested.

## Adversarial review pass 2: locality and accidental scans

Searched all relevant production paths for:

- `renderable_surfaces.iter().find(...)`;
- `renderable_surfaces.iter_mut().find(...)`;
- `renderable_surfaces.iter().any(...)`;
- `renderable_surfaces.retain(...)`;
- `renderable_surfaces.drain(...)`;
- every native call to global and filtered presentation capture;
- every pageflip settlement and `mark_render_damage_presented()` call.

The remaining retain/drain/ordered-vector traversals are real topology or teardown paths. Ordinary content, damage-only updates, incremental ActiveScene publication, native frame capture, cursor capture, and pageflip settlement are indexed, exact-sample, or transaction-owned. The legacy global capture caller is confined to compositor test/support behavior.

## Preserved behavior and non-goals

The closure preserves:

- Surface Commit Locality v1 same-size A/B/C buffer rotation semantics;
- BufferId resource/import/synchronization/Direct Scanout identity;
- first-map Full damage;
- Partial/Empty authority;
- conservative geometry and mapping transitions;
- rejected-output conservative repair;
- O1 policy/controller/predictor;
- KMS-worker policy/default;
- triple-buffer policy/default;
- Direct Scanout admission/default;
- VRR, tearing, scheduler target selection, workspace, Dwindle, and ActiveScene ownership.

The previous sentence in this section was stale documentation carried forward from the pre-v1 closure. The identity-to-footprint fallback had already been removed by Native Output Damage Authority v1: a generation-only or commit-sequence-only change with authoritative Empty remains Empty. Explicit separation of authoritative client Empty from output-attempt repair remains a later closure.

## Remaining uncertainty and hardware qualification

No real TTY/DRM/KMS/165 Hz qualification was executed in this environment. The deterministic tests establish state-machine ownership, lineage identity, and operation-count locality, but they do not substitute for native hardware measurements.

The known unrelated strict-Clippy and source-layout failures remain accurately reported above. No unsupported claim is made that this source closure alone fixes any observed approximately 30 FPS symptom.
