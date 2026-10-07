# Typhon effects engine

The effects engine is a renderer-independent, typed effect graph compiled only
when a visible effect instance exists. A frame with no visible effects keeps the
`LegacyScene` plan and the existing textured-quad renderer path.

## Trust model

There are three boundaries:

- internal Rust code builds and validates the typed effect IR;
- trusted local configuration defines named programs and may load bounded GLSL
  assets from an explicitly selected root;
- normal Wayland clients can request only the semantic
  `ext-background-effect-v1` blur interface. They cannot submit graphs, shader
  source, paths, texture handles, or GPU objects.

The private `astrea_effects_manager_v1` interface is advertised only with the
qualified background-effect capability. It accepts authenticated named
programs and typed values; every mutation is checked against the owning client,
the surface, the trusted registry, and the parameter schema.

Astrea also has a separate authenticated `astrea_background_effect_coverage_v1`
refinement for client-authored background blur. It keeps the public Wayland
contract intact: `ext-background-effect-v1` remains the interoperable blur
request and its integer `wl_region` remains the coarse work, damage, capture,
and visibility upper bound. The private refinement carries at most one
subpixel rounded rectangle and one triangle, is buffered until the associated
`wl_surface.commit`, and is evaluated only at final composition. `EffectRegion`
continues to describe coarse work; renderer-independent `EffectCoverage`
describes continuous visual coverage. Coverage does not apply to automatic or
rule-assigned blur, does not broaden the public region, and does not provide
arbitrary paths or texture masks. See
[`wayland/ASTREA_BACKGROUND_EFFECT_COVERAGE_V1.md`](wayland/ASTREA_BACKGROUND_EFFECT_COVERAGE_V1.md)
for the protocol and lifecycle details.

## Graph and rendering behavior

The initial production primitive is a region-local Dual Kawase backdrop blur.
Its capture is clipped to the effect dependency domain, its downsample levels
decrease geometrically, and its upsample levels reverse that topology. Radius,
scale, and pass count affect both the physical shader sampling and the derived
damage footprint.

After instance selection, the graph derives a bounded per-pass execution
demand by walking producer edges once in reverse pass order. Composite and
post-process sinks seed the walk; local stages propagate pointwise regions,
custom fragments use their declared footprint, and each Dual Kawase pass maps
its demanded output through the output/input texture dimensions, shader offset,
linear-filter support, and outward integer rounding. Regions are clipped to the
already allocated graph domains. A precise pass runs only its demanded output
rectangles, while invalid producer metadata or an unrepresentable region falls
back to full-domain execution for that effect instance's internal passes. Its
Composite or post-process sink remains constrained to `pass.damage` union the
validated output-influence region, so capture padding cannot become visible.
Pass traces distinguish precise demand, full internal conservative demand,
output-constrained conservative sinks, and conservative direct framebuffer
capture.

The graph also lowers these built-in stages:

`ColorMatrix`, `Tint`, `Noise`, `Mask`, and `Blend`.

Adjacent single-input local stages are fused only in the proven canonical
`ColorMatrix -> Tint -> Noise` order. The unfused graph remains the semantic
reference. Trusted custom fragment stages use a precompiled compositor-owned
program and are never compiled on the frame-critical path.

`BeforeSurface`, `ReplaceSurface`, `AfterSurface`, and `OutputPostProcess`
anchors are composed at their scene insertion points. Target-content capture is
distinct from backdrop capture and captures the requested target tree.

For checkpoint-dependent Replay `SceneCapture` passes, framebuffer shader-copy
is the production-preferred path when the current output is sampleable. The
existing execution planner falls back to framebuffer blit when no sampleable
output texture is available. This preference applies only to that eligible
checkpoint Replay case; other capture categories retain their existing planner
behavior.

Dependency-free Replay `SceneCapture` passes can also own persistent cache
entries while remaining on the Replay execution path. A new, stale, invalid,
or identity-changed entry receives a full-domain Replay capture. A consecutive
entry is reused without capture framebuffer work, clearing, command selection,
or Replay draws only when the existing causal model proves its source unchanged.
Changed or unproven sources receive a full-domain Replay refresh; these source
caches are never partially updated. Their validity serial and causal baseline
belong to the persistent Effects resource cache and follow the immediately
previous rendered cache state, including unpresented renders. Existing
dependency-bearing framebuffer checkpoint candidates retain admission priority
under the checkpoint soft budget.

