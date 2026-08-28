# Typhon Workspace Runtime v1.2 + Special Workspace v1

## 1. Baseline

- Baseline HEAD: `9d3fb34b45f6ce4ffc4582c3231e220b3643e959`.
- The checkout was intentionally dirty and remained authoritative.
- Existing dirty state included two deleted reports, cursor/frame/XWayland/native-input edits, and the untracked O1 v2 closure study. Those files were preserved.
- No commit, reset, restore, stash, clean, cargo clean, or worktree creation was performed.

## 2. Architecture discovered

Typhon already had canonical regular `WorkspaceId`, `WorkspaceManager`, orthogonal `LayoutMembership`, `WindowManagementState`, regular workspace switching, ActiveSceneView, scene render generation, hidden SurfaceCommit suppression, ext-workspace-v1 regular publication, XDG/X11 relationship handling, workspace-aware focus, and fullscreen/scanout qualification. The implementation extends those authorities instead of creating a second lifecycle model.

## 3. Confirmed v1.2 debt and closure

The remaining debt was closed as one runtime change: callback and feedback admission is partitioned at event time; scheduler-facing prepare work is indexed by typed scene owner; active-scene order changes are compared against the cached visible order; the test-only ActiveScene fallback is gone; visible counters are computed from the visible queues; and workspace-switch cleanup is limited to ownership that leaves the active scene.

## 4. SpecialWorkspaceId

`SpecialWorkspaceId` is a compact non-zero typed identity with `Copy`, equality, ordering, and hashing. `SpecialWorkspaceId::DEFAULT` is the only configured v1 Special. The manager rejects unknown Special IDs, leaving room for later named Specials without encoding them as regular workspaces.

## 5. WorkspaceLocation

`WorkspaceLocation` is `Regular(WorkspaceId)` or `Special(SpecialWorkspaceId)`. `WindowManagementState` stores location and layout separately. The explicit accessors are `location()`, `regular_workspace()`, and `special_workspace()`; the ambiguous numeric `workspace()` API and `with_workspace()` were removed.

## 6. WorkspaceManager

The manager retains an active regular workspace and an independent optional visible Special overlay. Regular activation never hides Special. The regular workspace collection remains ten entries and Special is not included in it. Toggle outcomes are typed as Opened, Closed, or UnknownSpecial.

## 7. ActiveSceneSelection

ActiveSceneView caches derived `ActiveSceneSelection { regular, special }`. It is presentation/input state only; WM membership remains `WorkspaceLocation`. Global/output-owned surfaces resolve as `SceneWorkOwner::Global`. Managed auxiliary surfaces resolve through their canonical managed root and inherit its location; they do not receive an independent membership merely because they are auxiliary.

## 8. Scene ordering

Event-time scene rebuilds order regular application trees below Special application trees, while layer-shell Background/Bottom remain below and Top/Overlay remain above both. Root stack keys preserve existing stacking and stable subsurface order. Special popups stay with their application root.

## 9. Frame-callback partitioning

Visible and hidden frame callbacks are held in separate queues at admission. Rendering takes only the visible queue. Hidden callbacks are repartitioned only when a scene-affecting event changes selection or ownership, so active output frames do not scan parked hidden callbacks.

## 10. Presentation-feedback partitioning

Pending presentation feedback uses the same visible/hidden queue model. Only visible feedback is captured for the active frame. Hidden feedback remains pending and is reconsidered on scene events; destruction still discards it through the existing terminal path.

## 11. Explicit-sync and scheduler quiescence

`SceneWorkIndex` records prepare work, callback work, feedback work, and unowned callback work by `Global` or `Location(WorkspaceLocation)`. It is rebuilt at queue/readiness/cancellation, scene-selection, and canonical relationship/membership boundaries. The scheduler-facing predicates consult the typed index instead of repeatedly scanning hidden FIFO, explicit-sync, and surface-tree work.

## 12. Relationship ownership freshness

XDG parentage, X11 `WM_TRANSIENT_FOR`, and other canonical relationship updates run the existing membership transition and then refresh the derived scene order and SceneWorkIndex together. Auxiliary scene ownership therefore migrates with the same atomic membership transition rather than becoming a stale side cache.

