# Typhon Render-Ahead Presentation-Damage Domain Design

Date: 2026-08-18

## Scope

This closure addresses persistent stale pixels after aggressive native move and
resize interaction when native composition uses partial repaint, buffer age,
and predictive triple buffering. It does not redesign Direct Scanout, shell
layers, decorations, or unrelated Wayland protocols.

The native qualification command is:

```text
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell
ASTREA_COMPOSITOR_BACKEND=typhon
TYPHON_XWAYLAND=eager
./bin/start-oblivion-one-tty
```

The launcher normally disables Direct Scanout. The relevant default path is
therefore native composition with Atomic EGL/GBM when available, partial
repaint, presentation-based explicit slot ages, and adaptive render-ahead.

## Findings

### CONFIRMED: render repair and presentation history shared one field

`PartialRepaintPlanner::plan()` creates a `RepaintPlan` before the frame is
presented. Its damage is the repair required to transform the acquired slot
into the candidate frame. Under render-ahead, that repair may be based on the
frame that is currently presented while another submitted frame is still
pending.

The old `commit_presented(&RepaintPlan)` then inserted
`RepaintPlan.current_damage` into the buffer-age journal. That made one value
serve two different timelines. The field is now named `render_damage`, and
presentation history advances only through an explicit
`commit_presented_transition(OutputDamage)` call.

### CONFIRMED: explicit Atomic slot age is presentation-domain state

`AtomicOutputSlot::buffer_age()` delegates to
`render_target_buffer_age(presentation_serial, last_presented_serial)`. The
slot's serial is updated only after confirmed pageflip completion. Its age is
therefore indexed by confirmed presentation order, not render order.

### STRONG HYPOTHESIS: the mismatch caused persistent move/resize ghosts

For A presented, B rendered, C rendered ahead, B pageflip, and C pageflip, the
render-time repair for C may use A→C. The consecutive presentation sequence is
A→B→C, so a presentation-domain journal must contain A→B and B→C. If it stores
A→C instead, a slot containing B can return with a valid age while the
journal omits B-only titlebar, button, border, or client pixels. Slot rotation
then makes those pixels reappear indefinitely.

The deterministic pixel test now proves the missing semantic distinction and
proves that the corrected B→C entry removes a B-only pixel from a reused slot.
Native causal proof requires the runtime matrix in the final report.

### NATIVE-PROVEN: not yet established in this environment

No native DRM/TTY qualification has been claimed unless the final report
records a successful run. Triple-buffering-off and forced-full-repaint
diagnostics are likewise reported only when actually executed.

## Domain model

Typhon has two damage concepts:

```text
RenderRepairDamage
    The repair needed immediately for an acquired framebuffer slot.
    It may compare the currently presented A with a candidate C rendered
    ahead of pending B.

PresentedTransitionDamage
    The transition from the exact frame that was actually presented before
    the pageflip to the exact submitted frame confirmed by that pageflip.
    It is the only damage that enters a presentation-serial journal.
```

The invariant is:

```text
buffer-age sequence domain == damage-journal sequence domain
```

For the explicit Atomic path, both are confirmed KMS presentation order.

## Chosen architecture

`RepaintPlan` describes render work only:

```rust
struct RepaintPlan {
    render_damage: OutputDamage,
    repair_damage: OutputDamage,
    buffer_age: Option<u32>,
    // mode and fallback metadata
}
```

`PartialRepaintPlanner::commit_presented_transition()` accepts the transition
explicitly. No production caller can derive presentation history from a
render plan implicitly.

`NativeSceneHistory` remains the exact submitted-scene authority. It stores the
presented snapshot, ready snapshot, and token-keyed submitted snapshots. At a
confirmed explicit composited pageflip it prepares a compact transition from:

```text
NativeSceneHistory.presented
    → submitted snapshot for pageflip token
```

The transition includes the predecessor/current frame IDs and renderer damage.
It uses the current cursor state from each exact snapshot, including software
cursor damage, and returns full-output damage when no valid composited
predecessor exists.

The Atomic pageflip path then:

1. identifies the pageflip token and transaction;
2. prepares the scene transition without mutating scene history;
3. completes the output swapchain token;
4. validates completed frame ID and transaction ID against the prepared scene;
5. commits the explicit renderer presentation with the prepared transition;
6. promotes the matching scene-history token.

