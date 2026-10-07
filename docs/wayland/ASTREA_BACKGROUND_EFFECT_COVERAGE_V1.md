# Astrea background-effect analytic coverage

`ext-background-effect-v1` remains the public, interoperable request for
background blur. Its committed `wl_region` is integer and rectangular. It is the
authoritative upper bound for effect work and visible output, including damage,
backdrop capture, blur processing, and coarse scissoring.

`astrea_background_effect_coverage_v1` is a private, authenticated Astrea
refinement of that request. It does not change the public protocol or its
behavior for ordinary clients. A coverage object belongs to one same-client
`wl_surface`; the existing Astrea shell authorization is required to create
and mutate it. The v1 shape is bounded to an optional rounded rectangle and an
optional triangle. The renderer evaluates their union; there is no arbitrary
path, alpha texture, GPU handle, or unbounded primitive list.

Coverage requests are pending surface state. Ordinary commits publish them;
synchronized subsurface commits cache them and publish them with the parent
transaction. `clear()` and destroying the coverage object queue removal for the
next associated surface commit. Stored coverage is visually inert without a
committed public client blur request. Automatic and rule-assigned blur never
consume the sidecar.

At effect-scene resolution, the committed surface-local logical coordinates
are translated once by the active scene's raw `wl_surface` origin into
output-local logical coordinates. `EffectRegion` remains the coarse work and
damage authority. `EffectCoverage` refines final visual coverage only, so
coverage cannot reveal blur outside the public committed region or alter
capture allocation and blur kernels.

The effect graph processes an opaque result as usual. Only the final composite
evaluates signed-distance coverage and its derivative-based antialiasing, then
scales premultiplied RGBA by the coverage before source-over blending. No mask
texture, extra capture, or extra blur pass is needed. Without analytic
coverage, the existing graph, blend choice, and rendered behavior are
preserved.
