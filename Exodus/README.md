# Exodus

Exodus is the Astrea component layer. It builds reusable UI components from
AstreaFramework primitives, but stays renderer-independent.

Current components:

- `Panel`: glass surface with padding and material.
- `Button`: primary, secondary, and ghost actions with Astrea hit targets.
- `TextField`: padded control layout with focused cursor geometry.
- `ToggleSwitch`: compact Astrea-style binary control.
- `Slider`: clamped value, track, fill, and thumb geometry.
- `ListRow`: shared row metrics used by Spotlight and future menus/lists.
- `Topbar` and `Spotlight`: shell-level components built on the same contracts.

The output of each component is a layout contract: rectangles, text, material,
colors, typography, and state. A compositor, GPU renderer, or future
Project-AstreaUI shell can paint those contracts without copying layout logic.
