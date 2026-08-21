# Typhon GTK/Flatpak Cursor Interoperability Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon's logical cursor configuration and cursor ownership consistent across compositor cursors, Wayland cursor surfaces, cursor-shape clients, GTK/Flatpak settings, software composition, and native presentation.

**Architecture:** Keep `CursorConfiguration` as the only desktop theme/size source. Add a pure cursor-geometry boundary shared by committed surface sizing and presentation tests; store the focused client's cursor as one typed choice (`Hidden`, `Surface`, or `Shape`) with interaction override precedence; expose the canonical configuration through the intentionally limited Settings portal; and resolve cursor-shape images through a bounded lazy theme cache.

**Tech Stack:** Rust, `wayland-server` 0.31.13, `wayland-protocols` 0.32.12 staging `cursor-shape-v1`, zbus, XCursor parsing, existing Typhon compositor/native-output test harness, Cargo, RTK wrappers, and repository source-layout checks.

## Global Constraints

- Preserve all pre-existing user changes in `.codex/config.toml` and `bin/`; do not reset, clean, restore, overwrite, or broadly stage them.
- Use the generated protocol from the existing `wayland-protocols` dependency; do not vendor duplicate XML.
- `CursorConfiguration.size_px` means logical cursor pixels; never multiply it by output scale for `XCURSOR_SIZE` or portal `cursor-size`.
- Apply client `buffer_scale` and `buffer_transform` once to obtain logical surface geometry; apply output scale only at presentation.
- Keep viewport cursors on software composition unless complete viewport-aware native conversion is implemented.
- Do not add filesystem reads, portal calls, theme parsing, heap-heavy shape lookup, or global mutex contention to cursor motion.
- Do not advertise `wp_cursor_shape_manager_v1` until capability, dispatch, validation, replacement semantics, and shape selection are implemented and tested.
- Linux GTK/Flatpak/DRM/KMS/native Wayland qualification is `NOT RUN — Linux target/environment required` unless actually executed in a suitable Linux environment.
- Before each task commit, run `rtk git diff --check` and the focused test/format command that is available.

---

### Task 1: Establish the cursor geometry model and regression tests

**Files:**
- Create: `src/cursor_geometry.rs`
- Modify: `src/lib.rs`
- Modify: `src/compositor/state_data.rs`
- Modify: `src/compositor/render.rs`
- Modify: `src/native_output/output/cursor.rs`
- Modify: `src/native_output/output/cursor_tests.rs`
- Test: `src/cursor_geometry.rs` unit tests

**Interfaces:**
- Consumes: committed buffer width/height, buffer scale, buffer transform, viewport state, logical hotspot, and output scale.
- Produces: `CursorGeometry` with transformed buffer size, committed logical size, logical hotspot, and physical raster size/hotspot; `PendingSurfaceBuffer` and native/software tests consume the same conversion rules.

- [ ] **Step 1: Write failing pure geometry tests.** Add tests named `buffer_scale_one_preserves_logical_size`, `integer_buffer_scale_converts_pixels_to_logical_size`, `non_square_buffer_scale_preserves_aspect`, `transform_rotations_swap_dimensions_and_transform_hotspot`, `fractional_output_scale_is_applied_once`, and `hardware_and_software_cursor_geometry_have_equal_visual_bounds`. Use these exact fixtures:

```rust
assert_eq!(logical_size(24, 24, 1, Transform::Normal), Size::new(24, 24));
assert_eq!(logical_size(48, 48, 2, Transform::Normal), Size::new(24, 24));
assert_eq!(logical_size(64, 32, 2, Transform::Normal), Size::new(32, 16));
assert_eq!(physical_size(Size::new(24, 24), 1.5), Size::new(36, 36));
```

Include all four transforms and assert that the hotspot stays attached to the same visual pixel after transformation. Assert that no test path computes `24 * 2 * 1.5`.

- [ ] **Step 2: Run the focused geometry tests to verify the new API is absent.**

Run: `rtk cargo test --locked cursor_geometry --lib`

Expected: FAIL because the geometry module/functions do not yet exist.

- [ ] **Step 3: Implement the pure geometry API.** Define `CursorSize`, `CursorHotspot`, and `CursorGeometry` plus functions with these signatures:

