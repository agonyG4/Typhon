# Typhon current CPU, GPU, memory, and presentation audit

Audit date: 2026-09-07. Source baseline: `d68a29e0cc68ab4b7de4b6481db5cd9902585e69` (`main`, initially clean). This report changes no production code or tests. The user's latest instruction to commit supersedes the attached brief's no-documentation/no-commit restriction for this audit artifact only. Recommendations below are not implementations.

## Ranked result

The strongest remaining opportunities are copies of owned SHM pixels, effect work with no damaged consumer, and unbounded effect eviction bookkeeping. The current native GPU path is already fundamentally different from the June CPU-composition/GBM-write architecture. Do not undertake another renderer rewrite on that historical evidence.

The ordering weighs likely benefit, confidence, and correctness risk qualitatively; it is not a benchmark score. “Confirmed” establishes the mechanism, not a measured frame-time improvement.

| Rank | Candidate | Evidence | Expected resource benefit | Risk |
| --- | --- | --- | --- | --- |
| 1 | A1: consume effect eviction notifications instead of retaining/copying their history | Confirmed | Stops growing RAM and growing checkout copy cost | Low |
| 2 | B1: share immutable compositor-owned SHM snapshots | Confirmed copies; benefit workload-dependent | Potentially several full-image allocations/copies per publication; lower steady RAM | Medium |
| 3 | B2: prune effect subgraphs with no damaged consumer | Confirmed unnecessary captures/downsamples under stated trigger | GPU passes, bandwidth, CPU setup, transient resource demand | Medium; buffer-age repair is a prerequisite |
| 4 | A2: export presentation trace only when needed, outside the cycle hot path | Confirmed, opt-in | Removes repeated serialization and synchronous file replacement | Low/medium |
| 5 | B3: cache effect uniform locations with the program generation | Strongly supported | Removes repeated GL string lookups per pass | Low |
| 6 | C1: defer uploads/imports until the complete frame consumer set is known | Measurement required | Hidden/off-damage SHM upload and import work | Medium/high |
| 7 | C2: reduce graph and scene metadata reconstruction | Measurement required | Frame allocations and repeated traversals | Medium |
| 8 | C3: byte-aware cache trimming and optional third-slot allocation | Measurement required | Retained GPU backing and cold resources | Medium/high |

No evidence here establishes that changing O1 credit policy, removing TEST_ONLY, skipping synchronization, or shortening pageflip validation would improve the current workload safely.

## Evidence boundary and qualification

This is an end-to-end source investigation with targeted call tracing and existing deterministic tests. It is not exhaustive hardware qualification, an allocation profile, or proof that every branch in this large tree is free of defects. Unmeasured subsystems are explicitly identified below rather than assigned invented bottlenecks.

Codebase Memory project: `home-agony-GitHub-Typhon`, full index generation `2026-09-07T17:27:13Z`, recording complete and generation matching. Tier 2 verification was used. Graph searches located symbols, inbound/outbound traces established relevant seams, and exact source reads verified material behavior. Graph edges occasionally resolve generic methods to unrelated types; only source-confirmed relationships support findings. Relevant narrowed search pages were consumed. The broad scanout symbol inventory was used only for discovery, not an exhaustive negative claim.

Coverage was checked for 51 source paths and the renderer/effects/runtime/worker scopes, then separately for the trace configuration in `src/native_output/runtime/bootstrap.rs` after the watcher advanced to generation `2026-09-07T21:27:00Z`. All checked paths had matching metadata; two reported partial parses: `src/native_output/runtime/cycle_dispatch.rs:1289` and `src/native_output/runtime/presentation_cycle.rs:154`. Both lines and surrounding destructuring were read directly. No recorded gap is a best-effort signal, not proof of completeness. Historical documents were read as context, not graph evidence for current implementation.

Commands used `rtk`, with `rtk proxy` for exact source reads and literal searches. Existing `target/` and `target/debug/` were reused; no duplicate checkout or alternate target directory was created. Validation executed:

| Command | Result | What it establishes |
| --- | --- | --- |
| `rtk cargo test --locked --lib` | 2,166 passed, 2 ignored; 38.04 s test summary | Existing library deterministic regressions pass |
| `rtk cargo test --locked --bins` | 1,280 passed across 5 suites; 9.81 s test summary | Existing binary tests, including renderer/output fixtures, pass |
| `rtk git diff --check` and `rtk git diff --cached --check` | Passed before commit | Working-tree and staged audit whitespace validation |

These commands do not run all integration tests, native GPU comparisons, or release profiling. `/dev/dri` exists, but `pgrep -a oblivion-one` found no live Typhon process. No DRM master acquisition, VT switch, display takeover, GPU capture, suspend/resume exercise, or 165 Hz benchmark was performed. Device-node presence is not hardware qualification. CPU usage, GPU duration, actual VRAM residency, PSS, wake frequency, latency percentiles, and cross-device traffic are unmeasured.

Source references below are repository-relative and refer to the baseline commit. Line ranges are navigation aids; function names identify the evidence when later edits move lines.

## A. Concrete performance/resource defects

### A1. Effect eviction history grows forever and is copied on every checkout

**Classification:** Confirmed. **Resources:** RAM, CPU, memory bandwidth. **Frequency:** each eviction grows storage; every subsequent effect texture acquisition copies it, including cache hits.

**Evidence:** `src/egl_renderer/effects/resources.rs:103-135` stores `evicted_ids: Vec<u64>`; `evict_until_fits():278-297` appends IDs; `evicted_texture_ids():253-255` returns a clone. `EffectGlResourceCache::acquire():354-424` calls this accessor before checkout for its length and again afterward to enumerate only new IDs. Literal inspection finds no clearing or draining of this history. `metrics():257-276` uses its length as a cumulative counter. `execute_graph_passes()` and `ensure_pass_textures()` in `src/egl_renderer/effects/executor.rs:163-263` reach acquisition during rendered effect frames.

**Mechanism and trigger:** repeated size/format changes or budget pressure evict idle textures. The physical texture pool remains budgeted, but eviction metadata grows with lifetime churn. A later stable workload continues cloning all past IDs even when it only reuses textures. `cleanup_size_history()` does not reset that history.

