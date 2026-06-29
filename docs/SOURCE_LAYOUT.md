# Source Layout

Typhon keeps the reusable compositor library separate from binary-only runtime
adapters.

```text
main
 ├─ native_output ──> egl_renderer
 │        └─────────> oblivion_one::{compositor,native,render_backend,session}
 └─ nested_output ──> nested_renderer

compositor protocols ──> compositor state/domain logic ──> render scene data
native_output runtime ──> reusable native KMS/event-loop primitives
```

## Boundaries

- `src/native/` is reusable native infrastructure: event-loop helpers,
  explicit-sync watchers, DRM helpers, scheduling, and KMS backends.
- `src/native_output/` is the binary-private native DRM/KMS output runtime. It
  owns launch policy, native input adapters, output selection, damage, cursor,
  scanout backends, and the runtime loop that calls `OwnCompositorServer`.
  The runtime entry point is `NativeRuntime`; it owns the long-lived native
  backend state and transfers control from bootstrap to the native cycle
  implementation.
- `src/compositor/protocols/` decodes Wayland requests and emits protocol
  events/errors.
- `src/compositor/state/` contains the physical implementation split for
  `CompositorState` domain methods. These are real Rust child modules with
  inherent `impl CompositorState` blocks; the current split is intentionally
  mechanical and preserves the existing state model.
- `src/compositor/tests/` keeps unit tests under the compositor module so
  white-box coverage can continue using private helpers. Shared client/server
  harness code lives under `src/compositor/tests/support/`; individual test
  modules remain connected through `src/compositor/tests/mod.rs`.
- `src/native_output/tests/` is the connected white-box test tree for native
  output, scanout, input, and frame-scheduling helpers.
- `src/egl_renderer.rs` remains the GLES renderer facade. Its existing
  submodules own damage, dmabuf probing, geometry, and shader program helpers.

## Rules

New code should go in the smallest domain file that owns the invariant being
changed. Protocol dispatch should call state/domain operations rather than
growing policy. Native runtime code should not move into `src/native/` while it
depends on binary-private renderer modules.

Architecture code must use Rust modules, not `include!()`. The source-layout
guard rejects `include!()` in `src/` because include-based organization hides
module boundaries from rust-analyzer, contributors, and coding agents.
Disconnected `.rs` fragments are not allowed: every retained source or test
file must be reachable from the Rust module tree.

Production files are capped at 1,500 lines, test files at 2,000 lines, and
`mod.rs` facades at 800 lines unless explicitly documented below. New runtime
or test splits should keep facades small and move behavior into named modules
that own a clear concern.

This refactor does not remove the nested backend and does not split the crate;
both would change public/runtime behavior outside the architecture-only scope.

## Temporary Size Exceptions

`bin/check-source-layout` allows these remaining oversized files because the
working tree already contains unrelated functional edits and further splitting
would obscure baseline failures:

- `src/compositor/render.rs`
- `src/egl_renderer.rs`
- `src/nested_output.rs`

The original 10,000-line modules are not allowlisted.

`src/native_output/runtime/cycle.rs` is no longer allowlisted. Native bootstrap,
event/input dispatch, acquire processing, frame preparation, presentation, and
metrics now live in named runtime modules and phase methods without changing
ordering.
