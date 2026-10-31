# Screen capture and sharing

Typhon supports screen sharing, recording and remote play through the standard
portal stack. This page covers the operator setup and the current limits.

## Architecture

```text
app (OBS, browser, Discord, Steam)
  -> xdg-desktop-portal (shared frontend, systemd user service)
  -> xdg-desktop-portal-hyprland (XDPH)
  -> wlr-screencopy (zwlr_screencopy_manager_v1, v3, SHM buffers)
  -> PipeWire stream -> app
```

XDPH implements ScreenCast and Screenshot on top of wlr-screencopy. The
portal frontend selects XDPH through the desktop-specific routing file, and
XDPH picks the output through a custom picker.

## Files Typhon installs

Typhon writes these at compositor launch, at `oblivion-one portal --install`,
and before launching apps. Existing user files are never overwritten.

| Path | Purpose |
|---|---|
| `~/.config/xdg-desktop-portal/oblivionone-portals.conf` | Routes ScreenCast and Screenshot to `hyprland`, and Secret to `gnome-keyring`, for the `OblivionOne` desktop. |
| `~/.config/xdg-desktop-portal/astrea-portals.conf` | Same routing for apps launched by Typhon (`XDG_CURRENT_DESKTOP=Astrea`). |
| `~/.config/hypr/xdph.conf` | Enables restore tokens by default and sets `custom_picker_binary`. |
| `~/.local/state/oblivion-one/bin/typhon-share-picker` | Picker script. Prints Typhon's output for Typhon sessions and falls back to `hyprland-share-picker` otherwise. |

If a file already exists, Typhon leaves it alone and prints a one-line hint
when a required line is missing. Add `org.freedesktop.impl.portal.ScreenCast=hyprland`
under `[preferred]`, or `custom_picker_binary = <picker path>` in the
`screencopy` block.

After the routing file is first written, restart the portal frontend once so
it reads the new configuration:

```sh
systemctl --user restart xdg-desktop-portal.service
```

## Steam Remote Play

- Steam captures the screen through PipeWire and the portal, so it needs the
  same setup as above.
- Steam injects controller and keyboard input through `/dev/uinput`. Typhon
  picks up those devices through libinput udev hotplug. Your user needs
  write access to `/dev/uinput`, for example from the `steam-devices` udev
  rules package, followed by a udev reload or re-login.

## Current limits

- Frames are copied from the GPU to CPU memory on every capture (SHM buffers).
  This costs CPU and can cap the frame rate of high-resolution captures.
- Output capture only. Window capture is not supported.
- No DMA-BUF buffers yet, so zero-copy capture paths are unavailable.
- Typhon is single-output, so the picker always selects `Oblivion-1`.

See also [known issues](KNOWN_ISSUES.md).