When GPU timing is active, `event=effect_gpu_timing` reports
`replay_capture_cache_hits`, `replay_capture_cache_full_refreshes`,
`replay_capture_cache_zero_copy_hits`, `replay_capture_cache_update_pixels`,
`replay_capture_cache_domain_pixels`, and `replay_capture_cache_saved_pixels`
for dependency-free persistent Replay captures. Hits count consecutive cache
populations, full refreshes count physical full-domain Replay updates, zero-copy
hits are a subset of hits, update pixels count physical Replay writes, domain
pixels count considered cache domains, and saved pixels are domain pixels minus
update pixels.

Newly admitted dependency-bearing checkpoints can elide their framebuffer
shader-copy when a changed capture layout replaces a resident checkpoint from
the same logical family and the capture is immediately followed by its unique
first Dual Kawase downsample. The logical `SceneCapture` remains in the graph.
Fusion is limited to that downsample and never reuses pixels from the replaced
checkpoint. Its allocated checkpoint stays unpopulated for that frame, so a
stable identity is materialized on the next frame before normal causal
zero-copy reuse can begin.

GPU timing reports `capture_downsample_fusion_candidates`,
`capture_downsample_fusion_executed`,
`capture_downsample_fusion_elided_capture_pixels`,
`capture_downsample_fusion_output_pixels`, and
`capture_downsample_fusion_ineligible`. A fused `SceneCapture` records zero
physical capture pixels. Its first downsample keeps the ordinary
`DualKawaseDownsample` timing, including the additional direct-output sampling
work.

`TYPHON_EFFECT_GPU_STALL_DIAGNOSTICS=1` adds opt-in host submission timings and
one optional GPU-memory sample per Effects graph. It is active only when
`TYPHON_EFFECT_GPU_TIMING=1` is also set. `effect_graph_host_cpu_ns` measures
host wall time across graph submission and timer-query submission;
`total_ns` remains the GPU timestamp interval. Pass host timings are attached
to the same asynchronous pass spans as their GPU timestamps and separate
query-submission time from `execute_pass` time. Diagnostics do not wait for GPU
completion.

When `GL_NVX_gpu_memory_info` is available, the profiler reports approximate
available video memory and eviction counters; memory quantities are in KiB.
Eviction counters are running values and may already be non-zero at the first
sample, which has no eviction delta. These values are telemetry only and do not
control Effects resource budgets or rendering decisions. An eviction caused
during a frame may first appear in the following graph's sample, so native spike
analysis should inspect the spike frame and its immediate neighbors. The
sampling host duration is reported separately as
`gpu_memory_sample_host_cpu_ns`.

`TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH=blit` forces the diagnostic
framebuffer-blit path. Setting it to `shader-copy` explicitly requests the
preferred shader-copy path while retaining the no-sampleable-output fallback.
When unset, the production default is the same shader-copy preference. Invalid
values emit the existing bounded warning and use that production default.

## Retained window lifecycle effects

`window.minimize` and `window.restore` use the retained lifecycle owner for
Lamp and Squash. The owner freezes the presented client geometry, retained
surface presentation snapshot, server-side decoration snapshot, and any
resolved owned-effect visual source. A reversal transfers that same payload to
a new exact presentation identity, keeps the active effect family, and starts
from the current semantic progress. Ownership retires only after the matching
retained identity is represented in a rendered frame and acknowledged by
physical presentation. Restore samples suppress the canonical live root until
that acknowledgement.

Squash maps semantic progress `p` from normal (`0`) to minimized (`1`) and uses
`q = clamp(p, 0, 1)^3`. The presented client rectangle is the component-wise
linear interpolation from the retained presented client rectangle to the
taskbar or dock icon rectangle by `q`; opacity is canonical opacity multiplied
by `1 - q`. Restore reverses the same progress and mapping, which yields an
OutCubic visual recovery. The complete retained visual, including subsurfaces,
frozen SSD, and a frozen resolved-effect source, receives that same affine
transform. Squash draws retained primitives as textured quads and does not use
Lamp's nonlinear mesh. Its base duration is 250 ms before global speed scaling.

The animation catalog advertises `minimize.squash` only when the active GLES
renderer has qualified its textured lifecycle path. The KDE preset selects
Squash for minimize and restore; Astrea continues to select Lamp. If no valid
taskbar or dock anchor is available, Squash admission fails and the window
follows the existing immediate lifecycle fallback.

## Color and alpha

Captured output is decoded from output-encoded sRGB once at the first effect
stage or blur downsample. Effect math and intermediates use linear sRGB, and
the final composite encodes once at the output boundary. The first milestone
does not advertise HDR or color-management capability and does not qualify
wide-gamut or scRGB behavior.

Effect textures preserve the renderer's premultiplied-alpha contract. Local
color stages preserve coverage, mask stages select normal or inverted alpha,
and blend stages apply bounded opacity with source-over/add/multiply/screen
modes.

## Trusted local configuration

