# Typhon Surface Presentation Locality v2.1 Closure Report

Date: 2026-08-27

Scope: cursor commit identity and the integrated physical output-swapchain
oracle. The current dirty checkout was treated as authoritative. Unrelated
working-tree changes were preserved.

## Outcome

This closure fixes the bufferless client-cursor commit identity gap and
replaces the integrated swapchain test model with explicit rendered,
submitted, and confirmed-presented states. The production fix is deliberately
narrow: the real Wayland commit sequence now reaches the cursor damage-only
path, and the cursor renderable and journal are updated with that sequence.

The v2 global renderable index, exact frame-local presentation capture,
XWayland content/topology split, O(K) capture and settlement, Direct Scanout
locality, and transaction-owned cursor lineage remain intact.

No O1, scheduler, KMS-worker policy, buffering policy, Direct Scanout policy,
VRR, tearing, workspace, Dwindle, or general NativeOutput damage-policy
change was made.

## Pre-change source audit

The requested cursor and output ownership paths were re-audited before the
v2.1 edits:

- `commit_surface_without_buffer()` received the new
  `SurfaceCommitSequence`, but the cursor branch discarded it before calling
  `commit_cursor_surface_damage_only()`.
- The cursor damage-only function updated pixels and accumulated damage while
  retaining the previous `RenderableSurface.commit_sequence` and recording
  the journal entry against that stale identity.
- `NativeCursorImageKey` already included `surface_id`, `buffer_id`, and
  `commit_sequence`; `NativeCursorSourceKey::Client` already carried that
  key through the native cursor path.
- `RenderedOutputFrame` already owned `SurfaceDamagePresentation` and a
  frozen cursor owner. The native cursor owner already had an optional
  `NativeCursorImageKey`.
- Composited capture already resolved the final scene in the atomic path and
  used exact scene surface IDs; the normal and initial paths had the same
  filtered-capture architecture from v2.
- Cursor-only `PlaneDelta` transactions already had a transaction-local
  surface-damage field but lacked a v2.1 regression proving explicit
  supersession behavior for that token.
- The previous integrated oracle treated physical output contents as if they
  changed only at `present()`, so it could not distinguish rendered-but-
  rejected pixels from confirmed presentation.
- The previous integrated oracle did not contain a true client buffer
  rotation with authoritative `Empty` logical damage and an independent
  full-reference comparison.

The historical v2 report at
`REPORT-2026-08-27-typhon-surface-presentation-locality-v2.md` was not
rewritten.

## Exact files changed for v2.1

Production change:

- `src/compositor/state/surface_commits.rs`
- `src/compositor/state/surface_commit_cursor.rs`

Deterministic test and support changes:

- `src/compositor/tests/support/frame_buffer_client.rs`
- `src/compositor/tests/support/input_client.rs`
- `src/compositor/tests/support/registry_state.rs`
- `src/compositor/tests/support/server_runtime.rs`
- `src/compositor/tests/input_output/output_keyboard_cursor.rs`
- `src/compositor/state/surfaces.rs`
- `src/native_output/output/cursor_tests.rs`
- `src/native_output/tests/presentation_transactions.rs`
- `src/native_output/tests/scanout.rs`
- `src/native_output/tests/integrated_swapchain_oracle.rs`

The checkout also contains broad unrelated dirty changes and earlier v2
changes. They were not reset, cleaned, stashed, discarded, or reformatted by
replacement.

## Cursor commit-sequence bug and fix

Before the fix, the cursor branch was equivalent to:

```text
receive commit sequence
discard commit sequence
call cursor damage-only update without the new identity
```

After the fix, the branch passes the real `SurfaceCommitSequence` to
`commit_cursor_surface_damage_only()`. That function now:

1. preserves the current buffer resource and `BufferId`;
2. reads only the changed pixels when the SHM buffer dimensions still match;
3. updates the cursor `RenderableSurface.commit_sequence` to the new
   sequence;
4. accumulates the new damage with existing unpresented damage; and
5. records the accumulated journal entry against the new sequence.

Consequently:

```text
same buffer A, commit 100 -> NativeCursorImageKey(..., buffer A, 100)
same buffer A, commit 101 -> NativeCursorImageKey(..., buffer A, 101)
```

