# Native render snapshot boundary — Phase 2A

Phase 2A gives the explicit Atomic EGL path and EGL capture an owned input
boundary. The reactor resolves the scene, freezes its renderer-visible values,
then calls the existing synchronous EGL/GLES renderer. No render worker is
created here, and this phase makes no claim about resize or interaction
performance.

## Frozen frame contract

`FrozenNativeRenderFrame` lives in `src/native_output/runtime/frame.rs`, where
the native runtime already understands both compositor state and
`EglSceneDrawRequest`. It consumes the existing
`ResolvedNativeFrameScene<'static>` and stores:

- the resolved canonical scene, effects, presentation sample/snapshot,
  lifecycle sample/snapshot, lifecycle surfaces/decorations, and exit evidence;
- the resolved desktop visual state and an owned client cursor surface snapshot;
- the surface resource sync journal for canonical, software cursor, and
  lifecycle surfaces;
- output damage, consumed by the first and only request projection; and
- `FrozenRenderSnapshotIdentity`, containing render/effects generations, scene
  and presentation signatures, lifecycle signature, exact client cursor
  surface/generation/commit/buffer identity, the frozen desktop visual state,
  and a signature of the captured resource journal.

The identity is diagnostic and assertion evidence. Scene and protocol truth
remain in their existing owners. The projection borrows the large scene
vectors and moves the already captured journal vector and damage into one
`EglSceneDrawRequest`; a second projection is rejected.

Freezing moves the existing `ResolvedNativeFrameScene<'static>` into the
payload. The freeze path does not explicitly clone its canonical scene vectors.
An optional software client cursor keeps one `RenderableSurface` clone; its SHM
pixels remain in their existing `Arc<Vec<u32>>` and DMA-BUF planes retain their
existing `Arc<OwnedFd>` handles. The renderer presentation visual signature is
computed once and reused for both `FrozenRenderSnapshotIdentity` and the draw
request.

The focused `snapshot_owned_clones == 0` assertion proves only that freeze did
not call `ResolvedNativeFrameScene::snapshot_owned()` and clone another
`NativeSceneSnapshot`. It is not a measurement of all scene-owned cloning or
allocations.

## Buffer and protocol ownership

Committed SHM content is represented by `ShmBufferSnapshot`, whose pixels are
owned `Arc<Vec<u32>>` storage. Renderer requests contain `RenderableSurface`
snapshots and never retain a borrowed `wl_shm_pool` mapping. Pool mapping and
materialization are commit-side work.

DMA-BUF `RenderableSurface` values retain `DmabufBufferHandle` planes backed by
`Arc<OwnedFd>`. The renderer's EGLImage cache stores the imported image, its
image key, and a `WeakBufferIdentity` lifetime token. The token points to the
Rust buffer identity lifetime, not a `wl_buffer` object; stale cached entries
are evicted when that token is no longer alive. The frozen frame therefore
retains the import descriptors it needs without carrying a live Wayland
resource into renderer ownership.

The frozen payload does not own protocol release obligations. DMA-BUF protocol
release remains in the existing active release, frame batch, lifecycle, and
GPU-release machinery. `OutputTransactionLedger`, frame-batch settlement,
frame callbacks, presentation feedback, KMS submission/pageflip settlement,
scene-history publication, pacing, and output-generation authority remain on
the reactor. Capture continues to avoid creating or settling output protocol
work.

Frame callback capture and admission remain on their current server/runtime
paths (`capture_frame_callbacks_for_render_with_presentation_samples`,
`complete_frame_callbacks_after_admission`, and the existing Atomic render
settlement). Freezing neither captures nor completes callbacks. Buffer-release
and presentation-feedback timing are unchanged.

## Current path scope

`AtomicEglGbmScanout::render_frame` performs its existing reactor-side
transaction and damage preparation, then moves the owned resolved scene into a
frozen frame immediately before slot rendering. The initial Atomic modeset
frame and synchronous output-reconfiguration candidate use the same boundary.
`render_to_slot` receives only that frozen frame for scene drawing; its
signature has no server, input state, cursor mode, or separate live scene.
Atomic capture and compatibility EGL capture consume an owned scene and use
the same frozen projection. Capture remains synchronous.

