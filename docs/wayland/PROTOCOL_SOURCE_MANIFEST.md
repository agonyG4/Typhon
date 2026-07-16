# Typhon Wayland Protocol Source Manifest

This manifest records the normative inputs used for the advertised Core and
stable `xdg-shell` contract audit. All repository paths below are relative to
the checkout. Cargo registry paths are written as reproducible crate-relative
paths; the locked crate version and XML SHA-256 identify the exact input.

## Repository baseline

- Repository: checkout root (no machine-specific path)
- Recorded branch: `main`
- Actual starting `HEAD`: `321090e48909fd83fedf9c22d1982d081ca894af`
- User-supplied baseline: `f8b2997466e9c68623f0572fbf1a71ee297afae6` (an ancestor of the actual starting HEAD)
- Actual starting worktree: intentionally dirty; the complete pre-existing
  compliance, Atomic KMS, explicit-sync, frame-owned release, orientation,
  and pacing edits were preserved in place. The exact initial status/diff was
  recorded by the task runner before this checkpoint.
- The supplied native Atomic EGL/GBM, explicit-fence, framebuffer-origin,
  frame-owned release, callback-ownership, and adaptive-buffering changes are
  treated as protected regardless of whether they are committed in this tree.

## Locked Rust protocol stack

| crate | version | Cargo.lock checksum |
|---|---:|---|
| `wayland-backend` | 0.3.15 | `2857dd20b54e916ec7253b3d6b4d5c4d7d4ca2c33c2e11c6c76a99bd8744755d` |
| `wayland-server` | 0.31.13 | `cc1846eb04c49182e04f4a099e2a830a2b745610bbc1d61246e206f29c7000a0` |
| `wayland-client` | 0.31.14 | `645c7c96bb74690c3189b5c9cb4ca1627062bb23693a4fad9d8c3de958260144` |
| `wayland-scanner` | 0.31.10 | `9c324a910fd86ebdc364a3e61ec1f11737d3b1d6c273c0239ee8ff4bc0d24b4a` |
| `wayland-protocols` | 0.32.12 | `563a85523cade2429938e790815fd7319062103b9f4a2dc806e9b53b95982d8f` |
| `input` | 0.10.0 | `f9793345a65d71317763a33066b5d8351f8760dde8d4930fe9e39b5f14a7959d` |
| `wayland-protocols-wlr` | 0.3.12 | `eb04e52f7836d7c7976c78ca0250d61e33873c34156a2a1fc9474828ec268234` |

## Exact XML and generated-source paths

### Core and generated core interfaces

- Core XML: `wayland-server-0.31.13/wayland.xml`
- Client-side core XML copy used by generated test proxies:
  `wayland-client-0.31.14/wayland.xml`
- Core XML SHA-256 (both copies):
  `08fb558d96742b41eab330c386f7b0c17d82169e69edad612ea89c17d6b1d53e`
- Server generator invocation: `wayland-server-0.31.13/src/lib.rs:114-123`
- Client generator invocation: `wayland-client-0.31.14/src/lib.rs:212-221`

### Native scroll metadata source

- Locked libinput Rust wrapper: `input-0.10.0/src/event/pointer.rs`
- Source SHA-256: `7716bec599aa37d43abb9061cc55c97a9393b422989206ce33c094588a1ebbb8`
- The audit uses `PointerEvent::ScrollWheel`, `ScrollFinger`, and
  `ScrollContinuous`, including `time_usec`, `scroll_value`, and
  `scroll_value_v120`; the deprecated `Axis` event is retained as unknown
  source compatibility. Wheel v120 values are accumulated independently by
  libinput device and horizontal/vertical axis before conversion to
  `wl_pointer.axis_discrete` steps (120 units equals one step).

### Stable and already-advertised extension XML

