# Source Layout

```text
main
  ↓
native_output
  ├─ native bootstrap/runtime
  ├─ native input and shell launch
  ├─ output target, cursor, damage, and scanout
  └─ egl_renderer
       └─ oblivion_one::{compositor,native,render_backend,session}
```

## Boundaries

- `src/native/` is reusable native infrastructure: event-loop helpers,
  explicit-sync watchers, DRM helpers, scheduling, and KMS backends.
- `src/native_output/` is the binary-private native DRM/KMS runtime. It owns
  launch policy, native input, output selection, damage, cursor, scanout, and
  the `NativeRuntime` event loop.
- `src/compositor/` owns Wayland protocol dispatch and compositor state.
- `src/compositor/tests/` and `src/native_output/tests/` are connected white-box
  test trees.
- `src/egl_renderer.rs` is the native EGL/GLES renderer façade. Its child
  modules split short-lived frame orchestration, ordinary scene/output/
  checkpoint state, effect execution and resource pooling, retained lifecycle
  visual ownership (including Lamp and Squash), ordinary image resources,
  damage, dmabuf, geometry, and shader helpers. `scene/` owns scene identity and
  shared command construction, `checkpoint.rs` defines the neutral causal data
  model, and `egl_support/` contains stateless native config/context/presentation
  helpers. `frame/` borrows the persistent scene, lifecycle, effect, and ordinary
  resource owners for one render call; it does not retain frame state. The root
  remains the renderer composition point and native-facing capture/presentation
  lifecycle boundary.
- `src/core/geometry.rs` contains reusable geometry types.

Architecture code must use Rust modules, not `include!()`. Disconnected Rust
files are rejected because they hide ownership from tooling and contributors.
Production files are capped at 1,500 lines, test files at 2,000 lines, and
`mod.rs` facades at 800 lines unless an existing exception is documented in
`bin/check-source-layout`. Only `src/compositor/render.rs` and
`src/egl_renderer.rs` are exempt today.

> Status note (2026-10-19): the tree currently exceeds these limits.
> `./bin/check-source-layout` exits with status 1 and reports 104 files over
> their limit (71 production files, 30 test files, and 3 `mod.rs` facades). The
> largest are `src/effects/render_graph.rs` (9,484 lines),
> `src/egl_renderer/effects/gpu_timing.rs` (7,543), `src/native_output/pacing.rs`
> (6,339), `src/native_output/scanout/output_swapchain.rs` (5,466), and
> `src/native_output/scanout/atomic_egl_gbm.rs` (5,192). Until those files are
> split or the policy is relaxed, the check does not pass.

The runtime graph has one product path: native bootstrap into `NativeRuntime`.
