# Typhon Documentation Architecture Study

> Historical architecture study.
> This document describes the source snapshot identified below and is not the
> source of truth for current runtime defaults, feature maturity, or
> qualification status.

**Study date:** 2026-09-24  
**Authoritative source snapshot:** `Typhon-source(20260923-213927).zip`  
**Historical supporting baseline:** `TYPHON_ARCHITECTURE_FEATURES_READINESS_AUDIT_2026-09-14(1).md`  
**Repository mutation:** none  
**Source snapshot SHA-256:** `9fd5991fece906dcae4d91d75eee1bbdf9d730b052d8be656763c3a5fd69d56e`  
**Primary inspection method:** Codebase MCP index of the extracted snapshot, followed by direct source reads for central producers/consumers and all graph-ambiguous paths.

> This is an architecture-recovery study, not a feature audit. The central question is not “what files exist?” but “which object owns each truth, how that truth moves, what evidence completes it, and why the implementation has been shaped around exact ownership.”

## Evidence posture and status vocabulary

The source snapshot is authoritative. Historical documentation is used only to recover context or identify design drift. A definition, protocol XML file, test, capability bit, or old design document is **not** treated as product wiring by itself. Central abstractions were followed from producer to consumer, and where tests exposed code that had no production caller the report classifies that code as an inactive foundation rather than support.

The report uses these labels deliberately:

- **Implemented:** production-capable code exists.
- **Product-wired:** the real startup/runtime path constructs and calls it.
- **Enabled by default:** ordinary startup selects it without opt-in.
- **Optional:** product-wired but gated by policy/configuration/environment.
- **Experimental:** implemented and callable, but intentionally not part of the qualified default contract.
- **Hardware-qualified:** the repository contains current evidence for the target native hardware path. This is narrower than deterministic tests.
- **Deterministically tested:** state-machine or pure/runtime-model tests exercise the invariant without claiming hardware qualification.
- **Documented only:** documentation describes a behavior not proven in the current production path.
- **Planned:** catalog/protocol/design names exist, but executable implementation does not.
- **Inactive foundation:** implementation or model code exists, but a required production ingress/egress caller is absent.
- **Historical:** useful context that no longer describes the current implementation.
- **Unknown:** intent or behavior cannot be established from the supplied snapshot.

Codebase MCP indexed the extracted snapshot at approximately 30.6k nodes and 225.5k relationships. It reported two one-line partial parse ranges (`src/native_output/runtime/cycle_dispatch.rs:1653` and `src/native_output/runtime/presentation_cycle.rs:200`); claims touching those files were confirmed from direct text reads instead of relying on graph completeness.

---

## 1. Executive mental model

Typhon is easiest to understand as a sequence of **authority domains** joined by typed evidence, rather than as one mutable compositor object that gradually accumulates “the current frame.”

At the semantic end, `CompositorState` is the canonical authority for client-visible and compositor-owned meaning: surface state, window identity, workspace membership, focus, pending/committed protocol state, semantic effects, animation intent, selection state, and the canonical scene. Wayland requests generally enter **pending protocol state** first and become semantic truth only at a commit/publication boundary. Synchronized subsurfaces do not create an independent compositor truth; their cached commits are folded into a single surface-tree transaction authority.

At the presentation end, Typhon refuses to equate semantic truth with pixels. A semantic scene can have a presentation intent, a renderer can produce evidence that pixels were generated, a KMS transaction can be submitted, and only a matching pageflip can establish physical presentation truth. Those stages have distinct identities and owners. `PresentationEngine` owns property-level presentation transactions and revisions. `OutputTransactionLedger` owns native output submission lifecycle. `NativeSceneHistory` separates rendered/submitted scene evidence from the physically presented predecessor. `PresentedPlaneSnapshot` represents physically promoted plane state. A pageflip token and output/DRM generation determine whether native work is allowed to become current physical truth.

This separation explains much of Typhon's apparent strictness. A frame may still be the same physical object while scheduling metadata changes; therefore `OutputFrameKey` excludes mutable target state and instead binds stable physical fields such as logical output, frame, protocol batch, output transaction, slot, framebuffer, render generation, and pool generation. An animation may be mathematically finished but still physically visible; therefore the Presentation Engine does not retire the track until the exact transaction/revision is acknowledged from a physically presented frame. A stale KMS pageflip may arrive after session recovery; therefore generation and token validation prevent it from completing new ownership. A future KMS worker is useful only if it does not become a hidden queue; therefore the atomic lane is explicitly bounded to one kernel-submitted commit plus one worker-queued-next commit.

Typhon's architecture is consequently organized around five recurring questions:

1. **Who owns this state?** There should be one canonical authority for a concept.
2. **What exact identity does this work belong to?** Generations, transaction IDs, revisions, pageflip tokens, buffer IDs, pool generations, and logical output IDs are not interchangeable.
3. **What evidence proves completion?** Mathematical convergence, render completion, successful submission, and physical presentation are different events.
4. **What invalidates stale work?** Session generations, surface lifetimes, XWayland generations, selection generations, and transaction supersession explicitly terminate old ownership.
5. **What happens when an optimization cannot prove safety?** The common answer is a bounded, observable fallback: composition instead of Direct Scanout, synchronous KMS instead of an unavailable worker, software cursor instead of hardware cursor, conservative/full effect repaint instead of under-repair, or terminal rejection rather than silent obligation loss.

The current product remains a **single physical output compositor** even though it now has a real typed `OutputId` foundation. The native runtime owns one selected connector/CRTC/mode pipeline. This distinction matters: internal identity work is active and useful, but it is not evidence of a multi-output product architecture.

The architecture is also intentionally asymmetric between the shell and compositor. Window geometry, workspace membership, tiling state, focus, presentation ownership, protocol correctness, and physical output state are compositor authorities. The shell may provide user-facing policy and UI, but it is not a second compositor state store. X11 follows the same principle: XWayland/XWM adapts X11 into canonical Typhon window and selection models instead of becoming a second compositor.

### Source anchors

- `src/compositor/mod.rs` — `CompositorState`: central semantic state owner; fields include surfaces, scene, workspaces, selection, protocol state, animations, retained lifecycle payloads, and publication generations.
- `src/compositor/state_data.rs` — `SurfaceData`: per-`wl_surface` pending/double-buffered protocol state before semantic publication.
- `src/compositor/state/surface_transactions.rs` — `PendingSurfaceTreeTransaction`, `SurfaceTreeTransactionId`: semantic tree-transaction identity and readiness dependencies.
- `src/presentation_animation/engine.rs` — `PresentationEngine`, `sample`, `acknowledge_presented_geometry`, `acknowledge_presented_opacity`, `acknowledge_presented_clip`: property presentation authority and exact physical ACK retirement.
- `src/native_output/presentation/ledger.rs` — `OutputTransactionLedger`: output transaction ownership and terminal state machine.
- `src/native_output/runtime/scene_history.rs` — `NativeSceneHistory`: submitted versus physically presented scene evidence.
- `src/native_output/scanout/output_swapchain.rs` — `OutputFrameKey`: immutable physical frame identity.
- `src/native_output/runtime/atomic_commit.rs` — `AtomicCommitArbiter`: bounded KMS submission ownership.
- `src/core/output_id.rs` — `OutputId`, `OutputIdAllocator`: nonzero typed logical output identity.

---

## 2. Runtime architecture map

### 2.1 Real process entrypoint

The real binary entrypoint is `src/main.rs::main`, which delegates to `run`. `run` first recognizes the internal application-scope execution mode, then dispatches normal CLI commands to Help, Doctor, Compositor, or Portal. The compositor path is `own_compositor`.

`own_compositor` deliberately blocks process-directed `SIGCHLD` before creating the Wayland server or entering native graphics/worker initialization. It creates the compositor plan, reports the protocol list, constructs `OwnCompositorServer`, and only then calls `native_output::run`. `--check` stops after server binding; the live path enters the native backend.

The production server construction is materially important because it is also the location of a current integration inconsistency: `main` calls `OwnCompositorServer::bind_with_capabilities_and_frame_pacing`, which injects `PresentationProtocolCapabilities::safe_baseline()`. The stronger `bind_native_base()` constructor passes `qualified_native()` presentation capabilities, but it is not the constructor used by `main`. Consequently tearing/content-type implementation exists but the normal product registry does not advertise those globals through this path. This report treats that as current wiring truth, not intended architecture.

### 2.2 Server construction and semantic ownership

`OwnCompositorServer` owns the Wayland display/socket and `CompositorState`. Global registration is capability-gated in `server_globals.rs`. The server exposes methods used by the native runtime, but the semantic state itself remains inside the compositor state rather than being cloned into native output structures.

Important capability families are separate: input, selection, renderer, frame pacing, presentation, and GPU-buffer capabilities. This is structurally useful because protocol advertisement can be tied to qualified implementation domains. The current main/convenience-constructor mismatch shows the cost of having more than one capability-construction path.

### 2.3 Native bootstrap

`native_output::run` enters `runtime::cycle::run`, which creates `NativeRuntime` through `NativeRuntime::bootstrap` / `bootstrap_native`. Bootstrap performs the platform ownership work that must exist before a frame loop can be trusted:

- obtain the logical `OutputId` from the compositor server;
- load persisted keyboard state and optional persistence workers;
- construct performance/diagnostic state;
- discover a native output candidate;
- establish a seat/session and DRM device ownership;
- allocate a DRM-file generation;
- choose KMS backend policy and timestamp clock;
- choose connector, CRTC, mode and refresh;
- initialize cursor theme/manager and cursor policy;
- choose and initialize the scanout backend;
- create native renderer state;
- register Wayland, input, DRM/worker/reactor, process and control event sources;
- initialize the frame scheduler, presentation timing/deadline planning, adaptive buffering, output transaction ledger, scene history and explicit-sync watch registry;
- initialize optional XWayland and resource-management state.

The `NativeRuntime` struct is intentionally explicit rather than hiding these domains behind a generic “backend” queue. It owns KMS target/device, scanout resources, scheduler, commit arbiter, output transactions, presented plane snapshot, scene history, explicit sync watches, cursor planes/workers, XWayland service, process supervisor, shutdown lifecycle, and observability state.

### 2.4 Event loop

`NativeRuntime::run` repeatedly executes `run_native_cycle`. A cycle begins by waiting for events/pageflips, classifying wake reasons into work domains, then servicing only the relevant domains. Process child reaping, XWayland progression, Wayland client dispatch, native input, explicit-sync readiness, surface pacing, commit-timing deadlines, rendering, output submission, capture, control requests, persistence workers and shutdown all meet here.

The loop does not blindly “tick everything.” Work-domain classification and bounded continuations are part of the design: a wake reason authorizes a bounded set of operations, while the scheduler and deadline planner decide when frame-critical work should resume.

### 2.5 Session loss and recovery

Session suspension is not modeled as a boolean around otherwise-live KMS objects. Recovery is a lifecycle:

1. suspend input;
2. park explicit-sync ownership;
3. quiesce and join the KMS worker;
4. quarantine any pageflip/commit ownership that can no longer be trusted;
5. retire hardware cursor ownership;
6. unregister DRM reactor state;
7. recover/rebuild the KMS pipeline and allocate a new DRM generation;
8. retire or terminalize quarantined old-generation work;
9. rearm explicit-sync watches against the new generation;
10. recover cursor resources;
11. register DRM/event sources;
12. resume scheduler/input and finally return the session lifecycle to Active.

The logical `OutputId` survives this process; the DRM generation does not. That is a clean separation between logical product identity and physical session ownership.

### 2.6 Shutdown

Shutdown is similarly stateful. New foreground/application work is stopped, pending capture is failed, KMS worker admission is closed, outstanding pageflip ownership is tracked, session-owned children are quiesced and signaled, and native resources are restored/retired in an order that avoids destructor I/O after session authority is gone. `ChildSupervisor` distinguishes session-owned processes from unrelated launched applications, so compositor shutdown does not imply killing everything the user launched.

### Runtime map

```text
main
  └─ run
      └─ own_compositor
          ├─ block SIGCHLD
          ├─ OwnCompositorServer { Display<CompositorState>, socket }
          └─ native_output::run
              └─ NativeRuntime::bootstrap
                  ├─ seat/session + DRM generation
                  ├─ connector/CRTC/mode
                  ├─ scanout backend + renderer
                  ├─ scheduler/pacing/deadlines
                  ├─ output transaction ledger
                  ├─ scene history/presented planes
                  ├─ explicit-sync watches
                  ├─ input/cursor
                  ├─ XWayland service
                  ├─ child/resource management
                  └─ event loop sources
                      └─ NativeRuntime::run_cycle
                          ├─ protocol/input/process progression
                          ├─ semantic transaction readiness
                          ├─ scene + presentation sampling
                          ├─ render/direct-scanout decision
                          ├─ KMS submit / worker queue
                          ├─ pageflip -> physical promotion
                          └─ exact semantic/presentation ACKs
```

### Source anchors

- `src/main.rs` — `run`, `own_compositor`, `native_protocol_names`: real product construction.
- `src/compositor/server.rs` — `OwnCompositorServer::bind_native_base`, `bind_with_capabilities_and_frame_pacing`: demonstrates the current presentation-capability constructor mismatch.
- `src/compositor/server_globals.rs` — global registration and capability gates.
- `src/native_output/runtime/bootstrap.rs` — `NativeRuntime::bootstrap_native`: native ownership construction.
- `src/native_output/runtime/mod.rs` — `NativeRuntime`: runtime ownership inventory.
- `src/native_output/runtime/cycle.rs` — `run`, `NativeRuntime::run_cycle`: event-loop control flow.
- `src/native_output/runtime/session.rs`, `session_io.rs` — session lifecycle and ordered recovery.
- `src/native_output/runtime/shutdown_cycle.rs`, `shutdown.rs` — shutdown state machine.
- `src/process.rs` — `ChildSupervisor`: child/process-group ownership and bounded shutdown.

### Representative invariant tests

- `src/native_output/runtime/session.rs::session_transitions_active_to_suspended_only_after_quiesce`
- `src/native_output/runtime/session.rs::session_transitions_suspended_to_active_only_after_recovery`
- `src/native_output/runtime/session_io.rs::session_recovery_keeps_logical_output_id_when_drm_generation_changes`
- `src/native_output/runtime/session_io.rs::suspended_wakeups_and_destruction_perform_zero_native_output_io`
- `src/native_output/runtime/session_io.rs::lifecycle_becomes_active_only_after_recorded_recovery_and_input_resume`

---

## 3. Architectural ownership map

| Concept | Canonical owner | Identity | Lifecycle | Completion evidence | Failure/fallback |
|---|---|---|---|---|---|
| Wayland compositor semantic state | `CompositorState` | resource IDs + typed internal IDs | process lifetime | semantic publication/commit | protocol error, bounded rejection, client cleanup |
| Logical output | `CompositorState` / native runtime share one allocated `OutputId` | `OutputId` | compositor lifetime | identity remains stable | allocation failure is explicit; current product still one physical output |
| `wl_surface` pending state | `SurfaceData` | `SurfaceId` + Wayland resource | resource lifetime | `wl_surface.commit` capture | protocol error / resource destruction |
| Surface-tree semantic transaction | compositor pending transaction queue | `SurfaceTreeTransactionId` + commit lineage | queued -> ready -> publish/terminal | readiness predicates + atomic publication | reject stale lifetime, bounded admission, supersession/coalescing only when legal |
| Synchronized subsurface cache | `SubsurfaceTransactionState` | surface/client + commit lineage | synchronized commits until parent publication | folded into owning tree transaction | hard per-surface/client/global bounds; offending client rejected |
| Window semantic state | `CompositorState` desktop/window registries | window/group/surface identity | map -> mutate -> unmap/destroy | canonical state update | protocol-specific repair/cleanup |
| Workspace state | `WorkspaceManager` inside compositor | `WorkspaceId`, special workspace ID | compositor lifetime | canonical activation/move | invalid target rejected; invisible tiled reflow deferred |
| Tiling layout | compositor tiled-layout state + Dwindle solver | window/group IDs, layout tree nodes | insert/remove/reflow | atomic canonical geometry application | typed layout error, preserve/fallback geometry |
| Selection | `SelectionState` | channel generation + source key | source publish -> replace/clear | exact generation/source match | stale clear/request rejected; X11 acts as adapter |
| Presentation property intent | `PresentationEngine` | `PresentationTransactionId` + per-property revision + `SceneNodeId` | install/retarget -> sample -> mathematical settle -> physical ACK | exact physically presented transaction/revision/value | stale/wrong-output ACK ignored; explicit cancellation |
| Retained lifecycle visual | Presentation retained registry + compositor retained payload store | `PresentationRetainedVisualIdentity` + payload ID | capture -> animate/reverse -> physical replacement/settlement | exact physical lifecycle evidence | rollback on failed install; old payload survives reversal where required |
| Rendered scene evidence | renderer + `NativeSceneHistory` submitted entries | frame/render identity + pageflip token | rendered -> submitted | renderer records exact scene snapshot | failed render/fallback does not become presented truth |
| Physically presented scene | `NativeSceneHistory.presented` | matching submitted pageflip token/output | pageflip promotions | matching pageflip | stale/wrong-output token rejected |
| Output transaction | `OutputTransactionLedger` | `OutputTransactionId` + output/generation | Built -> Ready/Queued -> Submitted -> Presented or terminal | exact token + generation pageflip | Dropped/Superseded/Failed; obligations have single owner |
| Physical frame object | output swapchain | `OutputFrameKey` | reservation/render/submission/presentation | same immutable key survives mutable scheduling changes | key mismatch invalidates stale completion |
| Atomic KMS lane | `AtomicCommitArbiter` | pageflip token + generation + commit kind | free -> kernel submitted (+ optional worker queued-next) -> complete | matching pageflip / worker submit ACK | at most one submitted + one queued; rejection restores/terminalizes ownership |
| KMS worker | native runtime | worker generation/job/token/reservation | start -> prepare/queue -> submit -> settle -> stop | worker result plus KMS/pageflip evidence | Auto degrades to synchronous; Force errors; bounded shutdown quarantine |
| Explicit-sync acquire | `ExplicitSyncWatchRegistry` | commit ID + watch token + DRM generation | register -> ready/cancel/backend mismatch | syncobj signaled + matching generation | superseded watch canceled; stale generation terminalized |
| Buffer release | compositor/native release obligations | surface/buffer/release point + owning transaction | attach -> render/direct use -> terminal release | GPU completion and/or pageflip contract | exactly-once terminalization; fallback preserves obligation |
| Direct Scanout qualification | compositor semantic analysis + native validation cache | exact semantic candidate + `DirectPlaneValidationKey` | discover -> semantic proof -> KMS proof -> TEST_ONLY -> submit/present | TEST_ONLY + real submit + pageflip | any blocker -> composition; default off |
| Effects semantic scene | compositor effect resolution | scene generation + effect instance identity | committed semantic effect -> resolved scene | canonical scene resolution | invalid/hidden effects omitted or rejected |
| Effects execution graph | effect compiler/planner | pass/texture/instance IDs | compile -> validate -> demand plan -> execute | renderer execution evidence | conservative/full repaint or legacy composition fallback |
| Input seat/focus | compositor input state + native input router | seat/resource serials + focus identities | device events -> routed protocol state | exact serial/focus/constraint state | stale serial/client rejected; unsupported classes not advertised |
| Cursor presentation | compositor desired state + native cursor arbitration | cursor epoch/image/plane identity | desired -> frozen submission -> physical promotion | pageflip/frozen cursor evidence | hardware -> software fallback; cursor can block direct/async paths |
| XWayland process | `XwaylandService` | generation + display lease + child identity | Off/Starting -> managed Running -> stopping/restart | generation-qualified startup/XWM gates | stale generation ignored; private display/auth cleanup |
| XWM windows | XWM adapter -> compositor canonical window model | `X11WindowHandle(generation,xid)` | observe/adopt -> manage -> destroy | X11 events plus compositor adoption | stale generation/invalid transient/unsupported bridge rejected |
| Child processes | `ChildSupervisor` | internal process ID + PID/PGID + ownership kind | spawn -> running -> reaped; shutdown quiesce | SIGCHLD/wait status | bounded TERM/KILL only for session-owned children |
| Application scope helper | application-scope subsystem | request/scope identity | direct/scope helper -> migration verification | helper status + systemd result | bounded timeout/failure -> direct spawn fallback |
| Observability history | per-subsystem bounded rings/counters | frame/transaction/generation keys | append/overwrite bounded history | diagnostic evidence only | disabled paths avoid expensive work where designed |

