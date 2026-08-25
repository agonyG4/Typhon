# Typhon Resource Efficiency v1

## 1. Baseline HEAD

The public investigation baseline was `9d3fb34` (`style(native): format O1 closure code`). The approved design and implementation plan were already present at `37dddd4` when implementation began. Task commits from this work are `b53296f`, `b42808f`, `83ff2c8`, `3c2bb37`, `5b839c1`, `970ee8d`, `bc8e79e`, `8269c9a`, and `8158482`.

## 2. Initial dirty-tree boundary

The checkout was intentionally dirty. It contained Workspace Runtime, ActiveScene/SceneWorkIndex, Special Workspace, Dwindle, resize/coalescing, O1, cursor, and report/design changes, including deleted older reports and untracked newer reports/plans. The checkout was treated as authoritative. No reset, stash, clean, worktree, or unrelated restoration was performed.

## 3. Source findings revalidated

The source review confirmed the investigation findings: pointer diagnostics had eager formatting at some call sites, libinput device identity was not needed for ordinary motion, input batching needed retained ownership, XWayland state was generation-stable, and native-cycle orchestration could perform unrelated work around input dispatch. Exact-coordinate pointer caching already had scene and pointer-hit generations; it was not upgraded to an unsafe spatial shortcut.

## 4. Findings fixed before this task

The dirty checkout already contained substantial ActiveScene/SceneWorkIndex, Dwindle, Special Workspace, resize, cursor, and O1 changes. Those changes were preserved. The resource-efficiency series added its own metrics, pointer-debug, pointer-identity, retained-batch, relative-recipient, XWayland, and runtime-gating commits.

## 5. Phase 0 metric architecture

`ResourceEfficiencyMetrics` owns bounded integer counters for native cycles, input, raw/coalesced events, pointer samples, primary work, cursor-only work, protocol/pure-input completions, hit-test evidence, XWayland synchronization, pacing, acquire/prepare, and presentation planning. The existing control snapshot projects these counters without per-event logging, formatting, serialization, file I/O, or environment lookup.

## 6. Pointer-debug zero-cost-disabled closure

Pointer-debug enablement is cached with `OnceLock`, and lazy diagnostics use a closure seam. The deterministic test `disabled_lazy_logging_does_not_evaluate_formatter` proves the disabled branch does not invoke its formatter. Native pointer diagnostics use the lazy path. No runtime benchmark was available to measure allocation rate directly.

## 7. Libinput device-identity allocation closure

Per-device wheel/v120 state uses a borrowed/static device-key representation at the conversion boundary. Ordinary pointer motion does not need to own a sysname string. Focused input tests passed after the conversion changes.

## 8. Input batch and scratch ownership

`NativeInputBatch` retains raw and coalesced vectors across cycles and coalesces in place. Its capacity-retention test passes. A later cleanup removed an unused batch-wide `clear` wrapper while retaining direct vector clearing at the ingestion boundary.

## 9. XWayland app-environment design

`XwaylandAppEnvironment` is materialized once per stable service generation and borrowed through normal launch paths. An external shell launch clones only at the ownership boundary. The service metric records environment materialization; no stable pointer cycle reconstructs the environment.

## 10. XWayland reactor-generation design

XWayland registration state carries a service generation. The generation-aware synchronization path returns early before building desired registrations when the generation is unchanged. Registration, restart, deadline, command, and shutdown transitions advance the generation. Service metrics distinguish sync requests, actual reconciliations, and unchanged skips.

## 11. Phase 1 before/after counter evidence

No native compositor workload was executed in this environment, so there are no valid before/after rates for CPU, cycles/s, renders/s, or XWayland reconciliations/s. The code now exposes the counters needed for a matched release-build run; this report does not substitute static counter presence for measured improvement.

## 12. Pointer locality design

The existing exact-coordinate hit cache remains keyed by coordinates, scene render generation, and pointer-hit generation. A same-coordinate hit can reuse the exact target; changed coordinates fall back to the authoritative scene hit path. No containment shortcut or duplicated input-region authority was introduced.

## 13. Pointer locality invalidation rules

