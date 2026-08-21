# Typhon GTK/Flatpak Cursor Interoperability Design

**Date:** 2026-08-21  
**Scope:** Source/model closure that can be implemented and verified from the Windows development environment. Linux GTK, Flatpak, DRM/KMS, native Wayland, and hardware-cursor qualification remain separate runtime work unless a usable WSL environment is already available.

## Goal

Make Typhon's compositor-owned cursor, Wayland client-owned cursor surfaces, GTK3, GTK4/Libadwaita, Flatpak GTK applications, XWayland, and `wp_cursor_shape_manager_v1` share one logical cursor configuration and one unambiguous cursor-geometry model.

The source/model work must prove that client buffer scale is applied exactly once, output scale is applied only at presentation, hardware and software paths use equivalent logical geometry, and sandboxed GTK clients can read Typhon's canonical cursor theme and size through the Settings portal.

## Baseline and constraints

The checkout is `C:\Users\vitor_crispim\GitHub\Typhon` at:

```text
HEAD 8b59805d85f23dabb636536aa975c3a2a005484f
branch cursor-gtk-flatpak-closure
```

The worktree already contains unrelated user changes in `.codex/config.toml` and the executable mode of files under `bin/`. Those paths are out of scope. No reset, clean, restore, overwrite, or broad staging is allowed.

The current source already contains meaningful cursor-surface scaling, viewport rejection, native image conversion, persistence, command-local `XCURSOR_THEME`/`XCURSOR_SIZE`, and bounded eager theme loading. The implementation must extend or test those paths rather than replace correct behavior.

The current `wayland-protocols` dependency is `0.32.12` with staging support and supplies `cursor-shape-v1` version 2. The generated dependency protocol is the source of truth; no XML will be vendored.

## Architecture

### 1. Canonical cursor configuration

`CursorConfiguration { theme, size_px }` remains the only desktop cursor configuration. `size_px` always means logical cursor pixels.

The same configuration feeds:

```text
CursorThemeManager
  -> compositor-owned XCursor image loading
  -> command-local XCURSOR_THEME/XCURSOR_SIZE
  -> portal org.gnome.desktop.interface cursor-theme/cursor-size
  -> compositor image selection for cursor-shape requests
```

No output scale is written into `XCURSOR_SIZE`, portal `cursor-size`, or the persisted configuration.

The existing launch boundary remains command-local. The compositor's process-global environment is not modified. The portal backend reads the persisted canonical configuration when serving a request, so it does not require the compositor process or a shared mutable memory object to be alive.

### 2. Client cursor ownership

The focused client's cursor choice becomes the authoritative typed state:

```rust
enum ClientCursorChoice {
    Hidden { pointer: wl_pointer::WlPointer },
    Surface {
        pointer: wl_pointer::WlPointer,
        surface_id: u32,
        hotspot_x: i32,
        hotspot_y: i32,
    },
    Shape {
        pointer: wl_pointer::WlPointer,
        shape: ProtocolCursorShape,
    },
}
```

The existing visibility fields may remain as derived backend-visibility state while the migration is in progress, but request handlers will update them through one synchronization helper. They are not independent sources of cursor ownership.

`InteractionCursorOverride` stays separate. Effective selection is:

```text
interaction override
  or focused client choice
  or canonical default pointer
```

`wl_pointer.set_cursor` and `wp_cursor_shape_device_v1.set_shape` both validate the same pointer focus and latest enter serial authority. A valid later request replaces the previous `Surface`, `Shape`, or `Hidden` choice. A null `set_cursor` always produces `Hidden`. Pointer or device destruction makes the associated shape device inert and cannot mutate another client's cursor.

### 3. Cursor geometry

The geometry boundary is explicit:

```text
buffer pixels
  -- buffer_transform --> transformed buffer pixels
  -- buffer_scale / viewport --> committed logical surface size
  -- output scale at presentation --> physical raster size
```

`PendingSurfaceBuffer::surface_size_for_state` remains authoritative for committed logical dimensions. A focused pure geometry helper/test layer will expose the same rules without requiring a live compositor.

Required invariants:

- `24x24 @ buffer_scale 1` is `24x24` logical.
- `48x48 @ buffer_scale 2` is `24x24` logical.
- `64x32 @ buffer_scale 2` is `32x16` logical.
- 90/270 degree transforms swap dimensions and transform hotspots correctly.
- Output scales `1.25`, `1.5`, and `1.75` convert logical size to physical raster once.
- `24 logical px @ 1.5` is approximately `36 physical px`, never `72`.
- Software composition and native image upload derive equivalent visual bounds and hotspot.
- Viewport source/destination remains software-only unless complete viewport-aware native conversion is implemented.