```rust
pub fn logical_size(
    buffer_width: u32,
    buffer_height: u32,
    buffer_scale: u32,
    transform: wl_output::Transform,
) -> Result<CursorSize, CursorGeometryError>;

pub fn physical_size(logical: CursorSize, output_scale: f64) -> CursorSize;

pub fn transform_hotspot(
    hotspot: CursorHotspot,
    source: CursorSize,
    transform: wl_output::Transform,
) -> Result<CursorHotspot, CursorGeometryError>;

pub fn geometry_for_surface(
    buffer: CursorSize,
    buffer_scale: u32,
    transform: wl_output::Transform,
    viewport_destination: Option<CursorSize>,
    hotspot: CursorHotspot,
    output_scale: f64,
) -> Result<CursorGeometry, CursorGeometryError>;
```

Reject zero scales, non-divisible integer buffer dimensions, invalid hotspots, and overflow instead of guessing. Keep viewport destination as an explicit logical override and preserve the existing safe native fallback for viewport source/destination.

- [ ] **Step 4: Route existing surface-size logic through the model without changing valid behavior.** Make `PendingSurfaceBuffer::surface_size_for_buffer_scale` use the new logical-size rule. Keep `surface_size_for_state`'s destination/source precedence unchanged. Add an assertion-level test around the existing `PendingSurfaceBuffer` path proving `48x48 @ 2` commits as `24x24`.

- [ ] **Step 5: Align software and native test geometry.** Update `draw_client_cursor` tests and `client_cursor_image` tests to consume the same logical/physical expected dimensions. Add output-scale fixtures for `1.25`, `1.5`, and `1.75`; verify the software render target and native uploaded image use one output-scale application. Keep viewport images returning `None` from `client_cursor_image` and add a regression test proving they cannot enter the hardware path.

- [ ] **Step 6: Run focused verification and commit the invariant.**

Run:

```text
rtk cargo test --locked cursor_geometry --lib
rtk cargo test --locked native_output::output::cursor_tests --lib
rtk cargo test --locked compositor::render --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit only geometry-owned files:

```text
git add -- src/cursor_geometry.rs src/lib.rs src/compositor/state_data.rs src/compositor/render.rs src/native_output/output/cursor.rs src/native_output/output/cursor_tests.rs
git commit -m "test(cursor): cover client cursor logical scaling"
```

### Task 2: Expose canonical cursor settings through the portal

**Files:**
- Modify: `src/portal.rs`
- Modify: `src/main.rs`
- Modify: `src/cursor_persistence.rs` only if a public read constructor/helper is required
- Modify: `src/lib.rs` portal tests

**Interfaces:**
- Consumes: `CursorConfigurationStore::read()` and `default_cursor_configuration()`.
- Produces: `settings_for_configuration(namespaces, configuration)`, request-time `SettingsBackend` reads, and `PortalSettingValue::String`/`I32` values with correct zvariant conversion.

- [ ] **Step 1: Write failing portal tests.** Add tests for exact namespace filtering and types:

```rust
let config = CursorConfiguration::new("Bibata", 32).unwrap();
let values = settings_for_configuration(
    &["org.gnome.desktop.interface".to_string()],
    &config,
);
assert_eq!(values["org.gnome.desktop.interface"]["cursor-theme"], PortalSettingValue::String("Bibata".into()));
assert_eq!(values["org.gnome.desktop.interface"]["cursor-size"], PortalSettingValue::I32(32));
```

Cover `[]`, `[""]`, exact GNOME namespace, `org.gnome.desktop.*`, appearance, an unknown namespace, and an unknown key. Add a persistence-observation test that writes two valid canonical configurations and asserts a new backend read returns the second one.

- [ ] **Step 2: Run the focused portal tests to verify the missing behavior.**

Run: `rtk cargo test --locked portal_settings --lib`

Expected: FAIL because the GNOME namespace and request-time configuration source are not implemented.

- [ ] **Step 3: Implement typed portal values.** Extend `PortalSettingValue` with `String(String)` and signed `I32(i32)`. Convert them to zbus `OwnedValue` using the exact variant types `s` and `i`. Keep appearance values and their existing types unchanged.

- [ ] **Step 4: Implement bounded namespace/key filtering.** Add `settings_for_configuration(namespaces, configuration)` and make `settings_for_namespaces` call it with the validated default for pure compatibility tests. Return only `cursor-theme` and `cursor-size` for `org.gnome.desktop.interface`; do not mirror unrelated GNOME schema keys. Preserve exact, empty, and `.*` matching semantics.

- [ ] **Step 5: Make the live backend read the persisted configuration per request.** Give `SettingsBackend` a request-time configuration source built from the existing secure `CursorConfigurationStore`. On read failure, use `default_cursor_configuration()` without writing it back. Update `run_backend` and `main::portal` only as needed to construct the backend with the same environment/config boundary used by the compositor. Do not add polling or shared mutable process memory.

- [ ] **Step 6: Add explicit live-notification scope.** Inspect existing control/subscription code for a safe reuse path. If none is already suitable, keep `Read`/`ReadAll` correct and add a source comment/documentation stating that `SettingChanged` for cursor keys remains pending Linux qualification; do not emit a fabricated notification.

- [ ] **Step 7: Run portal and launch-environment verification and commit.**

Run:

```text
rtk cargo test --locked portal --lib
rtk cargo test --locked supervised_child_cursor_environment_is_command_local --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit:

