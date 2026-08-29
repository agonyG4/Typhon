# Typhon NoVisualChange Lineage and Decoration-Aware Order Damage Correction

**Date:** 2026-08-29

## Scope

This corrective closure restores the logical damage-baseline terminal that was
present in the Native Output Damage Authority v1 implementation and extends
the existing regional order transition to include server-side decorations.
It does not redesign frame lineage, output buffer-age repair, surface storage,
or the already-qualified O1, SHM, DMA-BUF, KMS, or Direct Scanout paths.

The local checkout is authoritative. It is on `main` at `b5d7701`; the
configured public `origin/main` is `a0d5b8a`. The checkout is ahead of that
public ref and contains the prior regional-damage closure plus unrelated
pointer-lock commits. Those existing changes remain untouched.

## Current-source findings

The exact `SurfaceDamagePresentation` token already records the frozen
surface-generation and journal commit for the rendered scene. Physical pageflip
settlement consumes it through `commit_surface_damage_presented()`.

The regression is that `complete_no_visual_change_frame_batch()` takes the
token and drops it, and the batchless `settle_no_visual_change_work()` path
also drops it. This makes 128 authoritative Empty logical commits look
unsettled, so a bounded journal can report `HistoryLost` for a later Partial.
The prior implementation in git used one keyed settlement helper with an
explicit `SurfaceDamageSettlement::{Presented, NoVisualChange}` disposition.

The correction restores that single helper. Both dispositions advance only
the logical surface damage baseline and update the corresponding bounded
metric. NoVisualChange remains callable only by a terminal path that has
already proven output damage is empty. Rendered-but-rejected non-empty work
continues to retain its token for retry/presentation and never calls this
logical terminal.

The physical authorities remain outside compositor surface settlement:
native scene presented history, output serials, slot presentation state,
pageflip completion, presentation feedback, KMS transaction state, and the
partial-repaint planner are not touched by NoVisualChange.

## Decoration-aware regional order damage

The existing common-prefix/common-suffix algorithm remains the authority for
finding the changed ordered surface span. `NativeSceneSurfaceSnapshot` gains
the smallest immutable visual-root identity needed by damage comparison. The
identity is populated from `visual_stack_groups()` using the exact popup IDs
selected for the resolved scene; it is not inferred from numeric ID ranges or
a second z-order model.

For each old/current surface in the changed span, the correction collects its
visual root. It then damages the old and current bounds of every client or
subsurface snapshot owned by those roots, plus the old and current
`DecorationSceneSnapshot` bounds whose `root_surface_id` is one of those
roots. Popup roots remain independent groups and therefore do not acquire a
parent window's SSD. If the ordered IDs are unchanged, no order-specific
damage is added; unchanged decoration geometry/signatures therefore remain
free of repaint.

Decoration geometry/signature changes continue to use
`from_decoration_bounds_changes()`. Order transitions use the same immutable
decoration identities and bounds without requiring a decoration mutation.

## Integrated oracle hardening

The existing topology/output-buffer-age oracle is extended in place. Its
sequence explicitly rotates client logical buffers A/B/C/A, rotates output
slots 0/1/2/0 with ages 1/2/3, and includes partial and authoritative Empty
content transitions, popup and subsurface topology, and overlapping SSD
decorations.

A rejected candidate is rendered into its actual output slot and then rejected
without committing presented planner history or the presented scene. The next
selected slot performs a real retry/presentation using the unchanged presented
predecessor and its actual age/history. Each physical presentation is compared
pixel-for-pixel with a full-reference render.

The compatibility path keeps its existing protocol-only ownership sequence.
The captured scene signature and ordered surface IDs are asserted against the
second resolved scene used for painting, documenting and checking the
protocol-only mutation assumption without redesigning that path.

## Test strategy

RED tests first change the existing no-visual tests to require logical
settlement, add physical-authority and rejected-non-empty coverage, and add
decorated reorder coverage. The integrated topology oracle is strengthened
after the production fix so it exercises true rejection and the combined
client/output/topology/SSD state.

The focused suites cover surface journals, frame batches, native output damage,
decorations/visual groups, partial repaint and buffer age, compatibility
scene identity, Atomic lineage, Direct Scanout identity, O1 callback
ownership, and SHM materialization. Full verification uses the checkout's
`rtk` commands. No native qualification is claimed unless a real DRM/KMS TTY
is available and actually exercised.

## Non-goals

No O1 predictor or callback policy, SHM release timing, DMA-BUF release
authority, KMS scheduling, READY admission, Direct Scanout policy, resize,
pointer-lock, trace volume, VRR, tearing, color, or scene-graph rewrite is
included.