Compatibility painting (`NativeEglGbmScanout::paint_server_frame`) retains its
current synchronous request construction. Converting that path would require
changing its borrowed scene ownership, while Phase 2B initially targets only
the explicit Atomic path. It remains a documented live-state boundary for a
later compatibility migration.

## Phase 2B worker contract

Future ownership target:

```text
RT reactor:
    Wayland and NativeInputState
    presentation planning and frame pacing
    OutputTransactionLedger and protocol frame batches
    logical swapchain ownership and KMS scheduling
    render_worker_busy: bool
    render_demand: bool

render worker:
    EGL and GL contexts
    GlesSceneRenderer, EffectRuntime, RendererResourceState
    render-side GL/EGL output resources
    exactly one executing RenderJob

KMS worker:
    timed DRM/KMS submission
```

### Renderer control ownership

Moving `GlesSceneRenderer` to the worker is not enough. The reactor currently
mutates renderer causal state outside drawing, and those operations must cross
an explicit renderer-control boundary:

| Operation | Current source effect | Phase 2B treatment |
| --- | --- | --- |
| `commit_presented(frame, damage)` | Commits repaint history and damage-tracker state, publishes the presented scene key, and updates history depth after confirmed pageflip. | Lossless terminal operation. `CommitPresented(N)` must stay ordered relative to later operations that read or extend presented-damage history. |
| `discard_rendered(frame)` | Discards the repaint plan and invalidates effect checkpoint capture contents. | Lossless terminal operation; preserve checkpoint invalidation. |
| `frame_swap_failed()` | Resets repaint history after a failed compatibility swap. | Lossless failure control operation. |
| `invalidate_presented_damage_history()` | Clears presented repaint history and scene key, and invalidates checkpoint capture contents. | Ordered control operation. Adjacent invalidations may be folded only when no commit, discard, or render-admission boundary lies between them. |
| `publish_material_effect_generation(...)` | Publishes material state and invalidates presented history. | Configuration update with an invalidation side effect; a newer complete generation may replace an unobserved one only behind a revision barrier. |
| `publish_effect_registry_generation(...)` | Compiles and publishes a registry generation, returns an error on failure, and invalidates presented history on success. | Ordered configuration operation unless Phase 2B proves coalescing preserves intermediate compile failures and the last successfully active generation. The job must name the generation it consumes. |
| `set_cursor_image(...)` | Replaces the cursor image, updates damage tracking, marks cursor resources stale, and invalidates repaint history. | Latest-value configuration can replace an unobserved image only when the worker retains the required damage invalidation and revision ordering. |
| `reload_trusted_effect_registry(...)` | Runs the trusted reload publisher and returns a success/error plus the resulting generation; registry publication reaches the GL renderer. | Ordered request/completion, not a fire-and-forget latest-value update. Its admission/backpressure and response ownership need a bounded policy before Phase 2B. |

The lifecycle and squash capability queries are also renderer-owned facts:
`lifecycle_animation_available()` and `squash_animation_available()` are read
when runtime capabilities are published to the compositor during bootstrap or
scanout replacement. The worker must report a generation-tagged capability
snapshot during startup/reconfiguration; the reactor applies that snapshot to
Wayland/lifecycle state. It must not synchronously query a live worker-owned
renderer. `finish_render_work()` is a GL completion barrier and must likewise
remain a worker-side operation with asynchronous completion if reactor code
needs its result. The current presentation-cycle fallback paths call this
barrier synchronously, so they must not retain a direct reactor-to-renderer
call in Phase 2B.

The physical Atomic swapchain has three slots
(`EXPLICIT_OUTPUT_SLOT_CAPACITY == 3`). Its invariants prevent slot-role
aliasing and more than three outstanding output-frame renderer tokens. The
future control lane keeps each terminal token attached to its occupied physical
slot until `CommitPresented` or `DiscardRendered` is applied exactly once. Each
operation names its frame's token; it is never selected by “newest render.”
Use a fixed-capacity, lossless control path: at most three reactor-owned
per-slot terminal commands, retained until applied, with those commands drained
in order before admitting another render. This bound comes from the swapchain's
three physical slots. The reactor records completion and continues its event
loop if the worker is busy; it never waits for control-lane capacity.