The map is deliberately redundant with the subsystem sections: it is the shortest reusable reference for public documentation writers. The important pattern is that “canonical owner” changes as information crosses semantic, presentation, rendered and physical domains; it does not mean one mega-object owns every phase.

---

## 4. End-to-end frame lifecycle

This is the architecture's most important cross-subsystem story.

### 4.1 Protocol requests accumulate pending semantic input

A client mutates protocol objects: attaches a buffer, adds damage, changes viewport/scale/transform, requests callbacks or presentation feedback, changes explicit-sync state, sets background-effect state, and may attach FIFO/Commit Timing or presentation-mode metadata. `SurfaceData` is the immediate pending owner. These requests are not automatically a canonical rendered scene.

For synchronized subsurfaces, commits can be cached rather than published. The cache preserves obligations and commit lineage while the parent synchronization relationship determines when the tree can become atomic semantic truth.

### 4.2 Commit forms or joins a semantic publication transaction

`wl_surface.commit` captures pending state. A normal root may publish directly if all semantic/readiness rules allow it. A synchronized tree forms `PendingSurfaceTreeTransaction`, carrying exact node commits, lifetimes, acquire dependencies, external-content dependencies, pacing obligations and timing readiness.

`merge_or_queue_surface_tree_transaction` may coalesce only transactions whose lineage and obligation semantics make replacement legal. Pacing-protected work is not silently discarded. The queue is bounded indirectly through synchronized-cache and obligation limits; resource pressure can reject admission rather than allow unbounded frame-critical state.

### 4.3 Readiness gates publication

`commit_ready_surface_tree_transactions` checks whether the transaction still belongs to live surface/client generations and whether all required dependencies are terminal or ready. Important gates include explicit-sync acquire, FIFO readiness and Commit Timing lower bounds. Destroyed/stale resources do not get “completed” by later unrelated readiness.

When ready, `publish_surface_tree` applies the nodes through one semantic authority. That publication advances canonical render/content generations and makes the tree eligible for scene construction.

### 4.4 Canonical scene is resolved

The compositor builds its active scene from canonical window/surface/workspace/stacking state. Semantic effects are resolved here; animation intent is not allowed to mutate canonical window truth in place. Instead the Presentation Engine samples property-level visual state for the target presentation time.

This yields a **presentation snapshot**: canonical content plus presentation transforms/opacities/clips, lifecycle visual state, decorations, cursor requirements and effect scene. The snapshot is an intention/evidence input, not yet physical truth.

### 4.5 Direct Scanout proof or composition

The runtime first asks whether a direct candidate exists semantically. Direct Scanout requires a solitary output-covering eligible source/owner, no composition-requiring effects/content above it, compatible geometry/transform/scale/viewport, no disqualifying presentation clip/animation/opacity, proven dmabuf/opaque content, safe cursor state and no unpublished semantic work.

If semantic proof succeeds, the native backend constructs an exact `DirectPlaneValidationKey` including output generation, CRTC/primary plane, mode, FourCC/modifier, buffer geometry/layout, cursor atomic key, synchronization contract, presentation mode and DRM content type. The path is deliberately conservative. TEST_ONLY validates the exact hardware state before real submit, and any failure returns to composition.

If direct proof fails or policy is off, the renderer composes the scene. The effects subsystem may compile a typed graph and damage plan; otherwise the legacy scene path is used. The renderer writes into a swapchain slot/framebuffer and records a rendered scene snapshot. Rendering alone does not advance physical presentation state.

### 4.6 Physical identity is frozen

The output path associates the physical object with `OutputFrameKey` and an `OutputTransaction`. The key is intentionally independent of mutable future target timing. This allows a scheduler to revise *when* a physical frame is intended to present without changing *which physical frame* it is.

The output transaction freezes output generation, content kind, plane plan, protocol obligations, sync/release contract, presentation mode/content type and related ownership needed at submission.

### 4.7 Scheduler/worker selects submission ownership

Reactive double or predictive triple policy determines whether the runtime renders only the immediate frame or reserves future overlap. Predictive O1 uses its own attempt identity and worker reservation; it must remain attached to the same `OutputFrameKey` to settle successfully.

For atomic KMS, the commit arbiter permits one kernel-submitted commit and, when the worker is active, one ordered worker-queued-next commit. The worker may prepare the next job, but it cannot turn KMS into an arbitrary FIFO.

### 4.8 KMS submit is not presentation

A successful real atomic commit transitions the output transaction to Submitted and associates it with the exact pageflip token/generation. `NativeSceneHistory` keeps the submitted scene evidence. Neither operation proves that the user has seen the frame.

### 4.9 Pageflip establishes physical truth

A pageflip event is accepted only if token, logical output and generation match current ownership. The output transaction can then become Presented. `NativeSceneHistory::promote_pageflip` moves the exact submitted scene to presented; `PresentedPlaneSnapshot` promotes the exact plane bundle.

At this point Typhon has physical evidence. It can:

- complete presentation feedback;
- release pageflip-bound buffer obligations;
- update cadence/timing models;
- publish the physically presented compositor frame snapshot;
- send exact ACK evidence back to Presentation Engine property/lifecycle transactions;
- retire mathematically settled animations only if the presented transaction/revision/value is the exact target.

### 4.10 Stale or rejected work never becomes current by implication

If render fails, TEST_ONLY rejects, real submit rejects, a session generation changes, a worker is quiesced, or a pageflip token is stale, the relevant owners are explicitly dropped/failed/superseded/quarantined. The system does not treat “time passed” as presentation.

### Lifecycle summary

```text
client protocol mutation
  -> SurfaceData pending state
  -> wl_surface.commit
  -> surface/tree transaction
  -> readiness (lifetime + acquire + FIFO + commit timing)
  -> canonical semantic publication
  -> active scene + semantic effects
  -> PresentationEngine sample / lifecycle retained state
  -> direct-scanout proof OR renderer composition
  -> rendered evidence + immutable OutputFrameKey
  -> OutputTransaction Built/Ready/Queued
  -> KMS submit -> Submitted(pageflip token)
  -> pageflip exact token/generation
  -> Presented physical scene/planes
  -> protocol feedback/releases + exact PresentationEngine ACK
```

### Source anchors

- `src/compositor/state_data.rs` — `SurfaceData`.
- `src/compositor/subsurface.rs` — `CachedSubsurfaceCommit`, `SubsurfaceTransactionState`.
- `src/compositor/state/surface_transactions.rs` — `PendingSurfaceTreeTransaction`.
- `src/compositor/state/surface_tree_readiness.rs` — `commit_ready_surface_tree_transactions`.
- `src/compositor/state/subsurfaces.rs` — `merge_or_queue_surface_tree_transaction`, `publish_surface_tree`.
- `src/compositor/state/direct_scanout.rs` — `direct_scanout_scene_analysis`.
- `src/presentation_animation/engine.rs` — `PresentationEngine::sample`, physical ACK methods.
- `src/native_output/runtime/frame.rs` — `NativeFrameRenderer`.
- `src/effects/render_graph.rs` — `compile_frame_execution_plan`, `CompiledFrameGraph`.
- `src/native_output/scanout/direct_validation.rs` — `DirectPlaneValidationKey`.
- `src/native_output/scanout/output_swapchain.rs` — `OutputFrameKey`.
- `src/native_output/presentation/transaction.rs` — `OutputTransaction`.
- `src/native_output/presentation/ledger.rs` — `OutputTransactionLedger`.
- `src/native_output/runtime/scene_history.rs` — `NativeSceneHistory`.
- `src/native_output/runtime/cycle/pageflip.rs` — pageflip settlement path.

---

## 5. Presentation Engine

### 5.1 Problem solved

The Presentation Engine solves a problem that is easy to underestimate: semantic state may change immediately while the screen must move continuously through intermediate visual states, and the compositor must know which intermediate state was **actually presented** before it can safely retire old presentation ownership.

A simpler design could store “current animated geometry” directly on the window. That would collapse semantic truth, animation interpolation and physical presentation into one mutable value. Typhon instead keeps canonical window state canonical and makes presentation state a separate transactional layer.

### 5.2 Authoritative state and identities

`PresentationEngine` owns active geometry, opacity and clip tracks keyed by `SceneNodeId`, plus a transaction table and retained visuals. Every installed member carries:

- `PresentationTransactionId` — groups semantically related presentation changes;
- a distinct revision ID for the property member;
- `SceneNodeId` — the stable scene identity;
- property kind (geometry, opacity, clip, retained visual kind).

Retained lifecycle visuals use `PresentationRetainedVisualIdentity`, also binding scene node, kind, transaction and revision. Identity allocation is monotonic and exhaustion-aware.

### 5.3 Semantic state versus presentation state

The engine does not redefine canonical geometry/opacity/clip. It receives semantic endpoints and produces a `PresentationFrameSnapshot` for a target time. Sampling computes presentation transforms/opacities/clips and records whether a property is mathematically settled. The same transaction may contain multiple properties with separate revisions and independent settlement.

This is why `PresentationClip` belongs here rather than being implemented as a destructive “surface aperture” mutation. A clip is presentation state with its own revision and physical ACK lifecycle, not a rewrite of the surface's semantic buffer content.

### 5.4 Completion

Mathematical completion is necessary but insufficient. `acknowledge_presented_geometry`, `acknowledge_presented_opacity` and `acknowledge_presented_clip` require:

- the correct logical output;
- the correct property kind;
- exact transaction ID;
- exact revision;
- a mathematically settled sample;
- physically presented value equal to the final target.

Only then can the member/track retire. A wrong-output or stale-revision ACK is ignored rather than interpreted as “close enough.”

### 5.5 Retargeting and reversal

Geometry/clip/opacity retargeting preserves continuity where the track model supports it. Lifecycle reversal is handled at the lifecycle layer, but it shares the same identity discipline: the old retained payload can remain physically authoritative while a new motion direction takes ownership only after the new transition is installed consistently.

### 5.6 Why this design appears to have been chosen

**Source-proven fact:** the code and tests enforce exact revision ACK, separate canonical versus presented state, and retained visual identity.

**Likely rationale:** the architecture is designed to eliminate a class of races in which a newer semantic transition replaces an older animation before the old pixels have actually left the scanout. Requiring physical ACK allows Typhon to retain resources and ownership exactly as long as the screen may still reference them.

### 5.7 Trade-offs

- More identities and state machines must be carried across compositor, renderer and native output.
- Bugs are possible at projection boundaries if a renderer forgets to preserve transaction/revision evidence.
- More testing is required for stale ACKs and reversal.
- In exchange, the architecture makes “what the screen is still allowed to show” explicit, which is especially valuable for minimize/restore, clip animations and render-ahead.

### Source anchors

- `src/presentation_animation/engine.rs` — `PresentationEngine`, `sample`, `acknowledge_presented_*`.
- `src/presentation_animation/transaction.rs` — `PresentationTransactionMember`.
- `src/presentation_animation/retained.rs` — `PresentationRetainedVisualIdentity`.
- `src/presentation_animation/frame.rs` — `PresentationFrameSnapshot` and property samples.
- `src/compositor/presented_frame.rs` — `publish_presented_frame`: physical evidence publication back into compositor presentation state.

### Representative invariant tests

- `src/presentation_animation/transactions_tests.rs::geometry_opacity_and_clip_share_one_transaction_and_ack_independently`
- `...::settled_track_requires_matching_physical_revision_ack`
- `...::transaction_member_retirement_requires_exact_revision_evidence`
- `...::stale_and_wrong_output_opacity_acks_preserve_the_current_revision`
- `...::clip_identity_noop_and_wrong_output_or_stale_ack_do_not_retire_track`
- `...::stale_retained_identity_cannot_retire_a_newer_identity_for_the_same_node`

**Status:** Implemented; product-wired; deterministically tested; evolving. Its physical-ACK model is a stable architectural principle, while the set of animated properties/effects can expand.

---

## 6. Surface transaction architecture

### 6.1 Problem solved

Wayland surface state is double-buffered, and synchronized subsurfaces require multiple resource commits to become visible atomically at an ancestor commit. Typhon adds explicit sync, FIFO, Commit Timing, presentation feedback, viewport/transform state and renderer obligations on top of that. A naive “apply each commit to the scene as it arrives” design would violate atomicity and lose obligations when newer commits supersede older cached content.

### 6.2 Pending and committed protocol state

`SurfaceData` stores pending attachment, offset, surface/buffer damage, callbacks, explicit-sync data, pacing requests, presentation state, viewport/scale/transform, input/opaque region and background-effect state. Protocol requests mutate this pending area; commit captures it.

`commit_surface_buffer` treats a new attachment commit as new content even if the same `wl_buffer` object is reused. It allocates a render generation, records visual generation, applies committed geometry/placement rules and then updates canonical buffer state. This avoids using Wayland object identity as content identity.

### 6.3 Synchronized cache authority

`SubsurfaceTransactionState` owns synchronized cached commits and associated accounting. It imposes explicit limits:

- 8 cached commits per surface;
- 256 per client;
- 4096 globally;
- bounded obligation counts per surface/client/global.

This is architectural, not only defensive programming: a client cannot turn synchronization semantics into an unbounded compositor-owned queue.

### 6.4 Tree transaction authority

`PendingSurfaceTreeTransaction` contains the root, exact nodes and cached commits, captured publication lifetimes, acquire dependencies, external content dependencies, Commit Timing readiness and transaction timing. `SurfaceTreeTransactionId` provides stable semantic identity.

`merge_or_queue_surface_tree_transaction` coalesces only when commit lineage and obligation constraints permit it. A pacing-protected transaction cannot be silently replaced simply because a newer visual state exists. Dependencies are preserved in transaction order.

`commit_ready_surface_tree_transactions` is the readiness arbiter. It verifies that captured lifetimes and generations are still valid and that all dependent readiness conditions have reached a legal terminal/ready state. Publication uses `publish_surface_tree`, retaining one semantic transaction authority.

### 6.5 Supersession and destruction

Supersession is allowed only where newer state can legally replace older state without losing protected protocol obligations. Destruction turns lifetime dependencies terminal; it does not make an unrelated later surface commit prove that the destroyed transaction succeeded.

### 6.6 Why one transaction authority matters

**Likely rationale:** synchronized subsurface correctness becomes unmanageable if explicit sync, FIFO, callback delivery, presentation feedback and visual content each maintain independent “pending queues.” Typhon centralizes semantic publication, then lets neighboring systems own only their phase-specific evidence.

### Source anchors

- `src/compositor/state_data.rs` — `SurfaceData`.
- `src/compositor/subsurface.rs` — `SubsurfaceTransactionState`, cached-commit limits and `CachedSubsurfaceCommit`.
- `src/compositor/state/surface_transactions.rs` — `SurfaceTreeTransactionId`, `PendingSurfaceTreeTransaction`.
- `src/compositor/state/subsurfaces.rs` — transaction extraction, coalescing and `publish_surface_tree`.
- `src/compositor/state/surface_tree_readiness.rs` — readiness/publish loop.
- `src/compositor/state/surface_commits.rs` — canonical buffer commit/update paths.

### Representative invariant tests

The subsystem has unusually dense tests. Particularly architectural examples include tests for discontinuous lineage rejection, dependency preservation, synchronized viewport/scale/transform projection, cached-prefix ordering, external dependency waiting, bounded cache admission, and tree publication after readiness. Examples:

- `src/compositor/state/subsurfaces.rs::pending_coalescing_rejects_discontinuous_lineage_without_mutating_target`
- `...::pending_coalescing_requires_contiguous_lineage_for_same_surface_replacement`
- `...::coalesced_predecessor_is_internalized_and_publishes_after_other_readiness_clears`
- `...::external_content_update_dependencies_wait_for_their_owner`
- `...::cached_same_surface_prefix_is_emitted_in_predecessor_order`

**Status:** Implemented; product-wired; deterministically tested; stable semantic model with evolving complexity.

---

## 7. Rendering architecture

### 7.1 Logical scene versus physical resources

Typhon separates the logical scene from physical render targets. The compositor resolves renderable surfaces/windows/effects/presentation properties. `NativeFrameRenderer` translates that scene into backend-specific rendering, while native scanout owns physical buffers/framebuffers and KMS submission.

