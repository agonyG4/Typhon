# Agent Kernel: Minimize and Window State Research

Date: 2026-06-11

Scope: read-only investigation of minimize/restore/window-state behavior around
the compositor core, native SDDM output, shell dock expectations, and current
tests.

Non-goals:

- Do not edit production source code in this task.
- Do not redesign WM policy or renderer ownership.
- Do not change protocol, CLI, JSON, or service contracts.

## Summary

The current minimize model is not just a visibility flag. It removes a toplevel
tree from `renderable_surfaces` and stores cloned `RenderableSurface` snapshots
inside `WindowState`. Restore extends those snapshots back into
`renderable_surfaces`.

The highest-risk bug is that normal client commits do not check whether the root
toplevel is minimized. After a focused window is minimized, an active client can
commit another buffer and `commit_surface_buffer()` can recreate a visible
`RenderableSurface` for that same minimized toplevel. This explains a native
SDDM symptom where minimize appears to freeze, bounce back, or fail for browsers
and other clients that keep rendering.

The second risk is the opposite edge of the same model: if commits are later
blocked while minimized, restore must not resurrect stale snapshots. The
minimized state needs either to absorb latest commits or to track "hidden but
mapped" state separately from renderable output state.

## Current Architecture

The architecture doc says `window_state.rs` owns minimize/maximize/fullscreen and
restore state, while WM policy owns focus, placement, move, resize, maximize,
minimize, close, and future workspace rules. Renderer/output should not absorb
WM policy.

Evidence:

- `docs/ARCHITECTURE.md:20-30`
- `docs/ARCHITECTURE.md:32-40`
- `docs/research/reference-compositor-performance-architecture.md:337-348`

That boundary is the right shape. The bug is not "native output should know
about minimized windows." The bug is that compositor state treats "renderable"
as both presentation visibility and latest committed surface storage.

## Current Minimize/Restore Flow

### State container

`WindowState` stores:

- current `ToplevelMode`;
- optional restore geometry;
- `minimized_surfaces: Vec<RenderableSurface>`.

Evidence:

- `src/compositor/window_state.rs:5-10`
- `src/compositor/window_state.rs:21-32`

This means minimize preserves cloned renderable snapshots, not live committed
surface state.

### Minimize command path

The public server method calls into compositor state and flushes clients:

- `src/compositor/server.rs:196-205`

The state path:

1. `minimize_focused_window()` resolves the focused root surface.
2. `minimize_root_window()` drains `renderable_surfaces`.
3. Surfaces whose root matches the target are moved into `minimized_surfaces`.
4. The remaining surfaces become the visible set.
5. Focus and keyboard focus are cleared if the minimized root was focused.
6. Pointer focus is cleared if the pointer target belonged to that root.
7. The topmost remaining renderable toplevel is focused.
8. Render generation advances with `WindowMinimize`.

Evidence:

- `src/compositor/mod.rs:1907-1912`
- `src/compositor/mod.rs:1964-2006`
- `src/compositor/mod.rs:2146-2159`

### Restore command path

Restore selects the first minimized toplevel by hash-map iteration order, takes
its stored surfaces, extends `renderable_surfaces`, focuses the root surface, and
advances render generation with `WindowRestore`.

Evidence:

- `src/compositor/mod.rs:1914-1926`
- `src/compositor/mod.rs:2009-2024`

This is simple and usually works for one app, but selection order is not a
user-visible MRU order, and restored surfaces are appended without explicitly
re-raising/re-stacking the surface tree.

### Dock expectations

The shell dock item has a `minimized` flag and is drawn differently when
minimized.

Evidence:

- `src/compositor/shell/dock.rs:3-19`
- `src/compositor/shell/dock.rs:78-90`

`shell_dock_items()` includes renderable toplevel roots first, then any
toplevels not already known. This keeps minimized-only toplevels in the dock.

Evidence:

- `src/compositor/mod.rs:2185-2217`

Native and nested output both use dock hit-testing to activate/restore a window:

- `src/native_output.rs:2987-2998`
- `src/nested_output.rs:620-631`
- `src/compositor/mod.rs:1928-1948`

## Current Native SDDM Flow

Native keyboard shortcuts are compositor-owned, so they still work after focus
is cleared:

- Alt+M -> minimize.
- Alt+R -> restore next minimized.
- Alt+F -> maximize.
- Alt+Enter/F11 -> fullscreen.

Evidence:

- `src/native_output.rs:1756-1789`
- `src/native_output.rs:3023-3044`

