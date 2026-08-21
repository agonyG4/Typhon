# Typhon GTK/Flatpak Cursor Interoperability Closure

Date: 2026-08-21

Status: source/model closure implemented; Linux runtime qualification pending.

## Scope and baseline

The task investigated the oversized cursor reported when the pointer enters
GTK/GNOME applications, especially sandboxed Flatpak applications, while
Typhon-owned cursors remain at the expected size. The work was performed from
the Windows/source checkout under the user-authorized constraint that existing
changes must be preserved.

Reference baseline: `8b59805d85f23dabb636536aa975c3a2a005484f`.

Implementation branch: `cursor-gtk-flatpak-closure`.

The pre-existing worktree changes were not staged or modified: the deleted
`.codex/config.toml` and the mode-only changes under `bin/` remain in the
worktree and are not part of this task's commits.

## Source investigation and root-cause categories

The source already had meaningful client-cursor scaling. The closure therefore
keeps the existing logical-size model and makes its invariants explicit rather
than forcing scale 1, hardcoding size 24, disabling client cursors, or changing
Flatpak environment overrides.

The interoperability gaps were separated into four categories:

1. GTK-compatible settings were not exposed by the local Settings portal. A
   sandboxed client could therefore observe a different cursor configuration
   through `org.gnome.desktop.interface` than Typhon's compositor.
2. Cursor surface geometry had no single source/model covering buffer scale,
   transform, hotspot, logical extent, and output scale across software and
   native conversion.
3. Client surface ownership and future cursor-shape ownership were represented
   through separate state paths, making mixed replacement and cleanup harder to
   reason about.
4. Shape lookup and cursor-path diagnostics needed bounded, lazy behavior so
   cursor motion does not trigger theme reads, portal calls, or diagnostic
   formatting.

## Implemented closure

### Geometry and presentation

- Added `CursorGeometry` as the shared source/model for transformed buffer
  dimensions, logical dimensions, logical and physical hotspots, and physical
  output bounds.
- Covered scale 1, integer buffer scale, non-square buffers, rotations and
  hotspot transforms, fractional output scale, invalid dimensions, and
  hardware/software visual-bound parity with deterministic tests.
- Applied output scale once at presentation. Native client cursor conversion
  uses the same model as the software path and keeps viewport-transformed
  client cursors on software composition.

### Canonical settings and portal compatibility

- `CursorConfiguration { theme, size_px }` remains the canonical logical
  configuration for the compositor, child launch environment, portal, and
  cursor-shape rendering.
- The Settings backend now exposes only the intentional compatibility keys:
  `org.gnome.desktop.interface.cursor-theme` as a string and
  `cursor-size` as a signed 32-bit integer.
- `Read` and `ReadAll` read the persisted configuration boundary per request;
  failures use the validated default without writing it back.
- Exact, empty, wildcard, GNOME, and unknown namespace filtering is covered by
  source/model tests. Live `SettingChanged` propagation remains pending Linux
  qualification rather than being fabricated.

### Ownership, protocol, and cache

- Added typed `ClientCursorChoice::Hidden`, `Surface`, and `Shape` state.
- Surface and shape requests share focus/serial validation and replacement
  semantics; pointer cleanup, surface removal, focus loss, lock/unlock, and
  visibility state use the same ownership record.
- Advertised `wp_cursor_shape_manager_v1` version 2 only under the cursor-shape
  input capability. The pointer-device path validates ownership, focus, enter
  serials, invalid values, and version-2-only values.
- Added lazy, alias-bounded protocol shape lookup with a 16-image loaded cache
  limit per theme generation and pointer fallback for missing/malformed shapes.
  Theme publication clears the cache.

### Diagnostics and launch boundaries

- Cursor activation and commit diagnostics are gated by
  `TYPHON_POINTER_DEBUG`, format lazily, report client/surface/geometry/
  transform/hotspot/output-scale inputs, and never include pixels.
- Native cursor-path diagnostics remain change-time only; existing fallback
  events report hardware degradation without adding motion spam.
- `XCURSOR_THEME` and `XCURSOR_SIZE` remain command-local to the supervised
  application launcher. The process-global environment is unchanged, and the
  no-override path is tested.

## Deterministic source/model evidence

The following tests were added or extended; they were not executed to
completion in this Windows environment because the Rust build is blocked before
Typhon compilation by the native Wayland dependency described below.

- `cursor_geometry` scaling, transform, hotspot, fractional output, and parity
  tests.
- Portal namespace/type and request-time persisted-configuration tests.
- Cursor ownership, replacement, cleanup, and cursor-shape protocol tests.
- Protocol plan/global/version contract tests for
  `wp_cursor_shape_manager_v1`.
- Bounded protocol-shape cache, theme-generation invalidation, and pointer
  fallback tests.
- Gated diagnostic formatting and command-local launch-environment tests.

## Windows verification

Passed:

```text
rtk cargo fmt --check
rtk git diff --check
rtk git diff --check main...HEAD
```

The following build/test commands were attempted and all stopped at the same
native dependency boundary before Typhon compilation:

```text
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets --all-features
rtk cargo test --locked --all-targets
rtk cargo test --locked cursor_manager --lib
```

It stopped before compiling Typhon because `wayland-sys v0.31.11` could not
find the required `wayland-client`/`wayland-server` native libraries through
pkg-config on Windows. This is an environment limitation, not evidence that
the Rust tests passed or failed functionally.

WSL was checked but is unavailable in this environment:

```text
wsl --status
Este programa está bloqueado por uma política de grupo... (os error 1260)
```

No Linux distribution or machine-wide configuration was installed or changed.

The requested source-layout script was also attempted. Windows policy blocked
the `bash` executable before the script could run:

```text
rtk run "bash bin/check-source-layout"
Este programa está bloqueado por uma política de grupo... (os error 1260)
```

## Linux qualification checklist

`NOT RUN — Linux target/environment required`

On a suitable Linux session, run at minimum:

```bash
astreactl cursor get
echo "$XCURSOR_THEME"
echo "$XCURSOR_SIZE"
gsettings get org.gnome.desktop.interface cursor-theme
gsettings get org.gnome.desktop.interface cursor-size
```

Then qualify D-Bus Settings reads for both keys, GTK3, GTK4/Libadwaita,
Flatpak GTK, Sober when available, Qt Wayland, XWayland, native Wayland
cursor-shape clients, gated cursor diagnostics, DRM/KMS hardware cursor,
software fallback, fractional scaling, transforms, suspend/resume, and
restart persistence. The permitted `flatpak override --env=XCURSOR_SIZE=...`
A/B experiment may be recorded as diagnostic evidence only; it is not a
product fix.

The following remain unclaimed until those checks execute on Linux: GTK,
Flatpak, D-Bus portal runtime behavior, `SettingChanged`, libinput, native
Wayland, DRM/KMS, hardware cursor, XWayland, and compositor-session
qualification.

## Reviewable commits

- `a97382a6` — design GTK/Flatpak cursor interoperability.
- `96ba419a` — deterministic client cursor logical-scaling tests.
- `e4994416` — canonical cursor settings through the portal.
- `65cfc2cb` — typed focused client cursor ownership.
- `cbe46fe4` — implementation closure plan.
- `f0d79d2d` — cursor-shape protocol support.
- `e3d09058` — shape cache and hardware/software scaling closure.
- `5834f519` — gated interoperability diagnostics and launch-boundary tests.

The documentation commit for this report is intentionally separate from the
source/model commits.