`DesktopSceneRenderer` maintains reusable CPU-side scene/damage state. Composition can choose no-copy, partial-copy or full-copy behavior based on buffer age/damage. Cursor and overlay composition are applied as part of the render request rather than treated as semantic window content.

The native backend can be a CPU/dumb/GBM path or EGL/GLES path, depending on selected scanout backend. `GlesSceneRenderer` receives an `EglSceneDrawRequest` carrying surfaces, effect graph, presentation transforms/clips, lifecycle resources and synchronization evidence.

### 7.2 Renderer evidence

When a frame is rendered, native code records a scene snapshot. This is **rendered evidence**, not physical presentation. `NativeSceneHistory` can hold submitted snapshots keyed by pageflip token while retaining a separate `presented` snapshot.

This matters during render-ahead: a later frame can be rendered while an earlier submitted frame is still physically on screen. Damage for the next transition must be calculated from the actual presented predecessor, not from “last rendered.” The tests explicitly exercise rejected render-ahead candidates and delayed pageflips.

### 7.3 Effects and fallback

Visible effects are compiled into an effect graph; no visible effects use the legacy scene path. Effect demand planning determines which passes/regions need repaint. Unsupported or unsafe effect metadata expands work conservatively rather than allowing missing pixels. Renderer/executor failure does not create physical evidence; the surrounding path can fall back to a composition-safe path.

### 7.4 Framebuffer ownership

Physical framebuffer/slot ownership lives below the logical renderer. `OutputFrameKey` captures immutable physical identity and the swapchain/pool generation. This prevents a scheduling target update from changing the identity of pixels already rendered into a concrete framebuffer.

### Source anchors

- `src/compositor/render.rs` — `DesktopSceneRenderer` and scene composition/damage paths.
- `src/native_output/runtime/frame.rs` — `NativeFrameRenderer`, native scene resolution.
- `src/egl_renderer.rs` — `GlesSceneRenderer`, `EglSceneDrawRequest` and GLES scene draw paths.
- `src/egl_renderer/effects/executor.rs` — effect-graph execution.
- `src/native_output/scanout/*` — backend-specific render targets and scanout.
- `src/native_output/runtime/scene_history.rs` — physical render/submission/presentation evidence separation.

### Representative invariant tests

- `src/native_output/runtime/scene_history.rs::rendered_snapshot_advances_presented_history_only_on_matching_pageflip`
- `...::presented_window_projection_advances_only_on_physical_promotion`
- `...::pageflip_transition_uses_the_actual_presented_predecessor`
- `...::rejected_render_ahead_candidate_does_not_change_next_transition_predecessor`
- `...::stale_pageflip_token_cannot_regress_newer_presented_scene`

**Status:** Implemented; product-wired. CPU/EGL paths differ in qualification and performance characteristics. Physical presentation remains KMS-owned, not renderer-owned.

---

## 8. Native output / KMS architecture

### 8.1 Ownership model

The native runtime owns one physical KMS target in the current product: connector, CRTC, primary plane, mode, scanout backend, cursor resources, atomic submission lane, scheduler and physical presentation ledger. It shares the compositor's logical `OutputId` but allocates a separate DRM-file/output generation that changes when physical session ownership changes.

This is an important current-state distinction: a typed `OutputId` foundation is implemented and active, but there is no production collection of independent `OutputRuntime` instances, no hotplug lifecycle and no multi-output desktop layout. The protocol manifest explicitly still calls Typhon a single-output product.

### 8.2 Swapchain and frame identity

The atomic output pool uses explicit slots. `OutputFrameKey` includes:

- `OutputId`;
- frame ID;
- protocol batch ID;
- output transaction ID;
- output slot;
- framebuffer ID;
- render generation;
- pool generation.

The key intentionally excludes mutable presentation target state. The physical object does not change merely because pacing revises a desired vblank.

### 8.3 TEST_ONLY and real submit

Atomic KMS paths use TEST_ONLY where qualification is required (notably Direct Scanout and complex atomic changes) before real submission. A successful TEST_ONLY is evidence about the exact atomic state, not a guarantee that a later different state is valid. Therefore Direct Scanout validation keys include all state that matters to qualification, and rejection invalidates the relevant proof/fallback path.

### 8.4 Commit worker

`KmsCommitWorkerPolicy` is Off/Auto/Force and defaults to Off. In Auto, worker startup/eligibility failure can degrade to synchronous submission. Force turns lack of worker support into an explicit failure.

`AtomicCommitArbiter` exposes the architectural bound directly: one `kernel_submitted` commit plus one `worker_queued` commit. A worker-queued job is not considered submitted until the worker reports the submit transition. A pageflip cannot complete arbitrary queued work merely because the token resembles a future job.

### 8.5 Pageflip and generation

Pageflip completion is keyed by token and generation. Mismatched or stale events are counted/ignored, not used to “advance” the pipeline. Session recovery increments physical generation and rebuilds ownership while the logical output identity remains stable.

### 8.6 Cursor

Hardware cursor state is part of KMS ownership, not an afterthought. Cursor visibility/content/position can affect async presentation and Direct Scanout qualification. When hardware cursor cannot be used, the renderer can fall back to software cursor, which correctly implies composition requirements.

### 8.7 Failure model

- worker startup failure in Auto -> synchronous submission;
- TEST_ONLY reject -> do not real-submit that qualified state;
- real submit reject -> invalidate matching proof and fall back/terminalize exact ownership;
- session loss -> quiesce, quarantine, rebuild generation;
- cursor plane failure -> software cursor where policy permits;
- stale pageflip -> no completion.

### Why explicit ownership rather than implicit queueing?

**Source-proven fact:** the arbiter and worker tests reject additional queued ownership and distinguish queued from submitted state.

**Likely rationale:** hidden DRM queues make latency and ownership impossible to reason about. Bounding the lane ensures at most one future commit exists outside the kernel, preserves pageflip causality, and gives the scheduler a concrete definition of pipeline depth.

### Source anchors

- `src/native_output/runtime/mod.rs` — `NativeRuntime` native owners.
- `src/native_output/scanout/output_swapchain.rs` — output slots and `OutputFrameKey`.
- `src/native_output/runtime/atomic_commit.rs` — `AtomicCommitArbiter`.
- `src/native_output/kms_worker/policy.rs` — `KmsCommitWorkerPolicy`.
- `src/native_output/kms_worker/thread.rs` — worker execution.
- `src/native_output/runtime/kms_worker.rs`, `presentation_worker.rs` — runtime ownership transitions.
- `src/native_output/runtime/session_io.rs` — recovery and generation rebind.

### Representative invariant tests

- `src/native_output/runtime/atomic_commit.rs::runtime_tracks_one_kernel_submitted_plus_one_worker_queued`
- `...::worker_queued_commit_does_not_arm_watchdog`
- KMS worker tests covering queued rejection, early pageflip deferral and exact promotion.
- `src/native_output/runtime/session_io.rs::session_recovery_keeps_logical_output_id_when_drm_generation_changes`

**Status:** Atomic KMS active; KMS worker implemented/product-wired but default Off; multi-output not product-wired; hardware qualification is path-specific.

---

## 9. Frame pacing architecture

### 9.1 Reasoning model

Typhon's frame pacing architecture is not “triple buffering on/off.” It models whether the compositor has enough evidence and pipeline capacity to safely overlap future work without losing physical identity or creating an unbounded queue.

The two primary output pacing modes are reactive double buffering and predictive triple buffering. Reactive double keeps the pipeline conservative: produce/submit based on current demand and confirmed progress. Predictive triple allows one bounded future primary path when capability/policy and Predictive O1 evidence permit it.

### 9.2 Identities

The pacing layer deliberately distinguishes:

- physical `OutputFrameKey`;
- compositor frame ID;
- `PredictiveO1AttemptId`;
- `WorkerPacingReservationId`;
- output transaction ID;
- pageflip token.

A predictive attempt is a scheduling experiment, not the identity of the framebuffer. `reserve_worker_submission` binds predictive reservation to exact physical frame evidence. `note_pageflip_exact` only settles the attempt as physically presented when the completed key is the expected key.

### 9.3 Predictive O1

Predictive O1 tracks whether overlap actually produced useful readiness at the right point in the pipeline. It is not allowed to infer success merely because a worker ran. Attempts have lifecycle/terminal accounting, and stale/mismatched physical completions do not count as the current attempt.

### 9.4 Deadlines and target selection

`PresentationDeadlinePlanner` models target presentation sequence/time, earliest/appropriate submit windows, render-start deadline, refresh interval, clock generation and selection reason. KMS timing distinguishes render-readiness misses, worker/dispatch timing and apply-guard misses rather than compressing every miss into one number.

Commit Timing lower bounds and FIFO obligations enter the same scheduling picture. A surface transaction can be semantically ready except for a timing lower bound; the event loop can arm a wake rather than spin or publish early. A blocked root should not globally starve unrelated eligible work.

### 9.5 Async presentation interaction

Async/tearing presentation forces reactive double in the current design. This is a deliberate simplification: predictive future-vblank targeting is not meaningful in the same way when the selected presentation contract is asynchronous.

### 9.6 Adaptive buffering

The triple-buffer policy defaults to Auto. `AdaptiveBufferingController` tracks capability, overlap opportunities, O1 credits/blockers and observed timing. “Auto” is therefore a policy that can decline triple behavior, not a promise that every frame has three in flight.

### 9.7 Fallback

If the predictor lacks safe evidence, pipeline capacity, worker ownership or timing confidence, Typhon falls back toward reactive behavior. This is consistent with the project's broader rule that an optimization should prove eligibility rather than make the safe path prove its innocence.

### Source anchors

- `src/native_output/pacing.rs` — `NativeFramePacing`, `PredictiveO1AttemptId`, `WorkerPacingReservationId`, `reserve_worker_submission`, `note_pageflip_exact`.
- `src/native_output/runtime/presentation_pipeline.rs` — current pipeline view/ownership.
- `src/native_output/presentation/kms_timing.rs` — presentation timing/apply-guard model.
- `src/native_output/runtime/bootstrap.rs` — adaptive buffering/triple policy construction.
- `src/native_output/runtime/cycle.rs` — deadline planning/wake arbitration.

### Representative tests

- predictive O1 tests assert physical-key preservation and stale-attempt rejection;
- worker pacing tests bind reservations to exact physical frame keys;
- `src/native_output/presentation/kms_timing.rs` tests distinguish render-readiness, dispatch and apply-guard misses and reject stale pageflip tuning.

**Status:** Implemented; product-wired; triple policy defaults Auto; predictive paths are active when eligibility permits. Performance benefit is a qualification/measurement question, not implied by architecture.

---

## 10. Explicit synchronization

### 10.1 Problem solved

Explicit synchronization makes buffer readiness and release ownership first-class. Typhon cannot assume a dmabuf is ready because a commit arrived, nor can it release a buffer merely because the compositor stopped mentioning it semantically. The buffer may still be used by GPU composition or physical scanout.

### 10.2 Acquire ownership

`AcquireWatchRequest` binds a compositor commit, surface, buffer and acquire point. `ExplicitSyncWatchRegistry` owns active watch tokens, commit-to-token mapping, DRM generation and bounded fallback/event state.

`register_owned` supersedes an earlier watch for the same commit explicitly, checks already-signaled state, installs eventfd/fallback watching, binds generation, and performs a final post-registration signaled check to close the registration race.

`handle_ready` accepts readiness only for the current watch and DRM generation. Unknown recent tokens are distinguishable from valid current work; generation mismatch terminalizes as backend mismatch rather than leaking ownership into the new session.

`cancel_commit` is an explicit terminal path.

### 10.3 Release ownership

Surface buffer release is represented separately from acquire. Dmabuf release obligations can be satisfied by GPU completion, pageflip, or the exact direct-scanout out-fence/pageflip contract selected by the native path. The output transaction carries synchronization/release plan so fallback from direct to composition does not accidentally drop the obligation.

### 10.4 Synchronized subsurface interaction

Acquire readiness is captured into surface-tree transaction dependencies. A synchronized child with an unsignaled acquire cannot become visible just because the parent commits; the tree waits. If the resource/generation is destroyed or invalidated, the dependency becomes terminal according to that ownership rather than being silently treated as ready.

### 10.5 Session recovery

Acquire watches are parked/quiesced while DRM ownership is absent and rearmed against the new generation. Tests cover unsignaled suspended fences and ensure input/scheduler are not resumed until required recovery evidence exists.

### 10.6 Exactly-once invariant

The architectural obligation is stronger than “eventually release”: every acquire watch and release obligation must reach one terminal state exactly once. Supersession, fallback, generation changes and shutdown are terminal transitions, not exceptions outside the model.

### Source anchors

- `src/compositor/explicit_sync.rs` — `AcquireWatchRequest` and commit-side sync capture.
- `src/native/explicit_sync.rs` — `ExplicitSyncWatchRegistry`, `register_owned`, `handle_ready`, `cancel_commit`.
- `src/compositor/state_data.rs` — `SurfaceBufferRelease`, `DmabufReleaseObligation`.
- `src/native_output/presentation/transaction.rs` — output synchronization/release plan.
- `src/native_output/runtime/dmabuf_release.rs` — native GPU release ownership.
- `src/native_output/runtime/session_io.rs` — park/rearm across session generations.

### Representative tests

- explicit-sync registry tests for supersession, duplicate readiness, generation mismatch and cancellation;
- synchronized subsurface tests proving acquire readiness survives caching;
- session recovery tests proving unsignaled fences keep resume pending;
- Direct Scanout/KMS worker tests proving release ownership follows the actual accepted path.

**Status:** Implemented; product-wired where explicit sync capability/backend is available; deterministically tested; hardware behavior depends on DRM/syncobj support.


---

## 11. Direct Scanout

### 11.1 Problem solved

Direct Scanout is an optimization that replaces compositor composition with direct KMS presentation of a client's dmabuf on the primary plane. The performance upside is obvious; the correctness risk is not. The compositor must prove that bypassing composition produces the same visible result and preserves synchronization, cursor, presentation-mode, lifecycle and protocol obligations.

Typhon therefore models Direct Scanout as a **proof pipeline**, not a best-effort shortcut.

### 11.2 Policy and current status

`NativeDirectScanoutPreference` exposes Off and ExperimentalAuto. The environment default is Off; the old generic `auto` spelling is treated as an alias for the experimental path rather than a qualified normal default. There is no “force the unsafe state” mode.

The path is implemented and product-wired, but intentionally experimental/default-off. Source and current qualification language still treat target-hardware evidence as incomplete.

### 11.3 Candidate discovery: owner versus source

The compositor distinguishes the semantic window/group that owns the visible region from the surface that supplies the dmabuf source. That separation matters for real window trees: a root/window owner can have a specific source surface while unrelated content, popups, SSD, effects or auxiliary content affect whether the *group* remains equivalent to direct scanout.

`direct_scanout_scene_analysis` rejects candidates when semantic equivalence cannot be established. Important rejection classes include:

- no eligible solitary output-covering window/group;
- owner missing, minimized or not current;
- content above the source;
- SSD/popup/overlay content requiring composition;
- visible effect/background effect;
- non-dmabuf or unproven opaque content;
- scale/transform/viewport/geometry mismatch;
- presentation clip or non-unit canonical/presentation opacity;
- active geometry/lifecycle animation requiring transformed composition;
- resize preview or transient compositor-owned visual;
- pending/unpublished semantic work.

The important design choice is that Direct Scanout eligibility is derived from **canonical scene semantics**, not from “the top buffer looks fullscreen.”

### 11.4 Native/KMS proof

After semantic discovery, the native path builds `DirectPlaneValidationKey`. The key includes the physical facts that make a previous TEST_ONLY result reusable:

- logical output and output generation;
- CRTC and primary-plane identity;
- mode dimensions;
- DRM FourCC and modifier;
- buffer dimensions and plane-layout hash;
- complete cursor atomic key, including cursor content/position/plane state;
- synchronization contract;
- effective presentation mode;
- DRM content type.

If any input changes, the key changes and old proof is not silently reused.

The atomic direct path validates format/modifier, explicit-sync readiness, presentation policy and cursor constraints. For the qualified experimental path, KMS worker availability is part of the expected contract. TEST_ONLY is performed before real submission; real-submit rejection invalidates the matching proof and falls back.

### 11.5 Presentation mode and tearing

Current source is more advanced than older Stage-4 prose that categorically excluded tearing. The direct validation key and runtime path include effective presentation mode and DRM content type. Async eligibility is still restrictive and policy-default-off; it does not mean “fullscreen client asked for tearing, therefore direct async.” Cursor mutation, sync state, KMS lane state and other proofs still participate.

This is a documentation-drift case: historical qualification boundaries remain useful context, but the source now models presentation mode inside direct qualification.

### 11.6 Synchronization and release

Direct Scanout does not bypass explicit-sync ownership. The candidate buffer's acquire must be ready under the exact publication/DRM generation contract. Release is tied to the direct transaction's physical terminal state/out-fence-pageflip contract. If direct validation fails and the frame is composited, release ownership follows the composited path instead.

### 11.7 Why conservative eligibility is fundamental

**Source-proven fact:** policy defaults Off, semantic blockers are numerous and named, the native validation key is broad, and TEST_ONLY precedes real submit.

**Likely rationale:** Direct Scanout has asymmetric failure cost. Missing an optimization loses efficiency; accepting an invalid candidate can produce missing overlays, stale animations, synchronization violations, corrupted output or misleading presentation feedback. The source consistently chooses false negatives over false positives.

### 11.8 Trade-offs

- More fullscreen cases fall back to composition than on a heuristic compositor.
- Validation state is large and must remain synchronized with every feature that changes visible equivalence.
- New presentation features must explicitly decide whether they block or extend the proof model.
- In return, rejection is inspectable and the optimization remains an optional layer over a correct composition path.

### Source anchors

- `src/compositor/state/direct_scanout.rs` — `direct_scanout_scene_analysis`: semantic eligibility.
- `src/native_output/scanout/direct_policy.rs` — `NativeDirectScanoutPreference` and default policy.
- `src/native_output/scanout/direct_validation.rs` — `DirectPlaneValidationKey`.
- `src/native_output/scanout/atomic_egl_gbm/direct.rs` — atomic direct execution / TEST_ONLY / real submission.
- `src/native_output/scanout/direct_transition.rs` — direct/composited transitions.
- `src/native_output/runtime/presentation_direct.rs` and `cycle_direct.rs` — runtime transaction/physical settlement.
- `src/native_output/presentation/async_validation.rs` — async qualification identity.