**Scale:** after E evictions, the retained ID payload is `8E` bytes, excluding vector capacity. Two accessor clones copy approximately `16E` payload bytes per acquisition, plus allocation and read/write traffic. At E=100,000 this is 0.8 MB retained and 1.6 MB copied per checkout. At 10 checkouts/frame and 165 frames/s, that hypothetical history yields 2.64 GB/s of copied payload. This is a scaling example, not an observed eviction rate or bandwidth measurement.

**Correctness constraints:** each evicted GL texture must still be deleted exactly once; checked-out textures cannot be evicted; failed acquisitions must not lose pending deletion ownership. Preserve cumulative eviction observability separately.

**Recommended direction:** replace the historical vector with consumable pending-deletion IDs and a scalar cumulative eviction counter. Have the GL owner drain pending IDs at a defined success/error boundary. Merely returning a slice removes clones but leaves unbounded retention.

**Verification:** force thousands of budget evictions with existing pool fixtures; prove pending metadata empties after consumption, allocation/reuse/deletion counts agree, and live checked-out resources survive. Measure checkout allocations after warm-up and after a large prior churn history. The latter must not depend on lifetime eviction count.

### A2. Opt-in trace export rewrites the complete ring on every completed native cycle

**Classification:** Confirmed. **Resources:** CPU, RAM allocation, filesystem writes/syscalls, latency. **Frequency:** per cycle reaching its tail, including input-only cycles, when a trace path is configured.

**Evidence:** `src/native_output/runtime/cycle.rs:489-554` calls `flush_presentation_trace()` unconditionally at the ordinary cycle tail. It creates parent directories and calls `std::fs::write(path, export_jsonl())`. `src/native_output/presentation/trace.rs:206-323` stores a bounded ring and serializes every retained event into a new string. Default ring capacity is 4,096, capped at 65,536. `src/native_output/runtime/bootstrap.rs:734-738` independently reads `OBLIVION_ONE_PRESENTATION_TRACE_FILE`.

**Mechanism and trigger:** with that path configured, unchanged trace contents are repeatedly formatted and replaced synchronously on the compositor thread. The ring bound limits retained events, not total output bytes or time spent exporting. A path can also cause empty-file writes when event collection is disabled. No new wake is created by this function itself; it adds cost to existing wakes.

**Scale:** R retained events × average serialized length L × C cycles/s bytes written, not merely new events/s. For illustration, 4,096 × 100 bytes × 165 cycles/s is about 67.6 MB/s of logical writes. Actual filesystem/device traffic depends on buffering and is unmeasured. Input cycle frequency can differ from refresh rate.

**Correctness constraints:** preserve chronological event order, bounded in-memory capture, dropped-event counts, and shutdown export. Do not let a slow trace consumer block presentation or recreate the removed worker completion backpressure.

**Recommended direction:** explicit on-demand snapshots or a dirty-generation export at a bounded diagnostic cadence. If continuous streaming is required, use a separately bounded writer with observable overflow, not synchronous full-ring replacement. Keep the trace data useful.

**Verification:** injected writer counts must remain zero without configuration and must not grow for unchanged rings. Exercise input-only cycles, slow storage, shutdown, and writer failure; compare cycle-tail time with collection enabled but export disabled. Default users without a path receive no benefit from this change.

## B. High-confidence optimization opportunities

### B1. SHM surface clones duplicate complete pixel images

**Classification:** Confirmed copying mechanism. **Resources:** CPU, RAM, memory bandwidth, possible frame latency. **Frequency:** per SHM publication/scene refresh; also per frame in paths that clone selected surfaces.

**Evidence:** `src/render_backend/buffer.rs:280-317` defines cloneable `ShmBufferSnapshot` with an owned `Vec<u32>` inside cloneable `CommittedSurfaceBuffer`. `RenderableSurface` is cloneable (`src/compositor/surface.rs:222`). `PendingSurfaceBuffer::materialize_for_publication()` (`src/compositor/state_data.rs:1623-1655`) clones the previous committed buffer for a same-ID/same-size SHM update, then reads only damaged client pixels. `MaterializedSurfaceBuffer::to_renderable_surface():1696-1727` clones the buffer again. `refresh_active_scene_surface()` (`src/compositor/state/active_scene.rs:158-202`) clones the renderable surface into the active view; its tree variant does likewise. `native_frame_renderable_surfaces()` (`src/compositor/state/fullscreen.rs:568-591`) borrows the ordinary active slice but clones surfaces for solitary-fullscreen filtering.

**Mechanism and trigger:** these are deep pixel-vector clones, not just reference-count increments. Partial client damage saves SHM ingestion and GL upload bandwidth but does not make those whole-image clones partial. Fullscreen filtering can copy an unchanged SHM image simply to obtain a filtered list. DMA-BUF handles are a different representation and must not be charged this pixel-copy cost.

**Scale:** every 1080p snapshot clone allocates and copies 8,294,400 bytes. At 165 clones/s, one such copy is 1.369 GB/s of payload and roughly 2.737 GB/s of read-plus-write traffic before cache effects. Three coexisting copies occupy 23.73 MiB. Several copy sites are proven, but an exact copies-per-commit count depends on role, tree updates, and call path; it was not measured here.

**Correctness constraints:** a compositor-owned snapshot is required for early SHM release. Published, ready, pending, and presentation-owned content must stay immutable across later attachments. Do not retain readable released client backing or turn bufferless commits into client-memory rereads. Preserve same-object attachment identity, partial damage lineage, synchronized subsurface application, and XWayland/cursor semantics.

**Recommended direction:** share immutable owned pixel storage between current content, renderable state, and active/frame views. Use copy-on-write or an explicit new snapshot for mutations, retaining the mandatory client-to-owned copy. First remove representation-only clones; avoiding the previous-image copy for partial mutation is a separate problem and may still require a copy when readers exist.

