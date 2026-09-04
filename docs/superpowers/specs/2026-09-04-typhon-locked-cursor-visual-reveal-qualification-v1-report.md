# Typhon Locked Cursor Visual Reveal Qualification v1 — Closure Report

## Result

Gate 1, causal observability, is implemented. The locked-pointer release path
now records one correlated chain:

```text
unlock
    -> reveal authority
    -> cursor source and surface geometry
    -> desired cursor revision
    -> plane plan
    -> frozen owner
    -> KMS/sidecar submission
    -> page-flip promotion
    -> first visible cursor presentation
```

The implementation deliberately does not claim a Gate 2 semantic correction.
The required live Sober hardware/software A/B qualification was not performed
in this checkout, so no speculative generation filter, delay, synthetic motion,
or Sober-specific branch was added.

Implementation commits:

- `96a4d78` — design;
- `3c34f76` — implementation plan;
- `ca6caba` — unlock/reveal lifecycle trace;
- `685f383` — cursor presentation ownership trace and deterministic tests.

The exact starting `HEAD` was:

```text
2c4bb7e6c908c10d6d19312b17902eaf7cc0f2af
```

The ending implementation `HEAD` before this report commit is:

```text
685f3839a4162de329f8189663c3ddf3de395511
```

The report commit is the final repository `HEAD` for this qualification
record.

## Symptom and accepted boundaries

The remaining symptom is the visual cursor teleport seen on right-button
locked-pointer release. Pointer motion, backlog, and activation-anchor
ownership were already corrected before this task.

The solved ~28 ms pointer-constraint region-resolution/input-backlog issue was
not reopened.

The solved activation-anchor release fallback was not reintroduced.

The new code keeps the input result authoritative and makes visibility a
presentation-state transition. It does not manufacture motion to hide a
presentation error. A first visible cursor result must correspond to the
latest authoritative reveal position and cursor visual state.

## KWin analysis