### Representative tests

- `src/compositor/state/direct_scanout_tests.rs::canonical_presentation_clip_blocks_direct_scanout`
- direct tests covering owner/source separation, effects/content-above, opacity, geometry and animation blockers;
- `src/native_output/presentation/async_validation.rs::exact_key_changes_when_any_qualification_input_changes`
- KMS worker/direct tests proving TEST_ONLY rejection prevents real submission and direct lease/release ownership is returned exactly once.

**Status:** Implemented; product-wired; Experimental; default Off; deterministically tested; current hardware qualification remains intentionally narrower than implementation.

---

## 12. Effects engine

### 12.1 Problem solved

The effects subsystem needs to express compositor-owned and client-requested visual processing without turning the renderer into an untyped sequence of shader calls. It must answer not only “which shader runs?” but also:

- what semantic region an effect applies to;
- whether it reads target content, backdrop content or both;
- where it is anchored relative to the surface/output;
- which intermediate textures/passes are required;
- which damage in earlier/later content forces the effect to repaint;
- whether a trusted custom program is valid and bounded;
- whether the result still permits Direct Scanout;
- what happens when metadata or GPU execution fails.

### 12.2 Semantic effect authority

Effects originate as semantic compositor state. Client background blur is double-buffered with the surface and latches at commit. Trusted/private Astrea effects require the compositor's authenticated/trusted control path rather than arbitrary client GLSL.

`CompositorState::resolved_effect_scene` combines committed client background-effect semantics and trusted/internal effect assignments with scene ordering and visual-group regions. The result is `ResolvedEffectScene`, keyed to the current scene/render generation. This is the semantic effect description; it contains no physical framebuffer ownership.

### 12.3 Typed graph compilation

`compile_frame_execution_plan` converts the resolved semantic scene into one of two outcomes:

- `LegacyScene` when no visible effect graph is necessary;
- `EffectGraph(CompiledFrameGraph)` when typed passes/textures are required.

`CompiledFrameGraph` owns typed passes, textures, effect instances, final damage and statistics. Compilation resolves effect programs, footprints and backdrop dependencies, constructs pass-level texture dependencies, and can fuse compatible local stages.

This is architecturally different from chaining shaders in surface order. The graph is an intermediate representation with validation, resource accounting and explicit dependency metadata.

### 12.4 Damage and demand propagation

Backdrop effects create non-local repaint dependencies: a changed surface behind blur can require repainting the blur output even if the blur-owning surface itself did not change.

The current demand planner uses validated ordering and a reverse traversal. Each dependency edge is propagated at most once. `visited_edges` is part of the boundedness model. Missing, non-unique, cyclic/out-of-order or otherwise unrepresentable dependency metadata causes conservative treatment rather than an unsafe under-repaint.

This is an important architectural correction from a more open-ended “propagate until structural equality converges” style. Frame-critical work is now bounded by graph structure, and diagnostics report why conservative fallback was needed.

### 12.5 Trusted programs and resource limits

Trusted custom programs are configuration-controlled and validated. The registry separates configuration/load/compile lifecycle from frame execution. Tests explicitly protect against compiling or inserting shader programs during render lookup, which prevents an unbounded compile surprise in the frame-critical path.

Effect resource counts, texture/pass metadata and region operations are bounded/validated. The public/private trust distinction is a major design point: a client-visible semantic effect is not permission to inject arbitrary GPU code.

### 12.6 Renderer execution and failure

The GLES executor receives the compiled graph and executes pass stages against concrete render targets/textures. Execution produces render evidence only. If the effect path is unavailable or fails, the compositor must not mark the effect result as physically presented; fallback remains on a composition-safe path.

Visible effects block Direct Scanout unless an explicitly equivalent hardware path exists, because a direct primary-plane buffer cannot reproduce arbitrary compositor effect output.

### 12.7 Why graph compilation rather than shader chaining?

**Source-proven fact:** the source builds a typed `CompiledFrameGraph`, validates dependencies, plans per-pass demand and tracks bounded stats.

**Likely rationale:** once backdrop capture and partial repaint exist, shader order is a data-dependency problem. A typed graph makes those dependencies inspectable, allows bounded backward damage propagation, enables resource validation and provides a stable point for diagnostics and future renderer backends.

### 12.8 Trade-offs

- More compiler/planner complexity than a fixed effect stack.
- Every new pass kind must define footprint, damage and resource semantics.
- Conservative fallback can repaint more than strictly necessary.
- The payoff is deterministic execution planning, safer third-party/trusted extensibility and renderer-independent effect semantics.

### Source anchors

- `src/compositor/state/*effect*`, `src/compositor/blur_assignment.rs` — semantic effect assignment/resolution.
- `src/effects/render_graph.rs` — `CompiledFrameGraph`, `compile_frame_execution_plan`, demand planning and dependency validation.
- `src/effects/validation.rs`, `src/effects/config.rs` — program/config validation and limits.
- `src/effects/registry.rs` — trusted program registry/lookup.
- `src/egl_renderer/effects/executor.rs` — GLES graph execution.
- `src/egl_renderer/effects/trace.rs` — effect execution/repaint provenance.

### Representative tests

- `src/effects/render_graph.rs::fragmented_dependency_coverage_is_propagated_once`
- graph metadata tests rejecting missing/non-unique/not-earlier dependencies;
- tests proving no visible effects select the legacy path;
- tests proving malformed metadata produces conservative demand;
- registry test `render_lookup_never_compiles_or_inserts_a_program`;
- trace tests proving disabled effect-resolution tracing does not build snapshots.

**Status:** Implemented; product-wired for qualified renderer paths; deterministically tested. Real target-hardware performance/165 Hz qualification remains a separate evidence category.

---

## 13. Animation engine

Typhon's animation architecture is two related systems rather than one monolithic animator:

1. an **animation control plane** that says which semantic slots request which effect family and at what speed;
2. **presentation/lifecycle execution** that owns motion, visual resources and physical retirement.

### 13.1 Control plane

`AnimationSlot` defines stable semantic slots:

- window move;
- window resize;
- layout reflow;
- maximize;
- fullscreen;
- open;
- close;
- minimize;
- restore;
- workspace switch;
- workspace window move.

`AnimationEffect` currently catalogs:

- `none` — available;
- `geometry.kde` — available;
- `geometry.macos` — available;
- `window.scale` — planned;
- `window.glide` — planned;
- `minimize.lamp` — available only when runtime lamp-renderer capability exists;
- `minimize.squash` — planned;
- `workspace.slide` — planned.

The important distinction is visible in source: catalog membership does not imply executability. `is_available`, `availability_for_runtime` and `is_executable` explicitly separate planned, globally available and runtime-capability-qualified effects.

Presets are Astrea, KDE and macOS. Current Astrea/KDE geometry slots use KDE-family geometry motion; Astrea minimize/restore requests Lamp; macOS geometry slots use the macOS family.

### 13.2 Geometry/opacity/clip motion

Property animation is handled by the Presentation Engine described earlier. Canonical window geometry can jump to the semantic target while the presentation transform interpolates. This prevents the animation layer from becoming the canonical WM geometry store.

### 13.3 Lifecycle motion and retained payloads

Minimize/restore is harder because canonical content may be hidden/unmapped while its pixels must remain visible during the transition. Typhon separates four concepts:

- **motion state:** `WindowLifecycleAnimator`;
- **retained visual payload:** immutable `RetainedLifecyclePayload` held through `Arc`;
- **presentation identity:** `PresentationRetainedVisualIdentity` with exact transaction/revision;
- **physical ownership:** the currently presented lifecycle snapshot/retained member.

`RetainedLifecyclePayload` captures the visual group/effect scene needed to continue rendering after ordinary canonical visibility changes. It has its own payload ID and is stored by presentation retained identity.

### 13.4 Start, install and rollback

Lifecycle transition installation is transactional in spirit. The compositor resolves the window/group/presentation identity, installs motion, installs retained visual/executor ownership, and only then takes over the presentation geometry domain needed by Lamp. If installation fails, tests require rollback without destroying previous valid physical ownership.

### 13.5 Reversal

A minimize followed by restore should not throw away the visual payload and restart from an unrelated endpoint. `start_or_reverse` requires identity compatibility, samples previous progress, preserves continuity and scales duration to remaining distance. Tests prove reversal preserves the existing frozen retained payload/SSD snapshot where appropriate.

### 13.6 Settlement

Lifecycle mathematical settlement is not enough to release retained content if old Lamp pixels may still be physically presented. The compositor distinguishes:

- endpoint mathematically reached;
- no-visual-change settlement when exact conditions prove no pageflip replacement is needed;
- rendered replacement evidence;
- physical pageflip/ACK replacing the old lifecycle visual.

This is the same semantic/presentation/physical philosophy as property animation, applied to resource retention.

### 13.7 Effect ownership

Lamp takes the presentation geometry/effect domain for the lifecycle transition; ordinary geometry tracks are canceled/retired in a controlled way so two animators do not both claim the same visual property. Runtime slot changes can snap semantic motion while preserving old physical ownership until replacement evidence exists.

### Why separate motion from retained payload?

**Likely rationale:** motion can reverse or be reconfigured without changing the immutable content snapshot that still needs to be drawn. Coupling “current interpolation” to “which pixels/resources must remain alive” would cause premature release during reversals, cancellation or renderer fallback.

### Source anchors

- `src/animation_control/catalog.rs` — `AnimationSlot`, `AnimationEffect`, `AnimationPreset`, availability/runtime capability rules.
- `src/animation_control/config.rs`, `snapshot.rs`, `mod.rs` — control-plane configuration and effective/requested state.
- `src/presentation_animation/*` — property animation and transaction identities.
- `src/window_lifecycle_animation.rs` — `WindowLifecycleAnimator`, Lamp motion/geometry.
- `src/compositor/state/lifecycle_retained.rs` — `RetainedLifecyclePayload`, retained store.
- `src/compositor/state/lifecycle_animation.rs` — compositor install/restore/minimize integration.
- `src/compositor/state/lifecycle_animation_tests.rs` — cross-layer physical ownership regressions.

### Representative tests

- `fresh_restore_freezes_ssd_until_physical_settlement`
- `reversal_preserves_existing_frozen_ssd_snapshot`
- `failed_minimize_install_rolls_back_identity_without_taking_over_previous_state`
- `old_visible_physical_lamp_prevents_no_visual_settlement`
- `lifecycle_render_fallback_preserves_confirmed_physical_lamp_until_replacement`
- `runtime_slot_change_snaps_minimize_and_restore_but_retains_physical_ownership`

**Status:** Control plane implemented/product-wired. Geometry KDE/macOS available. Lamp available when runtime capability exists. Scale/glide/squash/workspace-slide are Planned, not supported merely because catalog IDs exist.

---

## 14. Window/workspace architecture

### 14.1 Canonical window authority

Typhon now owns meaningful WM mechanics. XDG/XWayland surfaces are adapted into canonical desktop/window state in `CompositorState`; window mapping, geometry, workspace location, focus and layout membership are not shell-owned shadow state.

A window's semantic geometry is distinct from presentation geometry. Programmatic move/resize/reflow/maximize/fullscreen updates canonical target geometry; animation projects from/to that canonical target through Presentation Engine state.

### 14.2 Floating and tiled membership

`WindowManagementState` distinguishes workspace location from layout membership. `LayoutMembership` separates Floating and Tiled. Floating geometry can be preserved as restore state when a window enters tiling or another semantic mode.

### 14.3 Dwindle

The Dwindle subsystem stores a layout tree and solves canonical rectangles under constraints. The solver can compute aggregate lower bounds, exact/aspect constraints, split ratios and output updates. It returns typed layout errors rather than silently emitting impossible geometry.

Compositor integration plans reflow and applies resulting canonical geometry across affected windows. Reflow is conceptually one WM transaction: multiple window targets are updated as one canonical layout decision rather than sequential shell commands that expose intermediate layouts.

### 14.4 Workspaces

`WorkspaceManager` owns active regular workspace state, stable typed `WorkspaceId`s, special workspaces and visible-special state. The current default product model is a fixed set of regular workspaces plus a default special workspace; this is product policy, not a fundamental architectural requirement.

Window families can migrate together. Workspace visibility gates whether tiled geometry is immediately applied or deferred. `ext-workspace-v1` publishes a protocol view of the canonical workspace model rather than becoming a separate workspace authority.

### 14.5 Focus and move/resize

Focus is compositor-owned and feeds both Wayland and XWM policy. Interactive move/resize operates through compositor input/window state, including pointer constraints and pending resize semantics. X11 configure requests are translated into canonical compositor actions instead of giving XWM an independent geometry truth.

### 14.6 Shell boundary

The source architecture explicitly keeps compositor/WM/shell layers distinct. The shell can expose decoration/UI/policy affordances and consume compositor control state, but window layout correctness, focus, workspace membership and protocol state remain compositor authorities.

This boundary is an architectural principle, while exact keybindings, workspace count and shell visual policy are implementation/product details.

### Source anchors

- `src/compositor/mod.rs` — canonical window/workspace state fields.
- `src/compositor/window_state.rs` — window mode/minimized/restore state.
- `src/compositor/state/windows.rs`, `desktop_windows.rs`, `window_interaction.rs` — canonical window lifecycle/focus/interaction.
- `src/compositor/state/tiled_layout.rs` — tiled layout integration.
- `src/wm/layout/*` — Dwindle tree/solver/constraints.
- `src/wm/workspace.rs` — `WorkspaceManager`.
- `src/compositor/protocols/workspace.rs` — ext-workspace protocol projection.

### Representative tests

- workspace tests proving activation/migration is atomic and protocol publication is independent from private Astrea control state;
- tiled-layout tests proving split/reflow behavior and floating-geometry preservation;
- `window_interaction_tests::tiled_resize_rebases_the_split_handle_without_replacing_canonical_geometry`;
- scene tests proving stable window-group identity across XWayland backing replacement.

**Status:** Workspaces and Dwindle are implemented/product-wired. Multi-output workspace policy is not implemented because the product remains single physical output.

---

## 15. Wayland protocol integration

### 15.1 The protocol surface is an adapter into canonical state

Typhon's protocol architecture is best documented by *state flow*, not by a checklist of globals. A protocol object does not become a new source of truth simply because the protocol has complex pending state.

The common pattern is:

```text
Wayland request
  -> protocol resource / SurfaceData pending state
  -> validation/capability/authentication
  -> commit/capture boundary
  -> semantic compositor transaction/state
  -> scene/presentation/output consequences
```

### 15.2 Pending/committed semantics

Surface-affecting protocols such as viewport, background effect, explicit synchronization, FIFO, Commit Timing, tearing hint/content type and ordinary `wl_surface` state capture at commit boundaries. Synchronized subsurface state is held until the owning parent/tree transaction publishes.

This preserves Wayland's double-buffered semantics and prevents a protocol request from changing physical presentation before the associated content commit exists.

### 15.3 Capability gating

`server_globals.rs` registers globals according to capability bundles. Input, selection, renderer, frame-pacing, presentation and GPU-buffer capabilities are separate. This allows unsupported functionality to be absent rather than exposed as a half-working protocol.

The current production presentation-capability constructor mismatch is a failure of *capability-source unification*, not evidence against the model itself: tearing/content-type are gated correctly, but the real product constructor supplies the wrong capability bundle.

### 15.4 Authentication

Private Astrea protocols use an explicit Astrea shell authorization boundary. For example, `astrea_screen_capture_manager_v1` is globally visible but a capture request checks `astrea_shell_mutation_allowed`, verifies that the `wl_output` resource is current and permits only one pending capture per client.

This distinction matters for documentation: “global exists” does not mean arbitrary clients have authority to mutate/capture compositor state.

### 15.5 Resource lifetime

Resource destruction is part of transaction validity. Pending surface-tree publications capture lifetimes and can be rejected if a resource/client/role generation no longer matches. Protocol errors are posted to the offending client/resource rather than poisoning global compositor state where possible.

### 15.6 Frame pacing protocols

FIFO and Commit Timing are real transactional compositor features. Their obligations are captured into exact surface transactions. FIFO can protect a transaction from unsafe coalescing; Commit Timing contributes readiness lower bounds/deadlines. They are product-wired through `FramePacingProtocolCapabilities::qualified_native()`.

### 15.7 Presentation protocols

Tearing control and content type have implementation/state/KMS integration, but current `main` suppresses advertisement by using a constructor with `PresentationProtocolCapabilities::safe_baseline()`. Correct current classification is:

| Feature | Implementation | Transaction/KMS integration | Product global from current `main` |
|---|---:|---:|---:|
| FIFO | Yes | Yes | Yes |
| Commit Timing | Yes | Yes | Yes |
| Tearing control | Yes | Yes | **No — constructor mismatch** |
| Content type | Yes | Yes | **No — constructor mismatch** |

### 15.8 Selection protocols

Wayland clipboard, PRIMARY selection and ext data control are enabled by the production `core_clipboard()` selection profile. `SelectionState` is the canonical broker. Protocol-specific offers/adapters project that source rather than maintaining separate clipboard truths.

### 15.9 Screenshot/capture

`astrea_screen_capture_v1` is implemented and unconditionally registered as a private Astrea global. Requests are authenticated, output-resource checked and bounded to one pending request/client. Native capture exports normalized RGBA8888 in a sealed memfd, with a 256 MiB hard payload bound and no claim of physical-presentation authority.

This is not equivalent to general desktop capture. The local portal backend advertises Settings, Notification and Access only; no ScreenCast/Screenshot portal or PipeWire streaming stack is present in the supplied source.

### 15.10 Input capability contract

The native input profile advertises relative pointer, pointer constraints, pointer warp, cursor shape, keyboard-shortcuts inhibit and idle inhibit. `wl_touch` is explicitly not advertised; `get_touch` is a required `missing_capability` protocol error. Cursor-shape supports pointer devices; tablet-tool creation is not advertised as capability.

### Source anchors

- `src/compositor/server_globals.rs` — global registration/auth-capability boundary.
- `src/compositor/plan.rs` — capability bundles and advertised protocol calculation.
- `src/compositor/state_data.rs` — surface pending state.
- `src/compositor/protocols/*` — protocol adapters into canonical state.
- `src/compositor/selection.rs`, `state/selection_runtime.rs` — canonical selection authority.
- `src/compositor/screen_capture.rs`, `src/native_output/screen_capture.rs` — private capture authority/export.
- `src/portal.rs` — portal backend interface list.

