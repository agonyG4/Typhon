# Client-Provided Wayland Cursor Overlay Design

Date: 2026-06-20

## Objective

Render non-null `wl_pointer.set_cursor` surfaces as the final client-provided
pointer image in native and nested GPU and CPU paths. Preserve exact pointer
resource, focused-client, and enter-serial validation; cursor hiding and locked
pointer behavior; ordinary surface isolation; buffer ownership; frame pacing;
and cached scene reuse.

The current implementation accepts a valid cursor request, permanently marks
the surface as a cursor surface, removes it from ordinary content, and hides
Typhon's built-in cursor. Its commit path stores the buffer but neither creates
dedicated renderable content nor passes that content to a renderer. The pointer
therefore remains functional while its image disappears.

## Protocol State

`CompositorState` will own two separate cursor concepts:

- `ActiveClientCursor` contains the exact owning `wl_pointer`, cursor surface
  ID, and logical hotspot.
- A cursor-surface store maps every permanently assigned cursor surface ID to
  its latest committed `RenderableSurface`, when it has visible content.

The existing cursor-role ID set remains the permanent role registry. Once an
ID enters it, that `wl_surface` can never become ordinary content, an XDG
window or popup, a subsurface stacking entry, a decoration, or a hit-test or
focus target. The compositor rejects or isolates later role and mapping
attempts using the same permanent registry.

The active selection and committed surface content are independent. Changing
the active cursor does not erase content committed to previously used cursor
surfaces. Re-selecting one can display its retained content without requiring
another attach. Replacement, null attachment, or destruction of a surface
changes that surface's committed content and follows normal buffer lifetime
rules.

The renderer-facing API will expose a borrowed, Wayland-resource-free snapshot:

```rust
pub struct ClientCursorRenderState<'a> {
    pub surface: &'a RenderableSurface,
    pub logical_x: i32,
    pub logical_y: i32,
}
```

The logical position is the rounded compositor pointer position minus the
logical hotspot. Renderers apply output scale once to the position and surface
dimensions. Buffer scale and viewport destination are already reflected in the
`RenderableSurface` logical dimensions by the generic commit conversion.

## Cursor Selection And Visibility

`set_pointer_cursor` retains the existing checks:

1. the request comes from the exact pointer resource belonging to the focused
   client;
2. the serial equals that resource's current enter serial for the focused
   surface;
3. invalid or stale requests have no state effect.

For a valid non-null request, the compositor permanently assigns the cursor
role, unmaps any accidental ordinary content, stores the exact pointer,
surface ID, and hotspot, and suppresses the built-in cursor. Repeated requests
may select another cursor surface or change only the hotspot. Both transitions
invalidate cursor overlay state without invalidating the cached window scene.

For a valid null request, the compositor clears active client cursor ownership
and records the exact pointer as explicitly hidden. No built-in cursor is
restored. This remains Sober's required self-rendered-cursor behavior.

An active cursor surface whose committed attachment is null has no render
snapshot. The built-in cursor remains hidden because client ownership is still
active. Destroying the active cursor surface clears active ownership and keeps
client cursor imagery hidden until another valid `set_cursor` request; it does
not invent a built-in fallback. Destroying an inactive cursor surface removes
only its retained content and role bookkeeping associated with the dead
resource.

Focus loss, focus transfer to another client, owning pointer destruction, and
client disconnect clear matching ownership and damage the previous cursor
rectangle. Cleanup always compares the exact Wayland pointer resource. Cursor
role assignment on a live surface remains permanent even when it is inactive.

Locked-pointer activation suppresses all compositor cursor imagery, including
the client overlay, without changing the stored selection. Existing pending
unlock reveal ordering remains authoritative: no selection or visibility
transition may expose a built-in or client cursor between backend unlock/warp
and the client's same-dispatch cursor request.

## Commit And Buffer Lifecycle

Cursor commits pass through the generic surface commit state before entering a
dedicated cursor sink. The sink receives committed damage and uses
`PendingSurfaceBuffer::to_renderable_surface`, including committed viewport
destination, buffer scale, SHM snapshots, dmabuf handles, logical dimensions,
and normalized damage.

Damage-only commits update SHM pixels, dimensions, generation, and damage in
the dedicated cursor store using the same rules as ordinary surfaces. A null
attachment removes only that cursor surface's visible content and damages its
previous active rectangle. The active selection and built-in suppression stay
unchanged.

Explicit-sync cursor commits use the existing acquire-point validation and
pending commit queue. Ready commits dispatch to the cursor sink based on the
permanent cursor role. No cursor-specific fence or release queue is introduced.
SHM releases, active dmabuf ownership, deferred dmabuf release, explicit
release signaling, and presentation completion remain in the existing maps and
frame lifecycle.

Cursor frame callbacks for visible active content join the normal pending frame
callbacks and complete only after the submitted frame completes. Inactive or
contentless cursor surfaces use the compositor's existing non-visible-surface
callback policy so callbacks cannot remain pending indefinitely. Presentation
feedback follows the same visible/non-visible policy as the generic commit
path.

Replacing a committed buffer releases the obsolete buffer only through the
existing safe deferred lifecycle. Selecting a different cursor surface does
not itself discard the old surface's committed state or prematurely release a
buffer still retained as that surface's content.

## Generation, Motion, And Damage

Add cursor-specific render generation causes for commit/content, motion, and
selection/state transitions. `as_str`, damage policy, logging, and exhaustive
matches will include them.

