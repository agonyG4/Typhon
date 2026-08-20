# Typhon Oversized Resize Presentation-Ghosting Design

Date: 2026-08-17

## Scope and observed symptom

This closure targets the severe native rendering regression reported after the
WindowVisual, SSD, input, fullscreen, and damage work: a decorated window that
is wider than, or partly outside, a 1920x1080 output accumulates duplicated
titlebar buttons, titlebar edges, and outer-frame pixels while it is resized.
Resize is the primary reproducer. Move is a secondary qualification case.

The logical scene must remain wider than the output until output-local damage
is clipped. This design does not change rectangle clipping, disable buffer age,
disable render-ahead, force full-output repaint, disable Direct Scanout, or
wait synchronously for pageflip before rendering.

## Baseline and source evidence

The baseline HEAD is `a08a480fb552e9d26f907390964930d2fdebd698`.
The worktree was already dirty before this closure, including 41 tracked
paths with unrelated protocol, launch, selection, fullscreen, renderer, and
native-output changes, a deleted prior SDD report, and untracked `.codex/`,
plans, specifications, and error-log artifacts. Those changes are preserved.

The codebase-memory graph project is `home-agony-GitHub-Typhon`, full/ready.
After the implementation refresh it reports one parse-partial range, line 115
of `src/native_output/runtime/presentation.rs`; that range was read directly.
All other operated paths had no recorded coverage issue, subject to the
graph's best-effort caveat.

Static source tracing found:

- `GlesSceneRenderer` already separates a rendered candidate from
  `presented_scene_key`; `commit_presented` is called by the explicit
  pageflip-completion path and `discard_rendered` does not mutate history.
- `NativeRuntime` separately owns `last_renderable_surfaces` and
  `last_decoration_scene`. They are initialized at bootstrap and overwritten
  from the current server scene in render-complete, ready, compatibility, and
  worker-queue paths. The native pageflip handler destructures these fields as
  ignored and does not promote a frame-owned scene snapshot.
- Native damage was therefore calculated from render/submission order rather
  than a scene associated with the confirmed output frame. The deterministic
  A-presented/B-rendered/C-rendered pixel-reference test reproduced the
  under-repair before the fix, proving Defect A in the model. The corrective
  path now uses `NativeSceneHistory`: compact ready/submitted snapshots are
  keyed by the output token and only the exact completed pageflip promotes the
  presented snapshot. Immediate promotion is retained only for the existing
  synchronous presentation contract; rejected/replaced/recovered candidates
  are discarded.
- `preview_resize_root_window_to` stores active `toplevel_visual_geometries`
  containing the preview width, height, and placement without changing the
  committed client buffer. The SSD render-instance and hit-test paths now
  query `current_visual_root_window_geometry`, so layout, visual bounds, and
  button hit testing use the same preview size. The focused right-edge and
  left-edge tests reproduced the committed-size mismatch before the fix and
  now pass.

## Rendered versus presented semantics

The logical compositor state may advance from A to B to C before any of those
frames is visible. Every render/submission record must carry the exact compact
scene snapshot that generated it. The presented snapshot is separate and is
advanced only by the terminal event that confirms the associated output frame
or by the existing synchronous/immediate presentation contract.

The native damage bridge uses one `NativeFrameSceneSnapshot` containing only
surface IDs, mapped damage/bounds, decoration scene metadata, cursor state,
logical/render generation, and a stable frame identity. It deliberately does
not retain client pixel buffers or theme assets. A candidate travels through
the bounded ready/submitted token map; the pageflip token selects the exact
frame, and the implementation never rebuilds presented history from mutable
`CompositorState` at pageflip time.

Frames that are rendered but skipped, rejected, replaced, abandoned during
session recovery, or otherwise never confirmed are discarded through the same
ownership path as their existing frame/buffer resources. The retained history
is bounded by the existing in-flight and buffer-age requirements; no unbounded
scene vector is introduced.

## Pageflip promotion and buffer age

Native damage planning must use the one presented predecessor and the current
candidate, while retaining the existing accumulated repaint planner semantics
for a buffer age greater than one. A buffer containing A must be repaired for
every visual transition since that buffer was valid, even when B was rendered
but never presented. The implementation will preserve the existing
`PartialRepaintPlanner` commit/discard contract and will not replace age-aware
repair with a single previous/current rectangle.

The explicit EGL path remains the reference ownership model: its
`EglSceneFrameCommit` is stored inside `RenderedOutputFrame`, committed by
`AtomicEglGbmScanout::complete_pageflip`, and discarded on failed ownership
transitions. The native compatibility and worker paths now use the same
semantic boundary with their existing submission token, without a parallel
latest-frame global. Buffer-age and render-ahead behavior remain enabled.

## Visual resize geometry

The current visual root geometry is the authority during an active interactive
resize. A per-frame resolved visual geometry will be captured once and reused
by client rendering, SSD layout, visual bounds/damage, and decoration hit
testing. The accessor falls back to committed surface geometry only when no
visual override exists.

The SSD layout uses the resolved visual width and height. For MacTahoe
borderless SSD, the titlebar left and right edges equal the visual client left
and right edges, with the existing titlebar-top contract unchanged. The
button cluster is anchored to the current visual titlebar right edge, so a
left-edge preview does not jump when a later client commit arrives.

Logical visual bounds are built before intersection with an output. A window
with a 2100px logical width remains 2100px wide in the snapshot even when the
damage sent to a 1920px output is clipped.

## Rejected approaches

- Rewriting negative-coordinate or oversized rectangle clipping without a
  failing clipping proof.
- Forcing every frame to full-output repaint.
- Disabling buffer age, triple buffering, render-ahead, or Direct Scanout.
- Rebuilding pageflip history from the latest mutable compositor state.
- Keeping independent surface and decoration histories that can describe
  different frames.
- Cloning pixel buffers, themes, SVG assets, or serialized scene state per
  frame.

## Test architecture

The first red test models presented A, rendered-but-not-presented B, and
rendered C while reusing a framebuffer containing A. It renders a full clean C
reference and compares it pixel-for-pixel with the partial path, sampling old
close/maximize/minimize positions, the old right edge, titlebar background,
and old frame/backing pixels. The sequence is exercised across widths 2200
down to 800, all eight resize-edge combinations, and offscreen directions.

The second red test keeps a committed 1800x1000 client while previewing 1700,
1600, 1500, and 1400 widths without client commits. It asserts that SSD
layout, button-right anchoring, visual bounds, damage bounds, and decoration
hit testing use the preview geometry. A dedicated left-edge sequence verifies
that client and titlebar right edges remain equal.

Additional tests cover presentation schedules and discarded/replaced frames;
the repository's existing suites cover buffer-age behavior, managed XWayland
backing, CSD zero extents, fullscreen/maximized restore, and Direct Scanout
eligibility. This closure adds the focused oversized history/geometry cases;
an explicit age-1/2/3 schedule table remains follow-up work. Native
qualification is reported separately and is not inferred from model tests.

## Qualification status

Defect A is proven by the deterministic red pixel test and fixed by the
pageflip-owned history; Defect B is proven by the red SSD preview tests and
fixed by visual-geometry parity. The repository has `/dev/dri/card0` and
`renderD128`, but `astreactl status` reports no Typhon instance, so native
DRM/KMS qualification was not available in this session. The report therefore
does not claim native visual confirmation.