[KWin commit 8f0882b2](https://invent.kde.org/plasma/kwin/-/commit/8f0882b2e0af66c0a04f54b823bc4aed10b49da7)
(`pointer_input: rework pointer constraints`) addresses a different but
related ownership boundary. It makes constraint creation, destruction, lock
properties, regions, and hints follow Wayland surface commits so delayed
payloads cannot be attributed to the wrong constraint generation or geometry.
KWin 520910 validates commit-exact pointer constraint ownership but the current
Typhon symptom is after the correct boundary; the KWin fix is therefore
relevant evidence, not a fix to copy into the presentation path.

[KWin commit fd068d10](https://invent.kde.org/plasma/kwin/-/commit/fd068d10b6559ce94b77508b267e1370274be729)
(`compositor: paint, enable and disable the cursor only in composite()`) makes
cursor painting and cursor-plane enable/disable coherent at composite time,
while retaining asynchronous cursor movement so primary-plane work does not
add cursor latency. Its relevant lesson is that cursor visibility and cursor
visual ownership are presentation decisions; motion may remain asynchronous.

## Current Typhon ownership model

The existing `NativeAtomicCursor` architecture remains intact:

- `desired`, `submitted`, and `current` remain distinct state snapshots;
- `desired_epoch`, `submitted_epoch`, and the existing image/motion/visibility
  revision tracker remain authoritative;
- `CursorDeltaClass` continues to distinguish position-only, visual,
  visibility, and delivery-mode transitions;
- plane assignment, capability validation, framebuffer pinning, KMS worker
  ownership, frozen owners, sidecars, page-flip promotion, and
  `PresentedCursorState` remain the presentation machinery.

The new trace accessors expose submitted state, queued worker state, submitted
revision, and presented revision without creating another cursor transaction
architecture. The implementation extends and validates the existing
desired/submitted/current ownership; it does not replace it.

At unlock, `CursorRevealAuthority` records the stable pointer-constraint
identity (`constraint_id` plus `generation`), the accepted final output
position, and whether cursor visibility is requested. The pending lifecycle
records unlock begin, backend restoration settlement, an accepted client warp
with its origin, and reveal finalization. The authority remains available to
the presentation trace until the first newly visible presentation is promoted.

Client cursor source tracing records the selected client/theme source and, for
client cursors, the surface id, buffer id, surface commit sequence, logical
cursor coordinates, surface offset, hotspot, and resolved output coordinates.
The coordinate relation follows the existing path:

```text
pointer position - hotspot = ClientCursorRenderState.logical_x/y
logical_x/y + surface.x/y + hotspot = cursor-plane output position
```

The plane trace records the previous and next delivery mode, delta class,
policy decision, desired revision and position, and presented base. Freeze
trace records the frozen revision, assignment, owner source/capability, and
framebuffer pin. Sidecar replacement/selection trace records transaction,
token, revision, delivery, and assignment. The KMS submit trace records the
exact enable/disable payload, framebuffer, CRTC property, source rectangle,
destination rectangle, hotspot, delivery, and submission kind. Page-flip
promotion records the presented revision and geometry, followed by a
single-slot `first_visible_cursor_presentation` event that compares the
presented integer position with the authoritative reveal position.

Tracing is opt-in through:

```text
TYPHON_CURSOR_PRESENTATION_TRACE=1
```

When disabled, the lazy trace closures do not format or allocate trace
messages, take trace-only timestamps, schedule work, or emit logs. The trace
uses a monotonic process-local sequence only after the opt-in check succeeds.

## Evidence and first divergence

The deterministic model now covers the important reveal sequences:

- hidden P0 → hide → hidden P1 → show, with P1 as the first visible state;
- hide/show with an unchanged logical pointer, preserving the same state;
- hidden image, hotspot, size, framebuffer, and position changes becoming one
  current visible assignment;
- hidden-to-hardware classification as a visibility transition;
- submitted and queued worker state remaining separately inspectable.

These tests show no stale first-visible state in the deterministic cursor model.
They do not establish the physical KMS result on the target Sober setup. No
live trace was available to identify a Class A–G divergence, so the first
divergence is recorded as **not yet observed**, not guessed.

The selected correction for this turn is therefore the observability and
deterministic ownership closure only. Generation filtering, delayed reveal,
synthetic motion, and DRM-specific correction remain rejected hypotheses until
the live trace proves they are necessary.

## RED evidence and corrections

The implementation was developed against compile-time RED tests before each
missing seam was added:

- the pointer-debug RED test initially could not resolve the lazy cursor trace
  helper;
- the reveal-authority RED test initially could not resolve
  `CursorRevealAuthority`;
- the cursor trace snapshot RED test initially could not resolve submitted and
  queued-state accessors.

The corrections were narrow: add the opt-in lazy logger, publish the reveal
authority through the compositor server, expose existing cursor state and
revision snapshots, and instrument the existing worker/page-flip boundaries.
No input timing, pointer movement, scheduler, activation-anchor, or cursor
policy semantics were changed.

## Focused verification

The following focused runs passed:

```text
rtk cargo test pointer_debug --lib                 4 passed
rtk cargo test pointer_cursor --lib               27 passed
rtk cargo test --locked --all-targets cursor      385 passed
rtk cargo test --locked --all-targets plane_scheduling_model  22 passed
rtk cargo test --locked --all-targets pageflip     92 passed
rtk cargo test --locked --all-targets presentation_transactions  62 passed
rtk cargo test --locked --all-targets input       342 passed
```

The required full repository suite passed:

```text
rtk cargo test --locked
3381 passed, 5 ignored, 40 filtered out
```

## Global verification

The required locked all-target check and lint passed before report finalization:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

The final report-only commit does not change Rust sources; the exact command
set is rerun after that commit before completion is reported.

## Manual hardware/software A/B qualification

Manual qualification was not performed here because it requires the user's
native Sober session. Sober was not automated. The required A/B commands are:

Hardware:

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
TYPHON_CURSOR_PRESENTATION_TRACE=1 \
OBLIVION_ONE_CURSOR=hardware \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

Software:

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
TYPHON_CURSOR_PRESENTATION_TRACE=1 \
OBLIVION_ONE_CURSOR=software \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

For each mode, collect at least 20 normal lock/unlock cycles, 10 cycles with
no locked motion, 10 cycles with large locked relative motion, edge-adjacent
cycles, and fast lock/unlock sequences. Record camera jump and visual cursor
teleport, then classify the first divergence as authority, source, desired,
frozen/submitted, presented, hardware-only, or both hardware and software.

The decision rule remains:

```text
hardware teleports, software does not -> prioritize hardware presentation
both teleport                         -> investigate shared path above DRM
hardware does not, software teleports -> investigate software/source path
neither teleports                     -> do not invent a correction
```

The first visible cursor after unlock must be the latest authoritative reveal,
regardless of which of the existing hardware or software delivery modes wins.