No synthetic buffer identity is created. A later hardware replacement sees a
different source key even though the client reused the same buffer resource.

## Same-buffer SHM reproduction

The new integration support retains one SHM buffer, writes the first pixel,
commits it, rewrites the same backing file, sends only surface damage and a
new `wl_surface.commit`, and snapshots the cursor again. The test proves:

- the cursor surface ID is unchanged;
- the `BufferId` is unchanged;
- the commit sequence advances by one;
- the first pixel changes;
- the journal contains the new commit sequence.

The required RED reproduction was run before the production edit with the
fully qualified test path. It failed at the commit identity assertion with:

```text
left: 6
right: 5
```

The first unqualified filter that matched no tests was not treated as test
evidence. After the fix, the correctly qualified test passed.

Test:

```text
compositor::tests::input_output::output_keyboard_cursor::same_buffer_cursor_damage_commit_advances_exact_content_identity
```

## NativeCursorImageKey and hardware replacement proof

The actual `NativeCursor::replace_image()` path was exercised with an old
client source key at commit 100 and a new key at commit 101 with the same
surface and buffer IDs. A cached cursor buffer avoids a physical DRM upload
dependency while still executing the replacement decision and installation
path.

The test proves:

- `K100 != K101`;
- the same-source early return is not taken;
- the new `CompositorCursorImage` is installed;
- the client source key becomes `K101`.

The test also records the expected cache-hit behavior and does not claim a
real GPU upload. Physical cursor hardware was not available to this
deterministic test.

Test:

```text
native_output::output::cursor::tests::hardware_client_cursor_replacement_uses_new_commit_key_with_cached_buffer
```

## Global renderable index architecture and audit

The ordered `Vec<RenderableSurface>` remains the authoritative render stack.
`CompositorState` has a sidecar `HashMap<u32, usize>` mapping each
`SurfaceId` to its current ordered-vector position. This is distinct from the
ActiveScene view's cloned `SurfaceId -> active-view-index` map.

The narrow API boundary is:

- `renderable_surface_index()`;
- `renderable_surface()`;
- `renderable_surface_mut()`;
- `append_renderable_surface()`;
- `replace_renderable_surface()`;
- `remove_renderable_surface()`;
- `retain_renderable_surfaces()`; and
- `rebuild_renderable_surface_index()`.

Append and same-ID replacement are O(1) average. Removal, retain, and bulk
reordering rebuild the sidecar index. Debug/test invariant validation proves
that map and vector lengths agree, every vector ID maps to its exact position,
every map entry points to the matching vector item, and duplicate IDs cannot
silently pass the length check.

All production mutation families were audited:

| Mutation family | Current ownership and index action |
| --- | --- |
| Initial insertion | `append_renderable_surface`; direct append, no rebuild |
| Same-ID content replacement | `replace_renderable_surface`; position preserved, no rebuild |
| Content update in place | indexed mutable position, no rebuild |
| Removal/unmap | remove/retain helper or explicit topology path, then rebuild |
| Committed-stack reorder | `scene_order` drains/reorders and rebuilds |
| Window stacking/raise | `windows` drains/reorders and rebuilds |
| Subsurface/tree raise | `subsurfaces` drains/reorders and rebuilds |
| Minimize/restore | `windows` removes/restores groups and rebuilds |
| XWayland initial/replacement topology | retain/append/reorder only when topology inputs changed, then rebuild |
| XWayland content replacement | indexed in-place replacement, no rebuild |
| Layer-shell and override-restack | call the real stack reorder helper |
| Visual geometry mutation | local indexed update where content-local; full tree refresh remains a cold geometry/tree path |
| Test-only direct mutation | tests that directly swap the vector explicitly rebuild before checking the invariant |

The checked invariant is not run on every release-build content commit.

## Wayland content locality

The ordinary mapped Wayland buffer path resolves the existing renderable index
once and derives surface state, buffer identity, mapping state, geometry
decisions, and damage publication from that authority before indexed mutation.
The previous independent global searches were removed from the steady-state
path.

The damage-only path uses the same indexed authority. Incremental
ActiveScene publication uses `renderable_surface(surface_id)` rather than a
global vector search.

Complexity is therefore:

```text
before: O(N) global renderable lookup per content commit
after:  O(1) average indexed lookup plus O(1) local update
```

The established 1,000-commit tests report exactly 1,001 content indexed
lookups, zero global index rebuilds, and no dependence on unrelated
population size. The extra lookup is the initial map/publication operation.

The corresponding damage-only test also reports 1,001 content indexed
lookups and zero index rebuilds.

The prior Surface Commit Locality v1 invariants remain covered: same-size
buffer rotation does not become logical Full damage, `BufferId` remains the
resource/sync/Direct Scanout identity, first mapping is live, content does not
reorder the stack, popup content does not refresh popup topology, identical
XDG geometry is a no-op, and authoritative EGL Empty remains Empty.

## XWayland content/topology separation

For an already mapped, non-minimized XWayland surface with unchanged
placement, root ownership, stack membership, and visual geometry, the buffer
commit now replaces the renderable at its existing indexed position. It does
not retain the global vector, append the surface, reorder the stack, or run a
full root visual assignment merely because content arrived.

The path continues to publish conservative Full XWayland content damage;
this closure does not invent X11 partial-damage authority.

If placement, visual assignment inputs, minimize/restore state, attachment
topology, or restack/family inputs change, the existing topology path remains
available and is allowed to retain/reinsert, reorder, and perform the full
visual assignment.

The 1,000-commit unchanged-placement test proves:

- stack order is unchanged;
- 1,000 in-place XWayland content replacements occur;
- zero XWayland topology reorders occur;
- zero full visual-tree reassignments occur;
- zero global index rebuilds occur; and
- 1,000 indexed content lookups occur.

## Exact composited frame lineage

Normal atomic composition resolves `ResolvedNativeFrameScene`, validates its
scene signature, obtains `surface_ids()` from that final scene, and captures
the filtered primary token at that boundary. The token is stored on the
rendered output frame and follows the frame through submission, worker
queueing, and confirmed pageflip.

The initial atomic modeset uses its already-created `initial_resolved_scene`
for the same exact surface-ID capture. The normal compatibility composition
path captures from the resolved scene before storing the token on its prepared
frame.

The filtered capture path uses keyed generation/journal access and a
`HashSet` for duplicate suppression. It does not scan
`renderable_surfaces` or all client cursors. For K requested samples it is
O(K) average.

The final scene authority already includes active workspace selection,
fullscreen culling, popup expansion, and subsurface-tree membership. The
token therefore excludes inactive workspace surfaces and fullscreen-culled
rear surfaces and includes the exact popup/subsurface surfaces in the final
scene.

The state test with 1,000 global journals and six final scene IDs samples
exactly six primary entries and records zero global presentation scans.

## Software client-cursor lineage

Software cursor composition is part of the primary framebuffer. The capture
uses the render-time `client_cursor_render_state()` result and freezes its
surface ID and commit sequence into the same primary token. The renderer is
given that same resolved client cursor state.

The exact cursor commit test samples cursor commit 41, changes the current
cursor surface to commit 42 before settlement, and settles the old token. It
proves that commit 41 is presented while commit 42 remains pending and the
current damage remains Full.

When client cursor render state is absent because the cursor is hidden,
explicitly hidden, overridden, or a theme cursor is active, no client cursor
surface is added to the token.

## Bundled hardware cursor lineage

The hardware cursor path uses the existing frozen
`NativeCursorSourceKey::Client(NativeCursorImageKey)` identity. The native
cursor source key includes the exact surface, buffer, and commit sequence.
`FrozenCursorPlaneOwner.client_source_key` is carried with the rendered frame
and is consulted for exact hardware cursor sampling; pageflip does not query
current focus.

The atomic path adds the frozen hardware source surface and exact commit to
the primary token only when the frozen delivery is Hardware. The rendered
frame's `SurfaceDamagePresentation` remains the settlement authority even if
worker replan code drops physical-owner metadata.

The scanout regression constructs a non-`None` client source key at commit 100
and proves that it remains attached to the ready frame and to the worker
transfer. This closes the prior proof gap around a real client key rather
than only a theme/anonymous cursor owner.

## Cursor-only PlaneDelta lineage