Phase 2B.2 uses one executing render job and a reactor-owned demand bit. The
reactor does not prepare another slot, transaction, frame batch, or frozen
payload while the worker is busy. Visual changes set `render_demand = true`;
repeated changes coalesce there. Once the executing job completes and existing
settlement permits another render, the reactor resolves the newest compositor
state, prepares one exact job, clears the demand bit, and dispatches it. Resize
configure/ACK state continues to advance and is never coalesced. This does not
add a second prepared physical render job.

## Phase 2B.2 implementation checkpoint

The current checkout contains the bounded worker bootstrap, render command and
completion payloads, renderer-control representation, and pure busy/demand
state model. The render-worker eventfd is registered as a dedicated native
event source when a worker transport is present, and its completion identity is
checked against the single executing-job identity before retaining the result
in the reactor's one-entry completion slot. Worker-busy presentation planning
records only redraw demand before scene, slot, transaction, frame-batch, or
frozen-payload preparation. Worker bootstrap follows the required two-stage
ownership path: EGL format discovery runs on `typhon-render`; the reactor then
selects the KMS-compatible format, allocates physical BOs, and transfers owned
render-slot descriptors back to that same worker for renderer initialization.
The worker and transport payloads are qualified as naturally `Send`.

The current source also has the reactor continuation and common Atomic outcome
settlement connected to the worker eventfd path. Screen capture and trusted
effect reload have one-at-a-time asynchronous worker admissions. Session
suspend stops new admission, drains the executing render continuation after
reactor demotion, and acknowledges render-pool generation rebind before the
reactor is rearmed.

The worker is still not an effective runtime transport. `auto` reports an
automatic startup degradation and uses the synchronous transport; `force`
fails startup. The remaining gating path is output-configuration candidate
preparation, promotion, and retirement: it still creates and renders the
candidate synchronously. Worker-mode output configuration must prepare the
candidate on the existing `typhon-render` thread, retain the old render domain
until KMS commits, then acknowledge old-domain destruction before releasing the
old physical pool. Failure paths also need candidate-destruction acknowledgments
before dropping candidate GBM BOs. Until this path is asynchronous and the
same-checkout binary build is qualified, do not use `auto` as a hardware A/B
setting.

The three-slot bound covers output-frame terminal tokens. Other worker work
uses separate fixed admission limits, not entries in an unbounded common queue:

- Trusted-effect reload has at most one in-flight asynchronous request. The
  runtime already has a deferred control-response pattern: a pending job retains
  request identity, then delivers completion through `queue_response`. A
  second reload is not admitted while that request is in flight.
- Screen capture has at most one admitted worker capture initially; protocol
  completion remains asynchronous. Further captures are not accumulated as
  render jobs.
- Output configuration remains owned by its existing reactor state machine;
  render-domain candidate preparation advances asynchronously as one active
  candidate at a time.
- `finish_render_work` is a single barrier request whose completion is reported
  asynchronously. It is not an unbounded list of barriers.
- Cursor/material revisions can coalesce only under the revision and
  invalidation ordering rules above. Registry publication and renderer
  terminal operations remain lossless and ordered.

The reactor never waits when an admission limit is occupied. No unbounded
channel or queue is permitted.

## Phase 2B.0 Atomic slot ownership split (baseline before 2B.1)

Phase 2B.0 first represented the explicit Atomic composited output pool's two
future ownership domains separately while construction, rendering,
presentation, and teardown remained synchronous:

```text
RT reactor / DRM domain:
    AtomicOutputSlotBacking[]
    GBM BO lifetime and DRM framebuffer IDs
    swapchain and OutputTransactionLedger
    presentation/content-validity history
    buffer-age calculation and KMS scheduling

render-worker / EGL / GL domain:
    AtomicRenderSlot[]
    EGLImage, GL texture, and GL framebuffer
    EGL/GL context, GlesSceneRenderer, RendererResourceState, EffectRuntime
```