**Status:** Protocol breadth is mixed by feature. Documentation must never collapse “protocol XML/implementation exists” into “advertised by current product.”

---

## 16. XWayland/XWM

### 16.1 Architectural role

XWayland is an adapter service, not a second compositor authority. Typhon owns the XWayland child lifecycle and an XWM that translates X11 windows, focus, geometry, stacking and selection into canonical compositor concepts.

### 16.2 Process lifecycle and generation

`XwaylandMode` defaults Off. Opt-in values include base/lazy/eager managed modes; only managed profiles reach the full XWM integration path. `XwaylandService` owns a generation, private display lease, Xauthority, private Wayland/XWM sockets, displayfd, stderr ring and child identity.

A restarted XWayland receives a new generation. X11 window handles include generation plus XID, preventing an XID reused by a new server instance from completing old ownership.

### 16.3 XWM startup/adoption

Managed XWM startup is gated: root redirection, Composite requirements, EWMH/supporting WM state, existing-window adoption and selection/XWM readiness are progressed before Running. Composite is required for the current rootless architecture. XFixes, Shape, RandR and Sync are optional/version-gated capabilities rather than prerequisites for basic XWM Running.

### 16.4 Managed windows

Normal managed and override-redirect windows have distinct policy. The XWM implements/adapts:

- ICCCM focus and `WM_TAKE_FOCUS`;
- configure requests/notify;
- stacking/transient relationships;
- `WM_STATE` and implemented EWMH state;
- Shape capability paths;
- XSync resize machinery;
- generation cleanup and private display ownership.

X11 window state is projected to canonical Typhon window/group state. XWM requests do not become a second geometry/workspace database.

### 16.5 Request stream and boundedness

The XWM's serialized output queue preserves byte ordering across short writes/EAGAIN and imposes a hard bound. This is an example of the same project philosophy applied outside KMS: nonblocking I/O should not create an implicit unbounded queue or reorder accepted protocol bytes.

### 16.6 Selection: active X11 -> Wayland direction

The current production path actively supports external X11 CLIPBOARD and PRIMARY becoming canonical Wayland selections. XFixes ownership, TARGETS/MIME discovery and direct/incoming-INCR payload reading feed `SelectionState`; data-control and Wayland clients consume the canonical source.

Generation and source keys prevent stale XWayland clear/events from deleting a newer Wayland/host source.

### 16.7 Wayland -> X11: advanced foundation, still inactive end-to-end

The source contains significantly more reverse-bridge machinery than a casual docs read suggests:

- generation-qualified proxy snapshots of canonical Wayland selections;
- MIME-to-atom catalog resolution;
- `selection_proxy` request state;
- bounded outgoing data transfers;
- outgoing INCR backpressure through X11 property deletion;
- MULTIPLE handling and regression tests;
- runtime code that can service proxy data requests by asking the canonical Wayland source.

However, tracing production callers changes the classification. `selection_proxy::handle_selection_request()` — the handler that admits X11 `SelectionRequestEvent` and starts direct/MULTIPLE/outgoing transfer — has no production caller in the supplied snapshot. `xwm/events.rs` handles `SelectionNotify` and XFixes notifications, but not `SelectionRequest`. In addition, `selection_wire::submit_proxy_selection_snapshots` explicitly states that it schedules metadata/catalog resolution and **does not mutate X11 selection ownership or handle conversion requests**.

Therefore Wayland -> X11 ownership/payload serving, outgoing INCR and MULTIPLE are correctly classified as **inactive foundation**, despite deep implementation/tests. This is a textbook example of why test coverage must not be treated as product wiring.

### 16.8 XDND, RandR and cursor boundaries

- XDND has adapter/model state, but no live end-to-end ClientMessage bridge was proven: **inactive foundation**.
- runtime RandR output publication remains inactive foundation; capability/version handling is not the same as publishing current Typhon output topology.
- X11 cursor ownership integration remains inactive foundation.

### 16.9 Remaining interoperability philosophy

The healthy architectural direction is visible even where wiring is incomplete: the Wayland selection broker remains canonical, and X11 should be an adapter around it. Completing the reverse bridge should connect the existing proxy ingress to production X11 event handling rather than inventing a second clipboard authority.

### Source anchors

- `src/xwayland/config.rs` — mode parsing/default policy.
- `src/xwayland/service.rs` — `XwaylandService`, generation-bound process/XWM lifecycle.
- `src/xwayland/xwm/mod.rs`, `events.rs`, `window.rs`, `commands.rs`, `properties.rs` — XWM state/event integration.
- `src/xwayland/xwm/selection_wire.rs` — incoming X11 selection and reverse metadata foundation.
- `src/xwayland/xwm/selection_payload.rs` — incoming payload/INCR handling.
- `src/xwayland/xwm/selection_proxy.rs` — reverse request model; `handle_selection_request` has test callers but no production caller.
- `src/xwayland/xwm/selection_outgoing.rs` — reverse outgoing direct/INCR transport foundation.
- `src/native_output/runtime/xwayland.rs` — runtime synchronization to/from canonical compositor selection.
- `src/compositor/tests/xwayland_selection.rs` — canonical selection generation/loop-prevention tests.

### Representative tests

- managed startup and generation tests in `src/xwayland/*`;
- XSync alarm/counter resize tests;
- selection tests proving X11 offer -> canonical selection and stale clear rejection;
- reverse proxy regression tests for MULTIPLE/outgoing INCR — these prove foundation behavior, **not** product ingress wiring.

**Status:** XWayland/XWM implemented and product-wired when explicitly enabled; default Off. Window-management core active. X11 -> Wayland CLIPBOARD/PRIMARY active. Reverse selection/INCR/MULTIPLE foundation deep but not end-to-end product-wired. XDND/RandR publication/X11 cursor ownership inactive foundations.

---

## 17. Input architecture

### 17.1 Seat authority

Typhon's compositor state owns logical seat resources/focus; native input backends supply device events. Protocol serial/focus state is canonical at the compositor boundary, while backend-specific libinput/device details remain native.

The native capability profile enables:

- relative pointer;
- pointer constraints;
- pointer warp;
- cursor shape for pointer path;
- keyboard-shortcuts inhibition;
- idle inhibition.

Keyboard and pointer are the intended current desktop input classes.

### 17.2 Pointer motion and constraints

Absolute motion, relative motion, pointer lock/confine and one-shot warp are modeled separately. A locked pointer suppresses absolute movement while preserving relative deltas. Constraint activation is tied to committed surface/focus state; stale serials and wrong-client resources do not gain authority.

Backend activation/deactivation and compositor protocol state have explicit reconciliation paths so a backend failure cannot leave a protocol constraint permanently “active” in semantic state.

### 17.3 Focus and grabs

Pointer focus, implicit button grabs and keyboard focus are compositor-owned. Implicit grabs keep delivery pinned until terminal button release even when coordinates leave the original surface. Surface destruction tears down the grab/constraint rather than leaving stale input ownership.

### 17.4 Shortcut inhibition

Client shortcut inhibition changes ordinary compositor shortcut routing for the focused/inhibiting client. Emergency/session-critical compositor actions remain outside ordinary client authority. The inhibitor has a generation/effective-state model and is reconciled against focus/policy.

### 17.5 Cursor

Client cursor resources, protocol cursor-shape state, compositor resize/move cursor policy and native hardware/software rendering are different layers. The compositor selects desired logical cursor; native output decides whether it can be a hardware cursor for the exact KMS state. Software fallback is an explicit presentation choice.

### 17.6 Unsupported classes

The following should be documented as **unsupported input classes/protocol breadth**, not as flaws in the existing keyboard/mouse architecture:

- `wl_touch` is explicitly not advertised;
- cursor-shape tablet-tool creation is not advertised;
- no broad production pointer-gestures stack was found;
- no broad tablet stack was found;
- no text-input/input-method stack was found;
- no virtual keyboard/pointer product stack was found.

### Source anchors

- `src/compositor/plan.rs` — `InputProtocolCapabilities::native_libinput`.
- `src/compositor/protocols/input.rs` — seat resource creation and explicit touch rejection.
- `src/compositor/protocols/cursor_shape.rs` — pointer cursor-shape adapter.
- `src/compositor/state/input_dispatch.rs`, `input_resources.rs`, `pointer_constraints.rs`, `window_interaction.rs` — logical routing/focus/constraint state.
- `src/native_output/input/*` — libinput/native event normalization/routing and shortcut policy.
- `src/native_output/runtime/presentation_cursor.rs` — presentation/KMS cursor evidence.

### Representative tests

- pointer constraint tests for activation, lock, confinement, stale serials and destruction;
- relative-pointer tests ensuring raw delta continues under lock;
- protocol error test `get_touch_without_advertised_touch_capability_is_a_wire_error`;
- shortcut-inhibit tests preserving emergency compositor behavior;
- hardware/software cursor transition tests.

**Status:** Keyboard/mouse architecture implemented/product-wired. Device-class breadth is intentionally incomplete.

---

## 18. Process/session/resource ownership

### 18.1 Child supervision

`ChildSupervisor` owns child records, monotonic internal process IDs, PID/PGID relationships, restart suppression, quiescing and SIGCHLD processing. The compositor blocks SIGCHLD before worker/graphics startup so one controlled process subsystem owns reaping semantics.

Session-owned children are distinct from ordinary applications. Shutdown begins by quiescing new session work, signals only session-owned children with TERM, waits a bounded grace period, then KILLs remaining session-owned children if necessary. Unrelated applications are not implicitly destroyed with the compositor.

### 18.2 Process groups

Session-owned helpers can receive dedicated process groups so group cleanup is precise. Process-group identity is tracked rather than inferred from command strings at shutdown.

### 18.3 XWayland as a supervised resource

XWayland is a particularly strict child: generation, private sockets, display lease and Xauthority artifacts are part of the service ownership. Cleanup removes only artifacts proven to belong to the current lease/generation; security tests protect against symlink/replacement races.

### 18.4 Application scopes

`ApplicationScopePolicy` supports Auto/On/Off. When the systemd user-scope helper is used, spawn/migration status is communicated through a bounded internal helper/status protocol. Scope/migration failure can fall back to direct spawning rather than leaving the application half-owned.

The scope mechanism is an Astrea-specific resource-management integration, not a portable Wayland compositor requirement.

### 18.5 Foreground dmem

The optional foreground dmem subsystem manages `dmem.low` only when the kernel/driver/cgroup environment exposes the expected controller. It validates process/cgroup identity and generations before writing. Unsupported environments degrade rather than being advertised as universal memory QoS.

### 18.6 Session authority

Seat/session state is the authority for whether native DRM/input I/O may occur. The recovery state machine explicitly prevents native output I/O while suspended. Destructor/shutdown paths are also guarded so losing session ownership cannot trigger unsafe late DRM calls.

### Source anchors

- `src/process.rs` — `ChildSupervisor`, process records, shutdown.
- `src/main.rs` — early `block_sigchld_for_current_thread` call.
- `src/xwayland/service.rs`, `fs_security.rs`, `auth.rs` — generation/private-resource ownership.
- `src/application_scope.rs` — scope policy/helper/status/fallback.
- `src/native/dmem_foreground.rs` (and related modules) — foreground dmem policy/identity checks.
- `src/native_output/runtime/session.rs`, `session_io.rs` — session authority over native I/O.

**Status:** Implemented/product-wired; application scopes/dmem are optional/environment-dependent. Session/process ownership is a stable architectural core.

---

## 19. Observability architecture

### 19.1 Observability as ownership evidence

Typhon's diagnostic design is unusually aligned with its state machines. Traces are often keyed by the same IDs used for correctness: transaction ID, revision, output generation, pageflip token, physical frame key, worker reservation and XWayland generation. This makes logs useful for answering “which ownership was rejected?” instead of only “frame failed.”

### 19.2 Native performance and pacing

Native perf logging records frame/pacing timing, queue/worker behavior and presentation cadence. Predictive O1 has lifecycle metrics rather than only aggregate FPS. KMS timing distinguishes different miss causes. Slow-cycle tracing records which work domains consumed a cycle.

### 19.3 Presentation evidence

The runtime maintains a bounded presentation transaction trace ring. It can report built/queued/submitted/presented/terminal ownership and explain whether a physical event matched current generation/token. Scene history and plane snapshots give post-hoc evidence about what actually became presented.

### 19.4 Direct Scanout blockers

Direct Scanout rejection reasons are stable, named diagnostic facts. This is important for public debugging documentation: “Direct Scanout is off” should be explainable as effects, geometry, modifier, cursor, sync, KMS worker, TEST_ONLY or another precise blocker.

### 19.5 Effects evidence

Effects emit graph/demand statistics, pass/provenance information and optional GPU timing. The repaint-provenance trace captures input/scene/merged damage and initial/final repaint plans. Disabled tracing paths are tested to avoid constructing expensive snapshots when observability is off.

### 19.6 XWayland diagnostics

XWayland has bounded stderr capture, trace categories, session-check tooling and XWM event/state diagnostics. Generations are visible in the ownership model, which is critical when differentiating current versus stale child events.

### 19.7 Resource/process diagnostics

Application scopes, dmem, cursor workers, keyboard persistence and shutdown expose state/doctor snapshots rather than failing silently. The native control path aggregates current runtime health without becoming an authority that can overwrite subsystem state arbitrarily.

### 19.8 Philosophy

The source supports the principle that an optimization should be able to explain **why it was rejected**. This is strongest in Direct Scanout/effects/pacing and is valuable enough to make a public design-philosophy page.

The corresponding constraint is that observability must remain bounded and cheap when disabled. Trace rings have capacities; GPU timers and snapshot construction are gated; diagnostic state should not create a second unbounded frame-critical workload.

### Source anchors

- `src/native_output/perf.rs` and runtime metrics modules — native performance counters/logging.
- `src/native_output/presentation/trace.rs` — presentation transaction tracing.
- `src/native_output/runtime/scene_history.rs` — physical scene evidence.
- `src/native_output/scanout/direct_policy.rs` / blocker definitions — stable Direct Scanout reasons.
- `src/egl_renderer/effects/trace.rs`, `src/effects/render_graph.rs` — repaint/effect evidence.
- `src/xwayland/trace.rs`, service stderr/session tooling — XWayland diagnostics.
- `src/native_output/runtime/cycle_dispatch.rs` / control snapshot functions — runtime status/doctor aggregation.

**Status:** Implemented/product-wired and a major architectural strength. Public docs should teach identifiers and rejection reasons, not merely list environment variables.


---

## 20. Confirmed Typhon design principles

This section tests the proposed philosophy against current source. A principle is marked **Confirmed** only when multiple independent production subsystems enforce it. “Partially confirmed” means the code supports the principle but has a current counterexample or incomplete migration.

### 20.1 Explicit ownership — **Confirmed**

**Evidence 1: native output transactions.** `OutputTransactionLedger` records one state-machine owner for each output transaction and maps protocol obligations to exactly one transaction. Duplicate obligation ownership is rejected.

**Evidence 2: KMS submission.** `AtomicCommitArbiter` explicitly separates `kernel_submitted` and `worker_queued` ownership and refuses an extra queued-next owner.

**Evidence 3: explicit sync.** `ExplicitSyncWatchRegistry` maps commit -> exact watch token and terminalizes superseded/current ownership rather than accumulating ambiguous fence listeners.

**Evidence 4: lifecycle visuals.** retained visual identity and payload ownership are explicit; logical cancellation does not imply the old physical visual disappeared.

**Evidence 5: processes.** `ChildSupervisor` differentiates session-owned children from ordinary launched applications and shuts down only the former.

**Architectural consequence:** public docs should describe owners/state machines, not just data flow.

### 20.2 Immutable identities — **Confirmed**

**Evidence 1:** `OutputFrameKey` contains physical identity and deliberately excludes mutable presentation target state.

**Evidence 2:** `PresentationTransactionId` and property revision IDs bind presentation ACKs.

**Evidence 3:** `OutputIdAllocator` produces typed nonzero logical output IDs and does not use a mutable connector pointer as product identity.

**Evidence 4:** X11 windows use generation-qualified `X11WindowHandle` rather than raw XID alone.

**Evidence 5:** predictive attempts and worker reservations have identities separate from physical frame identity.

### 20.3 Bounded queues — **Confirmed**

Concrete bounds appear in independent subsystems:

- synchronized commit cache: per-surface, per-client and global caps;
- KMS atomic lane: one kernel-submitted + one worker-queued-next;
- XWM serialized output buffering: hard bounded queue;
- selection transfers/catalogs: bounded MIME/request/transfer counts;
- effects resources/pass planning: validated bounded structures;
- screen capture: one pending request/client and 256 MiB payload bound;
- diagnostic rings: bounded history;
- application-scope helper/status state: bounded pending resources.

This is not an isolated implementation habit; it is a repeated system-level constraint.

### 20.4 Bounded work in frame-critical paths — **Confirmed**

**Effects:** reverse demand propagation visits a dependency edge at most once; conservative fallback replaces open-ended convergence.

**KMS:** pipeline depth is explicit and bounded.

**Shader registry:** render lookup is tested not to compile/insert programs during frame execution.

**Event loop:** wake-domain classification and continuation budgets avoid indiscriminate work each cycle.

**Caveat:** some source modules remain large/complex; bounded algorithmic work does not imply low code complexity.

### 20.5 Exact generation validation — **Confirmed**

- DRM/session generation qualifies pageflips and explicit-sync watches.
- XWayland generation qualifies X11 handles/service events.
- selection channel generation/source key rejects stale clear/data requests.
- surface publication lifetimes/generations reject stale async transactions.
- Presentation Engine transaction/revision identity rejects stale ACKs.

The recurring rule is stronger than “latest wins”: stale work must prove it still belongs to current authority.

### 20.6 Transactional state — **Confirmed**

Multiple layers intentionally form transactions:

- Wayland surface pending state -> `wl_surface.commit`;
- synchronized subtree -> `PendingSurfaceTreeTransaction`;
- presentation properties -> `PresentationTransactionId` + revisions;
- native output -> `OutputTransaction`;
- KMS atomic state -> TEST_ONLY/real commit;
- XWM resize Sync state -> explicit acknowledgement/release lifecycle.