The versioned JSON manifest is parsed off the draw path with strict unknown
field rejection. It is capped at 1 MiB, effect names are bounded, programs and
nodes use bounded typed IDs, and custom shader files must be regular files
resolved below the explicitly supplied trusted root. Relative paths containing
absolute, parent, root, or prefix components are rejected. Shader source is
bounded by the effect shader-source limit.

Definitions include working space, alpha mode, frame demand, failure policy,
outsets, nodes, output, and typed parameters. Parameters carry a type,
default, optional scalar or component-wise vector bounds, and an impact
classification:
`UniformOnly`, `Footprint`, or `Structure`. Built-in and custom assets are
published as immutable registry generations. A reload compiles every changed
trusted shader before publication; a failed generation leaves the previous
generation active.

Trusted v1 manifests accept only `UniformOnly` parameters. `Footprint` and
`Structure` remain internal classifications reserved for a future version that
can recompute graph damage or topology from parameter changes; they are rejected
with a typed configuration error today.

The reserved `system.*` namespace and the built-in `system.background_blur`
program are owned by Typhon. Their program and internal shader-module IDs
cannot be overridden by a trusted manifest. V1 supports `OnDamage` and
`Continuous` frame demand; `Manual` is rejected until a manual trigger API
exists. The only advertised failure policy is `passthrough`; `disable-instance`
is rejected in v1. Protocol mutation is provided for `Float`, `Vec2`, and
`Vec4`; `Vec3` and `Int` remain trusted-default-only and are not protocol
mutable in v1.

`StaticTexture` remains an internal IR shape so a future bounded immutable
trusted-asset table can be added without changing the effect IR or shader ABI.
It is explicitly unsupported in v1: validation and registry publication reject
any program that requires it with `UnsupportedStaticTexture`. No blank or
fallback texture is allocated, and the semantics are never silently degraded.

## Global material programs

The Trusted Effect Registry contains every valid trusted-local named effect.
The Material Program Catalog is a qualified subset for global background
material selection. A catalog candidate must already be validated, use
`OnDamage`, read at least one `Backdrop` source, avoid `TargetContent` and
`StaticTexture`, and declare only `UniformOnly` parameters. Valid trusted
effects that do not meet these material-role rules stay in the trusted
registry and are omitted from the catalog. Continuous effects are not global
material programs in this phase.

`system.background_blur` is the first catalog entry and the default and
fallback program. Trusted-local entries follow in stable name order. Catalog
responses expose only the stable name, origin, schema signature, and parameter
count; they do not expose program IDs, graphs, shader source, or shader paths.

`material.program.get` reports the requested configuration separately from
the effective program. If a persisted request disappears from a trusted
registry generation or becomes unqualified, Typhon temporarily resolves it to
`system.background_blur` and reports `missing` or `unqualified`. The requested
name remains persisted and becomes effective automatically if a later
successful generation restores its qualification. A failed registry reload
keeps the previous generation and its effective selection.

The semantic `ext-background-effect-v1` client still requests only a blur
assignment. For each visible assignment, Typhon supplies the globally
effective material program, manifest parameter defaults overlaid with valid
persisted overrides for the matching schema, and that program's frame demand
to the existing effect renderer. A stale or invalid persisted override set
falls back to the complete manifest defaults and remains stored for a later
matching schema. Selecting a program or changing its parameters without a
visible blur assignment creates no effect instance. Phase 2A
`MaterialConfiguration` remains separate and continues to update the built-in
program while a trusted-local program is selected.

## Custom shader ABI

Trusted GLSL ES 3.00 assets export:

```glsl
vec4 typhon_effect_main(TyphonEffectContext ctx);
```

Typhon owns `main()`, the vertex shader, the primary sampler, context uniforms,
precision, output, and reserved names. The context is value-only and supplies
texture size, content rectangle, output size, scale, time, delta, and normalized
UV. The fields have these exact meanings:

- `ctx.texture_size` is the physical pixel dimensions of the primary effect
  input texture.
- `ctx.output_size` is the physical pixel dimensions of the current compositor
  output, not the region-local stage texture.
- `ctx.scale` is the actual output scale metadata.
- `ctx.content_rect` is the primary effect/input domain in output-space
  coordinates (`x`, `y`, `width`, `height`).
- `ctx.uv` is normalized to the current stage texture, and `ctx.time`/`ctx.delta`
  are compositor-owned monotonic seconds and bounded frame delta.