The cursor-only worker path freezes the source key, captures an exact
surface/commit token, and passes it to `PlaneDelta` through
`OutputTransaction::with_surface_damage()`. The transaction has no primary
frame-batch obligation. Its matching cursor pageflip settles that token
independently of a primary frame.

The transaction tests prove:

- a cursor-only PlaneDelta stores its exact token;
- the descriptor has no primary frame batch owner;
- a superseded cursor-only transaction no longer exposes its old token; and
- the replacement transaction retains only its own token.

Theme cursors have `NativeCursorSourceKey::Theme` and add no client surface
sample. Hidden cursors likewise add no sample.

## Direct Scanout preservation

Direct Scanout continues to capture the candidate surface ID only, plus an
exact client cursor source key only when that same transaction presents one.
It does not use global capture. The existing distinction remains intact:
client buffer identity can change for Direct Scanout even when composited
logical damage is Empty, because Direct Scanout selects a new framebuffer
resource rather than repainting a composited logical surface.

## Presentation settlement

Settlement iterates only token entries. For each entry it:

1. checks the current presentation generation;
2. looks up the keyed journal and sampled commit counter;
3. advances only that surface's presented counter monotonically;
4. updates a normal renderable through the global `SurfaceId -> index` map or
   a client cursor through `client_cursor_surfaces.get_mut(surface_id)`; and
5. preserves `HistoryLost -> Full`.

This is O(K) average for K sampled entries, independent of unrelated global
surface count. Destroyed surfaces and reused numeric IDs are rejected by the
generation check. A newer commit after render is not consumed by an older
token because capture stores the sampled commit counter and settlement is
monotonic.

Failed render preparation, failed render, failed KMS submission, skipped
frames, and superseded transactions drop or terminalize ownership without
calling presentation settlement. Only the matching confirmed pageflip
settles the frozen token.

## Deterministic operation-count evidence

The bounded locality counters record indexed lookups, index rebuilds,
sampled entries, journal lookups, settlement entries, global presentation
scans, XWayland content replacements, topology reorders, full visual
reassignments, and hardware/software cursor samples.

Observed test evidence:

| Scenario | Population | Sample/commit work | Global scans/rebuilds |
| --- | ---: | ---: | ---: |
| Wayland content commits | 1 target + large unrelated set | 1,001 indexed content lookups | 0 rebuilds |
| Wayland damage-only commits | 1 target + large unrelated set | 1,001 indexed content lookups | 0 rebuilds |
| XWayland content commits | 1 target + 1,000 unrelated surfaces | 1,000 in-place replacements/lookups | 0 reorders, 0 visual reassignments, 0 rebuilds |
| Presentation capture and settlement | 1,000 journals, 4 sampled IDs | 4 capture entries, 4 capture journal lookups, 4 settlement entries, 4 settlement journal lookups | 0 global scans |
| Final scene capture | 1,000 global surfaces, 6 final IDs | exactly 6 primary samples | 0 global scans |

The legacy `capture_surface_damage_presentation()` wrapper remains for the
compatibility/test `mark_render_damage_presented()` API. It is explicitly
instrumented as a global scan and is not called by normal native frame,
Direct Scanout, initial modeset, cursor-only, or pageflip production paths.

## Integrated client/output swapchain oracle

`src/native_output/tests/integrated_swapchain_oracle.rs` now models:

- `ClientCommit`: commit sequence, client buffer ID, logical image, and
  logical damage;
- `OutputSlot`: physical pixels, last confirmed presentation serial, and
  Available/Quarantined state;
- `RenderedCandidate`: the slot's physical pixels after render, sampled
  damage, client commit, and buffer age;
- `SubmittedCandidate`: a transaction identity owning a rendered candidate;
- `PresentedState`: the confirmed slot, serial, full logical reference image,
  and presented surface commit; and
- bounded presented output-damage history.

Rendering mutates the output slot's physical pixels before submission. A
rejection therefore leaves an unpresented physical candidate; it does not
advance presentation serial/history or the presented client commit. The
quarantine path marks the slot unavailable. The retry path retains and later
resubmits the exact rendered candidate without pretending that rejection
reverted the slot.

The oracle also has an independent `full_reference_image_through()` lookup.
Every final presented result in the rotation/rejection cases is compared to
that reference, not only to the output slot's own computed pixels.

