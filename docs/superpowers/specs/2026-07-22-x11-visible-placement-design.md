# X11 Visible Placement Design

## Problem

Typhon's collision-avoiding managed X11 allocator can exhaust all cascade candidates for a window that nearly fills the output. Its current fallback returns the final unchecked cascade coordinate, which can place the complete window outside the output. Mode transitions and interactive resize then capture or restore that invalid floating geometry, making the window disappear again when leaving fullscreen or maximized mode.

## Design

Managed X11 placement keeps searching for a non-overlapping cascade slot, but every candidate must first be clamped to the usable output. If no collision-free position exists, the allocator returns the first clamped cascade position. Visibility takes precedence over avoiding overlap because overlapping windows remain interactive while off-output windows do not.

Windows larger than the usable output are anchored at its top-left edge. Client-positioned dialogs, popups, notifications, and override-redirect windows retain their existing policies.

## Verification

An automated state test fills the usable output with an existing desktop window, admits a near-full-screen managed X11 window, and asserts that its origin and visible frame remain within output bounds. It also enters fullscreen and restores floating mode, proving the visible placement survives the transition used by resize. Existing placement and Xwayland suites must remain green.