Pointer-hit generation advances at the existing active-scene and surface/subsurface invalidation boundaries. Cache metrics now count full scene scans, locality fast hits, and pointer-hit generation invalidations. The current implementation reports zero locality fast hits because no safe containment proof was established. Existing decoration, popup, workspace, scene, and pointer-generation tests passed.

## 14. Runtime work-domain/quiescence architecture

`NativeWorkDomains` classifies `NativeWakeup` plus compact runtime state into independent input, Wayland dispatch, scene, cursor, presentation, explicit-sync, surface-pacing, XWayland, control, child, session, and shutdown domains. Work classes remain `NoOutputWork`, `ProtocolOnly`, `CursorOnly`, and `PrimaryScene`.

## 15. Pure-input completion proof

The classifier test `input_with_stable_hardware_cursor_is_a_pure_input_fast_path` selects `NoOutputWork`. `NativeCycleState` records the selected class and whether prepare/presentation were skipped. The runtime gates dispatch and returns to the reactor when no output work is selected. This is deterministic unit evidence; no native TTY execution was performed.

## 16. Surface-pacing quiescence proof

`OwnCompositorServer::has_surface_pacing_work` reports active FIFO barriers, pending surface-tree transactions, or pending commit timing. `NativeRuntime::should_progress_surface_pacing` progresses only for active pacing state or a due pacing deadline. The classifier test covers input plus a due timer/pacing deadline.

## 17. Explicit-sync/acquire quiescence proof

Explicit-sync readiness, ready tokens, and pending compositor explicit-sync work select the explicit-sync domain. Acquire/prepare is no longer implied by input or unrelated protocol dispatch; it requires scene, cursor, explicit-sync, or recovery work. The classifier test covers input plus explicit-sync readiness.

## 18. Primary-scene/Dwindle quiescence proof

Primary work is selected from semantic scene dirtiness, queued redraw, pending visible frame preparation, or recovery state. Stable input does not select primary work. Focused frame, tiled-layout, tiled-resize, and Special Workspace suites passed. No claim is made that Dwindle is globally idle under a native workload because no native workload was run.

## 19. Cursor-only scheduling behavior

Cursor I/O readiness, cursor completions, and due cursor arbitration are separate from primary scene work. Cursor-only work has a distinct class and counter. Primary work retains priority when both are present. Existing cursor/frame tests passed.

## 20. Software-cursor behavior

The existing software-cursor planner and old/new damage contracts were preserved. The focused `frame` suite passed. Forced software-cursor execution on a real output was not available, so this report makes no hardware or frame-rate claim.

## 21. Relative-pointer and pointer-lock hot-path changes

Locked relative recipients are cached by resource generation, active constraint generation, surface identity, and source pointer identity. Resource add/remove/death/client-cleanup paths advance the resource generation. Delivery preserves timestamps, accelerated and unaccelerated deltas, recipient filtering, order, and one frame per source pointer. Existing locked-relative and relative-pointer suites passed; no 8,000-sample native benchmark was run.

## 22. High-rate synthetic test results

The deterministic classifier, retained-batch, hit-test, locked-relative, relative-pointer, frame, and runtime suites passed. The requested 500/1,000/2,000/4,000/8,000 sample end-to-end stream matrix and high-poll native run were not executed.

## 23. Stable-frame allocation audit

Static review verified retained input storage, borrowed stable XWayland environment state, generation-gated reactor synchronization, and runtime gates. No new unbounded task-owned history was added. A native allocation profiler was not available in the executed validation.

## 24. Memory/PSS/cache findings

The retained input batch and relative-recipient tests cover capacity reuse/invalidation. No `/proc` PSS/smaps rollup collection or long-duration native memory run was executed. Monotonic memory stability therefore remains unqualified by measurement.

## 25. XWayland off/eager evidence

The focused XWayland service suite passed 69 tests with one ignored test. The broader XWayland selection passed 404 tests and exposed three pre-existing trace-test failures: one retention expectation mismatch followed by mutex-poison cascades. The XWayland reactor-focused suite passed 2 tests. No eager/off native A/B run was executed.

## 26. O1/KMS-worker A/B evidence

No matched O1-off/on or KMS-worker-off/on native output run was executed. Existing unit/runtime coverage was retained and passed where selected.

