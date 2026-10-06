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
`bin/check-source-layout`.

The runtime graph has one product path: native bootstrap into `NativeRuntime`.