## 13. Stack-generation fix

Window-stack reorder now compares the resulting cached active-scene order. Hidden-only reorder produces no active scene generation increment; visible order changes do. The comparison is independent of unrelated renderables elsewhere in the compositor.

## 14. Test fallback removal

`ActiveSceneView` no longer substitutes all renderable surfaces when the active scene is empty under `cfg(test)`. Test fixtures now explicitly rebuild the scene when they construct renderables. Empty active Regular workspaces behave identically in test and production builds.

## 15. Exact accounting

Callback and feedback discard paths count removals from visible queues before adjusting visible counters. Removing hidden work cannot decrement visible counts. Restore/requeue paths place resources back into the appropriate queue and update the exact counter.

## 16. Super+S

Linux evdev `KEY_S` is wired through `BindingAction`, `NativeWindowAction`, native routing, and compositor Special toggle. The default binding is exact Super+S, press-only, repeat-disabled, inhibition-aware, and consumed without leaking the key sequence.

## 17. Super+Shift+S

The exact Super+Shift+S binding moves the focused managed family to `Special(DEFAULT)` or back to the active Regular location. Moving into Special is intentionally silent: it does not open the overlay. Location changes preserve geometry, mode, layout, minimize state, constraints, and client buffers; no location-only configure is issued.

## 18. Family and auxiliary inheritance

Moving a focused child resolves its canonical family root and transitions the whole managed family. Parent/transient children inherit the exact `WorkspaceLocation` while retaining their own `LayoutMembership`. Global layer-shell and cursor ownership remains independent of workspace selection.

## 19. Focus behavior

Opening Special preserves exclusive layer-shell focus. Otherwise the most eligible Special candidate wins through existing focus serial and stack ordering. If no Special candidate exists, the current regular focus is not displaced. Closing Special clears hidden Special application focus and restores the best eligible active-Regular candidate. Authorized activation of a hidden Special toplevel opens that Special before raising and focusing it; activation authorization remains unchanged.

## 20. Pointer and interaction ownership

Scene transitions cancel pointer grabs, constraints, popup grabs, move/resize interactions, and related focus only for windows whose ownership leaves the active scene. Unaffected regular or global ownership survives unrelated Special transitions. Existing terminal interaction cleanup remains the authority for resize completion.

## 21. Fullscreen and direct scanout

Visible Special application content is treated as an overlay over regular fullscreen content. It prevents regular fullscreen from culling Special or qualifying as solitary direct scanout. Special fullscreen remains composited conservatively; direct scanout is rejected when visible Special application content is present.

## 22. Geometry and persistence

Special toggle and Regular↔Special membership changes do not scale, transform, resize, recreate, unmap, or reconfigure clients. Hidden Special windows remain mapped, alive, buffered, and stateful. Existing visual geometry and layout authorities remain intact.

## 23. XWayland ClearWorkspace

The typed pipeline now supports `WindowBackendCommand::ClearWorkspace` through `XwmCommand::ClearWorkspace`. Execution deletes `_NET_WM_DESKTOP`. Regular locations continue to publish the existing one-based Typhon to zero-based EWMH conversion.

## 24. EWMH behavior

Regular→Special clears `_NET_WM_DESKTOP`; Special→Regular publishes the destination regular desktop. Valid EWMH desktop move requests on a Special X11 family move it to the requested Regular workspace. Special toggle does not publish a root current-desktop change.

## 25. ext-workspace-v1

The regular workspace bridge remains regular-only. Special is not WorkspaceId 11, does not increase the regular count, does not receive a regular handle, and does not change the active regular protocol handle.

## 26. Control snapshots

Regular windows report numeric workspace labels. Special windows report `"special"`, including hidden Special membership; mapped state remains derived from retained geometry/mapping, not from active-scene visibility.

## 27. Astrea publication

Regular↔Special membership changes mark the affected toplevel and structure snapshots dirty. Scene changes and Special toggles mark structure state without generating fake regular workspace publication.

## 28. Performance evidence