**Verification:** reuse SHM lifetime and two-buffer/O1 regressions, then retain an old snapshot while publishing new damage and verify byte-for-byte isolation. Measure allocation bytes for small damage, full damage, two-buffer rotation, fullscreen filtering, cursor replacement, and 100 surfaces. Report PSS after warm-up and after resize, not only allocation counts.

### B2. Effects capture and downsample even when their output damage is empty

**Classification:** Confirmed under the stated trigger. **Resources:** GPU, CPU, bandwidth, transient VRAM demand. **Frequency:** per rendered frame with visible effects, including frames damaged elsewhere.

**Evidence:** `plan_effect_damage()` (`src/effects/damage.rs:239-282`) derives capture coverage from the visible effect region plus its footprint, independent of whether source damage intersects it. `compile_frame_execution_plan()` (`src/effects/render_graph.rs:396-477`) compiles every nonempty visible instance. `compile_instance():553-680` assigns that capture region to capture and downsample passes; upsample/composite damage can be empty. `execute_graph_passes()` (`src/egl_renderer/effects/executor.rs:163-244`) runs all non-composite passes without pruning by consumer need. `execute_capture():727-809` binds and clears the capture target with GL scissor disabled, then replays selected source commands. `execute_fullscreen_pass():811-950` draws downsample scissors from the full capture coverage.

**Mechanism and trigger:** update a window far from a static blur panel. Overall output damage is nonempty, so the renderer executes a frame; the panel's effect output damage is empty, yet its capture and downsample passes still execute. Upsample draws can be zero, but setup/acquisition still occurs. Even when the panel is partially affected, capture/downsample work covers its bounded capture domain rather than only the minimal backward dependency region.

**Scale:** for blur with p levels, up to one capture plus p downsample passes are unnecessary for a completely unaffected instance. For a full-resolution capture area A and unit processing scale, downsample pixel areas sum to less than A/3; the capture itself clears its full target and replays contributing surfaces. Actual texture read cost includes shader taps. For multiple panels this repeats per instance. Completely idle frames that never enter rendering are not charged this cost.

**Correctness constraints:** output repair for an older swapchain slot can require an unchanged effect even when current logical damage is empty. Pruning must use the final repaired output consumer region and propagate dependencies backward. Preserve blur halo, sRGB/linear boundaries, anchor ordering, target replacement, program parameters, transforms, and fallback. Pooled textures are scratch resources: reuse does not prove their previous pixels belong to this effect.

**Recommended direction:** first add consumer-driven graph pruning with buffer-age-aware dependency propagation. Then evaluate capture narrowing. Persistent blurred-content caching is a larger optional follow-up requiring source/stack/program/generation identity; do not simply retain an arbitrary pooled texture or lower blur quality.

**Verification:** force ages 1, 2, and 3; damage outside a panel, at halo boundaries, underneath an opaque cover, and during move/removal. Compare against full repaint on the GPU. Existing geometry/graph tests alone cannot prove pixel equivalence. Use pass counts and GPU timestamps; unaffected instances should execute no passes only where the repair proof permits it.

### B3. Effect uniform locations are repeatedly looked up during execution

**Classification:** Strongly supported. **Resources:** CPU/driver overhead. **Frequency:** per effect pass and parameter binding.

**Evidence:** `src/egl_renderer/effects/executor.rs:410-721`, `execute_capture():727-809`, and `execute_fullscreen_pass():811-950` repeatedly call `get_uniform_location`. `CachedShaderProgram` (`src/egl_renderer/effects/shader_cache.rs:76-88`) caches the program but not these locations. The draw path uses `lookup()` rather than compiling a shader each frame.

**Mechanism and scale:** several name-based GL lookups per capture/blur/composite pass, plus custom bindings. The calls and repeated work are visible; driver latency is not measured. This is a safer small candidate than replacing the shader cache or changing algorithms.

**Correctness constraints:** locations belong to one linked program/context generation, including inactive uniforms and custom shader bindings. Program replacement must invalidate them. Uniform values still need updating when parameters change.

**Recommended direction:** cache typed built-in locations and validated custom-binding locations alongside the linked program. Keep render lookup allocation-free and compilation at the existing publication boundary.

**Verification:** count lookups during warm steady-state execution; they should be zero after compilation/publication. Exercise relink, program eviction, missing uniforms, context recreation, and custom parameters. Profile before claiming a material frame-time gain.

## C. Architectural opportunities requiring measurement

Each entry below is deliberately not a confirmed performance defect.