```text
git add -- src/portal.rs src/main.rs src/cursor_persistence.rs src/lib.rs
git commit -m "fix(portal): expose canonical cursor settings to GTK clients"
```

### Task 3: Consolidate focused client cursor ownership

**Files:**
- Modify: `src/compositor/state/support_types.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/surface_commit_cursor.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/compositor/tests/input_output/pointer_cursor.rs`
- Test: `src/compositor/tests/input_output/output_keyboard_cursor.rs`

**Interfaces:**
- Consumes: existing pointer focus/enter serial helpers and `InteractionCursorOverride`.
- Produces: `ClientCursorChoice`, `client_cursor_shape()`, `client_cursor_render_state()`, and one replacement path used by both legacy and shape requests.

- [ ] **Step 1: Add state-transition tests before refactoring.** Extend the compositor protocol test harness with model-level assertions for:

```text
Surface(A) -> Shape(pointer) == Shape(pointer)
Shape(text) -> Surface(B) == Surface(B)
Shape(pointer) -> Hidden == Hidden
focus change clears the previous client's choice
surface destruction clears only a matching Surface choice
pointer destruction makes the choice inert
```

Retain and run the current hidden/move/resize restoration tests as regression coverage.

- [ ] **Step 2: Run the focused cursor tests.**

Run: `rtk cargo test --locked pointer_cursor --lib`

Expected: the new replacement tests fail or cannot compile until the typed state exists.

- [ ] **Step 3: Define the typed choice and derived visibility synchronization.** Add `ClientCursorChoice` to `state/support_types.rs` and replace direct mutation of `active_client_cursor`/hidden ownership with one authoritative `focused_client_cursor` field. Keep the existing backend visibility mirror only behind `sync_cursor_visibility_request`.

- [ ] **Step 4: Refactor `set_pointer_cursor`.** Validate pointer focus and the current enter serial exactly as today, then store `Hidden` for null or `Surface` for a valid surface. Preserve surface-role assignment, pending-unlock behavior, frame callbacks, damage, and cursor generation updates.

- [ ] **Step 5: Refactor cleanup and restoration paths.** Update pointer release, surface destruction, focus changes, cursor commits, and damage-only commits to pattern-match the typed choice. Preserve hidden state across interaction override and restore the prior `Surface`, `Shape`, or `Hidden` choice after move/resize.

- [ ] **Step 6: Expose effective choice to server/native/render code.** Add `OwnCompositorServer::client_cursor_shape()` and keep `client_cursor_render_state()` surface-only. A shape choice must not be converted into a fake client surface.

- [ ] **Step 7: Run the full focused cursor transition set and commit.**

Run:

```text
rtk cargo test --locked pointer_cursor --lib
rtk cargo test --locked output_keyboard_cursor --lib
rtk cargo test --locked window_interaction --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit:

```text
git add -- src/compositor/state/support_types.rs src/compositor/mod.rs src/compositor/state/input_resources.rs src/compositor/state/surface_commit_cursor.rs src/compositor/state/surfaces.rs src/compositor/state/hit_testing.rs src/compositor/server.rs src/compositor/tests/input_output/pointer_cursor.rs src/compositor/tests/input_output/output_keyboard_cursor.rs
git commit -m "refactor(cursor): model focused client cursor choice explicitly"
```

### Task 4: Implement and gate `wp_cursor_shape_manager_v1`

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/server_globals.rs`
- Modify: `src/compositor/plan.rs`
- Modify: `src/compositor/protocols/versions.rs`
- Create: `src/compositor/protocols/cursor_shape.rs`
- Modify: `src/compositor/protocols/input.rs`
- Modify: `src/compositor/protocols/core.rs` only if generated-resource cleanup needs a core helper
- Modify: `src/compositor/tests/mod.rs`
- Test: `src/compositor/tests/input_output/pointer_cursor.rs`
- Test: `src/compositor/tests/protocol_contract.rs`