- The repeated native frame resolution test performs 1000 resolutions while inactive Regular and hidden Special content exists; it borrows the stable cached scene and reports zero additional ActiveScene rebuilds and zero incremental surface updates.
- Visible callback and feedback extraction is queue take-only in the frame path. Hidden queue repartition is event-boundary work, not steady-state frame work.
- Typed SceneWorkIndex tests demonstrate hidden prepare work is indexed under its hidden `WorkspaceLocation` and does not make `has_pending_frame_prepare_work()` true for the active scene.
- Hidden-only stack reorder is covered by a zero active-scene-generation test; Special open/close and visible overlay ordering are covered by scene tests.
- No DRM/KMS hardware qualification was claimed.

## 29. Focused tests

Fresh focused results after the final correction include:

- `workspace`: 22 passed.
- `scene`: 69 passed.
- `frame`: 228 passed.
- `special`: 13 passed.
- `fullscreen`: 53 passed.
- `presentation`: 190 passed.
- Special binding resolution: 1 passed.
- Special native routing/no-key-leak: 1 passed.
- `cargo check --locked --all-targets`: passed.

## 30. Full-suite result

The final repository-wide `cargo test --locked` run completed 1,753 passing tests, 36 failing tests, and 2 ignored tests. Thirty-five failures were XWayland/Astrea path-sensitive tests beginning with `Error { kind: InvalidInput, message: "path must be shorter than SUN_LEN" }`, followed by test-state poisoning. One pre-existing cursor-persistence lock test also observed `Err(Busy)` where it expected `Err(Insecure)` during the parallel full-suite run; its isolated rerun passed. These are environment/order-sensitive failures rather than Special or quiescence assertions. A focused native-input filter also had one unrelated `direct_test_only_does_not_consume_input_fence` file-descriptor identity failure; it is outside this feature and was not changed.

## 31. Review Pass 1 — correctness and ownership

The review specifically checked for numeric Special encoding, fake ext-workspace/EWMH publication, duplicate lifecycle authority, minimize/unmap implementation, geometry mutation, layout coupling, split families, hidden focus, layer-shell focus, over-broad interaction cancellation, fullscreen culling, layer ordering, and XDG/X11 divergence. The actionable issue found was that the first move-to-Special implementation opened the overlay; it was corrected so Super+Shift+S remains silent and the regression test asserts Special stays hidden.

## 32. Review Pass 2 — performance and tiled future

The performance/future-layout review checked frame-path filtering, callback/feedback scans, typed prepare indexing, parent walks, hidden-only generation churn, hidden commits, popup identity, XWayland publication, locks/threads, and location/layout coupling. The resulting model keeps `WorkspaceLocation` independent from `LayoutMembership`, so future Dwindle trees can key naturally by Regular or Special location without another WM state rewrite.

## 33. Final status and scope boundary

The implementation is complete in the current dirty working tree, with no commit created. Final `git status --short` contains the two pre-existing deleted reports; the pre-existing modified cursor/frame/XWayland/native-control files; task-owned modifications across WM, compositor state, native Special bindings, control, and XWayland command plumbing; and these untracked additions:

- `REPORT-2026-08-22-typhon-special-workspace-v1-quiescence.md`
- `docs/superpowers/specs/2026-08-22-typhon-workspace-runtime-v1-2-special-workspace-design.md`
- `docs/superpowers/plans/2026-08-22-typhon-workspace-runtime-v1-2-special-workspace-plan.md`
- `src/wm/special_workspace.rs`
- `src/compositor/state/scene_work.rs`

The pre-existing untracked `TYPHON_O1_V2_CLOSURE_STUDY_2026-08-21.md` remains present. The design and execution plan are recorded in:

- `docs/superpowers/specs/2026-08-22-typhon-workspace-runtime-v1-2-special-workspace-design.md`
- `docs/superpowers/plans/2026-08-22-typhon-workspace-runtime-v1-2-special-workspace-plan.md`

Hybrid Floating/Tiled runtime policy, Dwindle trees, Niri/Spatial/Infinite Canvas behavior, gaps, animation, dim/blur, and other explicitly out-of-scope layout work remain unimplemented.
