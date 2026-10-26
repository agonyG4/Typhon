# Window icons

Applications using xdg-toplevel-icon-v1 can provide theme names and square
wl_shm pixel buffers. Typhon snapshots client memory, freezes the icon at
set_icon, applies the selected icon on the next wl_surface.commit, and keeps
the assigned snapshot alive after the icon protocol object is destroyed.

The Astrea toplevel protocol version 4 publishes optional icon_name and
icon_pixels events inside the existing atomic handle/manager transaction.
An icon_name event clears old pixel data; icon_pixels supplies a fallback
image via a sealed memfd. The image uses native-endian premultiplied ARGB8888,
is tightly packed and square, and has a maximum edge of 256 pixels. Clients
bound to versions 1–3 receive no new events. Names are limited to 256 bytes;
source buffers to 1024 square pixels; each icon to eight size/scale variants;
live icon objects to 16 per client and 128 total; icon-bearing surface state
to 256 entries. Surface/role teardown discards both pending and current state.

Eclipse resolves available theme names and otherwise reads the bounded pixel
fd, closes it on every path, and converts it to a PNG data URL. Icons flow
through the shared transactional model to Dock and AltTab, with icon changes
included in identity cache and stale-result fingerprints.

Older clients still use desktop entries. For example, LoSeSb's bundled SDL
2.0.20 does not send Wayland window icons. Its existing gui/window_icon.png
asset can be registered locally with LoSeSb.desktop and StartupWMClass=LoSeSb.
No game asset is distributed in either repository.

Implementation steps: add failing protocol discovery and resolver regressions;
implement bounded immutable icon storage and version-4 publication; consume
icons in Eclipse with decoding, transaction, identity, and QML coverage; build
and test using only the stable Aether output directories; commit changes on
main without including other work in progress. A new compositor session is
required to advertise the new global; registering a desktop entry does not
require restarting the compositor.