**Interfaces:**
- Consumes: generated `wayland_protocols::wp::cursor_shape::v1::server` types, `ClientCursorChoice`, and the shared pointer focus/serial authority.
- Produces: capability-gated manager/device globals, `ProtocolCursorShape::try_from(u32)`, and validated `set_shape` dispatch.

- [ ] **Step 1: Add capability and advertisement tests.** Add a `cursor_shape` field to `InputProtocolCapabilities`, set it only in capability profiles that can execute the implementation, and test that the global is absent when false and advertised at dependency version 2 when true. Add the global/version to the protocol inventory and compliance matrix test.

- [ ] **Step 2: Add generated client imports and request tests.** Import the generated client manager/device modules in `src/compositor/tests/mod.rs`. Add tests for same-client `get_pointer`, valid shape, invalid shape protocol error, stale/foreign serial ignore, focus isolation, and device inertness after pointer destruction.

- [ ] **Step 3: Implement the manager global and device registry.** Register `wp_cursor_shape_manager_v1` only when `input_capabilities.cursor_shape` is true. `get_pointer` must verify the supplied `wl_pointer` belongs to the requesting client, initialize the device, and record its pointer identity in a bounded state map. `destroy` removes the device state.

- [ ] **Step 4: Implement `set_shape` validation.** Convert only values represented by the dependency's version-2 enum. Invalid values send `wp_cursor_shape_device_v1::Error::InvalidShape` and do not mutate state. Valid values call the same pointer focus/current-enter-serial validator as `wl_pointer.set_cursor`; invalid serials and foreign focus are ignored. Removed pointer/device entries are inert.

- [ ] **Step 5: Wire replacement semantics.** Make `set_shape` store `ClientCursorChoice::Shape`, make `set_cursor(surface)` store `Surface`, and make `set_cursor(NULL)` store `Hidden`. Add both request-order tests and assert that the later valid request wins.

- [ ] **Step 6: Run protocol-focused verification and commit.**

Run:

```text
rtk cargo test --locked pointer_cursor --lib
rtk cargo test --locked protocol_contract --lib
rtk cargo test --locked plan --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit:

```text
git add -- src/compositor/mod.rs src/compositor/server_globals.rs src/compositor/plan.rs src/compositor/protocols/versions.rs src/compositor/protocols/cursor_shape.rs src/compositor/protocols/input.rs src/compositor/protocols/core.rs src/compositor/tests/mod.rs src/compositor/tests/input_output/pointer_cursor.rs src/compositor/tests/protocol_contract.rs
git commit -m "feat(wayland): implement cursor shape protocol"
```

### Task 5: Add bounded lazy protocol-shape theme loading

**Files:**
- Modify: `src/cursor_theme.rs`
- Modify: `src/cursor_manager.rs`
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/cursor_theme.rs`
- Test: `tests/cursor_manager.rs`

**Interfaces:**
- Consumes: `ProtocolCursorShape` and the canonical `CursorConfiguration`.
- Produces: explicit exhaustive aliases, lazy `active_image_for_protocol_shape`, bounded cache metrics, and effective theme-image selection for software/native presentation.

- [ ] **Step 1: Add exhaustive shape mapping tests.** Define all 36 version-2 values (`default` through `all_resize`) in `ProtocolCursorShape`. Test `try_from(1..=36)` succeeds, every value has a non-empty alias list, aliases include the expected CSS/XCursor names, and values outside the range are rejected. Add a compile-time-sized array or explicit match that requires review when a generated enum version changes.

- [ ] **Step 2: Add failing cache tests.** Add tests named `protocol_shape_cache_is_bounded`, `theme_generation_replaces_protocol_shape_cache`, `size_change_replaces_protocol_shape_cache`, `reload_replaces_protocol_shape_cache`, and `missing_protocol_shape_falls_back_to_pointer`. Use a temporary synthetic XCursor search path so tests never depend on host themes.