The renderer's scene key and damage tracker still promote the exact
`EglSceneFrameCommit` that completed. They are not replaced by damage equality.

If a predecessor is absent after bootstrap recovery, Direct Scanout, or
history invalidation, the transition is full-output. No fake previous scene is
manufactured.

## Bootstrap and backend domains

The bootstrap path renders and modesets the initial Atomic framebuffer before
constructing the runtime's `NativeSceneHistory`; its initial scene is therefore
audited against an actual initial scanout rather than assumed from mutable
server state. The first runtime transition still has a full-repair fallback if
history is invalidated.

Compatibility EGL/GBM queries EGL buffer age for the EGL surface and settles
the renderer around EGL buffer swap. That backend's journal remains coupled to
its EGL swap/render sequence until a separate backend audit proves a different
domain. The explicit Atomic path alone uses confirmed KMS presentation serials
for slot age and pageflip transition journal entries.

CPU/dumb paths do not use the explicit presentation-serial partial repaint
journal. Their existing copy/damage ownership remains unchanged.

## Comparison invariants

KWin's `DamageJournal` and EGL swapchain slot age advance in one coherent
render/release sequence. Its Direct Scanout path also explicitly forgets
composited damage when ownership leaves the compositor. The relevant invariant
for Typhon is that a history entry and the age used to consume it share one
sequence domain; Typhon retains its stronger confirmed-pageflip domain.

Hyprland's fullscreen renderer selects the scene it actually draws separately
from its output damage ring, and the ring is consumed with the swapchain buffer
age. The relevant invariant for Typhon is the same separation: render repair
can be resolved for the acquired slot, while presentation history records only
actual transitions.

## Considered approaches

### Chosen: presentation-domain transition journal

Keep explicit slot age indexed by confirmed presentation serials and calculate
the transition at pageflip from exact submitted snapshots. This preserves
transaction tokens, retry/rejection semantics, worker ownership, and current
explicit slot behavior.

### Rejected: convert all histories to render order

This would make rendered-but-rejected frames appear in the same sequence as
presented frames and would weaken the existing pageflip ownership model. It is
unnecessary for the confirmed explicit Atomic age contract.

### Rejected: invalidate history whenever render-ahead occurs

This would hide the semantic bug by forcing repeated full repairs, defeat the
purpose of predictive buffering, and make ordinary render-ahead needlessly
expensive. Full repair remains valid only at a genuine history discontinuity.

## Test architecture

The regression suite contains these layers:

* a red planner test for A presented, B rendered, C rendered ahead, B
  presented, C presented, asserting B→C rather than A→C;
* a `NativeSceneHistory` test that prepares A→B then B→C and a rejection test
  that prepares A→C when B is discarded;
* a warmed three-slot pixel oracle with a B-only probe pixel and a physical
  B-slot reuse at age 2;
* existing age 1/2/3 full-reference tests, retained and rerun after the API
  split;
* rejection, retry, out-of-order, cursor, content, decoration, fullscreen,
  and Direct→composited history-boundary tests from the previous closure.

The slot model tracks physical pixels, slot ID, last confirmed presentation
serial, and logical scene identity. It applies only the planner's repair
rectangles and compares the result with a fresh reference framebuffer.

## Performance and safety constraints

Pageflip transition preparation compares compact scene snapshots and does not
read GPU pixels, clone framebuffer contents, rerasterize decorations, or
re-resolve mutable server state. Damage signatures used by bounded tracing are
compact FNV-style identities over damage kinds and rectangles.

No global triple-buffering disable, buffer-age disable, partial-repaint
disable, synchronous pageflip wait, generic move/resize full repaint, or
render-ahead history invalidation is part of the production fix.

## Qualification boundary

If the final normal-settings native run is clean after move/resize stress,
that closes this presentation-damage hypothesis. If old complete frame images
still appear without the move/resize-triggered stale-slot path, the remaining
issue must be traced separately with frame ID, slot ID, framebuffer ID,
transaction/token, and kernel pageflip sequence. Damage logic must not be
expanded to conceal an independent KMS ordering fault.
