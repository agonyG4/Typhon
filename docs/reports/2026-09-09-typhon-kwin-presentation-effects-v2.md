# Typhon KWin-Inspired Presentation Effects v2

Date: 2026-09-09

## Scope and hardware symptoms

This corrective pass responds to real 165 Hz hardware feedback. The observed
symptoms were:

- maximize, fullscreen, and layout transitions could begin at the output
  origin instead of the window's actual pre-transition rectangle;
- starting a titlebar drag while a size-changing transition was active could
  rebuild the SSD relationship from a scaled intermediate size; and
- the existing macOS-style spring geometry transitions felt too slow for
  desktop window management.

The implementation keeps the accepted Typhon presentation foundation intact:
absolute presentation sampling, transition identity, pageflip-confirmed
settlement, native-frame membership, fullscreen coverage, Direct Scanout
blockers, resource ownership, and tiled-resize ratio authority remain in
place.

## Root causes

The source rectangle was sometimes discovered by an installer after canonical
placement had already been changed. That made the target origin, commonly
`(0, 0)`, available as a false source or allowed a source rectangle to mix a
new origin with an old size. Tiled reflow and managed XWayland mode changes had
the same authority error in their respective paths.

Interaction takeover had a different cause. It converted the physically
presented rectangle into canonical placement and size. A presentation scale
therefore became a logical window size, after which normal decoration layout
recomputed the titlebar from theme metrics in a different coordinate system.

Maximized move was also semantically incomplete: arbitrary pointer placement
could leave the logical mode maximized. Maximized resize was unsupported and
must be rejected rather than treated as floating geometry.

## KWin study and Typhon adaptation

The referenced KWin areas were studied for behavior and architecture:
`src/scene/windowitem.{h,cpp}`, `src/effect/animationeffect.cpp`,
`src/plugins/maximize/package/contents/code/main.js`,
`src/plugins/scale/package/contents/code/main.js`,
`src/plugins/glide/glide.cpp`, `src/plugins/squash/package/contents/code/main.js`,
and `src/window.cpp`. The directly observed design lessons were:

- a `WindowItem`-like visual owner groups client content, decoration, and
  shadow;
- effects operate on that visual representation instead of rewriting logical
  window state with effect intermediates;
- maximize captures the old geometry before logical geometry changes;
- current maximize geometry uses approximately 250 ms `OutCubic`; and
- an unexpected geometry change during the maximize effect cancels the effect
  instead of committing its intermediate transform as logical geometry.

