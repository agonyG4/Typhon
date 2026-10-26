# Ren'Py DMA-BUF feedback investigation

## Scope and evidence

LoSeSb 26.8.0d uses Ren'Py 8.5.3 and NVIDIA egl-wayland2 1.0.2 on this host. A native Wayland run reached the main window after approximately two minutes of initialization and submitted thousands of buffers without EGL_BAD_ALLOC. This does not reproduce the ticket's failure. The unmodified launcher is being tested separately.

A small fullscreen project using the same Ren'Py runtime produced 1641 scanout advertisements and 3285 render-tranche events for 256 attached buffers during a 32-second trace. The user was switching workspaces during that observation, so those totals alone cannot establish a bug. A second trace still contained 52 scanout advertisements between 17:27:33.300 and 17:27:39.999 UTC, after keyboard focus entered at 17:27:33.279 with no subsequent keyboard leave in the trace. Feedback repeatedly alternated between scanout plus renderer and renderer only. Source inspection shows that native scanout eligibility includes PendingOrUnpublishedWork, so ordinary pending commits can revoke and immediately restore a surface's allocation preference. The existing allocation floor is reset on withdrawal. The regression tests reproduce withdrawal from pending work without any workspace transition.

The unmodified LoSeSb.sh launcher also reached its main window without recorded Flip failed or EGL_BAD_ALLOC errors. The user confirmed that it opened normally in this run. The original ticket failure remains intermittent and its cause is not established.

The NVIDIA EGL import query marks LINEAR external_only for XR24, AR24, XB24 and AB24. Do not add unsupported LINEAR pairs or remove compressed modifiers without allocation/import evidence.

## Design

Keep allocation feedback separate from immediate presentation readiness. A scene that passes every direct-scanout check except PendingOrUnpublishedWork retains its covering source as its feedback candidate. Presentation still requires the original fully validated candidate. Actual blockers such as overlays, resize, unsupported geometry, nonopaque content and missing buffers continue to withdraw the hint. Existing renderer fallback and explicit teardown remain unchanged.

## Implementation and verification plan

1. Add regression coverage for pending-only feedback stability, actual blockers and missing coverage. Confirm the pending-only case fails with current ready-candidate semantics.
2. Expose the feedback candidate on DirectScanoutSceneAnalysis and use it only for feedback reconciliation in the native presentation inspection.
3. Run focused compositor and native presentation tests using CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target and TMPDIR under that directory. Build the release binary in the same stable target tree.
4. Repeat the original launcher and fullscreen probe, record errors and feedback counts, and distinguish hardware validation from unit-test results. Commit only the files changed for this fix on main.

## Verification results

- The regression run with ready-candidate-only semantics failed the pending-frame selection and hint/allocation-preservation cases. After the change, all five feedback tests pass, including rejection during WorkspacePresentationTransition.
- All 11 compositor direct-scanout protocol tests and all four native presentation-direct tests pass. Formatting and whitespace checks pass.
- The broader `cargo test --lib direct_scanout` selection has 54 passes and one failure, reproduced with a single test thread: `compositor::state::window_exit_physical_tests::direct_scanout_pageflip_evidence_flows_through_xdg_close_render_settlement_and_release`, at line 767, compares `SceneNodeId(1)` with `SceneNodeId(2)`. The failing test does not invoke the new feedback method. That failure is outside this patch; the broader selection is not clean.
- The release binary builds successfully at `/mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one` and its help command executes successfully. This build includes the other uncommitted changes already in the working tree; they are not part of this patch's commit.
- The live compositor still runs the previous binary. Post-change hardware feedback counts require starting the new binary and are not claimed here. The original game's intermittent startup failure has not been reproduced or proven fixed.

Captures and command logs are under `/mnt/Aether/Desktop/GitHub/Typhon-target/tmp`, including `renpy-feedback-final.log`, `renpy-direct-protocol-tests.log`, `renpy-native-presentation-tests.log`, `renpy-release-build.log`, and `renpy-egl-fullscreen-stationary.log`.