The architecture is not one global transaction. Each authority domain owns a transaction whose evidence is handed to the next domain.

### 20.7 Semantic state separated from physical presentation state — **Confirmed, fundamental**

This is one of the strongest principles in current source.

- canonical window geometry can change while a presentation transform interpolates;
- rendered scene snapshots do not advance `NativeSceneHistory.presented`;
- submitted KMS state is not presented until pageflip;
- mathematically settled animations remain owned until exact physical ACK;
- lifecycle retained payload can outlive canonical visibility;
- “last rendered scene generation” is explicitly not physical presentation evidence.

### 20.8 One canonical authority per concept — **Confirmed with one current capability-truth exception**

**Positive examples:**

- `CompositorState` for semantic compositor state;
- `SelectionState` for clipboard/PRIMARY truth;
- `WorkspaceManager` for workspace activation;
- `OutputTransactionLedger` for output transaction lifecycle;
- `NativeSceneHistory`/presented-plane snapshot for physical presentation evidence;
- X11 adapters feed canonical compositor state rather than becoming peer authorities.

**Current exception:** capability truth is fragmented between capability factories/convenience constructors and `main`, demonstrated by tearing/content-type being qualified in one constructor and suppressed by the production constructor. This is a violation of the philosophy at the product-configuration level, not a counterexample to semantic-state architecture.

### 20.9 Safe fallback instead of undefined behavior — **Confirmed**

Examples:

- Direct Scanout -> composition;
- KMS worker Auto startup/eligibility failure -> synchronous submission;
- hardware cursor -> software cursor when policy permits;
- effect metadata uncertainty -> conservative repaint;
- no visible effects -> legacy scene path;
- application-scope helper failure -> direct spawn fallback;
- unsupported protocol capability -> do not advertise / explicit protocol error;
- XWayland default -> Off rather than auto-start an unqualified bridge.

### 20.10 Conservative hardware qualification — **Confirmed**

- Direct Scanout default Off / experimental opt-in;
- KMS worker default Off;
- tearing policy default Off;
- VRR discovery/planning exists but code/docs explicitly do not write `VRR_ENABLED`;
- Direct Scanout requires exact format/modifier/plane/sync/cursor/presentation proof and TEST_ONLY;
- effects/native performance qualification is kept distinct from deterministic correctness.

### 20.11 Optimizations must prove eligibility — **Confirmed**

Direct Scanout is the clearest example, but Predictive O1 and partial-effect repaint follow the same pattern. The optimized path has capability/evidence gates; the safe path remains the fallback.

### 20.12 Stale work must not complete current ownership — **Confirmed**

Cross-subsystem examples:

- stale pageflip token/generation cannot present a current output transaction;
- stale presentation revision cannot retire a new track;
- stale XWayland generation cannot manage current XIDs;
- stale selection generation cannot clear a newer source;
- stale surface lifetime cannot publish a destroyed tree;
- stale recovery fence cannot reactivate a new session generation.

### 20.13 Completion must be evidence-driven — **Confirmed**

- surface tree: all readiness dependencies satisfied/terminalized;
- renderer: render completion creates rendered evidence only;
- KMS: successful submit creates Submitted, pageflip creates Presented;
- animation: exact physical ACK retires final presentation member;
- explicit sync: actual signaled point plus matching generation;
- process: wait/SIGCHLD status, not elapsed timeout alone;
- XWM Sync: exact counter/alarm acknowledgement.

### 20.14 Observability should explain rejection/fallback reasons — **Confirmed**

Direct Scanout blocker names, effect demand reasons, KMS/pacing lifecycle metrics, presentation traces, XWayland diagnostics and resource-management doctor state all support this principle.

### 20.15 Deterministic state-machine testing — **Confirmed**

The repository contains dense tests around transaction transitions rather than only screenshot/end-to-end tests. Presentation revisions, scene-history promotion, KMS arbiter state, session recovery, synchronized subsurface lineage, XWM generation/selection and lifecycle animation physical ownership are all tested as explicit state machines.

### Overall philosophy conclusion

The proposed Typhon philosophy is strongly supported by current source. The most fundamental principles are:

1. explicit ownership;
2. stable identity separate from mutable policy;
3. transactional publication;
4. semantic/presentation/rendered/physical separation;
5. exact stale-work invalidation;
6. bounded queues/work;
7. evidence-driven completion;
8. conservative optimization qualification and safe fallback;
9. observability keyed to rejection/ownership evidence.

Features such as a fixed workspace count, a particular animation curve, Lamp as the Astrea minimize effect, or a specific predictor coefficient are **implementation/product choices**, not fundamental philosophy.

---

## 21. ADR candidates

The following decisions are strong enough in current source to deserve public ADRs. Exact Git commits are not available in the supplied source archive, so references use current symbols/tests and existing docs rather than invented commit provenance.

### ADR-001 — Separate semantic state from rendered and physically presented state

**Context:** render-ahead, animation, KMS submission and Direct Scanout create states where canonical truth and visible pixels legitimately differ.  
**Problem:** one mutable “current frame/current geometry” value cannot represent semantic intent, rendered resources and pageflip-confirmed physical truth without races.  
**Constraints:** asynchronous GPU/KMS; session loss; predictive rendering; lifecycle animations; presentation feedback.  
**Alternatives:** (a) canonical state doubles as visual state; (b) “last rendered” equals presented; (c) explicit multi-stage evidence.  
**Chosen design:** canonical `CompositorState`, separate presentation snapshot, rendered/submitted `NativeSceneHistory`, and pageflip-promoted presented state.  
**Why this appears chosen:** source/tests repeatedly reject promotion without physical evidence.  
**Trade-offs:** more state projections and identities; far better causal correctness.  
**Invariants:** render != present; submit != present; stale pageflip cannot regress current presentation.  
**Failure modes:** lost evidence can delay retirement; projection bugs can retain too long, but should not falsely present.  
**Affected subsystems:** compositor, Presentation Engine, renderer, KMS, effects, lifecycle animation.  
**Source:** `src/compositor/mod.rs`; `src/native_output/runtime/scene_history.rs`; `src/compositor/presented_frame.rs`.  
**Tests:** scene-history physical promotion tests; Presentation Engine ACK tests.  
**Status:** **Stable**.

### ADR-002 — Use immutable `OutputFrameKey` for physical frame identity

**Context:** predictive scheduling may retarget presentation time after a framebuffer is rendered.  
**Problem:** using mutable scheduling target as frame identity can make the same physical buffer appear to be a different lifecycle object.  
**Constraints:** render-ahead, worker queueing, triple buffering, pageflip correlation.  
**Alternatives:** mutable identity snapshot; framebuffer ID alone; composite stable key.  
**Chosen design:** `OutputFrameKey` includes output, frame, protocol batch, transaction, slot, framebuffer, render generation and pool generation, excluding mutable target state.  
**Trade-offs:** wider keys and more propagation; exact physical matching.  
**Invariants:** target mutation does not change physical identity; pool/framebuffer reuse cannot alias an older frame.  
**Failure modes:** missing a physical discriminator would allow aliasing; including mutable policy would recreate instability.  
**Affected:** pacing, swapchain, KMS worker, scene history.  
**Source/tests:** `src/native_output/scanout/output_swapchain.rs`; predictive-O1/worker-pacing tests including target-mutation preservation.  
**Status:** **Stable**.

### ADR-003 — Bound the asynchronous KMS lane to one submitted plus one queued-next commit

**Context:** KMS submission can block or add CPU latency; a worker can overlap preparation/submission.  
**Problem:** a generic asynchronous queue hides latency, permits stale buildup and weakens pageflip causality.  
**Constraints:** pageflip ordering, kernel in-flight state, predictive buffering, shutdown/session recovery.  
**Alternatives:** synchronous only; arbitrary worker FIFO; one submitted + one queued-next.  
**Chosen design:** `AtomicCommitArbiter { kernel_submitted, worker_queued }`.  
**Why:** current tests explicitly reject deeper ownership and distinguish queued from submitted.  
**Trade-offs:** less throughput speculation; bounded latency and explainable ownership.  
**Invariants:** at most two lane positions; queued-next cannot be completed as submitted before worker transition.  
**Failure modes:** worker rejection/timeouts require exact rollback/quarantine.  
**Affected:** KMS worker, pacing, cursor/plane changes, Direct Scanout.  
**Source/tests:** `runtime/atomic_commit.rs`; worker/runtime tests.  
**Status:** **Stable**, worker policy still **Evolving/qualification-dependent**.

### ADR-004 — Make `OutputTransactionLedger` the native presentation transaction authority

**Context:** frame callbacks, presentation feedback, releases, direct/composited content and pageflip ownership converge at output submission.  
**Problem:** independent callback/release/pageflip queues can double-complete or lose obligations.  
**Alternatives:** per-protocol completion queues; implicit ownership in swapchain slots; explicit ledger.  
**Chosen design:** immutable `OutputTransaction` plus ledger state machine with obligation-owner maps and exact terminal transitions.  
**Trade-offs:** transaction construction is more verbose; terminal accounting is auditable.  
**Invariants:** one obligation owner; exact token/generation presentation; one terminal outcome.  
**Failure modes:** insertion capacity/duplicate ownership rejected rather than silently overwritten.  
**Affected:** frame callbacks, presentation feedback, explicit sync releases, KMS, Direct Scanout.  
**Source:** `native_output/presentation/transaction.rs`, `ledger.rs`.  
**Status:** **Stable**.

### ADR-005 — Require exact generation/token validation for native completion

**Context:** DRM/session/XWayland resources can be torn down and recreated while asynchronous events remain in flight.  
**Problem:** late events from old ownership can corrupt current state.  
**Alternatives:** best-effort “latest token”; global reset without qualified identities; exact generation validation.  
**Chosen design:** DRM generation + pageflip token, XWayland generation + XID, sync watch generation, selection generation/source key.  
**Trade-offs:** more bookkeeping; stale events become harmless and measurable.  
**Invariants:** old generation can never complete new generation.  
**Failure modes:** if a generation is not propagated to a boundary, that boundary becomes a stale-work risk.  
**Affected:** KMS, explicit sync, XWayland, selection, session recovery.  
**Status:** **Stable**.

### ADR-006 — Retire Presentation Engine members only on exact physical ACK

**Context:** an interpolation can mathematically finish before its final pixels are physically presented.  
**Problem:** retiring on time/math can free/reassign presentation ownership while older pixels remain on screen.  
**Alternatives:** retire at duration end; retire at render completion; retire at exact pageflip-derived ACK.  
**Chosen:** exact transaction/revision/value/output ACK after physical presentation.  
**Trade-offs:** retained state may live longer; avoids premature retirement.  
**Invariants:** wrong output/stale revision cannot retire; settled-but-unacked remains active evidence.  
**Affected:** geometry, opacity, clip, lifecycle transitions.  
**Source/tests:** `presentation_animation/engine.rs`, transaction tests.  
**Status:** **Stable**.

### ADR-007 — Separate lifecycle motion from retained visual payload

**Context:** minimize/restore can hide canonical content while pixels must remain renderable; reversal should preserve the current visual.  
**Problem:** coupling payload/resources to current motion object causes premature release or re-capture on reversal.  
**Alternatives:** copy every frame; keep canonical surface mapped; retain immutable payload separately.  
**Chosen:** `WindowLifecycleAnimator` motion + `RetainedLifecyclePayload` + presentation retained identity + physical snapshot.  
**Trade-offs:** extra retained memory/state; correct reversal/physical ownership.  
**Invariants:** reversal can preserve payload; logical cancellation cannot erase physically presented old lifecycle visual.  
**Affected:** minimize/restore, decorations, effects, renderer, Presentation Engine.  
**Status:** **Evolving**, core separation appears stable.

### ADR-008 — Treat Direct Scanout as an exact proof with conservative fallback

**Context:** direct plane scanout bypasses composition.  
**Problem:** false-positive eligibility breaks visible equivalence or synchronization.  
**Constraints:** modifiers, geometry, cursor, effects, sync, animation, presentation mode, content type.  
**Alternatives:** fullscreen heuristic; force mode; exact semantic + KMS proof.  
**Chosen:** semantic scene analysis + broad `DirectPlaneValidationKey` + TEST_ONLY + real submit; default Off/experimental.  
**Trade-offs:** false negatives/more composition; strong correctness/auditability.  
**Invariants:** unproven state falls back; any key input change invalidates proof.  
**Status:** **Experimental/Evolving**.

### ADR-009 — Model explicit sync as exactly-once terminal ownership

**Context:** acquire/release fences cross surface transactions, GPU composition, direct scanout, KMS worker and session recovery.  
**Problem:** “just wait on fence” is insufficient when work can be superseded, destroyed or moved to a new DRM generation.  
**Alternatives:** ad hoc fence callbacks; registry/obligation state machines.  
**Chosen:** generation-qualified acquire watch registry + explicit release obligations carried through output transaction.  
**Trade-offs:** more terminal paths; no silent leak/double release.  
**Status:** **Stable/Evolving** as more backends qualify.

### ADR-010 — Use a typed effects graph with bounded reverse demand propagation

**Context:** backdrop effects create dependencies on earlier scene content.  
**Problem:** linear shader chains and open-ended damage propagation are difficult to validate/bound.  
**Alternatives:** always full repaint; iterative fixpoint; typed DAG + reverse traversal.  
**Chosen:** `CompiledFrameGraph` + validated earlier dependencies + each edge visited at most once, with conservative fallback.  
**Trade-offs:** compiler/planner complexity; bounded frame-critical work and better partial repaint.  
**Status:** **Stable recent design**.

### ADR-011 — Preserve one canonical selection authority and adapt X11 around it

**Context:** Wayland clipboard, PRIMARY, data-control and X11 selection semantics overlap but have different ownership protocols.  
**Problem:** two independent clipboard states create loops/stale ownership.  
**Alternatives:** peer Wayland/X11 authorities; canonical Wayland broker + X11 adapter.  
**Chosen:** `SelectionState` generation/source-key authority; X11 incoming events publish into it; reverse proxy foundation reads from it and refuses XWayland-origin loopback.  
**Trade-offs:** adapters are complex; current reverse bridge remains not end-to-end wired.  
**Invariants:** stale generation cannot clear newer source; XWayland-origin source is not mirrored back.  
**Status:** **Evolving**; incoming direction active, reverse direction inactive foundation.

### ADR-012 — Keep shell policy separate from compositor authority

**Context:** Astrea shell needs rich customization, but WM/protocol correctness cannot depend on a mutable shell UI process.  
**Problem:** shell-owned geometry/workspaces would create duplicate authority and fragile recovery.  
**Alternatives:** shell as WM; compositor canonical WM with shell as policy/UI.  
**Chosen:** compositor owns window/workspace/focus/layout/protocol state; shell is external/deferred policy/UI layer.  
**Trade-offs:** explicit control/protocol APIs are needed; compositor remains independently correct.  
**Status:** **Stable direction, evolving interface**.

### ADR-013 — Introduce non-recycled typed `OutputId` before multi-output product support

**Context:** physical DRM generations can change while logical output identity should remain stable; future multi-output needs typed identity.  
**Problem:** connector/CRTC IDs are physical/session-scoped and cannot be product identity.  
**Alternatives:** singleton implicit output; raw DRM IDs; logical `OutputId`.  
**Chosen:** typed allocator and compositor/native propagation, while product remains one physical output.  
**Trade-offs:** foundation exists before collection/hotplug architecture; docs must not call it multi-output.  
**Status:** **Evolving foundation**.

### ADR-014 — Capture Wayland surface semantics at commit/tree-transaction boundaries

**Context:** many protocols are double-buffered and synchronized subsurfaces defer publication.  
**Problem:** applying requests immediately breaks Wayland atomicity and cross-protocol consistency.  
**Chosen:** `SurfaceData` pending state captured at commit; synchronized nodes folded into one tree transaction.  
**Trade-offs:** more pending/cached state; precise semantic publication.  
**Status:** **Stable**.

### ADR-015 — Make rejection/fallback reasons first-class observability

**Context:** conservative systems can look “slow” or “disabled” unless operators know why an optimization was rejected.  
**Problem:** opaque fallback makes qualification/debugging impossible.  
**Chosen:** stable blocker/reason enums, bounded trace rings, transaction/pacing/effect evidence.  
**Trade-offs:** diagnostic maintenance; substantially better qualification and field debugging.  
**Status:** **Stable philosophy, evolving coverage**.

An expanded ADR-ready extraction is included in `ADR_CANDIDATES.md` in this package.

---

## 22. Research article candidates

These topics are better suited to research/engineering articles than static architecture reference because the interesting story is comparison, experimentation, measurement or architecture correction.

### Research 1 — Physical identity versus mutable frame targets

**Research question:** How should a compositor identify a physical rendered frame when predicted presentation targets can change after rendering?  
**Why it mattered:** using logical/pacing target state as identity can mis-correlate render-ahead and pageflip completion.  
**Initial hypothesis:** a rich “frame identity snapshot” including target metadata might be sufficient.  
**Competing approaches:** mutable logical identity; framebuffer-only identity; stable physical composite key.  
**Evidence in repository:** `OutputFrameKey`; predictive-O1/worker tests that mutate target but require identical physical key.  
**Result:** physical identity excludes mutable scheduling target.  
**Architectural consequence:** pacing attempts are separate identities layered over a stable physical frame key.  
**Open questions:** future multi-output/cross-GPU fields required in the key.  
**Potential title:** **“A Frame Is Not Its Deadline: Stable Physical Identity in a Predictive Wayland Compositor.”**

### Research 2 — Bounded asynchronous KMS submission

**Research question:** Can a KMS worker reduce critical-path latency without creating hidden queue latency?  
**Competing approaches:** synchronous commit; generic worker FIFO; explicit one-submitted/one-queued lane.  
**Evidence:** `AtomicCommitArbiter`, worker reservations, watchdog/early-pageflip tests, timing metrics.  
**Result:** bounded lane preserves causality and makes queue depth explicit.  
**Consequence:** predictive triple can overlap one future job without arbitrary backlog.  
**Open questions:** target hardware latency distribution and whether Auto should eventually become default.  
**Title:** **“Asynchronous KMS Without a Hidden Queue.”**

### Research 3 — Predictive O1 and adaptive buffering