Pool setup exports one owned `AtomicRenderSlotDescriptor` per BO. Its
`DmabufBufferHandle` retains the exported plane descriptors through the
existing `DmabufPlane` / `Arc<OwnedFd>` ownership. EGL import happens when the
pool is created or replaced; normal frame rendering performs no DMA-BUF FD
export. The descriptor is compile-time qualified as `Send`. The render slot
does not retain a GBM BO, DRM framebuffer authority, or presentation history.

Each render target request contains the exact slot ID, pool generation, and
buffer age. The reactor calculates that age from the matching physical
backing's content-validity state, including after an unpresented render or a
confirmed presentation. `render_to_slot` resolves the render resource by the
same ID and generation before drawing. Pool validation checks identity pairing
across both slot arrays; array position is not pairing authority.

Initial Atomic bootstrap and output-configuration replacement both use
`AtomicEglGbmScanout::create_unattached_pool`, so they share the split model.
Session recovery preserves the current synchronous reuse/rebind behavior and
updates the generation metadata in both halves together after swapchain
ownership is rebound. A future worker implementation must deliver that
generation transition to the worker as an ordered pool-control operation or
recreate the pool. Direct scanout remains reactor/scanout-owned and does not
use these composited render slots.

Current teardown makes the EGL context current and finishes pending GL work,
then destroys every render slot's GL framebuffer, texture, and EGLImage before
removing any physical DRM framebuffer and dropping the GBM BOs. Revoked DRM
teardown still destroys all EGL/GL resources first and skips only `rm_fb`.
Phase 2B must turn this synchronous order into an acknowledgement boundary:

```text
worker proves AtomicRenderSlot generation G destroyed
        ↓
reactor may remove DRM framebuffer generation G
        ↓
reactor may drop GBM BO generation G
```

That acknowledgement must be nonblocking for the RT reactor during normal
operation. Slot identity, pool generation, and calculated buffer age form the
physical-to-render boundary for each future job; transaction and output
generation authority stays in its future `RenderJob` envelope. This split does
not change current rendering, fence creation, buffer-age policy, swapchain
ownership, or protocol settlement.

Every future `RenderJob` envelope must pair the frozen renderer input identity
with the physical output transaction it serves. Use source-grounded equivalents
of `frame_id`, `transaction_id`, `output_generation`, `pool_generation`, slot
identity, render generation, and `FrozenRenderSnapshotIdentity`. Keep this
physical authority in the job envelope, not in `FrozenNativeRenderFrame`; it
is needed to settle supersession, output-generation cancellation, shutdown,
reconfiguration, and session recovery exactly.

The fundamental invariant is:

> **The RT reactor never waits for the render worker.**

The reactor must not use a blocking receive, a mutex owned by the render worker,
a condition-variable wait, synchronous D-Bus, or a normal-operation `join()`.
Phase 2B.2 must not add a second prepared render job. The Atomic swapchain has
one active rendering slot, so the initial topology is:

```text
render worker:
    at most one executing RenderJob

reactor:
    render_worker_busy: bool
    render_demand: bool
```

While the worker is busy, compositor, input, and resize state continue to
advance on the reactor. Visual changes set `render_demand = true`; repeated
changes coalesce without allocating another slot, transaction, frame batch, or
frozen payload. When the executing job completes and normal settlement permits
another render, the reactor resolves the newest state, prepares one exact job,
clears the demand bit, and dispatches it. Configure/ACK protocol state
continues to advance and is never coalesced. Renderer terminal/control
operations are lossless and remain ordered independently of visual demand.

This is a latest-state render policy, not render-job supersession: visual states
that arrive while a job executes do not become prepared jobs of their own.

Each admitted job must have one terminal result, using existing
transaction/failure ownership where possible:

```text
Rendered
SkippedNoVisualChange
LifecycleFallback
SupersededBeforeGpu
CancelledForGeneration
RenderFailureBeforeGpu
RenderFailureAfterGpu
ShutdownAbandonment
```