Covered cases:

- client buffer rotation `A -> B -> C -> A` and output slot rotation
  `0 -> 1 -> 2 -> 0`;
- true authoritative Empty commits where BufferId changes but the logical
  image and logical damage remain unchanged;
- real partial changes interleaved with Empty commits;
- output ages 1, 2, and 3+ (plus age 0 for initial paint);
- rejected rendered candidates that quarantine a slot;
- rejected candidates retried using the exact rendered pixels;
- old rejected frames leaving newer client commits pending;
- old successfully presented frames settling only their frozen client commit;
- Direct Scanout buffer identity remaining separate from composited logical
  damage; and
- final output equality against the independent full-reference image.

The six oracle tests pass.

## Focused tests

Passed focused groups and tests include:

- `rtk cargo test --lib compositor::tests::surface_frames -- --nocapture`:
  46 passed;
- `rtk cargo test --lib compositor::state::surfaces::ordered_publication_tests -- --nocapture`:
  15 passed;
- `rtk cargo test --lib compositor::state::task_05_8_tests -- --nocapture`:
  24 passed;
- `TMPDIR=/tmp rtk cargo test --lib xwayland -- --nocapture`:
  408 passed, 1 ignored;
- compositor same-buffer SHM cursor identity test: passed;
- hardware client-cursor replacement test: passed;
- `rtk cargo test --bin oblivion-one native_output::tests::presentation_transactions -- --nocapture`:
  57 passed;
- `rtk cargo test --bin oblivion-one native_output::tests::scanout -- --nocapture`:
  66 passed;
- `rtk cargo test --bin oblivion-one native_output::tests::integrated_swapchain_oracle -- --nocapture`:
  6 passed; and
- existing cursor, presentation, output-damage, plane-scheduling, Direct
  Scanout, and Surface Commit Locality v1 regressions remain green in the
  full suite.

## Verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked
TMPDIR=/tmp rtk cargo test --locked
rtk git diff --check
```

Final full-suite result:

```text
cargo test: 3028 passed, 5 ignored, 40 filtered out (30 suites, 44.46s)
```

`rtk cargo check --locked` completed with 0 errors and 7 existing dead-code
warnings in native input/runtime/debug paths.

Failed verification commands and classification:

### Clippy

```text
rtk cargo clippy --locked --all-targets --all-features -- -D warnings
```

Result: 22 errors and 1 warning. The diagnostics are outside the v2.1
production change and are existing dirty-checkout issues:

- `needless_return`: `src/compositor/protocols/workspace.rs:77`;
- `derivable_impls`: `src/compositor/surface.rs:155` and
  `src/wm/layout/constraints.rs:19`;
- `mutable_key_type`: `src/compositor/workspace_protocol.rs:154`;
- `obfuscated_if_else`: `src/compositor/state/fullscreen.rs:523`;
- `question_mark`: five locations in `src/compositor/state/tiled_layout.rs`
  at lines 367, 385, 394, 414, and 674;
- `if_same_then_else`: `src/compositor/state/tiled_layout.rs:910`;
- `too_many_arguments`: `src/compositor/state/tiled_resize.rs:59`;
- `collapsible_if`: `src/compositor/state/tiled_resize.rs:237` and `:250`,
  `src/compositor/state/xwayland_mode.rs:45`, and two additional existing
  locations reported by the command;
- `nonminimal_bool`: `src/compositor/state/tiled_resize.rs:310`;
- `collapsible_if`: `src/native/adaptive_buffering.rs:589`;
- `useless_conversion`: `src/compositor/state/window_interaction_tests.rs:440`;
- `needless_update`: `src/wm/layout/constraints.rs:721` and `:748`; and
- `option_map_unit_fn`: `src/wm/layout/solve.rs:995`.

The complete command output is retained by rtk at
`/home/agony/.local/share/rtk/tee/1787858592_cargo-clippy.log`, with the
diagnostic-only log at
`/home/agony/.local/share/rtk/tee/1787858592_cargo-clippy-errors.log`.
No task-owned v2.1 lint failure was observed, and unrelated code was not
modified to manufacture a green result.

### Source layout

```text
rtk run "bin/check-source-layout"
```

Result: failure due to these current line-count limits:

```text
src/compositor/tests/support/frame_buffer_client.rs: 2119 > 2000
src/compositor/tests/windows.rs: 2087 > 2000
src/compositor/state/desktop_windows.rs: 1516 > 1500
src/compositor/state/window_interaction_tests.rs: 2104 > 2000
src/compositor/state/window_interaction.rs: 1606 > 1500
src/compositor/state/windows.rs: 1696 > 1500
src/compositor/state/surfaces.rs: 1787 > 1500
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

