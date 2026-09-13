# Typhon Moving-Blur Renderer Hard-Freeze Design

## Context

The known native reproduction freezes only when Kitty has Typhon compositor-side background blur and the window is moved rapidly. The latest evidence places the first missing forward-progress boundary inside native EGL/GLES rendering for the next frame, after the preceding frame's KMS submission and pageflip have completed. KMS worker ownership, cursor-plane churn, and predictive primary ordering are therefore preserved as already-corrected behavior and are not fix targets for this investigation.

Eclipse remains a read-only integration reference. Its effect bridge owns only its own `QQuickWindow` surfaces and does not own the WaylandAuto Kitty blur.

## Goals and non-goals

Goals:

- Add one opt-in, bounded effect-execution flight recorder controlled by `TYPHON_EFFECT_EXEC_TRACE=1`.
- Localize the first renderer phase and exact graph pass that stops producing a CPU end marker in the native reproduction.
- Validate graph-resource, framebuffer, domain, scissor, and orientation invariants before issuing draws.
- Add deterministic moving-domain, transition-damage, resource-reuse, and real-GLES-prefix regression coverage using the existing effect machinery.
- Make the smallest correction supported by the resulting evidence.
- Preserve WaylandAuto blur, client-requested blur, public Eclipse effects, partial repaint, buffer age, Dual Kawase, output orientations, lifecycle capture, Magic Lamp capture, and Atomic EGL/GBM.

Non-goals:

- No global blur disable, movement throttling, full-repaint workaround, cursor/KMS/scheduler workaround, sleep, synchronous GL completion, Kitty special case, or speculative NVIDIA workaround.
- No Eclipse modification unless native evidence proves an Eclipse-side protocol defect.
- No wholesale revert of the logical/sample-coordinate work in `e6715b5` or `917ced5`.

## Design

### Bounded trace

The renderer will use a cached process-level gate for `TYPHON_EFFECT_EXEC_TRACE=1`. The disabled path checks the gate before constructing event payloads, regions, lists, or formatted strings, and does not take a logging lock. Enabled events are single lines with bounded scalar fields and bounded IDs; they never contain shader source, complete command arrays, or unbounded regions.

Frame-level begin/end events will surround scene resolution, graph compilation, demand planning, effect resource synchronization, graph execution, graph release, renderer draw completion, and render-fence export. Each frame context will report available frame/render/scene identity, repaint mode, render and repair damage signatures, visible/selected effect counts, graph pass/texture counts, and peak live intermediates.

Every selected pass will emit a begin event immediately before pass setup/draw and an end event only after the pass's CPU-side GL submission and state restoration. Events will include the pass and instance IDs, pass kind, anchor/scope/group, graph and physical texture IDs, logical domains, dimensions, origins/flips, damage summary, checkpoint count, and capture mode/command count for capture passes. The end marker will be explicitly documented as a CPU submission boundary, not proof of GPU completion.

### Safety invariants

Before each pass is issued, the executor will validate the graph metadata and realized resources needed by that pass. A sampled input's physical GL texture must differ from the physical output texture. Invalid feedback returns the ordinary renderer error and therefore uses the existing effect fallback path.

Framebuffer-blit capture will validate distinct read/draw framebuffer identities, a non-aliasing graph destination, and framebuffer completeness after attachment. Existing completeness checking remains in place.

Domains and dimensions will be checked for non-zero dimensions, finite/integer representability, required output clipping, non-overflowing domain mapping, in-bounds generated scissors, and Kawase sizes of at least one pixel. Impossible invalid states fail explicitly rather than being silently clamped.

### Moving-domain coverage

The regression will retain one effect identity, surface size, effect program, pool, and GLES context while changing only the logical x/y domain across hundreds or thousands of positions. It will cover adjacent and large moves, all output edges, legal clipping, reversals, and revisits. Each iteration will resolve/compile/plan/execute/release through the actual resource machinery. It will assert bounded cache bytes, plateaued allocations, rising reuse, no checked-out resources after release, no alias, valid FBO/scissors, stable translation-only physical dimensions, exact logical domains, and representative orientation/sample correctness.

If the existing surfaceless harness cannot provide client surface textures for complete scene replay, the test will execute the deepest real-GLES graph prefix available and cover the remaining scene-domain contract with deterministic model assertions. It will not claim to reproduce the NVIDIA freeze.

### Transition-damage coverage

The damage regression will translate a same-identity blur from old domain A to new domain B without a content commit. It will assert repair coverage for both old and new visible areas, current-domain capture rather than an old/new bounding capture, ordinary repair of old-only pixels, blurred composite at new pixels, and exclusion of unrelated output regions. Adjacent and large displacements will be covered.

### Coordinate contracts

Tests and code comments will state the coordinate contract for SceneCapture replay, framebuffer capture/blit, first and later Kawase downsample, Kawase upsample, normalization, final composite, lifecycle capture, and shared helpers:

- logical coordinate origin;
- physical texture storage origin;
- framebuffer origin;
- vertex UV orientation;
- input sample orientation;
- output-domain mapping.

The logical domain translation remains independent from the backing texture's Y orientation. The explicit `u_effect_target_flip_y` and input sample conversion semantics introduced by `e6715b5` and `917ced5` remain separate.

### Debug-only pass-prefix bisection

If CPU markers do not localize the native failure, a temporary opt-in pass-prefix limiter may submit only the first N selected passes and then enter the existing renderer effect-fallback path. It will release graph resources, restore ordinary GL state, and never report a partially rendered effect frame as successful. It will not use `glFinish()` or remain in the production commit without a separately justified diagnostic need.

### Correction boundary

No renderer behavior will change until a deterministic invariant failure, native pass trace, or controlled prefix result identifies the failing boundary. The correction will be one minimal change at the proven source, with a regression that is red before the change and green after it. If the native NVIDIA reproduction remains ambiguous or frozen after deterministic coverage passes, the result will record the new trace and stop short of a speculative workaround.

## Error handling

- Invalid graph/resource/FBO/domain state returns an ordinary renderer error.
- `execute_effect_graph` retains its existing state restoration and `release_graph` cleanup on both success and failure.
- Trace emission failures are not allowed to change rendering semantics.
- GL submission remains asynchronous; pass end markers do not imply GPU completion.
- The existing exceptional lifecycle `glFinish()` fallback remains unchanged.

## Verification and acceptance

TDD cycles will cover trace-disabled neutrality, invariant failures, moving-domain resource reuse, transition damage, coordinate mappings, and any proven correction. Focused effect damage, render graph, resource pool, executor, real GLES, Atomic EGL/GBM, and lifecycle suites will run before repository-wide verification.

The required commands are:

```text
rtk run -- cargo fmt --check
rtk run -- cargo check --locked --all-targets
rtk run -- cargo clippy --locked --all-targets -- -D warnings
rtk run -- cargo test --locked
rtk git diff --check
rtk run -- bash bin/check-source-layout
```

Native acceptance will use the original Kitty configuration with blur disabled/enabled, slow and large movement, output-edge placement, stationary content updates, concurrent Eclipse Dock/Bar effects, and rapid reversal. Results will record allocations, reuses, evictions, checked-out textures, capture pixels, pass counts, and fallback counts. The hard freeze will be called fixed only after the original NVIDIA Atomic EGL/GBM reproduction is hardware-qualified stable.