- [ ] **Step 3: Implement the bounded lazy cache.** Keep the six existing eager shapes unchanged. Add a generation-owned cache with a fixed independent limit, e.g. `MAX_PROTOCOL_CURSOR_SHAPES`, and load an alias only when `active_image_for_protocol_shape(shape)` is called. Reuse existing file/frame bounds and return the pointer image on missing optional aliases or cache pressure. Ensure each new `LoadedCursorTheme` starts with an empty protocol cache.

- [ ] **Step 4: Select shape images only on cursor-choice changes.** Extend the effective cursor-image synchronization to choose the interaction image, the focused client protocol-shape image, or the pointer default. Keep `DesktopSceneRenderer` and native presentation consuming the selected image; do not call lazy loading from motion-only updates.

- [ ] **Step 5: Preserve custom surface cursors.** Ensure a `Surface` choice continues through `client_cursor_render_state` and native/software surface paths without shape conversion. Add a regression test that a custom SHM surface remains selected while a protocol shape is requested by a different focus owner.

- [ ] **Step 6: Run theme/cache verification and commit.**

Run:

```text
rtk cargo test --locked cursor_theme --lib
rtk cargo test --locked cursor_manager --lib
rtk cargo test --locked native_output::runtime::presentation_cursor --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit:

```text
git add -- src/cursor_theme.rs src/cursor_manager.rs src/native_output/runtime/presentation_cursor.rs src/native_output/runtime/presentation.rs src/compositor/render.rs src/compositor/server.rs src/cursor_theme.rs tests/cursor_manager.rs
git commit -m "test(cursor): cover shape surface replacement and scaling"
```

### Task 6: Add gated cursor diagnostics and audit launch boundaries

**Files:**
- Modify: `src/compositor/state/support_types.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/surface_commit_cursor.rs`
- Modify: `src/native_output/runtime/cursor_cycle.rs`
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/launch_env.rs`
- Modify: `src/lib.rs`
- Test: `src/compositor/state/support_types.rs`
- Test: `src/lib.rs`

**Interfaces:**
- Consumes: typed cursor choice, committed surface geometry, canonical configuration, and `NativeClientCursorPath`.
- Produces: bounded change-time diagnostics and explicit tests proving child environment scope.

- [ ] **Step 1: Write diagnostic gating tests.** Assert that disabled pointer/cursor diagnostics do not format messages or log per motion, while an activation/commit diagnostic includes client, surface, buffer dimensions, scale, transform, logical size, hotspot, output scale, path, and uploaded size.

- [ ] **Step 2: Implement change-time diagnostics.** Reuse `pointer_debug_enabled()` and the native path-change logger. Emit only on client cursor activation, replacement, commit, path transition, or hardware fallback. Never include pixels and never emit a motion line when geometry/choice/path is unchanged.

- [ ] **Step 3: Audit launch routes.** Confirm command-local `XCURSOR_THEME`/`XCURSOR_SIZE` remain set only by `compositor_app_command_with_policy_and_xwayland_and_cursor`. Add tests covering compositor-supervised children, session shell, Astrea launcher, Flatpak/D-Bus activation boundary documentation, XWayland, and restartable children. Do not inject cursor variables into global environment.

- [ ] **Step 4: Run diagnostics and launch tests and commit.**

Run:

```text
rtk cargo test --locked support_types --lib
rtk cargo test --locked supervised_child_cursor_environment_is_command_local --lib
rtk cargo fmt --check
rtk git diff --check
```

Commit:

```text
git add -- src/compositor/state/support_types.rs src/compositor/state/input_resources.rs src/compositor/state/surface_commit_cursor.rs src/native_output/runtime/cursor_cycle.rs src/native_output/runtime/presentation_cursor.rs src/launch_env.rs src/lib.rs
git commit -m "feat(cursor): add gated interoperability diagnostics"
```

### Task 7: Document the closure and prepare Linux qualification

**Files:**
- Modify: `docs/cursor-control.md`
- Modify: `docs/wayland/CORE_COMPLIANCE_MATRIX.md`
- Modify: `docs/wayland/PROTOCOL_SOURCE_MANIFEST.md`
- Create: `REPORT-2026-08-21-gtk-flatpak-cursor-interoperability.md`

**Interfaces:**
- Consumes: completed source/model tests, actual commit hashes, Windows verification output, and the known WSL policy blocker.
- Produces: an English closure report that distinguishes source/model evidence from unrun Linux qualification.

