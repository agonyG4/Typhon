# AstreaFramework

AstreaFramework is the low-level UI/runtime foundation for Oblivion One and the
future Astrea desktop environment.

It owns primitives that should stay below component code:

- material and blur contracts
- theme tokens
- spacing, typography, and component interaction state
- layout primitives such as insets and box constraints
- animation speed and timing helpers
- frame-budget/performance policy
- render command planning
- geometry and color primitives

It does not own finished components. Exodus will build components such as
buttons, windows, popups, menus, and sliders by calling AstreaFramework and
customizing the resulting materials, layout, and render plans.

Current module split:

- `color`, `geometry`: copyable primitives for renderer-independent layout.
- `layout`: insets, spacing tokens, and size constraints.
- `typography`: Astrea font families, weights, and text styles.
- `component`: reusable interaction state and alpha policy.
- `material`, `theme`: glass, blur, radius, spacing, and typography tokens.
- `motion`, `performance`, `render`: animation timing, frame budget, and draw
  command planning.

Project-AstreaUI will be the shell layer that uses Exodus to build the real
desktop experience: dock, topbar, Spotlight, notifications, launcher, and
settings surfaces.