| Finding / classification | Source, mechanism, trigger and frequency | Scale and recommended direction | Correctness and verification |
| --- | --- | --- | --- |
| C1. Consumer-aware upload/import; Measurement required; CPU/GPU/bandwidth | `src/egl_renderer.rs:606-623,1000-1126,1741-1760`: resources synchronize before damage skip/visibility planning. A geometrically occluded active surface can upload before its draw is rejected. Per admitted frame/changed surface. | Potentially 4WH bytes per hidden SHM upload, plus pack cost. Build the full output/effect/cursor consumer set first, then defer unused resources. Existing fullscreen filtering already removes some surfaces. | Occluded surfaces can still be backdrop inputs; upload history must accumulate all missed damage. Preserve acquire/release bookkeeping even when no pixels are sampled. Measure `shm_upload_bytes`, import counts, and hidden-surface workloads before investing. |
| C2. Repeated metadata planning; Measurement required; CPU/RAM | `src/native_output/runtime/frame.rs:23-130` builds scene metadata and clones it in `snapshot()`/identity calculation. `src/native_output/output/damage.rs:297-350` builds elements and a root map. `src/effects/render_graph.rs:396-477,553-575` rebuilds graphs/maps; peak-live analysis scans textures for each pass. Per rendered frame with effects; scene snapshots also at presentation planning. | O(passes × textures) statistics plus graph allocations; repeated O(surface-count) metadata work. Cache stable graph topology separately from changing damage/parameters; reuse scratch and frame-local derived scene metadata. | Exact presented/ready/submitted snapshots must remain distinct. Do not replace them with mutable current-scene state. Profile 1/100+ surfaces, deep trees, and many effects; inspect allocation bytes and counts. Native scene-history snapshots contain metadata, not SHM pixel images. |
| C3. Cache/slot capacity; Measurement required; VRAM/RAM | `src/egl_renderer.rs:1199-1314` allows four cached DMA-BUF imports per surface plus active resource; pool default is 64 MiB (`effects/resources.rs:13`). `atomic_egl_gbm.rs:479-490` allocates all three slots at creation. Transition/idle retention, not necessarily per-frame allocation. | Byte-aware idle trimming or a lazily realized third slot could reduce capacity. DMA-BUF cache pruning scans the global cache per switching surface, a possible O(active rotating surfaces × cache entries) CPU cost. | Imports alias client backing; never double-count them as copies. Do not evict outstanding GPU/KMS ownership. Lazy slot creation moves allocation to a timing-sensitive transition. Measure live bytes, hit rates, churn and first-triple-frame latency; keep current policy if cheaper overall. |
| C4. XWM association reconciliation; Measurement required; CPU | `src/xwayland/xwm/resize_runtime.rs:532-580` copies all completed associations then revisits them. Verified callers are association ingestion and X11 surface-serial observation (`api.rs`), not every arbitrary input event. | O(X11 windows) work per association-related event. Consider a changed-association queue only if traces show this is frequent. | Preserve generation, map serial, configure timeline, adoption deadlines and pending-resize processing. Profile many X11 windows and resize; do not label ordinary idle XWayland expensive from this loop alone. |
| C5. Pacing, validation and dispatch overhead; Measurement required; CPU/latency/idle power | `runtime/metrics.rs:414-536`, `runtime/cycle.rs:120-497`, `kms_worker/thread.rs:809-1171`: repeated bounded state checks, predictors, worker phases and syscalls. Per actual cycle/submission. | Measure cost by phase and reason before combining calculations. A hypothetical 100 microseconds is 1.65% of a 6.06 ms budget, but no such cost is measured here. | Preserve revalidation after input/prepare changes, exact worker predecessor and one-kernel-inflight semantics. Compare ReactiveDouble/O1 with matched workloads, readiness evidence and missed-target counts. Never remove guards to make a benchmark faster. |

## D. Things that look expensive but are justified

- **Compositor-owned SHM pixels:** required by early release. `release_materialized_shm()` (`src/compositor/state/surface_commits.rs:1394-1413`) completes release after materialization. B1 removes redundant copies, not the ownership boundary.
- **Three explicit slots:** three are physically allocated even under a two-credit policy. This avoids allocation when overlap becomes useful. It is a capacity tradeoff, not evidence of three queued display frames or a leak.
- **Ready/pending/current distinction:** `output_swapchain.rs` and `runtime/scene_history.rs:25-128` retain exact identities. Scene history caps submitted snapshots at three. Metadata snapshot copies are much smaller than SHM pixel clones and protect presentation-time damage authority.
- **Render fence export and FD duplication:** `src/egl_renderer/native_fence.rs:67-170` creates an EGL native fence, flushes commands, exports a submission FD, and duplicates timing ownership. `poll(...,0)` is a nonblocking readiness query, not a blocking wait. A GPU may still be executing after the CPU render call returns.
- **Teardown `glFinish`:** the inspected production GL finish is `AtomicEglGbmScanout::drop()` (`atomic_egl_gbm.rs:1644-1652`), explicitly before imported/scanout cleanup. It is shutdown cost, not a per-frame stall. No per-frame `glFinish`, GL readback, or blocking client-fence wait was found in the inspected renderer path; this does not rule out driver-internal stalls in uploads, EGL, allocation, or DRM ioctls.
- **Async userspace fence readiness:** `presentation_ready.rs:16-36,129-148` checks the render fence for async presentation and registers reactor readiness if needed. Ordinary synchronized submission can carry the input fence. Do not generalize the async gate into “CPU waits for GPU every frame.”
- **Worker TEST_ONLY and predecessor waits:** `kms_worker/thread.rs:865-949` tests when policy requires it; the worker waits for exact pageflip acknowledgement before advancing kernel ownership. Test reuse must depend on the complete validated state. `direct_validation.rs:40-99` deliberately distinguishes cursor content identity from atomic cursor position/plane identity.
- **Lossless completion queue:** `kms_worker/thread.rs:1436-1459` appends before eventfd notification, with no result-space wait. It lacks a fixed event-capacity limit, but bounded job admission and acknowledgement constrain ordinary production. Retrofitting blocking capacity would restore a teardown deadlock. This is not analogous to A1's never-consumed historical vector.
- **Bounded percentiles:** `src/native/adaptive_buffering.rs:785-806` bounds histories to 120 and uses stack scratch plus selection. The old heap-allocation objection does not apply. Ten u64 histories of 120 samples are only about 9.6 KB of sample payload.
- **Partial-repaint thresholds:** eight rectangles and a 75% area threshold bound command replay overhead. Full clears occur after a full repair decision; partial repair clears only each scissor (`egl_renderer.rs:1532-1595`). Removing clears needs proof about all pixels, including transparent areas and failed effects.
- **Occlusion:** `src/egl_renderer/geometry.rs:150-205` already walks front-to-back coverage, subtracts opaque regions, and stops when covered; it uses a fixed 32-piece region with a conservative overdraw fallback. Effect capture has a separate selected-command visibility path. “Add occlusion” is not a valid blanket finding.
- **Persistent geometry and shader resources:** scene/overlay VBOs retain capacity and upload only when dirty (`egl_renderer.rs:1768-1813`); scene command keys reuse stable geometry. Shader lookup is not per-frame shader compilation. Effects already pool textures, alias compatible non-overlapping lifetimes, downsample blur, and fuse compatible local stages.
- **Disabled perf logging:** `src/native_output/perf.rs:28-41` calls the field-building closure only when enabled and below the 50,000-record process cap. `metrics.rs:631` puts substantial formatting inside that closure. Enabled text logging can still distort measurements; do not remove always-on bounded counters because text mode is expensive.

## End-to-end frame and ownership reconstruction