- [ ] **Step 1: Document the final architecture.** Describe the canonical logical size, surface geometry pipeline, typed cursor ownership, shape/surface replacement, bounded lazy cache, portal compatibility namespace, command-local environment boundaries, and viewport software fallback.

- [ ] **Step 2: Update protocol inventories.** Record `wp_cursor_shape_manager_v1` version 2, its capability gate, request validation, tests, and the fact that the GNOME settings namespace is an intentional compatibility extension rather than standardized XDG Settings.

- [ ] **Step 3: Write the Linux qualification checklist.** Include:

```bash
astreactl cursor get
echo "$XCURSOR_THEME"
echo "$XCURSOR_SIZE"
gsettings get org.gnome.desktop.interface cursor-theme
gsettings get org.gnome.desktop.interface cursor-size
```

Also include D-Bus Settings reads for both GNOME keys, native GTK3, GTK4/Libadwaita, Flatpak GTK, Sober when available, Qt Wayland, XWayland, and gated cursor diagnostics. Record the permitted A/B experiment with `flatpak override --env=XCURSOR_SIZE=...` only as evidence, never as a fix.

- [ ] **Step 4: Write the report with evidence labels.** Include baseline HEAD, dirty-worktree state, observed symptom, source investigation, root-cause categories, tests, Windows checks, Linux checks marked `NOT RUN — Linux target/environment required`, commit hashes, final status, and remaining risks. State explicitly that Linux runtime closure is not claimed without real GTK/Flatpak execution.

- [ ] **Step 5: Run documentation/source-layout verification and commit.**

Run:

```text
rtk git diff --check
rtk cargo fmt --check
rtk run "bash bin/check-source-layout"
```

Commit:

```text
git add -- docs/cursor-control.md docs/wayland/CORE_COMPLIANCE_MATRIX.md docs/wayland/PROTOCOL_SOURCE_MANIFEST.md REPORT-2026-08-21-gtk-flatpak-cursor-interoperability.md
git commit -m "docs(cursor): document GTK Flatpak interoperability"
```

### Task 8: Run final Windows/source-model verification and hand off Linux qualification

**Files:**
- No production files unless verification exposes a task-owned defect.
- Modify: `REPORT-2026-08-21-gtk-flatpak-cursor-interoperability.md` with final command results only.

- [ ] **Step 1: Inspect task-only diff and worktree state.**

Run:

```text
rtk git status --short
rtk git diff --stat main...HEAD
rtk git diff --check main...HEAD
```

Confirm no unrelated `.codex/config.toml` or `bin/` path is staged or committed.

- [ ] **Step 2: Run available platform-independent checks.**

Run:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run "bash bin/check-source-layout"
```

If Cargo or a check requires Unix/Wayland/DRM, record the exact result as `NOT RUN — Linux target/environment required`; do not convert it into a product change merely to make Windows compile.

- [ ] **Step 3: Record WSL and Linux qualification status.** The current `wsl --status` command is blocked by Windows Group Policy. Record that fact and mark DRM/KMS, libinput, native Wayland, GTK, Flatpak, D-Bus portal runtime, XWayland, and hardware cursor checks as pending unless a later suitable environment is actually used.

- [ ] **Step 4: Verify final history and status.**

Run:

```text
rtk git log --oneline --decorate --max-count=12
rtk git status --short
```

Update the report with the actual commit hashes and final status. Do not claim a clean worktree if the pre-existing user changes remain.

## Plan self-review

- Geometry requirements map to Task 1, including integer scale, transforms, fractional output scale, hotspot, viewport fallback, and hardware/software parity.
- Portal requirements map to Task 2, including correct DBus types, namespace filtering, canonical read semantics, and explicit notification qualification.
- Ownership and replacement requirements map to Task 3 and Task 4, including interaction restoration, stale/foreign serials, invalid shapes, inert devices, and mixed request replacement.
- Shape alias, lazy-cache, boundedness, and invalidation requirements map to Task 5.
- Diagnostics, launch routes, performance constraints, documentation, Linux qualification, commit discipline, and final verification map to Tasks 6–8.
- All task interfaces use the same names: `ClientCursorChoice`, `ProtocolCursorShape`, `CursorGeometry`, `settings_for_configuration`, and request-time `CursorConfigurationStore` reads.
- No implementation step relies on a placeholder or on unverified Linux behavior.