**Research question:** Under what conditions does predictive overlap reduce missed vblanks/latency rather than merely increasing queue residency?  
**Initial hypothesis:** one future bounded primary opportunity can improve readiness if physical frame identity remains exact.  
**Approaches:** reactive double; unconditional triple; adaptive predictive triple/O1 credits.  
**Evidence:** Predictive O1 lifecycle, attempt identities, target selection, timing/cadence metrics, historical pacing qualification notes.  
**Result:** architecture uses adaptive eligibility rather than unconditional triple buffering.  
**Open questions:** reproducible 60/120/165 Hz hardware matrix, p95/p99 latency and GPU-bound behavior.  
**Title:** **“Predict One, Prove One: Adaptive Triple Buffering in Typhon.”**

### Research 4 — Effects partial repaint as a graph problem

**Research question:** How can backdrop/effect damage be propagated without an unbounded convergence loop?  
**Initial approach:** iterative propagation until region structure converges.  
**Competing approaches:** full-frame effects; iterative fixpoint; validated DAG with reverse edge traversal.  
**Evidence:** current `plan_effect_pass_execution_demand`, `visited_edges`, regression tests for fragmented dependency coverage and malformed metadata.  
**Result:** each dependency edge is propagated at most once; uncertainty becomes conservative repaint.  
**Consequence:** bounded frame-critical cost and diagnosable fallback.  
**Title:** **“From Fixpoint to DAG: Bounding Backdrop Damage Propagation.”**

### Research 5 — Animation completion requires physical evidence

**Research question:** When is an animation actually finished in a compositor with render-ahead and asynchronous KMS?  
**Competing answers:** duration elapsed; final value rendered; final value physically presented.  
**Evidence:** exact transaction/revision ACK tests, lifecycle physical-retention tests, scene-history promotion.  
**Result:** mathematical settlement and physical retirement are separate states.  
**Consequence:** retained resources and visual ownership survive until exact presented evidence.  
**Title:** **“The Animation Ended, but the Pixels Did Not: Physical ACKs in Astrea's Presentation Engine.”**

### Research 6 — Direct Scanout as proof, not heuristic

**Research question:** How strict should a compositor be before bypassing composition?  
**Approaches:** top-fullscreen heuristic; permissive retry; exact semantic/KMS qualification.  
**Evidence:** blocker model, owner/source distinction, validation key, TEST_ONLY and real-submit fallback.  
**Result:** experimental conservative proof pipeline.  
**Open questions:** which blockers dominate on target NVIDIA hardware; cache hit rate; latency/power payoff.  
**Title:** **“Prove the Plane: A Conservative Direct Scanout Model.”**

### Research 7 — Explicit sync under supersession and session loss

**Research question:** How do acquire/release obligations remain exactly-once when surface commits are superseded and DRM ownership disappears?  
**Approaches:** fence callbacks tied to buffers; generation-qualified registry + output release plan.  
**Evidence:** acquire watch registry, park/rearm recovery, direct/composited release tests.  
**Result:** synchronization is modeled as terminal ownership, not a side channel.  
**Title:** **“Fences Have Owners: Explicit Synchronization Across a Compositor State Machine.”**

### Research 8 — Retained lifecycle payloads and reversible minimize animation

**Research question:** How can minimize/restore reverse without recapturing or prematurely destroying visual resources?  
**Evidence:** immutable retained payload IDs/`Arc`, reversal tests, SSD preservation, physical ownership after runtime config changes.  
**Result:** motion identity and retained visual payload are separate.  
**Open questions:** generalized lifecycle effects beyond Lamp.  
**Title:** **“Motion Is Not Content: Reversible Lifecycle Animation with Retained Visual Payloads.”**

### Research 9 — Session recovery as generation rebinding

**Research question:** How can the compositor preserve logical identity while all physical DRM authority is torn down and rebuilt?  
**Evidence:** stable `OutputId`, changing DRM generation, ordered session recovery and stale pageflip tests.  
**Result:** logical output identity survives; physical ownership is generation-scoped.  
**Title:** **“Same Output, New Hardware Authority: Generation-Based DRM Session Recovery.”**

### Research 10 — X11 selection adaptation without a second clipboard authority

**Research question:** How can X11 selection semantics be bridged while Wayland selection remains canonical?  
**Approaches:** dual authorities with synchronization; canonical broker + generation-qualified adapter.  
**Evidence:** active X11->Wayland publication, stale clear rejection, reverse proxy source-key checks, outgoing INCR/MULTIPLE foundation.  
**Result:** canonical `SelectionState`; reverse direction remains intentionally incomplete in product wiring.  
**Open questions:** connect `SelectionRequest` ingress; live XDND; restart/replay qualification with real applications.  
**Title:** **“One Clipboard Truth: Adapting X11 Selection into a Wayland-First Compositor.”**

An expanded article planning version is included in `RESEARCH_ARTICLE_CANDIDATES.md`.

---

## 23. Documentation website information architecture

The public site should separate *how it works* from *why it was chosen*, *what was measured*, and *what exact implementation surface exists*. Mixing those categories is a major cause of current documentation drift.

### 23.1 Overview

#### Page: **Typhon in One Mental Model**
- **Audience:** systems engineers, advanced users, contributors.
- **Purpose:** introduce semantic -> presentation -> rendered -> physical stages and the ownership philosophy.
- **Prerequisites:** basic Wayland/compositor concepts.
- **Key concepts:** canonical authority, transaction, generation, evidence, fallback.
- **Source modules:** `compositor/mod.rs`, `presentation_animation`, `native_output/presentation`, `runtime/scene_history.rs`.
- **Related ADRs:** 001, 004, 006.
- **Related research:** physical identity; animation ACK.
- **Diagrams:** full architecture; semantic-to-physical flow.

#### Page: **Design Philosophy**
- **Audience:** all technical readers.
- **Purpose:** explain explicit ownership, bounded work, evidence-driven completion, safe fallback and conservative qualification.
- **Prerequisites:** overview mental model.
- **Key concepts:** proof over heuristic; one authority; stale-work invalidation.
- **Source:** cross-cutting modules listed in Section 20.
- **Related ADRs:** all foundational ADRs.
- **Diagram:** “ownership/evidence ladder.”

#### Page: **Current Product Boundaries**
- **Audience:** users and integrators.
- **Purpose:** clearly distinguish active, optional, experimental, planned and inactive-foundation functionality.
- **Prerequisites:** none.
- **Key concepts:** single physical output, keyboard/mouse focus, optional XWayland, Direct Scanout experimental, VRR not active.
- **Source:** real `main`, capability profiles, runtime policies.
- **Diagram:** capability/status matrix, not code diagram.

### 23.2 Architecture

Recommended pages:

1. **Runtime and Process Architecture** — startup, server/native bootstrap, event loop, shutdown/session recovery. Source: `main.rs`, `server.rs`, `runtime/*`, `process.rs`. ADRs 005, 015. Diagrams: process/session ownership and event-loop map.
2. **Canonical Compositor State** — surfaces, windows, workspaces, focus, selection and why canonical state is not rendered state. Source: `compositor/mod.rs`, `state/*`. ADR 001.
3. **Surface Commit and Transaction Architecture** — pending state, synchronized cache, tree transactions, readiness/publication. ADR 014. Diagram: surface commit lifecycle.
4. **Rendering Architecture** — scene resolution, damage, CPU/EGL paths, effect integration and render evidence. ADR 001/010. Diagram: scene-to-render-target.
5. **Native Output and KMS** — logical output vs physical generation, scanout backends, slots, atomic lane, pageflip. ADRs 002–005. Diagram: render-to-pageflip lifecycle.
6. **Presentation Engine** — property transactions/revisions, samples, physical ACK. ADR 006. Diagram: Presentation Engine state machine.
7. **Frame Pacing** — reactive/predictive model, O1, deadlines and worker reservation. ADRs 002/003. Diagram: pacing pipeline.
8. **Explicit Synchronization** — acquire/release terminal ownership. ADR 009. Diagram: explicit-sync lifecycle.
9. **Direct Scanout** — semantic proof + hardware proof + fallback. ADR 008. Diagram: decision tree.
10. **Effects Architecture** — resolved effect scene -> typed graph -> demand planner -> renderer. ADR 010. Diagram: graph/damage propagation.
11. **Animation and Lifecycle Presentation** — control slots, property motion, retained payload, reversal/physical settlement. ADRs 006/007. Diagram: lifecycle animation ownership.
12. **Window and Workspace Architecture** — floating/tiled/Dwindle, workspace/family ownership, shell boundary. ADR 012. Diagram: WM/shell authority boundary.
13. **XWayland/XWM Architecture** — generation-bound service and adapter boundaries. ADR 011. Diagram: X11->canonical state.
14. **Input Architecture** — seat/focus/constraints/cursor/shortcut inhibition. Diagram: input routing and cursor ownership.
15. **Resource Management** — child supervisor, application scopes, dmem and session ownership. Diagrams: process groups/session lifecycle.

Each architecture page should begin with the six questions: **problem, canonical owner, identity, lifecycle, completion evidence, fallback**. That structure matches the code and keeps pages from degenerating into file tours.

### 23.3 Design Decisions

One page per accepted ADR, with a status banner: Stable / Evolving / Experimental / Historical. Keep ADRs immutable after acceptance except for supersession links; do not silently rewrite historical rationale when the implementation evolves.

Initial ADR set: ADR-001 through ADR-015 above.

### 23.4 Research

Research pages should be chronological narratives with methodology/evidence, not normative architecture reference. Initial series:

- A Frame Is Not Its Deadline;
- Asynchronous KMS Without a Hidden Queue;
- Predict One, Prove One;
- From Fixpoint to DAG;
- The Animation Ended, but the Pixels Did Not;
- Prove the Plane;
- Fences Have Owners;
- Motion Is Not Content;
- Same Output, New Hardware Authority;
- One Clipboard Truth.

### 23.5 Subsystem Reference

Reference pages should expose exact types/states and be versioned with source:

- `CompositorState` ownership index;
- surface pending/commit fields;
- surface-tree transaction states/dependencies;
- Presentation Engine types and transaction/revision rules;
- output transaction state machine;
- `OutputFrameKey` field semantics;
- KMS worker/arbiter state;
- pacing identities and metrics;
- effects graph IR/pass/texture/demand stats;
- animation catalog and runtime capability rules;
- workspace/Dwindle data model;
- selection state and source keys;
- XWayland generation/X11 handle model;
- input capability/seat state;
- process supervisor and scope policies;
- observability event/field glossary.

Reference pages are where symbol names belong. Overview pages should not require readers to know internal Rust type names.

### 23.6 Protocols

Organize protocol docs by how state enters canonical authority:

- **Core surface semantics:** `wl_surface`, subsurfaces, viewport, damage, callbacks.
- **Desktop shell:** xdg-shell, decorations, activation, layer shell where applicable.
- **Frame/presentation:** presentation feedback, FIFO, Commit Timing, tearing control, content type.
- **Synchronization/buffers:** linux-dmabuf, explicit sync/syncobj.
- **Input:** seat, relative pointer, constraints, warp, cursor shape, idle/shortcut inhibition.
- **Selection/data:** data-device, PRIMARY, data-control.
- **Workspaces:** ext-workspace projection.
- **Astrea private protocols:** auth, shell control, effects, screenshot, shortcuts/toplevel.
- **XWayland shell/private integration.**

Every protocol page should carry three separate boxes: **implementation**, **current product advertisement**, **qualification/default policy**. The presentation-capability mismatch proves why this is necessary.

### 23.7 Hardware / Qualification

Pages:

- **DRM/KMS Qualification Model** — TEST_ONLY vs real submit vs pageflip evidence.
- **NVIDIA Qualification** — exact tested GPU/driver/session matrix; do not generalize from deterministic tests.
- **Direct Scanout Qualification** — blockers, exact key and current experimental policy.
- **KMS Worker Qualification** — worker timing, watchdog, session/shutdown matrix.
- **Frame Pacing Qualification** — refresh-rate matrices and latency/miss metrics.
- **Effects Performance Qualification** — partial repaint, blur cost and 165 Hz evidence.
- **Explicit Sync Qualification** — composition/direct/recovery matrix.
- **Presentation Modes** — async/tearing evidence; VRR explicitly separated as inactive until `VRR_ENABLED` is written/qualified.

### 23.8 Debugging / Observability

Pages:

- **Reading Typhon IDs** — frame, output tx, pageflip, presentation tx/revision, generation.
- **Why Direct Scanout Was Rejected** — blocker glossary.
- **Why a Frame Missed Its Target** — render-readiness vs worker/dispatch vs apply-guard.
- **Presentation Trace Guide** — Built/Queued/Submitted/Presented/terminal.
- **Effects Repaint Provenance** — damage and demand-plan diagnostics.
- **XWayland Debugging** — generation, stderr ring, startup gates, selection direction status.
- **Session Recovery Debugging** — suspended/recovering/active evidence.
- **Doctor/Status Reference** — status values without implying support from existence.

### 23.9 Historical Design

Keep obsolete architecture as explicit history, not mixed into current pages:

- pre-typed-`OutputId` single-output model;
- earlier effect demand/fixpoint approach;
- historical Direct Scanout stages and qualification boundaries;
- pre-PresentationClip lifecycle model;
- earlier XWM stream/XSync defects and repairs;
- Sep-14 readiness audit as a dated snapshot;
- superseded pacing identity models.

A historical page must show **date/snapshot/what superseded it**. This preserves research value without misleading current readers.

A fully enumerated page catalog is included in `WEBSITE_INFORMATION_ARCHITECTURE.md`.

---

## 24. Diagram backlog

The following diagrams are high value. The descriptions are intentionally precise enough for a later designer/agent to produce them without re-researching architecture.

### D-01 — Full Typhon architecture

**Teach:** where semantic authority ends and native physical ownership begins.  
**Nodes:** Client protocols; `OwnCompositorServer`; `CompositorState`; WM/workspaces; surface-tree transactions; Presentation Engine; effects resolver/compiler; NativeRuntime; renderer; swapchain; OutputTransactionLedger; KMS arbiter/worker; DRM/KMS; pageflip; XWayland adapter; input; shell/control.  
**Ownership boundaries:** Compositor Semantic, Presentation, Render, Native Physical, External Adapters.  
**Arrows:** requests -> pending -> commit; semantic scene -> presentation sample -> render/direct proof; render -> output transaction -> KMS -> pageflip; pageflip -> presented snapshot/ACK.  
**Emphasis:** arrows back from physical presentation to Presentation Engine are evidence, not semantic commands.

### D-02 — Surface commit lifecycle

**Nodes:** protocol requests, `SurfaceData.pending`, `wl_surface.commit`, immediate commit vs synchronized cache, `PendingSurfaceTreeTransaction`, readiness gates, `publish_surface_tree`, canonical scene.  
**Transitions:** pending -> captured; cached -> tree-owned; wait -> ready/terminal; publish.  
**Teach:** protocol request != canonical state; synchronized child commit != visible until owning transaction publishes.

### D-03 — Synchronized subsurface transaction graph

**Nodes:** root + child/grandchild commits; lineage edges; acquire dependencies; external-content dependency; FIFO/Commit Timing.  
**Ownership boundary:** `SubsurfaceTransactionState` cache versus pending tree transaction.
**Teach:** why coalescing needs contiguous lineage and protected obligations.

### D-04 — Semantic -> presentation -> rendered -> physical

**Four horizontal bands:** Canonical semantic state; Presentation Engine sample; Renderer/submitted evidence; Pageflip-confirmed physical state.  
**Example:** window canonical rectangle jumps A->B, presentation transform interpolates, renderer emits frame F, KMS submits token T, pageflip T acknowledges revision R.  
**Teach:** the central Astrea/Typhon mental model.

### D-05 — Presentation Engine state machine

**Nodes:** install transaction/revision; Active; Sampled; Mathematically Settled; Rendered; Physically ACKed; Retired; Retargeted; Canceled.  
**Edges:** sample/retarget/cancel/pageflip-derived ACK.  
**Guard labels:** exact output, transaction, revision, final value.  
**Teach:** why elapsed duration is not completion.

### D-06 — Render-to-pageflip lifecycle

**Nodes:** scene snapshot, render target/slot, `OutputFrameKey`, OutputTransaction Built/Ready/Queued, worker queued-next, kernel submitted, pageflip, SceneHistory presented, protocol releases.  
**Ownership:** swapchain, ledger, arbiter, kernel.  
**Teach:** who owns the frame at each physical stage.

### D-07 — Frame pacing pipeline

**Nodes:** demand, deadline planner, ReactiveDouble/PredictiveTriple decision, Predictive O1 attempt, frame key, worker reservation, render, submit, pageflip, O1 terminal evidence.  
**Decision labels:** capability, credit, worker lane, async mode, deadline.  
**Teach:** predictive attempt identity is not physical frame identity.

### D-08 — KMS worker ownership

**Two-slot lane:** `kernel_submitted` and `worker_queued_next`.  
**Transitions:** synchronous reserve/submit; worker reserve -> execute -> mark kernel submitted; pageflip frees kernel and may promote next; rejection/quiesce paths.  
**Teach:** asynchronous does not mean unbounded queue.

### D-09 — `OutputFrameKey` lifecycle

**Fields displayed:** output, frame, protocol batch, output tx, slot, framebuffer, render generation, pool generation.  
**Side box:** mutable presentation target/deadline is explicitly *not* in key.  
**Teach:** stable physical identity under scheduling retarget.

### D-10 — Direct Scanout decision flow

**Stages:** policy enabled? -> semantic solitary/source proof -> effects/animation/cursor/sync checks -> format/modifier -> build exact validation key -> cached proof? / TEST_ONLY -> real submit -> pageflip. Every failure arrow -> composition with named blocker.  
**Teach:** proof model and conservative fallback.

### D-11 — Explicit synchronization lifecycle

**Acquire side:** commit -> watch register -> signaled/current generation -> tree ready; supersede/cancel/backend mismatch terminals.  
**Release side:** buffer use -> composition GPU completion OR direct KMS/out-fence/pageflip -> exactly-one release.  
**Session branch:** park -> new DRM generation -> rearm or terminalize.  
**Teach:** fences have owners and terminal states.

### D-12 — Lifecycle animation + retained payload

**Nodes:** canonical window/minimize state, `WindowLifecycleAnimator`, retained payload ID/Arc, presentation retained identity, renderer Lamp executor, submitted scene, physical lifecycle snapshot, ACK/retirement.  
**Reversal arrow:** restore reuses payload while replacing motion identity.  
**Teach:** separate motion/content/presentation/physical ownership.