```text
Wayland/XWayland attachment + committed surface metadata
  -> captured synchronization / pacing / synchronized-tree readiness
  -> publication
       SHM: copy into compositor-owned snapshot -> release client use
       DMA-BUF: retain identity + handle under GPU/KMS lifetime authority
  -> renderable surface state -> incremental ActiveScene view
  -> native frame scene, effects, damage evidence, admission
  -> acquire explicit output slot (exact pool/output generation)
  -> update textures/imports -> commands -> repair/visibility plan
  -> GLES draws directly into slot FBO
       optional effect captures -> blur/stages -> anchored composition
  -> exported render fence + rendered transaction/batch
  -> ready, or O1 ready-unbound while exact predecessor is live
  -> frozen primary/cursor assignment and validation base
  -> synchronous commit transport OR bounded worker job admission
  -> TEST_ONLY when required -> real KMS ioctl -> kernel-inflight
  -> matching physical pageflip -> presented damage/scene promotion
  -> callback/feedback and lifetime settlement -> safe reuse
```

Surface state, pacing, and protocol identity live in the compositor. GPU resources live in the renderer/import caches and explicit slot pool. The transaction, frame batch, fence, and cursor pins keep the required owners alive across render/submit/presentation. An import cache entry does not itself authorize client buffer release. GPU completion and KMS retirement are distinct events; Direct Scanout retains the client backing through physical scanout.

**Reactive double:** one slot is current, another can render/be ready/pending according to the pipeline. Submit the admitted frame with its fence and immutable target. Physical pageflip retires the prior scanout slot. The third allocated slot does not by itself grant render-ahead credit.

**Triple/O1:** with additional credit and a safe slot, render a successor while the predecessor is outstanding. Deferred O1 may be ready but physically unbound. `output_swapchain.rs:1385-1424` returns `WaitingForPredecessor` while the exact predecessor is still live; bind after its actual claim is available. Do not treat this required dependency as an avoidable GPU wait. An unchanged wait state should not generate a polling timer.

**Worker path:** one queued admission lane, executing/submitted ownership, sidecar freeze, conditional TEST_ONLY, real submit, `Submitted` publication, then acknowledgement wait. Timing records separate queue wait, planned/actual wake, pre-submit, ioctl, complete dispatch, target, and acknowledgement. Queue wait includes intentional scheduling residence and must not be interpreted as CPU busy time. The submit gate also coordinates session revocation; its serialization protects authority.

**Worker watchdog:** `thread.rs:1295-1389` anchors timeout to submit-return time and reports it once, then waits without periodic timeout polling. This is already the corrected implementation.

**Transport defaults:** `kms_worker/policy.rs:83-101` defaults to `off`; `scanout/direct_policy.rs` likewise defaults Direct Scanout to off, with `experimental-auto` opt-in. A profile of the default transport cannot establish worker benefit. A fullscreen game is not automatically direct-scanned-out.

**Suspend/recovery:** output-generation changes invalidate assumptions about presented planes, slot lineage, and imported ownership. Re-reading uncertain presented state, retaining quarantined ownership, and repairing after recovery are correctness work. No session-cycle memory growth was measured. A new generation can temporarily coexist with resources that are still legally pinned; elapsed lifetime alone is insufficient evidence of leakage.

## Damage effectiveness and GPU work by workload

Surface journals retain commit-sequenced damage (`src/compositor/surface.rs:42-141`), then render elements transform it into output-space evidence (`native_output/output/damage.rs:297-350`). Logical damage describes changes since presentation; repair damage additionally reconstructs the acquired slot. KMS/pageflip acceptance advances the presented lineage; a rendered or rejected candidate does not.

| Promotion / condition | Meaning and classification |
| --- | --- |
| Buffer size/transform/mapping change or lost surface history | Missing correspondence requires conservative surface damage; do not preserve small damage across an unproved mapping |
| Scene changed without usable damage authority | `egl_renderer.rs:664-688` resolves contradictory empty evidence conservatively; instrument why authority was lost rather than accepting empty damage |
| First frame, invalidation, unknown/zero/invalid age, insufficient history | `egl_renderer/damage.rs:535-610`; necessary recovery behavior, potentially expensive only if frequent |
| Forced-full or partial repair unavailable | Capability/configuration fallback; inspect active backend before diagnosing damage failure |
| More than eight repair rectangles or at least 75% repair area | `damage.rs:633-665`; performance heuristic, requires workload measurements before tuning |
| Blur/effect footprint | Local damage expansion is necessary for sampling. Full capture/downsample of an unaffected instance is the distinct B2 opportunity |
| Rectangle bounding/coalescing | Renderer output regions coalesce; effect regions retain rectangles until their bound and may expand to a bounding region. Sparse/disjoint damage and overlapping effect scissors merit measurement; do not assume all region implementations have identical normalization |
| Rejected render, Direct Scanout transition, output generation reset | Preserve full-repair fallback until new slot contents are proven |

Partial rendering is real: each repaired scissor clears that region, rejects commands outside it, and applies opaque coverage. It is not just damage metadata passed to KMS. The CPU command walk repeats per scissor, but at most eight partial scissors survive the planner. Geometry uploads are dirty-gated. Texture synchronization precedes these savings, explaining C1.

| Workload | Current expected GPU path; remaining uncertainty |
| --- | --- |
| Unchanged desktop with no protocol demand | Runtime admission should stay quiescent; if renderer is entered with valid empty repair it skips. Verify real wake counts; source cannot supply idle watts |
| Small surface damage | SHM pack/subupload or DMA-BUF reuse; buffer-age repair scissors; bounded draw culling. B1 can dominate CPU copies even when GPU damage is small |
| Browser scroll/video | Changed client area may legitimately cover most of the viewport. DMA-BUF avoids CPU pixel ingestion; identify actual client buffer type before applying SHM estimates |
| Window move | Old/new visual bounds need repaint; existing textures can be reused. Coverage, shadows/decorations, and exposed lower content determine GPU work |
| Window resize | New backing dimensions can require new SHM texture/import; old/new scene bounds and repair history matter. SHM copies, client production, and allocation churn are separate costs |
| Fullscreen game | Solitary-tree culling reduces scene content; composed path still draws client texture into output slot. Direct Scanout is conditional and opt-in |
| Blur/shaders | Captures, down/up chain or fragment stages, then ordered composition; targets are region-sized and pooled. B2, B3 and graph CPU planning remain candidates |
| Software cursor | Old/new cursor damage plus affected scene repair; latest useful cursor state does not imply a full-output repaint per input sample |
| Hardware cursor | Cursor-plane update/piggyback can avoid scene rendering. Image changes may upload a cursor BO; position-only movement is not a full-output upload |
| Direct Scanout candidate | Validate complete plane assignment, synchronization, format/modifier and ownership; accepted scanout can bypass composition. Rejection must compose correctly and preserve feedback |