`FrozenNativeRenderFrame` is compile-time qualified as `Send` without an
`unsafe impl`; `Sync` is not required for exclusive job transfer. This is only
a payload qualification. It does not establish worker queue ownership,
shutdown behavior, cancellation settlement, performance improvement, or
hardware qualification.

## Phase 2B.1 Atomic render ownership domain

Phase 2B.1 keeps execution synchronous and moves EGL/GL state behind one
`AtomicRenderDomain`. `AtomicEglGbmScanout` remains the reactor-side owner of
the physical output domain:

```text
AtomicEglGbmScanout:
    physical GBM device
    AtomicPhysicalOutputPool
    AtomicOutputSwapchain
    DirectScanoutControl and direct framebuffer cache
    DRM framebuffer IDs and content-validity history
    buffer-age calculation and KMS/presentation policy

AtomicRenderDomain:
    render GBM device backing its EGLDisplay
    EglInstance, EGLDisplay, EGLContext, glow::Context
    GlesSceneRenderer and renderer resource/effect state
    NativeFenceFunctions
    AtomicRenderPool (EGLImages, GL textures and framebuffers)
    output width and height
```

The KMS DRM fd is duplicated independently for the physical GBM device and
the render GBM device. Only the physical device allocates output BOs. The
render device supplies the native display for EGL and remains owned by the
render domain until after `eglTerminate`. It cannot be dropped before its
EGLDisplay.

The pool containers are separate. `AtomicPhysicalOutputPool` owns only GBM
BOs, DRM framebuffer IDs, and content-validity state. `AtomicRenderPool` is
private to `AtomicRenderDomain` and owns only EGL/GL slot resources. The setup
descriptor exports each physical BO's owned DMA-BUF planes once per pool
creation/replacement; it is naturally `Send` and retains its plane fds through
the existing `Arc<OwnedFd>` ownership. No descriptor or fd is exported during
frame rendering.

The reactor calculates `RenderTargetRequest { slot, pool_generation,
buffer_age }` from the physical pool and passes it with the
`FrozenNativeRenderFrame` to the render domain. The domain resolves its render
slot only by the matching ID and generation. It never reads physical slot
validity or swapchain state. Renderer execution, fence creation, capture,
finish barriers, lifecycle/squash queries, and renderer mutations all enter
through the domain. Terminal results return to the reactor for the unchanged
transaction, frame-batch, and KMS settlement.

Bootstrap and output-configuration candidates use the same independent-device
and split-pool constructor. Session recovery validates the current physical
and render ID/generation pairing, then rebinds the physical pool, render pool,
and swapchain synchronously. Phase 2B.2 must carry pool replacement or
generation rebind as ordered worker control; callers must not mutate render
slot generations directly. Direct scanout and its framebuffer cache remain
outside `AtomicRenderDomain`.

Teardown is explicit: make the EGL context current, finish GLES work, delete
render-pool GL framebuffers and textures, destroy EGLImages, drop
`GlesSceneRenderer` resources, unbind and destroy the EGL context, terminate
EGLDisplay, and then drop the render GBM device. Only after that completes may
the reactor remove physical DRM framebuffers and drop physical BOs. A revoked
DRM fd still skips `rm_fb`, while render resources are destroyed first. Phase
2B.2 must replace this synchronous ordering with a nonblocking worker
acknowledgement before physical teardown.

The initialized `AtomicRenderDomain` is not `Send`: the exact blocker is
`khronos_egl::Context`, which contains `*mut libc::c_void`. No unsafe trait
implementation is used. The future worker bootstrap instead transfers a
naturally `Send` payload containing only an owned render-device fd, dimensions,
the selected format/modifier, and the owned render-slot descriptors. The worker
constructs EGL and GL on its own thread. This test qualifies the setup payload,
not an initialized context handle.

## Freeze timing

When slow-cycle instrumentation is enabled, `render_snapshot_freeze_ns` records
the freeze duration in the existing native slow-cycle phase vector. It is a
nested detail of `output_transaction_preparation`, so it is reported but
excluded from the top-level phase sum to avoid double-counting.
