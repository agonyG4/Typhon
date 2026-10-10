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
    one replaceable pending RenderJob

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
(`EXPLICIT_OUTPUT_SLOT_CAPACITY == 3`), and its invariants prevent slot-role
aliasing or ownership beyond that capacity. This supplies a source-grounded
bound of three outstanding output-frame renderer tokens, provided Phase 2B
keeps a slot in a renderer-control-pending role until the worker acknowledges
that token's single terminal operation. A proposed terminal lane therefore has
three fixed records, one per occupied slot. When all are occupied, the reactor
does not wait: it leaves its one pending render replaceable and admits no job
that needs another slot. A `CommitPresented` or `DiscardRendered` consumes the
record belonging to that frame, never a record selected by “newest render.”
This three-slot proof covers output-frame renderer tokens only. The current
capture path is slotless and immediately discards its renderer commit; if
capture moves to the worker, Phase 2B must keep it outside this lane or give it
separate bounded admission and terminal ownership.

The bound does not by itself bound arbitrary reload requests or prove safe
configuration coalescing. Phase 2B may use revisioned fixed latest-value slots
for cursor and material configuration, retaining each revision referenced by
an admitted job and carrying invalidation across coalesced updates. Registry
generation publication remains ordered while GL compilation can fail; skipping
an intermediate successful publish before a later failure would change the
active generation. The worker must apply all operations in sequence order
relative to terminal controls and jobs. Trusted-registry reload requests still
need a bounded request/result policy; the source reviewed here provides no
renderer-control admission limit to reuse. Slotless capture also needs separate
bounded admission if it moves to the worker. Until those policies and the
revision barriers are designed and verified, complete control lane boundedness
remains a Phase 2B design blocker. No unbounded channel or reactor wait is
acceptable.

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
There is no unbounded render queue: at most one job executes, and at most one
replaceable pending job remains owned by the reactor. A pending physical render
job may be superseded before GPU ownership starts; configure/ACK protocol state
is never coalesced. Resize supersession must settle the displaced job before
the newer job replaces it. Renderer terminal/control operations may not be
dropped when a newer render arrives.

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