## RAM and VRAM ownership model

These are nominal uncompressed payload estimates, not measured GPU residency. GBM pitch/alignment, modifiers/compression, driver objects, imported client memory placement, and allocator overhead alter totals. An EGLImage, GL texture view, FBO and DRM framebuffer naming the same BO are not four pixel allocations. Shared-memory GPUs do not have a clean physical RAM/VRAM separation.

Let B=4WH for a 32-bit image; float intermediates use 8WH. Refresh rate changes bandwidth and useful buffering policy, not bytes per image.

| Resolution | One B image | Two images | Three explicit slots | One full-size RGBA16F |
| --- | ---: | ---: | ---: | ---: |
| 1920×1080 | 8,294,400 B / 7.91 MiB | 15.82 MiB | 23.73 MiB | 15.82 MiB |
| 2560×1440 | 14,745,600 B / 14.06 MiB | 28.13 MiB | 42.19 MiB | 28.13 MiB |
| 3840×2160 | 33,177,600 B / 31.64 MiB | 63.28 MiB | 94.92 MiB | 63.28 MiB |

| Resource and owner | Count / size / lifetime / reuse |
| --- | --- |
| Explicit output slots: `AtomicEglGbmScanout`, `AtomicOutputSlot` | Three full-output BOs allocated in `atomic_egl_gbm.rs:479-490`, with one EGLImage, texture, GL FBO and DRM FB per slot (`output_slot.rs:29-38`). Allocated at pool creation, reused through exact slot state, destroyed at safe pool teardown. Ordinary window resize does not resize the monitor pool |
| Effect textures: `EffectGlResourceCache` | Region-sized RGBA8 for encoded space or RGBA16F for linear intermediates; default 64 MiB payload budget. Logical graph resources alias physical targets according to live intervals. Returned textures remain reusable; pressure evicts idle ones. Output resize/deactivation does not itself imply immediate trimming; explicit cleanup exists. Count varies with key/size/pass overlap |
| DMA-BUF surface imports: renderer | One active import per rendered surface, up to four cached inactive imports per surface. Reused by buffer/layout identity; weak-lifetime cleanup and absent-surface cleanup happen during synchronization. They can retain driver references/backing but do not imply a new image-sized copy. No global byte budget was established |
| SHM texture: renderer | One active uploaded image per represented surface, recreated on dimension/source change, reused with partial uploads otherwise. Inactive scene removal destroys the resource on the next synchronization. CPU copies remain separately owned |
| SHM owned pixels: compositor | Each snapshot owns B bytes; current/materialized/renderable/active clones can duplicate them (B1). Pending synchronized/unassigned client leases have a distinct lifetime. No constant global maximum for client-provided surface count/size is established here |
| Upload scratch: renderer | `texture_upload_rgba` is reused and can retain its largest packed upload capacity. Approximately B for the largest full uploaded SHM surface; not an unconditional output-sized CPU composition buffer |
| Background/decoration textures: renderer | `egl_renderer.rs:908-998` keeps 1×1 textures for frame colors and decoration solid colors, and RGBA images at asset dimensions. Decoration keys deduplicate assets/colors and remove stale entries during synchronization. They are not per-window full-output render targets. Required-key sets/maps are rebuilt per frame, a small C2 profiling candidate |
| Cursor: `AtomicCursorResources` | Current plus optional theme/client caches, with retired resources until pins/current references allow `retire_safe()` (`cursor_buffer.rs:160-230`). Nominal 64×64×4 is 16 KiB per image; actual size follows cursor capability/image. Pins reference storage rather than duplicate pixels |
| Direct Scanout framebuffer cache | Capacity 64 (`scanout/direct.rs:350-432`), weak buffer identity plus `Arc` framebuffer ownership; idle LRU eviction, refusal when all entries are live. Client buffers remain their own allocations; current/queued/kernel uses extend necessary lifetime |
| Scene history | One presented, one ready, up to three submitted metadata snapshots (`runtime/scene_history.rs:25-128`). Surface bounds/damage/IDs, decorations and signatures, not full pixel buffers (`output/damage.rs:275-294`) |
| Damage/pacing histories | Output repair history bounded at eight; adaptive samples bounded at 120 each. Surface journal is capacity-bounded. Rectangle payload and client counts still affect aggregate size |
| Presentation trace | Configurable 1–65,536 events, default 4,096 (`presentation/trace.rs:206-254`); reserve exists even for disabled ring construction. Bounded observability memory; A2 is repeated export cost |
| Worker jobs/completions and synchronization | Jobs retain transaction/fence/cursor/direct ownership until settlement; completion events are drained, not a historical log. Explicit-sync registries retain outstanding waits, with cancellation/readiness retirement. Do not compare their lifetime with an unrelated frame timer |
| Shader/program resources | Bounded cache with compile metadata; driver binary size unmeasured. Compile/publication/context creation is transition/startup work, while B3 concerns repeated uniform lookup |
| Compatibility CPU backend | `scanout/gbm_cpu.rs:4-18,83-107` still has staging and three writable BOs. Its CPU composition/staging costs are fallback-specific, not normal explicit EGL/GBM work |

