# Typhon Effects Engine Final Closure Design

**Date:** 2026-09-08  
**Status:** Approved by the user for implementation  
**Scope:** Finish the current Typhon effects-engine implementation without restarting or redesigning its accepted renderer foundation.

## Goal

Close the remaining trusted-runtime, privileged-protocol, visual-group, overlapping-effect, GL-state, alpha/color, ABI, diagnostics, documentation, and qualification gates identified by the 2026-09-07 finishing prompt and second checkpoint review.

The implementation must preserve the existing `LegacyScene` no-effects fast path, anchored final composition, Dual Kawase pyramid, region-local graph domains, target-local offscreen coordinates, graph-liveness pooling, scratch FBO, transition damage identity, effect-owned scheduling, shader prewarm, one-time blur decode, Direct Scanout blocker, and presentation/buffer ownership work.

## Non-goals and v1 decision

- No effects-subsystem restart or renderer redesign.
- No trusted-asset texture table in this closure.
- `StaticTexture` may remain in the internal Effect IR for future compatibility, but v1 validation and registry publication reject programs containing it with a deterministic typed `UnsupportedStaticTexture`-equivalent error.
- No blank texture allocation, silent copy, or semantic degradation for unsupported static textures.
- No arbitrary client filesystem paths, shader source, GL handles, or texture handles.
- No claim of hardware qualification without a real native run on the documented target.

## Current architecture to preserve

The accepted data flow remains:

```text
trusted config / authenticated protocol / compositor policy
    -> validated Effect IR and immutable registry generation
    -> immutable ResolvedEffectScene
    -> semantic visual groups and effect dependency plan
    -> render graph with local captures and liveness metadata
    -> GLES executor with explicit pass state
    -> existing partial repaint and presentation ownership
```

Frames with no visible effects continue to select `LegacyScene` and call the existing legacy renderer path directly. Effects are visible to Direct Scanout planning before KMS candidate validation.

## Design sections

### 1. Atomic trusted registry publication

Add one authoritative runtime publication boundary that coordinates the existing compositor registry and GLES renderer registry. A reload performs the following phases:

1. Read only from a Typhon-owned trusted config path and root-constrained shader assets.
2. Parse and validate a candidate manifest, including v1 rejection of `StaticTexture` sources and all graph/resource limits.
3. Build a candidate immutable `EffectRegistryGeneration` with a single monotonic generation ID.
4. At the renderer's safe GL boundary, compile/prewarm every candidate custom shader and all required built-ins into candidate GL ownership.
5. Publish the candidate to renderer lookup and compositor/protocol lookup with the exact same generation ID.
6. Advance the effect/render epoch and invalidate affected effect bindings and presented-damage history.

Any parse, validation, asset, shader, link, or resource failure destroys candidate GL resources and leaves the previous compositor and renderer generation unchanged. The failure is bounded and typed. A candidate program is never protocol-resolvable before its renderer program is ready. Generation ID and reload/fallback state are exposed through diagnostics.

The publication coordinator will be testable without EGL by injecting a renderer-prewarm/publish boundary, while the real renderer method remains responsible for GL program ownership. The existing standalone registry builder remains useful, but it is no longer the production publication path by itself.

### 2. Typed surface slots and binding ownership

Introduce a typed protocol-facing `SurfaceEffectSlot` with at least:

```text
Background -> EffectAnchor::BeforeSurface(surface)
Content    -> EffectAnchor::ReplaceSurface(surface)
Foreground -> EffectAnchor::AfterSurface(surface)
```

`OutputPostProcess` is manager/output scoped and is rejected from a surface binding in v1. Raw slot strings are converted and validated at the protocol boundary, then do not flow through compositor state.

Protocol ownership is independent from visual enabled state. The compositor tracks a binding key `(surface_id, slot)` plus a stable binding/resource identity. Creation claims the key even while disabled. Enable/disable only changes the visual instance owned by that binding. Destroy removes only the instance owned by that binding and releases only its key. A stale resource cannot clear a replacement binding. Surface destruction releases every owned slot exactly once. Authorization remains bound to the existing authenticated Astrea shell-client check.