The hardware path will receive the output presentation scale/geometry needed to produce the physical cursor image and position. The client surface remains logical until that boundary. The source key and diagnostic record include all geometry inputs that can invalidate the native image.

### 4. Cursor-shape image selection

The six compositor-critical shapes remain eagerly loaded. Protocol-only shapes use a separate lazy cache keyed by:

```text
theme generation + canonical size + ProtocolCursorShape
```

The cache has an independent bounded capacity. Each protocol enum value in dependency version 2 has an explicit CSS/XCursor alias list. Alias lookup falls back to `pointer`, and missing optional files never invalidate the required pointer image. A new generated enum value cannot silently be accepted without an explicit mapping/test update.

Theme, size, and reload publication replace the generation-owned lazy cache. Cursor motion only consumes the already-selected image and updates position; it does not read the filesystem, call the portal, parse a theme, allocate shape maps, or take a new global lock.

### 5. Settings portal

The existing `org.freedesktop.appearance` values remain unchanged. Typhon additionally exposes only the intentionally owned compatibility keys:

```text
org.gnome.desktop.interface.cursor-theme : string
org.gnome.desktop.interface.cursor-size  : int32
```

The GNOME namespace is a compatibility extension, not a claim that the XDG Settings portal standardizes cursor settings. Namespace filtering continues to support the portal's empty, exact, and `.*` forms without fabricating unknown namespaces or keys.

`Read` and `ReadAll` obtain the latest valid persisted `CursorConfiguration` for each request. Missing or invalid persistence uses the validated Typhon default configuration only. The backend does not mirror the entire GNOME schema. Live `SettingChanged` is implemented only if an existing secure control/subscription mechanism can be reused and verified; otherwise the source/model closure documents notification as pending Linux qualification while guaranteeing correct reads for newly started applications.

### 6. Diagnostics

Cursor diagnostics are gated by the existing pointer-debug facility or a dedicated explicit switch. They run on cursor choice changes and commits, not cursor motion. A diagnostic record includes:

```text
client, surface_id, buffer pixel width/height, buffer_scale,
buffer_transform, viewport source/destination, committed logical size,
hotspot, output scale, selected path, uploaded physical size
```

No pixel data and no high-frequency motion spam are emitted.

## Workstream boundaries

The implementation is split into independently reviewable invariants:

1. Cursor geometry model/tests and native/software parity.
2. Canonical portal reads/types/filtering and persistence observation.
3. Typed client cursor choice and replacement/restoration behavior.
4. `wp_cursor_shape_manager_v1` capability, dispatch, validation, and protocol tests.
5. Lazy bounded protocol-shape theme cache and exhaustive alias tests.
6. Gated diagnostics, launch-boundary audit, docs, report, and Windows verification.

Each workstream must pass focused tests and `git diff --check` before its commit. Only task-owned paths may be staged.

## Error and fallback behavior

- Invalid cursor-shape enum produces the generated protocol's `invalid_shape` error.
- Stale, foreign, or unfocused serials are ignored using the same semantics as `wl_pointer.set_cursor`.
- An inert/destroyed pointer shape device has no effect.
- Invalid cursor surface geometry is rejected or software-composited safely; it is never uploaded with guessed dimensions.
- Missing optional theme aliases fall back to the pointer image.
- Missing/invalid persisted cursor configuration falls back to the validated canonical default for portal reads only; it does not overwrite user state.
- Native cursor conversion failures select software composition and remain observable through bounded diagnostics.

## Verification strategy

Windows/source-model verification will attempt:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo test --locked <focused tests>
rtk cargo clippy --locked --all-targets -- -D warnings
rtk git diff --check
bash bin/check-source-layout
```

Commands requiring Linux DRM/KMS, libinput, Wayland session state, D-Bus portal activation, GTK, Flatpak, or hardware planes will be marked `NOT RUN — Linux target/environment required` unless actually executed in a suitable environment.

The later Linux qualification checklist will query Typhon, the process environment, GNOME settings, the portal, native GTK3, GTK4/Libadwaita, Flatpak GTK, Qt Wayland, and XWayland. It will explicitly forbid using `flatpak override --env=XCURSOR_SIZE=...` as a product fix.

## Root-cause classification policy

The report will classify evidence as:

```text
source-proven defect
model-proven defect
Linux runtime hypothesis
Linux runtime verified
```

The current source proves the portal compatibility gap and the absence of cursor-shape advertisement. It does not, on Windows, prove that the observed Linux oversized cursor was caused only by portal settings rather than a toolkit request or a presentation-scale divergence. The final report will therefore avoid claiming runtime closure until the Linux matrix is executed.