For unit-scale p-level blur, the capture is roughly 4A bytes and individual linear down/up levels use `8A/4^level`. Do not sum every logical graph texture to estimate peak physical allocation: lifetimes alias. Multiple effects may keep outputs live until their anchors are composited, increasing peak demand. A full-size 4K RGBA16F target alone is 63.28 MiB, nearly the entire default effect budget; fallback under some 4K graphs is therefore plausible, not a measured failure. Use actual resource metrics and graph lifetimes before changing the budget.

**Transition peaks:** window resize may overlap old/new SHM snapshots and imported images; effect size changes can retain old cached sizes until pressure; fullscreen filtering can temporarily clone SHM; Direct Scanout fallback keeps the compositor pool and required client scanout owners; renderer/output generation changes can overlap safe-retirement owners. Cursor retirement explicitly checks pins. No current stale-generation leak is proven by this audit. Measure live owners by generation before treating deferred cleanup as waste.

## Bandwidth and 165 Hz scale

For 1080p B=8,294,400 bytes:

| Full-image operation rate | One image payload/s | Approximate copy read+write/s |
| --- | ---: | ---: |
| 60 Hz | 0.498 GB/s | 0.995 GB/s |
| 120 Hz | 0.995 GB/s | 1.991 GB/s |
| 165 Hz | 1.369 GB/s | 2.737 GB/s |

These use decimal GB. A GPU pass with one full-size read and write has a similar nominal scale, but blending, taps, caches, compression and partial coverage change real memory traffic. CPU ARGB-to-RGBA packing adds read/write traffic before upload. PCIe traffic is not equal to all CPU copies: only actual transfers across the device boundary count. No cross-device copy rate or GPU readback was measured. At 165 Hz the physical period is 6.060606 ms; source CPU duration alone excludes GPU execution and presentation waiting.

## Idle, input, XWayland, and telemetry conclusions

`NativeRuntime::run_cycle()` classifies ready domains, gates XWM work, separates Wayland read-side dispatch from input, gates acquire/prepare, and gates presentation. It rechecks state after potentially changing phases. `build_native_wake_plan()` (`runtime/wake_plan.rs:280-366`) selects owned deadlines and explicit continuation reasons, rather than imposing a refresh-period wake on an empty desktop. `current_scheduler_wake_deadline()` maps buffer/pageflip waits to external readiness plus a real watchdog, not expired render-target polling. This supports an idle fast path; real quiescence remains a counter-based qualification task.

Explicit-sync eventfd watches and fallback watches are distinct (`src/native/explicit_sync.rs`). Eventfd support removes ordinary periodic polling; unsupported/broken registration can activate timed fallback while work is outstanding. Count fallback watches before attributing idle wakeups to scheduler policy. Pointer-only hardware-cursor work should avoid primary rendering, while software cursor work damages the old/new cursor region. Keyboard-only input should primarily dispatch protocol events; focus, decorations, shortcuts and client responses can legitimately create visual work.

XWM's drain loop separately budgets events and property replies (`xwm/api.rs:233-285`); its reactor stream uses a bounded output queue and zero-time poll returning WouldBlock (`xwm/connection.rs:94-156`). The presence of trait methods named `wait_for_reply` does not prove a hot-path blocking round trip. Property/adoption/resize latency and flush frequency need actual traces. Association reconciliation is C4, with its verified limited trigger. The cycle tail now requests continuation for pending managed XWayland commands, so the recent dropped-continuation finding is already addressed.

Use existing counters first: renderer statistics include SHM upload bytes, import/reuse/eviction counts, cache entries/peak, repair reason and age, scene/VBO rebuilds, command visits/rejections/draw calls, scissor passes, effect graph/resource statistics and fallback reasons. Runtime metrics expose resource-efficiency service gates; adaptive history exposes render/wake/dispatch/service timing; the worker records queue residence, TEST_ONLY, pre-submit, ioctl, dispatch and pageflip acknowledgement. Presentation tracing records transaction event times. Add only missing measurements such as pixel clone bytes or per-pass GPU time, rather than another global trace framework.

## E. Historical issues and current classification

| Historical source / issue | Current classification | Current evidence / qualification |
| --- | --- | --- |
| June `agent-pulse-native-cpu-gpu-bottlenecks.md`, `native-gpu-path-gap-analysis.md`: native CPU scene + full GBM write | Obsolete due to architecture changes for explicit GPU output; still present in compatibility backend | Direct rendering into imported slot FBOs vs explicit `gbm_cpu.rs` staging. June timing numbers are not current measurements |
| June `native-repaint-damage-bottlenecks.md`, `agent-gauge-resize-followup-perf.md`: small damage cannot save normal scanout writes | Obsolete for normal GPU output; partially present as fallback cost | GPU scissor/visibility path; copy estimates apply only where CPU/SHM copies actually remain |
| August 15 atomic rendering closure: partial repair incorrectly coupled to EGL swap-damage | Already fixed | Separate capabilities and `PartialRepaintPlanner`; explicit FBO target does not need EGLSurface swap-damage support |
| August 15 closure: another pending slot destroys buffer age | Already fixed | `AtomicOutputSlot::buffer_age()` derives age from its own presented serial; confirmed presentation commits history |
| August 25 resource-efficiency design: all domains service on pointer wakes | Partially present as necessary state checks, broad unconditional service already fixed | Current operation-plan gates in `run_cycle`; exact pointer hit-testing and due work still have costs requiring measurement |
| August 25 design: diagnostic formatting paid before disabled logger | Already fixed at inspected perf logger seam; unknown for every logging call site | Lazy closure in `NativePerfLogger::log`; no repository-wide claim that all diagnostics are free |
| August 28 SHM lifetime design: SHM release delayed to presentation | Already fixed in materialized publication path | `SafeShmRelease` and `release_materialized_shm`; deep owned snapshot copies remain B1 |
| Prior SHM clone concern noted as non-goal in August 15 design | Still present | Owned `Vec<u32>` and clone sites in B1 |
| September 4 v3.1: live Deferred O1 predecessor misclassified stale | Already fixed | `WaitingForPredecessor` branch in `output_swapchain.rs:1385-1424` |
| September 7 audit-fixes design: fence timing lost after FD consumption | Already fixed | Stored `fence_timing_evidence`, `sample_fence_timing()` and one-shot accounting (`output_swapchain.rs:183,220-245`) |
| September 7 audit-fixes design: dispatch training excludes retries/complete dispatch | Already fixed | `thread.rs:995-1006` records complete wake-return-to-success dispatch duration |
| September 7 audit-fixes design: watchdog reset by notifications | Already fixed | Absolute submit-return deadline and one-shot timeout |
| September 7 completion-lane design: worker blocks behind full result queue | Already fixed | Lossless queue publication without result-space wait |
| September 7 audit-fixes design: presentation trace export cost | Still present, opt-in | A2 |
| Historical 165 Hz, NVIDIA, mouse/resize performance comparisons | Unknown without runtime measurement | No current native matched workload was captured; old session logs cannot qualify this revision |

