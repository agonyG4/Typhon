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

Only one extra `RenderableSurface` clone is made for an optional client cursor.
`RenderableSurface` cloning keeps SHM pixels in their existing `Arc<Vec<u32>>`
and DMA-BUF planes in their existing `Arc<OwnedFd>` handles. The owned scene is
moved into the frozen object; its scene vectors, effects, decorations,
presentation samples, and lifecycle values are not cloned to establish the
boundary.

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
    one replaceable pending RenderJob

render worker:
    EGL and GL contexts
    GlesSceneRenderer, EffectRuntime, RendererResourceState
    render-side GL/EGL output resources
    exactly one executing RenderJob

KMS worker:
    timed DRM/KMS submission
```

The fundamental invariant is:

> **The RT reactor never waits for the render worker.**

The reactor must not use a blocking receive, a mutex owned by the render worker,
a condition-variable wait, synchronous D-Bus, or a normal-operation `join()`.
There is no unbounded render queue: at most one job executes, and at most one
replaceable pending job remains owned by the reactor. A pending physical render
job may be superseded before GPU ownership starts; configure/ACK protocol state
is never coalesced. Resize supersession must settle the displaced job before
the newer job replaces it.

Each job must have one terminal result, using existing transaction/failure
ownership where possible:

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

## Freeze timing

When slow-cycle instrumentation is enabled, `render_snapshot_freeze_ns` records
the freeze duration in the existing native slow-cycle phase vector. It is a
nested detail of `output_transaction_preparation`, so it is reported but
excluded from the top-level phase sum to avoid double-counting.