The native loop repaints when render generation changes, including minimize and
restore. Because `WindowMinimize`/`WindowRestore` are not surface-damage causes,
native output uses full-output damage for those transitions.

Evidence:

- `src/native_output.rs:839-845`
- `src/native_output.rs:853-880`
- `src/native_output.rs:3580-3591`

Native perf logs already carry `render_cause`, `surfaces`, and
`render_generation`, which are useful for validating minimize behavior.

Evidence:

- `src/native_output.rs:883-909`

## Confirmed Bugs and Gaps

### 1. Commits can resurrect minimized windows

Confirmed by code.

`wl_surface.commit` with a new buffer always flows into
`state.commit_surface_request()`.

Evidence:

- `src/compositor/protocols/core.rs:46-72`

`commit_surface_request()` then calls `commit_surface_buffer()` for normal
buffers. `commit_surface_buffer()` updates an existing renderable if one exists;
otherwise it creates a new `RenderableSurface` and pushes it into
`renderable_surfaces`. It does not check whether the root toplevel is minimized.

Evidence:

- `src/compositor/mod.rs:1025-1042`
- `src/compositor/mod.rs:872-945`

Failure scenario:

1. Browser/terminal is focused and visible.
2. User presses Alt+M in native SDDM.
3. `minimize_root_window()` removes that root from `renderable_surfaces` and
   marks it minimized.
4. The client commits another buffer after animation, frame callback, cursor, or
   app-side repaint.
5. `commit_surface_buffer()` sees no existing renderable and pushes a new one.
6. The dock still marks the toplevel minimized, but the surface can be visible
   again.

This is likely the main "minimize failed" behavior for clients that continue
rendering while hidden.

### 2. Fixing resurrection naively can restore stale content

Confirmed design risk.

The minimized state currently stores cloned `RenderableSurface` snapshots.
If future code simply ignores commits for minimized roots, restore will reuse the
old snapshots from minimize time.

Evidence:

- `src/compositor/window_state.rs:25-32`
- `src/compositor/mod.rs:2009-2024`

Safe correction needs a place for latest hidden commits:

- update the minimized snapshot on root/subsurface commits; or
- split "mapped surface content" from "visible render tree" so minimized windows
  keep current buffers without participating in output composition.

### 3. Restore order is not deterministic MRU

Likely bug for UX, confirmed by code shape.

`restore_next_minimized_window()` picks the first minimized entry from
`HashMap::iter()`. Hash-map iteration is not a stable user-facing order.

Evidence:

- `src/compositor/mod.rs:1914-1926`

In native SDDM with multiple minimized apps, Alt+R may restore an arbitrary
window. This does not explain a single-window freeze, but it can make restore
look unreliable.

### 4. Restore does not explicitly re-stack the restored root tree

Likely bug.

`restore_minimized_root_window()` appends stored surfaces and focuses the root,
but it does not call `raise_root_window()` or the tree-stacking helper.

Evidence:

- `src/compositor/mod.rs:2009-2024`
- `src/compositor/mod.rs:2162-2183`

Appending usually puts the restored tree above existing surfaces, but relying on
snapshot order is fragile, especially for subsurfaces/popups. A restore path
should explicitly make the restored root active and topmost.

### 5. Minimize does not reuse the full unmap cleanup path

Likely bug.

`unmap_surface_content()` removes descendant renderables, releases active dmabuf
buffers, clears popup grabs, recent input serials, pointer focus, focused
surface, and keyboard focus, then advances generation.

Evidence:

- `src/compositor/mod.rs:1127-1178`

`minimize_root_window()` does its own lighter removal. It clears focus for the
focused root and clears pointer focus if the current pointer target belongs to
that root, but it does not clear popup grabs or recent input serials for the
minimized tree.

Evidence:

- `src/compositor/mod.rs:1964-2006`

This can leave stale serial/input state for a hidden toplevel. It is less likely
to be the main visible freeze than commit resurrection, but it is unsafe state.

### 6. Tests cover only a narrow minimize/restore happy path

Confirmed.

The only direct minimize test minimizes one focused SHM toplevel, restores it,
and checks the final renderable surface size. It does not assert the hidden
state immediately after minimize, dock minimized state, focus/key behavior,
commit while minimized, subsurfaces, multiple minimized windows, or native dock
click restore.

Evidence:

- `src/compositor/tests/windows.rs:68-86`

Dock tests only cover label and active state for a visible toplevel.

Evidence:

- `src/compositor/tests/xdg.rs:18-33`

Test server commands expose enough controls to add focused minimize/restore and
surface-count assertions.

