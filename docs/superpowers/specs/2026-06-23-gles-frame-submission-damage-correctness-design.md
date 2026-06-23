# GLES Frame Submission And Damage Correctness Design

Date: 2026-06-23

## Confirmed Failure

The current EGL path permits this sequence:

```text
logical damage empty
-> RepaintMode::Skip
-> RenderExecution::Skip
-> no GL clear or draw
-> eglSwapBuffers still runs
-> frame_presented advances repaint history
-> GBM locks the newly selected front buffer
-> KMS presents pixels that were never repaired
```

Both the nested renderer and native EGL/GBM renderer swap unconditionally after
`GlesSceneRenderer::draw_scene`. The native path additionally locks the front
buffer, creates or retrieves a framebuffer, marks it ready, and makes it
eligible for legacy or Atomic KMS submission. Surface commits can also replace
earlier damage, and `damage` and `damage_buffer` currently share one pending
rectangle list despite using different coordinate spaces.

## Frame Submission Contract

Scene preparation returns an explicit skipped or rendered outcome. Empty
logical output damage returns `Skipped` before GL execution and before any EGL
swap. A rendered outcome contains a full or scissored repaint plan. The GL
executor has no skip variant.

The caller attempts exactly one swap for a rendered outcome. Only a successful
swap commits the candidate scene snapshot and logical-damage history. Native
GBM front-buffer locking and framebuffer readiness happen after that commit.
Swap or draw failure leaves the last presented snapshot unchanged and
invalidates unsafe back-buffer-age assumptions, so rebuilding the same
candidate reproduces conservative retry damage. A skipped outcome creates no
ready frame and therefore no legacy or Atomic KMS work.

Protocol/event-loop progress, a rendered frame, and a KMS-presented frame are
separate events. Protocol work can finish without rotating the EGL swapchain.
Lifecycle work that explicitly requires presentation requests a real frame and
falls back to full damage when exact damage is unavailable.

## Surface Damage And Commit History

`RenderableSurfaceDamage` has explicit `Empty`, `Full`, and normalized
`Partial` states. Union clips rectangles, preserves all non-empty regions, lets
`Full` dominate, and never turns an empty rectangle list into `Full`.

Each renderable surface owns a bounded commit-counted journal. Every published
visual commit advances its counter and records its normalized damage. Renderers
query the union since the commit they last presented or uploaded. Readers older
than retained history receive `HistoryLost` and repaint or upload the complete
surface. Cached synchronized-subsurface state and explicit-sync-delayed state
do not enter the visible journal until their existing transactional publication
boundary. A new buffer identity or incompatible dimensions records full damage.

## Wayland Damage Coordinates

Pending `wl_surface.damage` rectangles and `wl_surface.damage_buffer`
rectangles use distinct types and lists. At commit, the compositor captures the
buffer scale and supported viewport mapping, converts surface-local damage into
attached-buffer coordinates with checked floor/ceil arithmetic, unions direct
buffer damage, clips to real buffer bounds, and normalizes the result. Invalid,
overflowing, fractional-unsupported, or otherwise ambiguous mappings produce
`Full`; under-damage is never accepted.

## Presented Scene Tracking

The GLES renderer compares a candidate scene with the last successfully
presented element snapshot. Stable element identity covers client surfaces,
subsurfaces, popups, wallpaper, shell overlays, and software cursors. New and
removed elements damage their visible bounds. Movement, resize, resource
identity, mapping, and relevant stacking changes damage conservative old and
new bounds. An unchanged element maps `damage_since(previous_commit)` into
output coordinates, with history loss or uncertain mapping expanding to full
element bounds. Hardware-cursor-only movement produces no GLES damage.

Preparing or drawing a candidate never mutates presented state. Successful EGL
swap is the only commit boundary. A scene/resource contradiction with empty
damage is asserted in tests/debug builds and conservatively promoted to full in
release behavior.

## Buffer Age And Partial Repaint

Logical damage describes output pixels changed from the last presented scene.
Repair damage adds the retained logical damage required by the selected back
buffer's age. History stores successful frames' logical damage, never expanded
repair damage. Skips and failures do not append history. Resize, EGL surface or
context recreation, backend generation change, invalid age, and swap failure
invalidate preservation assumptions and force a subsequent full repaint.

Partial repaint is opt-in through
`OBLIVION_ONE_ENABLE_PARTIAL_REPAINT=1`. `OBLIVION_ONE_FORCE_FULL_REPAINT=1`
has highest precedence. Without opt-in, every real visual frame is full. With
opt-in but disabled/invalid buffer age, rendering remains conservative full.
Partial repaint remains opt-in until the deterministic swapchain oracle and the
real legacy/Atomic hardware matrix pass.

## Resource Uploads And Scanout

SHM texture resources retain the surface commit they uploaded. Before drawing,
they upload the union since that commit; history loss uploads the complete
buffer. Upload state advances only after successful upload. Dmabuf imports keep
TASK 05.1 stable `BufferId` identity; raw fd numbers remain diagnostic only,
and explicit-sync readiness stays separate from visual identity and damage.

Legacy and Atomic KMS consume one shared ready-frame stream:

```text
logical damage -> draw -> successful swap -> lock GBM front buffer
-> framebuffer ready -> KMS submit -> pending -> pageflip -> current
```

The empty path terminates before draw and creates no KMS-visible state.

## Verification And Scope

Tests use pure planners and injected submission counters to prove skip, failure,
history, scene, upload, and KMS invariants. A deterministic three-buffer oracle
applies repair plans to aged buffers and compares every successful presentation
with full reference rendering. Focused tests run before workspace formatting,
checking, clippy, full tests, release build, and diff checks.

This work preserves TASK 05.1 stable buffer identity, TASK 05.2 resize flow
control, TASK 05.3 synchronized publication, explicit-sync semantics, and the
existing KMS/pageflip model. It does not add scanout features, output topology,
color work, XWayland, renderer threading, or broad compositor refactors.
