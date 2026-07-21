# X11 popup remap and client move/resize implementation plan

1. Add failing XWM lifecycle tests proving a reused popup XID cannot publish its
   destroyed surface and can become ready with a new association.
2. Clear per-window association, readiness, and snapshot state on X11 unmap and
   Wayland association removal.
3. Add failing client-message tests for `_NET_WM_MOVERESIZE` and
   `_NET_MOVERESIZE_WINDOW`.
4. Add typed EWMH move/resize events and one-shot configure decoding.
5. Add failing compositor interaction tests for held-button authorization,
   direction mapping, motion, release, and cancel.
6. Connect accepted X11 requests to the existing move/resize transaction engine.
7. Run focused tests, formatting, Clippy, the complete gate, and build release.
8. Restart and verify Steam hover menus, client titlebar move/resize, dialogs,
   stacking, and Xwayland stability.
