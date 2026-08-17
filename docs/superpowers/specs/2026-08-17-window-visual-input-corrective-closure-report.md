# Typhon WindowVisual, SSD, Input, Fullscreen, Damage, and Scroll Corrective Closure

Date: 2026-08-17

## Scope and baseline

This closure was implemented against the existing working tree at commit
`3d835df4d4622580aafad67291f7d0d8ce440973`. The tree already contained
substantial unrelated and earlier dirty work; it was preserved. The new work
is limited to the corrective paths described by the closure plan and is not a
claim that every dirty-tree change belongs to this closure.

The source audit confirmed the high-risk boundaries: CPU and GLES appended SSD
after unrelated client surfaces, mapped XDG toplevels used an unconditional
low-level focus call, Alt/Super shortcut consumption could strand a forwarded
modifier release, and the native wheel boundary retained legacy detents but
dropped raw v120 values. Native damage already had the right old/current
decoration-bound model; the closure verifies and preserves that complete
visual-diff path.

## Implemented closure

- Added a shared `WindowVisualGroup` stack order. Each normal window owns its
  root/descendant client surfaces and its SSD instance. CPU and GLES scene
  command generation consume that same grouping, preventing a lower window's
  decoration from covering a higher window's client content.
- Kept decoration movement in the complete scene damage path, including old
  and current decoration bounds and partial scene rebuilds. Existing bounded
  `DamageDebugStats` and native `NativeDamageSummary` fields remain the
  diagnostics surface; no global SSD post-pass or full-repaint fallback was
  added.
- Rejected fullscreen move and resize interaction creation before allocation,
  and retained existing mode-transition cleanup for active resize state,
  previews, pending updates, and interaction ownership.
- Reconciled forwarded deferred Alt/Super modifier releases before shortcut
  consumption can return. The ledger now has deterministic empty-state
  coverage after Alt-Tab, Alt-Shift-Tab, and unbound-key sequences.
- Routed XDG first-map focus through an explicit `InitialMap` policy reason.
  Map focus is blocked during active window interaction, held-button/implicit
  grab, popup/lock/confine, drag, or exclusive-layer ownership; ordinary
  authorized activation and pointer activation retain their existing paths.
- Made built-in MacTahoe visible geometry borderless (`left/right/bottom = 0`)
  while retaining the independent six-pixel resize hit region. X11 frame
  extents continue to derive from the same decoration layout.
- Raised the advertised `wl_seat` version to 8 and preserved raw libinput
  wheel v120 values. Pointer v8 clients receive `axis_value120`; older clients
  receive accumulated `axis_discrete` detents without duplicate v8 events.

## Deterministic verification

All commands were run through RTK.

- `TMPDIR=/tmp/t rtk cargo test --locked --lib -- --test-threads=1` — **1678 passed, 2 ignored**.
- `rtk cargo test --locked --bin oblivion-one` — **892 passed**.
- `rtk cargo check --locked --all-targets` — passed.
- `rtk cargo fmt --check` — passed after final formatting.
- Focused tests cover WindowVisual grouping, lower-decoration occlusion,
  fullscreen interaction eligibility, map-focus ownership, MacTahoe geometry,
  v120/legacy wheel conversion, and Wayland v8 axis dispatch.

The first full library run used the repository's default temporary root and
also exposed 42 pre-existing test-harness failures caused by Unix socket path
length (`SUN_LEN`) overflow, plus the dependent poisoned-lock failures. A
second full run with the short `/tmp/t` root passed completely; no production
code workaround was added for that harness condition.

## Native qualification

Native qualification was not possible in this environment. `/dev/dri` exists,
but `astreactl`, `astrea-launcher`, `weston`, and `sway` were not available on
`PATH`, so no real Astrea/Wayland/DRM session or Firefox/Kitty reproduction
was claimed. The remaining intermittent rollback, Firefox tear-off, and Kitty
drag-selection items therefore have deterministic coverage and bounded
diagnostic support, but remain not natively reproduced here.

## Remaining evidence boundary

The deterministic suites establish ownership, ordering, protocol delivery,
damage repair, and policy behavior. They do not substitute for a real native
session with page-flip timing, Direct Scanout hardware, XWayland, Firefox tab
tear-off, or Kitty drag-selection. Those should be qualified when the native
launcher/session tools are available.

## Commit scope

Focused closure commits:

- `c05c406` — `docs: plan Typhon corrective closure`
- `84a4e62` — `fix: close WindowVisual input and scroll ownership gaps`
- `d9326a7` — `test: align borderless MacTahoe extents`
- `13baca1` — `test: align moved MacTahoe decoration coordinates`

The pre-existing dirty worktree remains intentionally unmerged into those
commits.