The compositor's internal qualification path may continue to use a separate surface-effect assignment API, but its storage must no longer make final visual ownership depend only on `surface_id`.

### 3. Semantic target visual groups

Extend scene lowering so captures and anchor operations share a semantic visual-root/group identity. A group records the target root and its associated visual elements according to one explicit policy:

- root surface content belongs to the root group;
- synchronized/desynchronized subsurfaces follow their committed visual parent group;
- server-side decoration is included when it is part of the window visual root;
- popups are explicitly marked as separate visual roots unless the effect API contract explicitly includes them.

`TargetContent`, `ReplaceSurface`, `BeforeSurface`, and `AfterSurface` resolve against this group identity rather than matching one raw `Surface(surface_id)` command. Capture and final composition use the same grouping function, so a target cannot be captured as one set of pixels and replaced as another. Tests cover root, subsurface, decoration, and popup combinations.

### 4. Dependency-aware overlapping backdrop effects

Replace the current “all offscreen passes, then all composites” dependency assumption with ordered region-local visual checkpoints. For each effect anchor in z-order, the executor can materialize the resolved scene below the anchor into a bounded checkpoint domain. Lower effect composites are included in that checkpoint before a higher backdrop capture consumes it.

The graph records explicit dependencies from lower composite/checkpoint work to higher capture work. Checkpoints are cropped to the higher effect's padded capture region and obey normal first/last-use pooling. The executor never samples and writes the same texture simultaneously. Independent disjoint regions remain independently scissored; the architecture does not introduce a full-output copy per effect.

The dependency scheduler is deterministic for two and three overlapping stacks. Moving or removing a lower effect invalidates the dependent higher effect's source query/capture region and visible output region. Tests assert ordering, dependency edges, no feedback loop, preserved padding, and bounded peak live resources.

### 5. Explicit GL pass state and state restoration

Every capture, stage, checkpoint, composite, and output-post-process pass declares or establishes:

```text
framebuffer target
viewport
scissor rectangles and origin
blend enable/function
program
texture-unit bindings
write semantics
working-space and alpha policy
```

Ordinary offscreen capture/stage writes replace destination pixels and disable blending unless the IR explicitly requests framebuffer composition. The final output composite restores Typhon's premultiplied source-over state. An RAII-style guard or equivalent structured restore path returns the renderer to the pre-effect state after success and error. A pooled destination containing old color/alpha cannot affect a new stage result.

### 6. Alpha and color contracts

Executable final metadata carries `EffectAlphaMode`:

- `Opaque` forces the final effect result alpha to `1.0` before source-over composition.
- `Preserve` keeps valid premultiplied alpha and RGB.

Mask operations define their premultiplied behavior explicitly. Alpha remapping uses safe unpremultiply/re-premultiply or a coverage operation that changes RGB and alpha together, including near-zero-alpha handling. `SourceOver`, `Add`, `Multiply`, and `Screen` are defined in premultiplied space; straight-color math, if needed, is bounded by safe conversions.

Graph working-space metadata drives conversion boundaries. Encoded sources are decoded exactly once when linear math begins, and linear results are encoded exactly once before encoded output. Source-only encoded programs are not double-encoded; linear programs cannot skip encode; custom declared spaces must be validated or receive explicit compiler conversion passes.

### 7. Region-precise capture and custom ABI

Capture execution iterates actual effect-region rectangles after converting output coordinates into each target's local physical coordinates and GL origin. It does not use a bounding rectangle except for the explicit conservative-full representation. Region-local domains, edge clipping, fractional scale, and BottomLeft offscreen targets are covered by pure conversion tests.

Trusted custom shaders receive monotonic compositor time and a clamped frame delta only through the resolved frame context. Static effects remain static; only `Continuous` instances own recurring frames. Auxiliary inputs use a fixed, bounded sampler ABI with deterministic validated binding order and count. No raw handles or arbitrary texture access are exposed.

