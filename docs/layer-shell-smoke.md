# Layer-shell smoke checks

Typhon advertises `zwlr_layer_shell_v1` v4. Enable transition logging with:

```bash
OBLIVION_ONE_LAYER_SHELL_DEBUG=1
```

The expected log transitions are `create`, `configure`, `ack`, `map`, `arrange`,
`focus_take`, `focus_restore`, `layer_change`, `unmap`, and `destroy`.

## Eclipse-style overlay

Use an Eclipse build that links LayerShellQt and run the Spotlight or AltTab
entry point under Typhon. The client request must be:

```text
layer = Overlay
anchors = top | bottom | left | right
exclusive_zone = -1
keyboard_interactivity = Exclusive
namespace = astrea-spotlight or astrea-alt-tab
```

Expected Typhon debug output:

```text
namespace=astrea-spotlight layer=Overlay size=1280x800
namespace=astrea-alt-tab layer=Overlay size=1280x800
focus_take
```

The overlay must render above normal and fullscreen XDG windows and restore the
previously focused application after unmap.

## Quickshell top panel

Create a Quickshell panel with:

```text
layer = Top
anchors = top | left | right
width = 0
height = 32
exclusive_zone = 32
keyboard_interactivity = None
```

Expected result:

```text
configure width=<output logical width> height=32
usable geometry y=32 height=<output logical height - 32>
```

Additional zone-zero surfaces in `Top` should arrange against the reduced
usable geometry but must not reserve more space.

## Quickshell overlay

Create a Quickshell overlay with:

```text
layer = Overlay
anchors = top | bottom | left | right
width = 0
height = 0
exclusive_zone = -1
keyboard_interactivity = Exclusive
```

Expected result:

```text
configure width=<output logical width> height=<output logical height>
map layer=Overlay keyboard=Exclusive
focus_take
focus_restore after unmap
```

Report any `zwlr_layer_surface_v1` protocol error instead of retrying with an
XDG fallback; that means the smoke client submitted an invalid layer-shell
state.