- `xdg-shell`: `wayland-protocols-0.32.12/protocols/stable/xdg-shell/xdg-shell.xml`
- `viewporter`: `wayland-protocols-0.32.12/protocols/stable/viewporter/viewporter.xml`
- `presentation-time`: `wayland-protocols-0.32.12/protocols/stable/presentation-time/presentation-time.xml`
- `linux-dmabuf`: `wayland-protocols-0.32.12/protocols/stable/linux-dmabuf/linux-dmabuf-v1.xml`
- `fractional-scale`: `wayland-protocols-0.32.12/protocols/staging/fractional-scale/fractional-scale-v1.xml`
- `color-management`: `wayland-protocols-0.32.12/protocols/staging/color-management/color-management-v1.xml`
- `linux-drm-syncobj`: `wayland-protocols-0.32.12/protocols/staging/linux-drm-syncobj/linux-drm-syncobj-v1.xml`
- `pointer-warp`: `wayland-protocols-0.32.12/protocols/staging/pointer-warp/pointer-warp-v1.xml`
- `xdg-activation`: `wayland-protocols-0.32.12/protocols/staging/xdg-activation/xdg-activation-v1.xml`
- `pointer-constraints`: `wayland-protocols-0.32.12/protocols/unstable/pointer-constraints/pointer-constraints-unstable-v1.xml`
- `relative-pointer`: `wayland-protocols-0.32.12/protocols/unstable/relative-pointer/relative-pointer-unstable-v1.xml`
- `primary-selection`: `wayland-protocols-0.32.12/protocols/unstable/primary-selection/primary-selection-unstable-v1.xml`
- `idle-inhibit`: `wayland-protocols-0.32.12/protocols/unstable/idle-inhibit/idle-inhibit-unstable-v1.xml`
- `xdg-decoration`: `wayland-protocols-0.32.12/protocols/unstable/xdg-decoration/xdg-decoration-unstable-v1.xml`
- `ext-data-control`: `wayland-protocols-0.32.12/protocols/staging/ext-data-control/ext-data-control-v1.xml`
- `wlr-layer-shell`: `wayland-protocols-wlr-0.3.12/wlr-protocols/unstable/wlr-layer-shell-unstable-v1.xml`

Generated module routing used by the locked crates is in
`wayland-protocols-0.32.12/src/xdg.rs`, `src/wp.rs`, and
`src/protocol_macro.rs`, and in `wayland-protocols-wlr-0.3.12/src/lib.rs`.
Typhon’s local generated protocols are invoked from:

- `src/astrea_shortcuts.rs` -> `protocols/astrea-shortcuts-v1.xml`
- `src/astrea_shell_control.rs` -> `protocols/astrea-shell-control-v1.xml`
- `src/wayland_drm.rs` -> `protocols/wayland-drm.xml`

## XML hashes used by this audit

| XML | SHA-256 |
|---|---|
| Core `wayland.xml` (server/client copies) | `08fb558d96742b41eab330c386f7b0c17d82169e69edad612ea89c17d6b1d53e` |
| stable `xdg-shell.xml` | `5084e76386f6c3959bee957a784c57de204be0de3f57533ce07b4be0617b171a` |
| stable `viewporter.xml` | `dcb12279a03746301fe490aaed4b38a403485a925abfce2ccfceb644e104fe71` |
| stable `presentation-time.xml` | `dffac93bcb2bb1d8c385e72b8a8c2c0d4d79a336866322f3ba886dce2b27b1e2` |
| stable `linux-dmabuf-v1.xml` | `ef39de11196083a41e865737f71e89a9ce3d61b94d2dbbed9b156cd89d6bb97f` |
| WLR `wlr-layer-shell-unstable-v1.xml` | `87e0b9c837aecd6977f76f3c47d73088b7159871f5d979dc1840f6cadb5e2ed8` |

## Upstream comparison source

The primary comparison source for this checkpoint is the XML distributed in
the locked `wayland-server` 0.31.13 and `wayland-protocols` 0.32.12 source
packages. Those packages identify the upstream repositories as
`https://gitlab.freedesktop.org/wayland/wayland` and
`https://gitlab.freedesktop.org/wayland/wayland-protocols`; no unpinned live
checkout is mixed into the build. The stable `xdg-shell` revision used here is
the `wayland-protocols` 0.32.12 release source whose exact XML hash is recorded
above.

## Reproduction and regeneration

From a checkout with the locked Cargo.lock and an offline Cargo cache:

```text
cargo fetch --locked --offline
cargo fmt --check
cargo build --locked
sha256sum "$CARGO_HOME/registry/src/"*/wayland-server-0.31.13/wayland.xml
sha256sum "$CARGO_HOME/registry/src/"*/wayland-protocols-0.32.12/protocols/stable/xdg-shell/xdg-shell.xml
cargo test --lib output_production_model_runs_10_000_operations -- --test-threads=1
cargo test --lib dnd_production_state_seeded_model_runs_100_000_transitions -- --test-threads=1
```

The locked Wayland crates invoke `wayland-scanner` through their checked-in
`wayland_protocol!` macros during `cargo build`; no generated protocol source
is copied into the Typhon tree. Typhon-local XML is regenerated by the scanner
calls in `src/astrea_shortcuts.rs`, `src/astrea_shell_control.rs`, and
`src/wayland_drm.rs` during the same build.

## Typhon advertised global versions

The single source of truth is `src/compositor/protocols/versions.rs`, consumed
by registration in `src/compositor/server.rs` and `src/compositor/color.rs`.
The target contract is:

| global | advertised version | condition |
|---|---:|---|
| `wl_compositor` | 6 | always |
| `wl_subcompositor` | 1 | always |
| `wl_shm` | 2 | always |
| `wl_data_device_manager` | 3 | clipboard capability |
| `wp_viewporter` | 1 | always |
| `wp_fractional_scale_manager_v1` | 1 | always |
| `wp_presentation` | 2 | always |
| `zwlr_layer_shell_v1` | 4 | always |
| `zxdg_decoration_manager_v1` | 1 | always |
| `xdg_activation_v1` | 1 | always |
| `astrea_shortcuts_manager_v1` | 1 | always |
| `astrea_shell_control_manager_v1` | 1 | always |
| `xdg_wm_base` | 6 | always |
| `wl_output` | 4 | always |
| `wl_seat` | 7 | always |
| `xwayland_shell_v1` | 1 | registered; visible only to the active private XWayland client |
| `wp_color_manager_v1` | 1 | color capability |
| `zwp_relative_pointer_manager_v1` | 1 | relative-pointer capability |
| `zwp_pointer_constraints_v1` | 1 | pointer-constraints capability |
| `wp_pointer_warp_v1` | 1 | pointer-warp capability |
| `zwp_idle_inhibit_manager_v1` | 1 | idle-inhibit capability |
| `zwp_primary_selection_device_manager_v1` | 1 | primary-selection capability |
| `ext_data_control_manager_v1` | 1 | data-control capability |
| `zwp_linux_dmabuf_v1` | 4 | GPU-buffer capability |
| `wp_linux_drm_syncobj_manager_v1` | 1 | GPU + syncobj capability |
| `wl_drm` | 2 | GPU-buffer capability |

No version is upgraded by this milestone.

## Deterministic compliance evidence

- Output membership model: `compositor::tests::output_model::output_production_model_runs_10_000_operations`, fixed seed
  `0x4f55_5450_5554_3130`, exactly 10,000 operations. The adapter creates
  real server-side surfaces/output resources and calls production registration,
  reconciliation, mapping, movement, resize, and teardown paths; the
  reference side independently computes geometric overlap.
- DnD lifecycle model: `compositor::tests::data_device::dnd_production_state_seeded_model_runs_100_000_transitions`, fixed seed
  `0xdad5_0000_0042`, exactly 100,000 transitions. The adapter calls the real
  `CompositorState` drag/session/offer/source lifecycle methods; the reference
  side independently computes phases, actions, and terminal events.
- Both models are run with `--test-threads=1`; on failure they report the
  fixed seed and operation index.

## Deliberate compositor-policy choices

- Typhon remains single-output. The current output geometry/mode/scale/name/
  description and `done` policy is preserved; no `OutputId` or hotplug model
  is introduced.
- `wl_touch` is not advertised as a seat capability. A `get_touch` request is
  therefore a required `wl_seat.missing_capability` error.
- `wl_data_device_manager` remains v3 when clipboard capability is enabled;
  v3 drag-and-drop is part of the contract and is not removed by lowering the
  version.
- The compositor keeps default buffer scale 1 and normal transform until it
  has a preference to announce. A non-default single-output scale is announced
  with `wl_surface.preferred_buffer_scale` only once per resource generation
  and only at `wl_surface` v6+; no redundant default event is sent. The
  transform preference remains normal because this milestone remains
  single-output and does not add output-orientation policy.
- Renderer/KMS orientation remains independent from the client buffer
  transform; client transform is applied in validated surface geometry and
  texture coordinates only.
- Clipboard ownership remains keyboard-focus driven. Switching focus between
  surfaces of one client does not synthesize a duplicate selection event.
- DnD action choice must be deterministic and XML-compliant; modifier policy
  is optional and will be documented only if implemented.

## Ambiguities and resolution status

| topic | local XML observation | resolution |
|---|---|---|
| `xdg-shell` source interface maximum | locked XML defines v7; Typhon advertises v6 | use v6 only and gate v4/v5 events by bound resource version; no upgrade |
| output membership for hidden/unmapped surfaces | Core XML does not equate visibility and output membership | preserve one deterministic single-output policy and test enter/leave transitions |
| preferred scale/transform defaults | Core XML permits compositor preference events; defaults remain scale 1/normal transform | emit only when a non-default preference is actually announced, version-gated |
| DnD modifier policy | XML gives common guidance but leaves modifier behavior implementation-defined | deterministic bit-order action selection first; add modifier overrides only with tests/docs |
| unsolicited `xdg_wm_base.pong` | XML defines no pong error | do not invent one; track only pings emitted by Typhon |

Any unresolved request semantics encountered during later checkpoints must add
an exact XML line and a focused interoperability/reference test here before
implementation.