Reference source: [KWin maximize effect](https://raw.githubusercontent.com/KDE/kwin/master/src/plugins/maximize/package/contents/code/main.js),
[KWin WindowItem interface](https://github.com/KDE/kwin/blob/master/src/scene/windowitem.h),
and [KWin animation effect base](https://raw.githubusercontent.com/KDE/kwin/master/src/effect/animationeffect.cpp).

Typhon adapts those behaviors through its existing
`WindowVisualGroup`, `PresentationGroupTransform`, and
`DecorationRenderInstance::with_presentation_transform` paths. It does not
introduce a KWin `WindowItem` object graph, scene graph, or copied GPL
implementation code. Previous-content crossfade, Scale, Glide, Squash,
open/close/minimize effects, and workspace effects remain future work.

The 160 ms and 200 ms Typhon geometry values below are Typhon-specific
adaptations inspired by the KWin timing family. They are not claims about
private or exact KWin behavior. The 250 ms maximize family is aligned with the
directly observed current KWin maximize timing.

## Explicit source architecture

Animated installation now accepts an explicit `VisualGeometryTransition`:

```text
Immediate
Animated { source: pre_mutation_geometry, kind }
```

The XDG, tiled, and managed XWayland paths capture source geometry before
mutating mode, frame, configure, or canonical placement. The installer only
consumes the supplied source and never queries mutable compositor state to
reconstruct animation history after target installation.

The distinction between restore bookkeeping and animation source is retained.
For example, maximizing stores the current maximized source separately from a
normal restore geometry. Restoring captures the current mode geometry first,
then resolves the stored normal target. `WindowState` exposes the restore
geometry read-only for interaction planning and only consumes it when the
restore operation succeeds.

Tiled reflow passes the `current` geometry already captured before the Dwindle
target is installed. XWayland uses the pre-mutation visual/frame geometry and
uses the same semantic `MaximizeEnter`, `MaximizeExit`, `FullscreenEnter`, and
`FullscreenExit` kinds as XDG. The backend-only `XwaylandModeChange` kind was
removed because it no longer had a remaining caller.

## Interaction interruption semantics

The former generic geometry takeover is now the narrower
`rebase_interaction_to_presented_origin` operation. It cancels presentation,
may preserve the physically presented origin, and retains the canonical
width and height. It cannot install the physically presented scaled size as
canonical `ToplevelVisualGeometry`.

Normal floating move and resize therefore start from one coherent canonical
size. Translation-only interruption preserves the visible origin. A
size-changing interruption may snap size to the canonical target, but the
window, client, SSD, titlebar metrics, and buttons stay in one coordinate
system. Once interaction owns the window, no KWin or macOS presentation curve
continues to compete with pointer geometry.

Tiled resize keeps its existing physical client-edge boundary rebase. It
cancels the old presentation transition without calling the floating
takeover path; the Dwindle tree and split ratio remain canonical authority.

Maximized move uses explicit restore-then-move semantics. It reads the latest
physical rectangle and pointer anchor, resolves the stored normal geometry,
cancels the maximize transition, changes mode to `Normal`, positions the
restore geometry under the pointer using the existing placement coordinate
model, sends the backend update, installs it immediately, and begins direct
Move. Maximized resize and all fullscreen move/resize interactions are
rejected.

The visual group remains shared. Root client content, ordinary subsurfaces,
SSD titlebar, and SSD buttons continue to receive one
`PresentationGroupTransform`; no independent titlebar animation track was
added.

## KDE geometry policy

`PresentationAnimationStyle::Kde` is the default. Environment selection is:

```text
OBLIVION_ONE_ANIMATION_STYLE unset, empty, default, or kde -> Kde
OBLIVION_ONE_ANIMATION_STYLE=macos                       -> Macos
unknown values                                             -> Kde
```

The global `OBLIVION_ONE_ANIMATIONS=on|off` switch remains unchanged.

| Geometry kind | Duration | Curve | Classification |
| --- | ---: | --- | --- |
| ProgrammaticMove | 160 ms | EaseOutCubic | Typhon adaptation |
| ProgrammaticResize | 200 ms | EaseOutCubic | Typhon adaptation |
| LayoutReflow | 200 ms | EaseOutCubic | Typhon adaptation |
| MaximizeEnter | 250 ms | EaseOutCubic | aligned with observed KWin maximize timing |
| MaximizeExit | 250 ms | EaseOutCubic | aligned timing family |
| FullscreenEnter | 250 ms | EaseOutCubic | Typhon adaptation |
| FullscreenExit | 250 ms | EaseOutCubic | Typhon adaptation |

The cubic curves are analytic, not lookup-table based. Their midpoint,
endpoint derivative, and monotonicity tests are deterministic. Fixed-duration
transitions still render the exact target and wait for the existing physical
pageflip and `TransitionId` acknowledgement before retirement.

## RED/GREEN coverage

The regression matrix includes real compositor entry paths for:

- XDG maximize, fullscreen, and restore source authority, including the
  non-zero-origin `(640, 320)` top-left regression;
- Dwindle reflow source authority;
- managed XWayland mode source authority and semantic curve parity;
- shared client/subsurface/SSD/button visual-group transformation and identity
  geometry;
- floating presentation interruption without canonical scaled size;
- a decorated midpoint maximize followed by transformed titlebar Move;
- maximized titlebar restore-under-pointer with mode transition to `Normal`;
- tiled split-handle rebasing without replacing canonical geometry;
- maximized resize rejection and fullscreen interaction rejection;
- analytic cubic values, derivatives, and monotonic progression; and
- default KDE, explicit KDE, and explicit macOS policy selection and timing.

The old macOS spring policy remains covered by its critical-damping and
settlement tests for comparison.

The source-authority regressions were first run against the defect and were
intentionally RED: fullscreen and maximize sampled `(0, 0, 800, 600)` at the
transition start instead of `(640, 320, 800, 600)`, restore sampled the
post-target geometry, and managed XWayland sampled its fallback `(72, 72,
2, 2)` frame instead of `(100, 100, 640, 480)`. After the explicit source
was threaded through each mutation path, those same real-path tests were
GREEN. The tiled reflow, visual-group, floating-interruption, tiled-resize,
maximized-drag, maximized-resize, cubic-policy, and XWayland parity tests
also pass in the final suite.

## Verification

Focused RED/GREEN tests were run throughout the implementation using the
existing build directory. The final mandated verification results are:

- `rtk run -- cargo fmt --check` — passed.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — passed.
- `rtk run -- cargo test --locked` — an earlier complete run passed all
  presentation-related tests and 2,302 unit tests, with two ignored and the
  integration/doc-test targets passing. Two later complete attempts exposed
  unrelated timing-sensitive failures: one reported XDG popup descriptor
  growth (`rapid_xdg_popup_cycles_leave_no_stale_popup_state`), and another
  reported that the KMS worker did not reach its in-flight state
  (`completion_drain_is_one_shot_and_does_not_duplicate_settlement`). Each
  failed test passed when rerun in isolation. No presentation-effects test
  failed.
- `rtk git diff --check` — passed.
- `rtk run -- bash bin/check-source-layout` — failed on the repository's
  existing line-count policy violations. The task-touched files already over
  their limits at HEAD included `src/compositor/state/windows.rs` (1,757
  lines), `src/compositor/state/window_interaction.rs` (1,638),
  `src/compositor/state/desktop_windows.rs` (1,559),
  `src/compositor/state/window_interaction_tests.rs` (2,112), and
  `src/compositor/tests/windows.rs` (2,181). The unrelated renderer, native
  output, compositor, and test modules reported by the script remain
  preserved; no broad source-layout refactor was added for this task.

The final codebase-memory coverage check reported `no_recorded_issue` with
`metadata_match` for every operated source path. This is a best-effort index
signal and not a completeness proof.

The checkout contains unrelated renderer/Effects work in the shared history.
The one-line layer-shell command-barrier synchronization correction is kept
in the separate companion commit `2e39d88`; it does not alter presentation
architecture.

## Hardware qualification status

The known 1920x1080@165 Hz hardware environment was not available to this
coding session. The report records the supplied hardware symptoms, but makes
no claim of physical hardware qualification. The deterministic compositor
regressions and timing-policy tests are the evidence available here. Physical
qualification should repeat maximize, fullscreen, restore, layout reflow,
titlebar interruption, and representative XWayland transitions on the actual
165 Hz setup.

## v2.1 follow-up: interaction ownership closure

The v2.1 audit found that `restore_root_window_for_interaction` redundantly
called `cancel_presentation_for_root` after the immediate visual installer had
already cancelled the animator. That helper is destructive: it also removed
the last pageflip-confirmed `PresentedWindowGeometry` and any published
presentation transform. Interaction handoff now cancels only the animator and
keeps the physical ledger, including a non-identity transform, until the next
pageflip. The destructive helper remains reserved for root teardown.

The same audit found that tiled classification was computed before a
maximized Move restore. A `Maximized + Tiled` window could therefore become
`Normal + Tiled` while starting a floating Move. Interaction classification is
now reread after the mode handoff. Maximized tiled Move first prepares a
candidate Dwindle tree and surviving solution, then commits the detach and
changes membership to `Floating`; the dragged window is restored immediately,
while surviving tiled windows use the normal `LayoutReflow` policy. Preparation
failure leaves mode, membership, and the live tree unchanged. Normal tiled
Move remains rejected, maximized Resize remains rejected, and programmatic
unmaximize still follows the tiled restore path.

For a floating-managed maximized window, restore continues to use
`WindowState::restore_geometry()`. For a tiled-managed maximized window that
will detach, it prefers `DesktopWindow::floating_geometry`, falling back to
the mode restore geometry only when necessary. The existing physical-rectangle
horizontal ratio and titlebar vertical offset keep the restored window under
the pointer, with ordinary integer placement rounding. A successful maximize
handoff starts Move from that anchored target rather than rebasing it back to
the old physical origin.

The RED tests first observed the physical ledger changing from a promoted
frame to `(None, None)` before another pageflip, and observed the invalid
`Normal + Tiled + Move` ownership combination. GREEN coverage now exercises
the replacement publication path, floating and tiled XDG interaction, the
prepared-detach atomicity and single-leaf cases, and managed XWayland restore
and detach cases. No separate XWayland policy or scene/layout architecture was
introduced.

The focused closure checks passed on the current checkout: the maximized
floating ledger regression, the tiled-maximized detach/reflow/restore/anchor
regression, prepared-detach unit tests, and managed XWayland restore/detach
tests. The exact required verification commands were then run against the
shared checkout as follows:

- `rtk run -- cargo fmt --check` — failed only on unrelated, uncommitted
  blur-policy formatting changes; the closure files pass targeted rustfmt.
- `rtk run -- cargo check --locked --all-targets` — passed.
- `rtk run -- cargo clippy --locked --all-targets -- -D warnings` — blocked by
  unrelated blur-assignment test errors (`resolver()` calls in the shared
  uncommitted work).
- `rtk run -- cargo test --locked` — blocked by the same unrelated
  blur-assignment test errors; no closure test failed.
- `rtk git diff --check` — passed.
- `rtk run -- bash bin/check-source-layout` — retains the repository's
  existing source-layout debt; no broad refactor was added here.

Hardware qualification was not available in this session. The known
1920x1080@165 Hz maximize-drag and repeated-stress checks remain to be run on
the actual compositor hardware.
