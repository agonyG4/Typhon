# Oblivion Portal Design

## Goal

Provide a small, fast xdg-desktop-portal backend for apps launched inside
Oblivion One. The first version exists to remove portal startup noise and give
toolkits reliable answers for lightweight desktop integration queries.

## Scope

Implement an Oblivion backend for `xdg-desktop-portal`, not a replacement for
the frontend service. The backend owns:

- `org.freedesktop.impl.portal.Settings`
- `org.freedesktop.impl.portal.Notification`
- `org.freedesktop.impl.portal.Access`

The backend intentionally does not implement Camera, Location, ScreenCast,
RemoteDesktop, FileChooser, or OpenURI in this slice.

## Architecture

`oblivion-one portal` runs as a DBus-activatable backend named
`org.freedesktop.impl.portal.desktop.oblivion`. Runtime installation writes
activation metadata under the Oblivion state directory:

- `portal-share/dbus-1/services/org.freedesktop.impl.portal.desktop.oblivion.service`
- `portal-share/xdg-desktop-portal/portals/oblivion.portal`
- `portal-share/xdg-desktop-portal/portals/oblivionone-portals.conf`
- `portal-share/xdg-desktop-portal/oblivionone-portals.conf`

Apps launched through the compositor get `XDG_DATA_DIRS` prepended with
`portal-share` and `XDG_DESKTOP_PORTAL_DIR` pointed at the generated portal
directory.

## Behavior

Settings replies are in-memory constants:

- `org.freedesktop.appearance color-scheme = 1`
- `org.freedesktop.appearance contrast = 0`
- `org.freedesktop.appearance reduced-motion = 0`
- `org.freedesktop.appearance accent-color = (0.42, 0.64, 1.0)`

Notifications accept add/remove calls and keep a tiny in-memory record so calls
are fast and non-blocking. UI notification rendering is out of scope.

Access requests are denied immediately with `response=1`. This gives Camera and
Location a clean backend dependency without granting sensitive permissions or
blocking on UI that does not exist yet.

## Performance

The backend avoids shelling out, disk reads, and compositor calls on the DBus
method path. Runtime file generation is idempotent and only rewrites changed
files. Settings values are static, notification mutation is a single mutex
update, and Access returns immediately.

## Verification

- Unit tests cover activation file generation and launch environment wiring.
- Unit tests cover settings namespace filtering.
- `cargo test` and `cargo clippy --all-targets -- -D warnings` must pass.
- A `dbus-run-session` smoke test must prove `oblivion-one portal --check`
  can own the backend name and export the object.