Evidence:

- `src/compositor/tests/support.rs:3758-3768`
- `src/compositor/tests/support.rs:3823-3828`
- `src/compositor/tests/support.rs:3904-3911`

## Why Native SDDM Can Look Frozen

Native SDDM has no host WM to paper over compositor state. Once a window is
minimized:

- keyboard focus is intentionally cleared;
- the only restore affordances are the built-in dock and Alt+R;
- the native loop repaints from compositor render generation and shell overlay;
- active browser/client commits can keep arriving.

Likely visible outcomes:

- If the client commits after minimize, the hidden window can become renderable
  again while the dock still treats it as minimized.
- If focus is cleared and the dock does not visibly repaint or a user expects
  app-level focus to remain, the session can feel frozen even though compositor
  shortcuts still work.
- With the pageflip lifecycle issue documented separately, callbacks can be
  completed early, causing active clients to commit again quickly after minimize.
  That amplifies the resurrection path.

Related lifecycle note:

- `docs/research/native-frame-lifecycle-pageflip.md`

## Safe Fix Plan

### Step 1: Add failing tests before behavior changes

Add focused tests for confirmed gaps:

- minimize hides immediately: after `MinimizeFocused`, surface count is `0` for
  a single toplevel before restore;
- commit while minimized does not recreate a visible renderable;
- restore after a commit while minimized shows the newest committed size/content;
- dock item for minimized toplevel remains present and has `minimized=true`;
- dock activation of a minimized item restores and focuses it;
- minimizing a tree with a subsurface hides/restores the whole tree;
- multiple minimized windows restore in explicit MRU/order policy once that
  policy exists.

Keep these in compositor tests first; add native smoke logging after core state
is correct.

### Step 2: Split hidden state from renderable state

Do not let renderer/output own minimize. Keep WM policy in compositor state, but
introduce a clear hidden-surface path.

Conservative options:

1. Add `CompositorState::root_is_minimized(surface_id)` and have
   `commit_surface_buffer()` update minimized storage instead of
   `renderable_surfaces` when the root is minimized.
2. Better: store latest committed surface content separately from the visible
   render list, then derive `renderable_surfaces` only for visible roots.

The second option is cleaner long-term, but the first option is a smaller patch.

### Step 3: Reuse or mirror unmap cleanup safely

On minimize, clear state for the entire root tree:

- pointer focus and entered surfaces;
- keyboard/focused surface when inside the minimized tree;
- popup grab stack entries;
- recent input serials;
- active interactions for that root.

Avoid releasing buffers merely because a window is minimized if restore still
needs those buffers. This is why directly calling `unmap_surface_content()` is
probably too destructive without refactoring.

### Step 4: Make restore explicit

Restore should:

- take latest hidden surfaces;
- restore the full root tree;
- focus the root;
- explicitly raise the restored root tree;
- advance render generation with `WindowRestore`.

Also replace `HashMap::iter()` restore selection with a deterministic policy:
most-recently-minimized or dock order.

### Step 5: Native validation

Use existing perf first:

```sh
OBLIVION_ONE_PERF_LOG=1 OBLIVION_ONE_MODE=1920x1080@165 OBLIVION_ONE_CURSOR=hardware ./bin/oblivion-one --output native
```

Inspect:

```sh
rg "render_cause=window_minimize|render_cause=window_restore|surfaces=|app.toplevel|native.frame" ~/.local/state/oblivion-one/session.log
```

Expected after fixes:

- after Alt+M, the next repaint logs `render_cause=window_minimize` and
  `surfaces=0` for a single app;
- subsequent client commits while minimized do not increase visible surface
  count;
- dock remains visible with a minimized item;
- Alt+R or dock click logs `render_cause=window_restore` and restores latest
  content;
- no alternating `window_minimize -> surface_commit -> visible again` pattern.

## Risks

- Blocking minimized commits without storing the new buffer will restore stale
  content.
- Reusing unmap cleanup too aggressively can release buffers still needed for
  restore.
- Changing restore order is user-facing; choose and test an explicit policy.
- Native-only fixes would be wrong. The bug is in compositor state and should be
  fixed before output-specific work.

## Bottom Line

The likely native SDDM minimize failure is a compositor state bug: minimized
windows are removed from the visible render list, but later client commits can
put them back because commit handling does not respect minimized roots. Fix the
core state model first, with tests for commit-while-minimized and latest-content
restore, then validate in native with perf logs around `window_minimize`,
`surface_commit`, and `window_restore`.