This is dirty-checkout layout debt. The test-support file is task-touched and
grew to 2,119 lines because of the deterministic SHM cursor fixture; it was
not split as unrelated source-layout cleanup. The other reported files are
existing unrelated or earlier dirty changes. `rtk git diff --check` passed.

## Adversarial review pass 1: cursor identity and ownership

The following cases were manually traced against the source and existing or
new deterministic tests:

- initial cursor attachment creates the initial buffer and commit identity;
- same-buffer SHM updates advance commit identity without changing BufferId;
- multiple unpresented cursor commits accumulate journal damage;
- software cursor capture freezes the exact render-time cursor commit;
- hardware cursor source keys freeze surface, buffer, and commit sequence;
- bundled hardware cursor source ownership transfers through the ready frame;
- cursor-only PlaneDelta owns a token without a primary batch;
- a superseded cursor-only transaction drops the old active token;
- theme and hidden cursors add no client sample;
- client/theme transitions are selected from the frozen delivery/source plan;
- destruction before pageflip is safe through generation/keyed lookups;
- numeric SurfaceId reuse cannot accept an old generation;
- a newer cursor commit after an older transaction is frozen remains pending;
- render and submission failure paths do not call settlement;
- worker queue/replan keeps the surface token on the rendered frame; and
- Direct Scanout uses its candidate key rather than global cursor focus.

The review found one missing proof, not a production ownership defect: the
existing scanout tests checked frozen cursor pins but only supplied
`client_source_key: None`. The new non-`None` source-key transfer test was
added and passed. No further task-owned ownership issue remains in the
reviewed paths.

## Adversarial review pass 2: locality and accidental scans

The final source searches covered:

```text
renderable_surfaces.iter().find(...)
renderable_surfaces.iter_mut().find(...)
renderable_surfaces.iter().any(...)
renderable_surfaces.retain(...)
renderable_surfaces.drain(...)
```

Remaining retain/drain uses are real cold/topology paths:

- `scene_order.rs`: committed-stack reconstruction;
- `windows.rs`: minimize/restore and window raise;
- `subsurfaces.rs`: surface-tree raise/reorder; and
- `surfaces.rs`: index maintenance itself and debug invariant traversal.

Remaining vector traversals in geometry, hit testing, output membership, and
visual tree refresh are not ordinary content or presentation settlement
paths. The tree refresh intentionally operates on an entire affected tree
when visual topology/geometry changes.

All production capture and settlement call sites were checked. Native normal
composition, initial modeset, Direct Scanout, and cursor-only worker paths use
filtered or exact-commit capture. Pageflip paths settle only the stored
transaction/frame token. The only global capture call is the legacy
compatibility/test `mark_render_damage_presented()` route, which is explicitly
instrumented and absent from normal native frame production paths.

No accidental O(N) scan remains in ordinary Wayland content, damage-only
content, unchanged mapped XWayland content, incremental ActiveScene
publication, presentation capture, pageflip settlement, or cursor-token
ownership.

## Remaining NativeOutput Empty/repair question

The conservative NativeOutput behavior equivalent to

```text
content identity changed + current logical damage Empty
    -> repaint current surface footprint
```

was intentionally left unchanged. The oracle now supplies the missing
rendered-slot-versus-presented-state evidence needed for the next closure,
but this v2.1 task does not introduce output-damage authority separating
authoritative client Empty from repair after an unpresented output attempt.
Rejected-output conservative repair tests remain green.

## Hardware qualification and claims

No real TTY/DRM/KMS/165 Hz qualification was executed. No native hardware
measurement was made. This report does not claim to fix the observed 30 FPS
symptom, improve latency, or establish CPU/GPU/refresh-rate performance.