## Current upstream comparisons

Consulted 2026-09-07 via moving upstream branches; immutable upstream revisions were not pinned, so these are design references, not reproducible performance baselines.

- KWin's `GLTexture::update()` uses capability-dependent BGRA uploads and pixel-store row length/skips for damaged rectangles, then restores pixel-store state. Typhon already supports partial uploads but repacks ARGB into RGBA scratch. A capability-gated no-repack upload is a possible later B1-adjacent experiment after copy profiling; preserve channel order and fallback. [KWin source](https://raw.githubusercontent.com/KDE/kwin/master/src/opengl/gltexture.cpp)
- Hyprland's rectangle blur path checks empty damage, intersects the box, and selects a precomputed blur only under its explicit xray/provider condition. This illustrates that blur reuse depends on semantics, not just a texture cache. It supports investigating B2 without importing Hyprland's assumptions. [Hyprland source](https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/render/OpenGL.cpp)
- Aquamarine's swapchain avoids reconfiguration for unchanged options and adjusts length separately when size/format remain compatible. This is useful context for C3, but its rotation-based age model is not a replacement for Typhon's exact presented-slot lineage. The first attempted `src/swapchain/Swapchain.cpp` URL failed; the actual allocator source was retrieved. [Aquamarine source](https://raw.githubusercontent.com/hyprwm/aquamarine/main/src/allocator/Swapchain.cpp)

## Workload-specific priorities

| Workload | Most useful next investigation |
| --- | --- |
| Idle desktop | Confirm no primary renders/submits, owned wake reasons, fallback watches and trace-path configuration. Do not blame GPU composition before proving it runs |
| Normal interactive desktop | SHM clone bytes for actual shell/apps; scene metadata work only after copies are separated |
| Browser scrolling | Determine SHM versus DMA-BUF, upload/import reuse, client damage size, repair area and CPU/GPU split |
| Window movement | Old/new bounds repair, exposed content, effect captures and whether image-only snapshots are copied unnecessarily |
| Window resize | SHM snapshot allocation/copy, client buffer-size churn, retained imports/effect sizes, then client/configure latency |
| XWayland application | Event/reply budgets, association/resize timing, actual buffer type; no generic XWayland penalty asserted |
| Fullscreen game | Composed GPU pass cost, client readiness and physical target misses; O1 policy only after these are known |
| Fullscreen Direct Scanout candidate | Explicit opt-in, rejection reason and test-cache hit/miss, fallback and lifetime counters; no default bypass assumed |
| Blur/effects-heavy desktop | A1 long-session churn, B2 passes with no consumer, B3 GL lookups, graph peak-live budget and CPU compile work |
| 165 Hz | Same workload with a 6.06 ms budget; separate CPU render, GPU completion, worker residence, ioctl duration and actual presentation. No FPS gain claimed |

Pathological cases should include 100+ surfaces, deep subsurface trees, more than eight sparse output damage rectangles, many effect-enabled windows, overlapping blur regions, repeated resize/fullscreen changes, animated layers, high-rate pointer input, clients faster/slower than refresh, CPU/GPU saturation and KMS backpressure. The source mechanisms above predict what to inspect; they do not establish the dominant bottleneck for every case.

## Incremental implementation shortlist

| Order | Candidate | Benefit | Risk / complexity | Required measurement | Dependencies |
| --- | --- | --- | --- | --- | --- |
| 1 | Consume eviction IDs, preserve scalar count (A1) | Bounded RAM and constant-history checkout cost | Low / small | Churn and post-churn checkout allocation bytes | None |
| 2 | Change trace export policy (A2), if used | Removes cycle-tail I/O amplification | Low-medium / small | Writes per unchanged ring; slow-writer cycle latency | Preserve diagnostic contract |
| 3 | Share immutable SHM snapshots (B1) | Large copy/allocation and RAM reduction | Medium / medium | Bytes copied by publication, active refresh and fullscreen | Existing SHM/O1 isolation tests |
| 4 | Cache effect uniform locations (B3) | Removes repeated driver lookups | Low / small | Lookup count and CPU profile | Program generation invalidation |
| 5 | Prune effect consumers using repair demand (B2) | Avoids capture/downsample work | Medium / medium-large | GPU pass times and pixel equivalence across ages | Backward dependency/repair proof |
| 6 | Reuse graph topology and frame-derived metadata (C2) | Fewer CPU allocations/traversals | Medium / medium | Allocator/call profile under many surfaces/effects | Stable keys; keep dynamic damage separate |
| 7 | Defer invisible texture work (C1) | Avoid unused upload/import bandwidth | Medium-high / medium-large | Hidden SHM/import workload; reuse when exposed | Complete effect/cursor consumers and missed-damage accumulation |
| 8 | Evaluate byte-aware idle cache trim / lazy third slot (C3) | Lower retained GPU backing | Medium-high / medium | Actual residency, hit rate, transition latency | Safe ownership and generation retirement |

Keep each change independently reviewable. A1 requires no scheduling changes. B1 must not weaken SHM release semantics. B2 must not use logical damage alone to skip work needed for buffer-age repair. C4/C5 remain profiling tasks, not implementation commitments. The existing ownership, synchronization, and fallback architecture should remain the foundation.