When unlocked absolute pointer coordinates change and an active cursor has
visible content, compositor state advances the cursor-motion generation. This
causes nested redraw and native repaint scheduling through their existing
render-generation checks. Built-in hardware-cursor-only movement keeps its
current fast path. Locked absolute motion remains suppressed and does not move
or reveal compositor imagery.

The renderer snapshot defines the current logical rectangle. Each output
damage tracker records the previous physical client-cursor rectangle and
compares it with the new clipped rectangle and surface content generation.
Damage includes both rectangles for movement, hotspot changes, selection,
hide, null attach, destruction, viewport or buffer-scale changes, content size
changes, and output size or scale changes. A content-only update damages the
current rectangle. Rectangles are computed with wide/saturating arithmetic and
clipped on every edge. Full-output damage is the fallback only when a safe
bounded representation is unavailable.

Cursor motion is overlay-only state. It does not change the EGL window/shell
scene cache key or rebuild its scene commands.

## Renderer Integration

The snapshot is propagated independently through:

- `OwnCompositorServer`;
- `NativeFrameRequest` and native CPU/EGL request construction;
- `NestedSceneDrawRequest` and nested CPU/EGL request construction;
- `DesktopComposeRequest`;
- `EglSceneDrawRequest`.

The compositor-owned `DesktopVisualState.cursor` continues to represent only
Typhon's built-in cursor. It is `None` whenever a client cursor request or lock
suppresses compositor cursor visibility. The client cursor snapshot is never
translated back into that field. Nested mode keeps the host cursor hidden while
the client overlay owns the image.

Rendering order is:

```text
wallpaper
-> client windows and subsurfaces
-> shell overlay
-> client cursor surface
```

No existing critical compositor-owned overlay is currently required above the
cursor, so the client cursor is the final visual layer.

### EGL/GLES

The EGL renderer reuses `surface_resources` upload/import logic for the cursor
surface while passing the active cursor ID into resource liveness separately
from ordinary scene IDs. SHM damage updates only damaged texture regions;
dmabuf resources use the existing import and reuse cache. Resources become
stale only when absent from both ordinary scene and retained cursor needs.

Window commands remain cached by ordinary surface signatures and content
generation. Client-cursor commands are rebuilt with the other overlay commands
on every relevant overlay change and appended after shell commands. Cursor
generation changes trigger resource upload/import even when ordinary surfaces
are unchanged.

`EglOutputDamageTracker` tracks a generic client cursor rectangle and content
signature independently of the fixed built-in cursor rectangle. Client cursor
dimensions come from its logical surface size after output scaling; the built-in
`cursor_texture_size()` is never used for client damage.

### CPU

`DesktopSceneRenderer` maintains a cursor-free reusable base containing the
wallpaper, windows, and shell. It alpha-composites the client cursor last using
the same nearest sampling, logical-to-physical scaling, and premultiplied-alpha
rules as client surfaces. Sampling and writes are clipped safely at all output
edges.

The reusable-frame key includes client cursor identity, position, dimensions,
and content generation, or the renderer copies the cursor-free base before
every client-cursor frame. The initial implementation will prefer correctness:
restore/copy the base before drawing the client cursor so movement, removal,
and hiding cannot leave trails. Built-in and client cursors are mutually
exclusive.

## Logging And Observability

Under `TYPHON_POINTER_DEBUG`, log valid active cursor selection, hotspot
changes, buffer commits and removals, cleanup reason, and detailed invalid
request reasons. Ordinary motion remains silent unless existing motion debug
logging already applies.

Test support gains read-only captures for active cursor ID, hotspot and logical
position, render-content presence, generation/cause, and pending lifecycle
work. Renderer statistics expose enough state to prove cursor-only motion does
not rebuild the EGL window scene.

## Testing

Tests will be written before each implementation slice.

Protocol/state tests cover exact-client and serial validation, active snapshot
position, permanent role isolation, `set_cursor(None)`, null attachment, focus
loss, pointer and surface destruction, inactive retained content, hotspot-only
changes, frame callbacks, SHM and dmabuf replacement/release, explicit sync,
locking and pending reveal ordering, and exclusion from mapping, stacking,
focus, and hit testing.

EGL tests cover clipping, old-plus-new damage, hide/removal and content damage,
hotspot movement, cursor-only resource liveness, overlay ordering after shell,
SHM update/import signatures, and no scene-cache rebuild on cursor-only motion.

CPU deterministic pixel tests cover ordering above windows and shell, hotspot,
premultiplied alpha, every-edge clipping, movement without trails, removal
restoring underlying pixels, and built-in/client mutual exclusion.

Existing pointer-lock, Sober ownership, `set_cursor(None)`, unlock reveal,
cursor visibility backend, exact-pointer relative motion, surface lifecycle,
frame callback, dmabuf, and explicit-sync tests remain unchanged and must pass.

Validation commands are:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test <focused cursor filters>
cargo test --workspace
cargo build --workspace --release
git diff --check
```

Native Zen Browser, Sober, nested GPU/CPU, and native EGL/CPU manual checks are
reported only when the corresponding hardware and applications are actually
available and exercised.

## Non-Goals

This design does not add arbitrary hardware cursor-plane promotion, cursor
themes, XCursor, XWayland cursor integration, tablet/touch cursor behavior,
pointer acceleration changes, relative-motion changes, direct scanout, VRR,
atomic KMS work, or a general renderer architecture rewrite.