## 27. Real CPU/perf evidence

No real CPU, `perf stat`, `perf record`, context-switch, 165 Hz, high-poll, TTY, DRM, or KMS qualification was executed.

## 28. Hyprland comparison

No matched Hyprland comparison was executed. The reported order-of-magnitude symptom remains an investigation premise, not a benchmark result in this checkout.

## 29. Exact commands used

Commands included `rtk cargo fmt --check`, `rtk cargo check --locked`, focused `TMPDIR=/tmp rtk cargo test --locked ... -- --nocapture` suites, `rtk git diff --check`, `rtk git diff --cached --check`, `rtk git status --short`, `rtk git log`, and `rtk git commit`. The full validation commands and their final results are recorded below.

## 30. Focused tests run

Passing focused results included: work domains 7; native runtime 211; pointer-scene 5; pointer constraints 15; locked-relative 17; relative-pointer 6; frame 230; tiled layout 8; tiled resize 3; and Special Workspace 6.

## 31. Full validation results

`rtk cargo fmt --check`, `rtk cargo check --locked`, and `TMPDIR=/tmp rtk cargo test --locked` passed. The full test result was 2,936 passed, 5 ignored, and 40 filtered. Full clippy with `-D warnings` failed with 21 errors and 1 warning in pre-existing workspace/Dwindle/Special Workspace code. `rtk git diff --check` passed. The report was updated after these commands.

## 32. Source-layout result

`rtk run "bin/check-source-layout"` ran and failed on pre-existing line-count limits in compositor, Dwindle, native bootstrap/input routing, XWayland, and related files. No source-layout files were changed for this task.

## 33. Pre-existing/environment failures

The broad XWayland run’s three trace failures were pre-existing relative to this slice and independent of the resource-efficiency changes. The full clippy run is known to encounter unrelated dirty Dwindle/Special Workspace lint failures. Tests without `TMPDIR=/tmp` can hit Unix socket path-length failures under the desktop temporary directory; focused XWayland runs used `TMPDIR=/tmp`.

## 34. Review Pass 1 — correctness/ownership/protocol safety

The review checked explicit-sync and pacing gating, XWayland generation gating, retained batch ownership, pointer-hit generation invalidation, relative-recipient cache keys, cursor/planner preservation, and focused protocol tests. It found one task-owned issue: input was still included in the XWayland scene-batch condition. Commit `8269c9a` narrowed that condition to Wayland protocol or XWayland readiness, so stable input no longer enters that scene path.

## 35. Review Pass 2 — hot-path/165 Hz/high-poll efficiency

The static review checked task-owned paths for eager pointer diagnostics, transient input batches, stable XWayland environment construction, unchanged reactor reconciliation, pacing, acquire/prepare, and relative-recipient rebuilding. Retained vectors and generation keys are present. It also removed one task-owned needless borrow in the relative-recipient fast path (`8158482`). The review did not produce real allocation or CPU measurements; 165 Hz/high-poll qualification remains open.

## 36. Review Pass 3 — root-cause challenge

Source-proven waste was closed in the task-owned paths and the broad cycle now has explicit gates. The available evidence does not establish the remaining CPU gap, hit-test dominance, cursor submission rate, memory behavior, or latency impact under native execution. Those questions require the missing matched runtime profile rather than inference from unit tests.

## 37. Final git status

Task commits were created without staging the unrelated dirty checkout. At report time, the remaining dirty paths were the pre-existing compositor/WM/Dwindle/Special Workspace/report changes. The final handoff must rerun `rtk git status --short --branch` after the report commit and preserve that boundary.

## 38. Remaining measured bottlenecks

No native bottleneck was measured in this environment. The primary unqualified candidates are full-scene hit testing on changed coordinates, protocol/input dispatch cost, and output-policy cost when a semantic visual change exists. The implementation intentionally did not invent a spatial hit-test cache without a correctness proof.

## 39. Animation Engine recommendation

Animation Engine should remain blocked by: native runtime counters and 165 Hz/high-poll profiling were not executed in this environment. The code has the required domain separation for future presentation ticks, but hardware/runtime evidence is insufficient to declare the foundation qualified.