Multi-input normalization maps each source's output-space domain into the
primary stage domain. Pixels outside a source domain are written as transparent
black, and normalized validity covers the consuming stage's declared sampling
footprint without forcing full-output work. Trusted stages sample the primary input with `typhon_sample_primary(ctx.uv)`
and bounded auxiliary inputs with `typhon_sample_aux(index, uv)`; the fixed
sampler ABI is `u_typhon_primary`, `u_typhon_aux0` through `u_typhon_aux7`, and
`u_typhon_aux_count`. Declared typed uniforms are bound from the validated
parameter block. Shader bodies cannot define `main()` or collide with reserved
ABI declarations. The runtime also compiles and links a representative wrapper
inside a real GLES 3 context at the renderer boundary.

Trusted custom GLSL is responsible for returning finite, normalized
premultiplied SDR color. Typhon's built-in stages sanitize non-finite values,
clamp alpha to `[0,1]`, keep RGB within the premultiplied alpha range, and use
piecewise sRGB conversion that never evaluates `pow()` on a negative base.

The production loader uses the fixed per-user manifest
`$XDG_CONFIG_HOME/AstreaOS/typhon/effects.json`, falling back to
`$HOME/.config/AstreaOS/typhon/effects.json`, with that `typhon` directory as
the trusted shader root. Startup load and the explicit `astreactl effects
reload` command both prewarm the candidate generation before publication;
invalid candidates leave the previous generation active.

## Damage, presentation, and fallback

Effect source sampling expands capture dependency damage; zero-footprint local
stages do not expand it. Disjoint damage rectangles remain disjoint where the
region limit permits, and effect transition damage is identity-based so an
unchanged static effect does not dirty itself every frame. Continuous effects
request localized compositor-owned frame demand.

A successful trusted registry reload invalidates presented effect history and
queues one compositor-owned redraw. `OnDamage` effects remain one-shot after
that refresh, `Continuous` effects keep their existing cadence, and failed
reloads leave both the active generation and redraw state unchanged.

Any visible pixel-modifying effect requires composition, so Direct Scanout is
rejected with `effect_requires_composition`. Removing the effect invalidates
the relevant presented history and allows Direct Scanout recovery through the
existing planner. Explicit sync, KMS, pageflip, and buffer-lifetime ownership
remain outside the GLES graph executor.

Graph, resource, shader-availability, and execution failures fall back to the
legacy scene path and expose bounded stable diagnostics. They never substitute
a black or transparent frame. A failed graph/shader generation is not retried
on every frame until a new registry generation is published.

## Resource limits and diagnostics

The Effects resource pool has a 128 MiB default hard budget for all pooled
Effects textures. The persistent checkpoint cache has a separate 64 MiB default
soft budget. Graph transient work reserves hard-budget headroom first; the
optional checkpoint cache can be bypassed when either limit would be exceeded.
Idle reusable textures can still be evicted deterministically to make room for
new allocations, and idle size history is dropped on output resize.

Set `TYPHON_EFFECT_RESOURCE_BUDGET_MIB` to override the hard budget and
`TYPHON_EFFECT_CHECKPOINT_CACHE_BUDGET_MIB` to override the checkpoint soft
budget. Values must be positive integer MiB. A missing value uses its default;
an invalid, non-Unicode, zero, malformed, or overflowing value emits a warning
and uses that variable's default. If the requested checkpoint budget exceeds
the effective hard budget, Typhon warns and clamps it to the hard limit.
Typhon does not automatically size either budget from VRAM yet.

The 128 MiB default does not qualify Effects graphs broadly for 4K. The tested
fullscreen-blur graph estimate is 82,944,000 bytes, which is below this hard
cap, but other graph shapes and workloads are not covered by that result.

The shader cache is bounded and prewarms built-ins and trusted custom programs
at a safe GL boundary. Context teardown clears shader programs, effect
textures, scratch FBOs, and effect quad objects.

Per-frame native performance fields include visible/executed/failed effect
instances, graph pass and peak-live counts, allocated capture-texture pixels,
executed capture-region pixels, capture/output pixels, blur pass counts,
resource allocation/reuse/eviction counts, GPU-cache bytes, and a stable
effect failure reason. Allocated capture pixels are physical texture area;
executed capture pixels are the physical target area covered by the demanded
capture rectangles. GPU pass timing pixel fields remain bounded effect-space
region areas. `CompiledFrameGraph::explain()` provides a deterministic compact
graph description without shader source or uniform values.

## Capability and qualification status

`ext-background-effect-v1` and `astrea_effects_manager_v1` are constructed only
when `RendererProtocolCapabilities.background_effect` is true. Color management
remains disabled in the default capability profile. The full effects behavior
is deterministic-test qualified; real TTY/DRM hardware qualification at
1920x1080@165, including GPU timings and the native presentation matrix,
remains deferred. See [EFFECTS_QUALIFICATION.md](EFFECTS_QUALIFICATION.md).