Vector float ranges are checked component-wise according to the accepted parameter model. If a protocol v1 wire type is not mutable, the limitation is explicit. `StaticTexture` remains an internal source variant but is rejected during v1 validation/registry publication before frame execution.

### 8. Diagnostics and documentation

Metrics are audited against their names: cache-hit counters increment only on real cache hits; effect output pixels reflect planned effect influence/output work; capture pixels reflect actual capture domains; resource current/peak/budget/reuse/allocation/eviction remain bounded; shader prewarm and frame lookup are distinguishable; generation, typed fallback, Direct Scanout blocking, and continuous demand are observable. GPU timing remains `UNAVAILABLE` when a non-stalling timer-query path is not available.

`docs/EFFECTS.md` and `docs/EFFECTS_QUALIFICATION.md` distinguish modeled, implemented, deterministically tested, hardware-qualified, and production-default capability. They explicitly state that trusted static textures are unsupported in v1 and that native qualification is pending unless a real target run is completed.

## Verification and qualification

Each behavioral change gets a focused failing test before production code. After focused tests, use the existing build directory and run:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked effects
rtk cargo test --locked egl_renderer
rtk cargo test --locked native_output
rtk cargo test --locked
rtk bin/qualify-presentation --dry-run
```

No alternate target/build directory is created. Only after every deterministic command passes will the documented native matrix be attempted on `1920x1080 @ 165 Hz` with the RTX 3060 Ti environment when available. If TTY/DRM/NVIDIA access is unavailable, the result is explicitly deterministic-only and not production-qualified.

Native results record CPU p50/p95/p99, non-stalling GPU timing or `UNAVAILABLE`, missed-vblank/target-slip counters, effect work, resource peak/budget/reuse, fallback reasons, and Direct Scanout block/recovery. Existing explicit-sync, DMA-BUF/SHM, buffer-release, frame-batch, KMS, presentation, retry-debt, buffer-age, cursor, XWayland, pacing, and ordinary-damage invariants remain regression gates.

## Files and responsibilities

Expected focused changes follow the current checkout rather than older plan filenames:

- `src/effects/registry.rs` and `src/effects/config.rs`: candidate generation, trusted-path validation, v1 unsupported-source rejection, publication errors.
- `src/compositor/effects.rs`, `src/compositor/mod.rs`, `src/compositor/state/surfaces.rs`: typed effect bindings, semantic ownership, scene invalidation, visual-group resolution.
- `src/compositor/protocols/effects_control.rs`: typed slots, authorization, lifecycle, parameter validation.
- `src/effects/render_graph.rs` and new effect-plan helpers if required: alpha/working-space/dependency/liveness metadata.
- `src/egl_renderer/effects/executor.rs`, `capture.rs`, `shader_cache.rs`, `resources.rs`, `metrics.rs`: ordered checkpoint execution, explicit state, local capture, ABI, metrics.
- `src/egl_renderer.rs` and native scheduling seams: shared generation publication, frame timing, damage invalidation, effect demand.
- `docs/EFFECTS.md`, `docs/EFFECTS_QUALIFICATION.md`: factual capability state.
- Focused tests remain adjacent to their owning modules or in the existing effects/renderer/native-output test modules.

No unrelated workspace edits are included in this design or implementation scope.

## Self-review

- Scope is one closure pass over the currently accepted effects subsystem; it does not restart the engine.
- The v1 `StaticTexture` decision is explicit and appears in validation, registry publication, testing, and documentation.
- Renderer/compositor generation identity is single-source and failure retains the prior usable generation.
- Surface binding ownership is independent of enabled visual state.
- Capture and final composition share semantic target grouping.
- Overlap dependencies are explicit and region-local rather than relying on hardware-only validation.
- Alpha, working-space, capture-region, ABI, metrics, deterministic verification, and native qualification claims each have a corresponding test or evidence gate.
- The design is concrete enough to decompose into implementation tasks without placeholder steps.