### D-13 — Effects graph and damage propagation

**Left:** scene effect instances/backdrop checkpoints.  
**Middle:** compiled pass/texture DAG.  
**Right:** reverse demand traversal arrows, each dependency edge once; conservative expansion branch for invalid metadata.  
**Teach:** why effects are a typed graph rather than shader chain.

### D-14 — XWayland integration

**Nodes:** XWayland child/generation, XWM, X11 window events, canonical desktop window state; XFixes selection -> canonical `SelectionState` -> Wayland offers.  
**Dashed inactive branch:** canonical Wayland selection -> proxy metadata/outgoing machinery -> **missing production SelectionRequest ingress** -> X11 requestor.  
**Other dashed foundations:** XDND, runtime RandR publication, X11 cursor ownership.  
**Teach:** X11 is an adapter and distinguish active vs foundation directions.

### D-15 — Process/session ownership

**Nodes:** compositor process, ChildSupervisor, session-owned XWayland/helpers, ordinary applications, systemd app scopes, seat/session, DRM generation.  
**Shutdown arrows:** TERM/KILL only session-owned; apps remain.  
**Suspend arrows:** session -> quiesce worker/sync/cursor/DRM -> recover new generation.  
**Teach:** process lifetime and DRM authority are deliberate state machines.

### D-16 — Workspace/Dwindle/shell boundary

**Nodes:** shell UI/policy, compositor control/protocol boundary, WorkspaceManager, WindowManagementState, Dwindle tree/solver, canonical window geometry, Presentation Engine.  
**Teach:** shell can request policy; compositor owns layout truth and animation presentation is separate.

The package includes `DIAGRAM_BACKLOG.md` with these as implementation briefs.

---

## 25. Documentation/code inconsistencies

This section intentionally lists only source-proven disagreements relevant to public architecture documentation.

### 25.1 Presentation protocol qualification versus production registry — **current code integration inconsistency**

**Documentation/model claim:** qualified native presentation capabilities include tearing control and content type. `bind_native_base()` uses them.  
**Production source:** `src/main.rs::own_compositor` calls `bind_with_capabilities_and_frame_pacing`. `src/compositor/server.rs::bind_with_capabilities_and_frame_pacing` injects `PresentationProtocolCapabilities::safe_baseline()`. `native_protocol_names()` uses the frame-pacing-only capability helper, likewise omitting qualified presentation capabilities.  
**Result:** implementation/KMS integration exists, but current normal product construction does not advertise those two globals.  
**Documentation action:** describe as “implemented but not product-advertised due current constructor wiring,” not qualified active support.

### 25.2 Historical Direct Scanout prose versus current presentation-aware source — **stale qualification wording**

Older known-issues/Stage-4 language says the candidate excludes tearing. Current `DirectPlaneValidationKey` includes presentation mode and DRM content type, and the direct native path computes effective async eligibility.  
**Result:** historical Stage-4 boundary is no longer the exact current implementation model.  
**Action:** preserve Stage 4 under Historical Design; current Direct Scanout docs should describe presentation-aware proof while retaining default-off/hardware-qualification caveats.

### 25.3 Historical “no OutputId” conclusion versus current typed foundation — **historical baseline obsolete in part**

Current source contains `OutputId`, allocator, compositor `native_output_id`, propagation into native runtime and tests proving logical identity survives DRM generation recovery.  
**But:** there is still one production physical output runtime and no hotplug/multi-output product model.  
**Action:** document “typed logical output identity: active foundation; multi-output: not implemented.”

### 25.4 XWayland reverse selection source depth versus product wiring — **easy to mis-document in either direction**

`selection_proxy.rs` and `selection_outgoing.rs` implement substantial Wayland-source-to-X11 machinery including MULTIPLE/outgoing INCR tests. Runtime code can submit proxy metadata and service generated data requests.  
**But:** `selection_proxy::handle_selection_request()` has no production caller; `events.rs` does not route `SelectionRequest`; `submit_proxy_selection_snapshots` explicitly does not mutate X11 selection ownership or handle conversions.  
**Result:** current `docs/XWAYLAND.md` classification of reverse ownership/payload, outgoing INCR/MULTIPLE as inactive is materially correct even though the implementation foundation is deeper than that short sentence implies.  
**Action:** document both facts to avoid future agents “discovering” test-only code and falsely upgrading support.

### 25.5 Broad known-issues wording that “X11 compatibility is not enabled” — **overbroad if read literally**

The source has a real generation-bound XWayland/XWM product path selected by `TYPHON_XWAYLAND`, including managed modes and active window-management core. The default is Off, but opt-in product wiring exists.  
**Action:** replace “not enabled” with exact status: implemented/product-wired optional, default Off; list active/inactive bridge directions separately.

### 25.6 Capability truth is duplicated — **architecture/process inconsistency**

Protocol-name reporting, convenience server constructors, tests and product startup can use different capability helpers. The presentation-global mismatch is proof that these can diverge.  
**Action:** future docs should derive status from the exact production capability bundle; an ADR/engineering follow-up should consider one authoritative native capability factory. This report does not modify code.

### 25.7 Hardware qualification versus deterministic test evidence — **documentation discipline requirement**

Effects, Direct Scanout, KMS worker and advanced pacing have substantial deterministic state-machine coverage, while current docs retain target-hardware qualification caveats.  
**Action:** architecture pages may say “implemented/tested”; hardware pages must separately state the exact live hardware matrix. Do not turn test density into hardware qualification.

---

## 26. Unknowns and questions requiring human confirmation

The source establishes mechanism more strongly than historical intent. These questions should be answered by project maintainers before publishing “why” as fact.

1. **Was `PresentationProtocolCapabilities::safe_baseline()` in the production constructor intentionally retained for qualification, or is it an accidental wiring regression?** Source proves current behavior but not motive.
2. **What exact hardware/driver revisions are considered currently qualified for Direct Scanout, explicit sync, KMS worker, predictive triple and effects?** Source/docs show policy boundaries but the supplied snapshot is not a lab-results database.
3. **Is the long-term presentation protocol policy to advertise tearing/content type whenever implementation is available but keep tearing policy Off, or to hide globals until hardware qualification?** The current code/model disagree.
4. **What is the intended multi-output model?** Typed `OutputId` exists, but source does not prove future workspace-to-output semantics, hotplug policy, primary-output behavior or whether one process will own multiple `NativeRuntime`-like instances.
5. **Will Presentation Engine remain the single presentation-property authority as new scale/glide/workspace effects arrive, or will lifecycle/workspace effects gain separate engines?** Current source supports the former philosophy but cannot prove roadmap intent.
6. **Is Lamp intended as Astrea's permanent default minimize identity or only the first implementation?** Catalog says current Astrea preset requests Lamp; long-term product philosophy is not source-proven.
7. **Should reverse X11 selection ownership be completed using the existing proxy/outgoing foundation?** This appears likely from code investment, but the required production `SelectionRequest` ingress is absent.
8. **What exact shell-policy contract should be public/stable?** Source proves compositor authority and private control surfaces, but future public customization ABI is not established here.
9. **Will the typed effects graph become a stable external effect ABI, or remain an internal renderer-independent IR?** Current trusted effect APIs do not prove long-term ABI commitment.
10. **What is the intended VRR architecture?** Current source discovers/plans capability but explicitly does not write `VRR_ENABLED`; future transaction/interaction with tearing/pacing is not source-proven.
11. **What standard capture path is intended?** Private Astrea screenshot exists; PipeWire/portal/screencopy direction is not established in source.
12. **What constitutes promotion from experimental to default for Direct Scanout/KMS worker?** Current source exposes policy and diagnostics but not a formal acceptance threshold.
13. **Are application scopes/dmem core AstreaOS architecture or Linux-target optimizations that should live under optional platform integration docs?** Mechanism is clear; product philosophy needs maintainer confirmation.
14. **Should current fixed workspace count/special-workspace policy be documented as API stability?** Source suggests product policy rather than architectural invariant.
15. **Which old architecture/readiness documents should remain public historical records versus be removed from primary navigation?** The source cannot decide editorial policy.

Where these questions affect this report, rationale is explicitly labeled **Likely rationale** rather than presented as historical fact.

---

## 27. Source map

This is the high-value source map for future documentation agents. It prioritizes owners and phase boundaries rather than every helper file.

### Entry/runtime

- `src/main.rs` — `main`, `run`, `own_compositor`, `native_protocol_names`: real product entrypoint/capability construction.
- `src/compositor/server.rs` — `OwnCompositorServer`: Wayland server + compositor state owner; constructor variants.
- `src/compositor/server_globals.rs` — global advertisement and capability gates.
- `src/native_output/runtime/bootstrap.rs` — `NativeRuntime::bootstrap_native`: physical backend construction.
- `src/native_output/runtime/mod.rs` — `NativeRuntime`: full runtime ownership inventory.
- `src/native_output/runtime/cycle.rs` — event loop/work-domain orchestration.
- `src/native_output/runtime/shutdown.rs`, `shutdown_cycle.rs` — shutdown state machine.
- `src/native_output/runtime/session.rs`, `session_io.rs` — suspend/recovery state machine.

### Canonical compositor/surfaces

- `src/compositor/mod.rs` — `CompositorState`: central semantic authority.
- `src/compositor/state_data.rs` — `SurfaceData`, buffer/release data: Wayland pending state.
- `src/compositor/state/surface_commits.rs` — buffer/mapping commit into canonical state.
- `src/compositor/subsurface.rs` — synchronized cache/limits/transaction state.
- `src/compositor/state/surface_transactions.rs` — tree transaction identity and dependency model.
- `src/compositor/state/surface_tree_readiness.rs` — readiness/publish progression.
- `src/compositor/state/subsurfaces.rs` — extraction/coalescing/publication.
- `src/compositor/state/surfaces.rs` — renderable surfaces/publication decisions/damage.
- `src/compositor/state/roles.rs` — surface role authority/lifetimes.

### Presentation Engine / animations

- `src/presentation_animation/engine.rs` — presentation tracks, sampling and exact physical ACK.
- `src/presentation_animation/transaction.rs` — transaction/member identity.
- `src/presentation_animation/retained.rs` — retained visual identity.
- `src/presentation_animation/frame.rs` — frame snapshot/evidence model.
- `src/compositor/presented_frame.rs` — physical frame projection back into compositor state.
- `src/animation_control/catalog.rs` — stable slots/effect catalog/availability.
- `src/animation_control/config.rs`, `mod.rs`, `snapshot.rs` — control plane.
- `src/window_lifecycle_animation.rs` — lifecycle motion/Lamp model.
- `src/compositor/state/lifecycle_retained.rs` — immutable retained payload store.
- `src/compositor/state/lifecycle_animation.rs` — compositor lifecycle integration.
- `src/compositor/state/lifecycle_animation_tests.rs` — cross-layer physical ownership regressions.

### Rendering/effects

- `src/compositor/render.rs` — logical scene/damage composition.
- `src/native_output/runtime/frame.rs` — `NativeFrameRenderer` and native resolved frame.
- `src/egl_renderer.rs` — GLES scene renderer and native EGL integration.
- `src/effects/render_graph.rs` — typed graph compiler/demand planner.
- `src/effects/validation.rs`, `config.rs`, `registry.rs` — trusted program/resource validation.
- `src/egl_renderer/effects/executor.rs` — GPU graph execution.
- `src/egl_renderer/effects/trace.rs` — effect/repaint provenance.

### Native presentation/KMS

- `src/core/output_id.rs` — logical output identity.
- `src/native_output/scanout/output_swapchain.rs` — slots/framebuffer lifecycle and `OutputFrameKey`.
- `src/native_output/presentation/transaction.rs` — immutable output transaction.
- `src/native_output/presentation/ledger.rs` — transaction state/obligation authority.
- `src/native_output/presentation/plane.rs` — physically presented plane snapshot.
- `src/native_output/runtime/scene_history.rs` — rendered/submitted/presented scene evidence.
- `src/native_output/runtime/atomic_commit.rs` — bounded atomic lane.
- `src/native_output/kms_worker/*` — worker policy/thread/jobs.
- `src/native_output/runtime/kms_worker.rs`, `presentation_worker.rs` — worker/runtime ownership integration.
- `src/native_output/runtime/cycle/pageflip.rs` — physical completion path.
- `src/native_output/presentation/kms_timing.rs` — KMS timing/deadline evidence.

### Frame pacing

- `src/native_output/pacing.rs` — `NativeFramePacing`, predictive attempt/reservation IDs, O1 lifecycle.
- `src/native_output/runtime/presentation_pipeline.rs` — pipeline snapshot.
- `src/native_output/runtime/presentation_cycle.rs` — render/submit/pacing orchestration; direct source reading recommended around graph partial line 200.
- `src/native_output/runtime/metrics.rs` — pacing/runtime metrics.

### Explicit sync

- `src/compositor/explicit_sync.rs` — commit-side acquire request.
- `src/native/explicit_sync.rs` — watch registry and generation-qualified readiness.
- `src/compositor/state_data.rs` — release obligation types.
- `src/native_output/runtime/dmabuf_release.rs` — GPU release registry/settlement.
- output transaction/Direct Scanout modules — release contract propagation.

### Direct Scanout/presentation modes

- `src/compositor/state/direct_scanout.rs` — semantic source/owner eligibility.
- `src/compositor/state/direct_scanout_tests.rs` — semantic blockers.
- `src/native_output/scanout/direct_policy.rs` — policy/default.
- `src/native_output/scanout/direct_validation.rs` — exact validation key.
- `src/native_output/scanout/atomic_egl_gbm/direct.rs` — TEST_ONLY/real direct path.
- `src/native_output/runtime/presentation_direct.rs`, `cycle_direct.rs` — runtime direct ownership.
- `src/native_output/presentation/async_validation.rs` — async proof identity.
- `docs/wayland/PRESENTATION_MODES_V1.md` — supporting documentation; source remains authoritative.

### Window/workspaces

- `src/wm/workspace.rs` — `WorkspaceManager`.
- `src/wm/layout/*` — Dwindle tree/constraints/solver.
- `src/compositor/state/tiled_layout.rs` — canonical layout application.
- `src/compositor/state/windows.rs`, `desktop_windows.rs`, `window_interaction.rs` — window lifecycle/focus/move/resize.
- `src/compositor/window_state.rs` — mode/minimize/restore.
- workspace protocol module/tests — external projection of canonical workspace state.

### Wayland/protocols

- `src/compositor/plan.rs` — capability profiles and advertised protocol set.
- `src/compositor/server_globals.rs` — product global registration.
- `src/compositor/protocols/*` — protocol adapters.
- `src/compositor/screen_capture.rs` — authenticated private capture request.
- `src/native_output/screen_capture.rs` — sealed RGBA export.
- `src/portal.rs` — local portal backend interface surface.
- `docs/wayland/PROTOCOL_SOURCE_MANIFEST.md` — useful supporting manifest; verify against real constructor/global registration before publication.

### XWayland/XWM

- `src/xwayland/config.rs` — modes/default.
- `src/xwayland/service.rs` — child/XWM generation lifecycle.
- `src/xwayland/xwm/events.rs` — production X11 event normalization; notably no `SelectionRequest` reverse-proxy ingress.
- `src/xwayland/xwm/window.rs`, `commands.rs`, `properties.rs`, `focus.rs` — window management.
- `src/xwayland/xwm/selection_wire.rs` — incoming selection and reverse metadata preparation.
- `src/xwayland/xwm/selection_payload.rs` — X11->Wayland direct/incoming-INCR payload.
- `src/xwayland/xwm/selection_proxy.rs` — reverse X11 request model, currently test-only ingress.
- `src/xwayland/xwm/selection_outgoing.rs` — reverse direct/outgoing-INCR transport foundation.
- `src/native_output/runtime/xwayland.rs` — runtime adapter to canonical compositor state.
- `src/compositor/selection.rs`, `state/selection_runtime.rs` — canonical selection authority.
- `docs/XWAYLAND.md` — supporting status document; verify active/foundation statements against callers.

### Input

- `src/compositor/plan.rs` — native input capability profile.
- `src/compositor/protocols/input.rs`, `cursor_shape.rs` — protocol boundary.
- `src/compositor/state/input_dispatch.rs`, `input_resources.rs`, pointer constraint modules — canonical routing.
- `src/native_output/input/*` — native backend/event routing.
- `src/native_output/runtime/presentation_cursor.rs` — cursor presentation evidence.

### Process/resource management

- `src/process.rs` — `ChildSupervisor`.
- `src/application_scope.rs` — systemd scope integration/fallback.
- `src/native/dmem_foreground.rs` — optional dmem integration.
- XWayland fs/auth modules — private resource security.

### Observability

- `src/native_output/runtime/metrics.rs` and perf modules — native metrics.
- `src/native_output/presentation/trace.rs` — presentation transaction trace.
- `src/native_output/runtime/scene_history.rs` — physical evidence.
- `src/egl_renderer/effects/trace.rs` — effect/repaint provenance.
- `src/xwayland/trace.rs` — XWayland trace.
- native control/doctor snapshot paths in `runtime/cycle_dispatch.rs` — current runtime status; direct-read around the Codebase MCP partial range.

---

## Closing architectural assessment

Typhon's engineering model is coherent enough to document publicly now, but the public docs should be built around **authority, identity and evidence**, not around feature lists.

The project's most distinctive architecture is the refusal to let convenient proxies become truth:

- client commit is not publication;
- canonical state is not presentation state;
- animation settlement is not physical retirement;
- rendering is not presentation;
- KMS submit is not pageflip;
- a scheduling target is not a physical frame identity;
- a test is not product wiring;
- a capability flag is not hardware qualification;
- an X11 adapter is not a second compositor authority.

That same discipline also exposes the current weak points cleanly. Product capability construction can still diverge from qualified models. Single physical output remains a structural boundary despite the new typed `OutputId`. Some sophisticated XWayland reverse-selection code is still an inactive foundation because the production event ingress is absent. Advanced native optimizations are often intentionally default-off because deterministic state-machine confidence and target-hardware qualification are treated as different kinds of evidence.

Those are not reasons to dilute the documentation. They are exactly why AstreaOS should publish it: the architecture is defined as much by the evidence it refuses to fake as by the features it implements.
