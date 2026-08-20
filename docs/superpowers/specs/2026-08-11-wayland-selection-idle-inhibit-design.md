# Typhon Wayland Selection and Idle Inhibition Design

## Goal

Fully implement and qualify `zwp_idle_inhibit_manager_v1`,
`zwp_primary_selection_device_manager_v1`, and `ext_data_control_manager_v1`,
then advertise them only from capability profiles whose end-to-end behavior is
covered by deterministic tests.

## Current-state findings

- The normal `wl_data_device` clipboard path already owns source, offer, FD,
  host-bridge, focus-publication, generation, and drag-and-drop behavior.
- `SelectionState` contains useful MIME/generation helpers but only models a
  real clipboard selection; PRIMARY is currently a record without a live
  source or offer lifecycle.
- The three advanced globals are registered conditionally, but PRIMARY and
  ext-data-control dispatches are scaffolding and idle inhibitors discard their
  target surface.
- Input serials are retained as Wayland `u32` values and validation accepts
  serials from older focus generations, which permits stale selection requests.
- Idle inhibition currently counts live resources rather than deriving
  effectiveness from target-surface visibility.

## Design

### Canonical seat selection broker

Evolve the existing selection module into the single canonical runtime broker
for two independent channels: `Clipboard` and `Primary`. Each channel stores
its own nonzero generation, active source identity/backend, normalized MIME
types, and live offer bindings. An offer records its channel and generation;
replacing or clearing one channel invalidates only that channel's offers.

The broker will expose a protocol-neutral source record with a source kind
(`wl_data_source`, PRIMARY source, ext-data-control source, or host bridge),
owner identity where available, MIME types, and a stable source key. Protocol
resources remain in `CompositorState` so their generated request/event types and
FD ownership stay at the dispatch boundary. DnD use-state remains in the
existing `wl_data_source` binding and is never inserted into generic selection
state.

The normal clipboard and host bridge are migrated to broker channel state;
there will be no second active-selection model that can disagree with it.
Protocol-specific maps retain only resource bindings and transfer adapters.

### Source and offer lifecycle

Setting a channel first validates ownership, source liveness, MIME policy, and
the relevant protocol rules. On replacement, the previous source is cancelled
when required, the channel generation advances, and only that channel's offers
are retired. Clearing advances the same channel generation. Removing a source
clears a channel only when the removed source is still the active source for
that channel, so stale destruction cannot mutate newer state.

Publication always follows the protocol order: create the offer, send every
MIME offer event, then send the selection event. Focused normal devices receive
clipboard and PRIMARY according to Typhon's keyboard protocol focus
(`keyboard_surface`), while desktop/window-management focus (`focused_surface`)
remains a separate intent. Ext-data-control devices receive both channels
independent of keyboard focus and receive current state immediately on
registration. Every receive path checks
client, channel, generation, active source, and MIME membership before forwarding
the owned FD. Rejected FDs are dropped deterministically.

### Wrap-safe input validation

Each remembered input serial receives an internal monotonic `u64` epoch in
addition to its Wayland serial. Selection validation resolves a serial to an
epoch only when the serial belongs to the requesting client, is an eligible
input event, and belongs to the current focus generation. The implementation
does not compare raw Wayland serial numbers. Focus transitions invalidate older
selection tokens, preventing an old clear or replacement from changing a newer
selection.

### PRIMARY protocol

Add resource bindings for PRIMARY sources, devices, and offers. Implement source
MIME advertisement/cancellation, device selection publication, `set_selection`
with the normal focus/serial policy, offer MIME events, `receive`, destruction,
and client teardown. PRIMARY publication and offers use the broker's Primary
channel and never reuse clipboard generations.

### ext-data-control protocol

Add resource bindings for data-control sources, devices, and offers. Implement
the generated protocol requests/events/errors from the installed bindings,
including the exact `used_source` error variant. A data-control source is
single-use across both Clipboard and PRIMARY. Its `set_selection` requests do
not require focus or an input serial. All registered devices observe both
channels, and each new device gets both current states immediately.

### Surface-aware idle inhibition

Replace the resource-only idle inhibitor vector with `IdleInhibitorBinding`
records containing the inhibitor resource, owner/client identity, and exact
target surface. Add an idempotent `reconcile_idle_inhibition()` operation that
derives the effective count from live bindings whose target root belongs to a
currently renderable/mapped surface tree, whose window is not minimized, and
whose client and surface resources are alive. Subsurface inhibitors resolve to
the visible root. Occlusion does not affect eligibility.

`IdleManager` receives a reconciliation setter rather than relying on scattered
increment/decrement bookkeeping. Creation, explicit destruction, client
disconnect, surface teardown, map/unmap, minimize/restore, layer visibility,
and test lifecycle helpers invoke reconciliation.

### Module boundaries

Selection broker logic moves out of `state/surfaces.rs` into
`state/selection_runtime.rs` (or the repository-equivalent focused state
module). Idle bindings and reconciliation remain in the input/idle state area.
Advanced dispatches are split into `protocols/primary_selection.rs`,
`protocols/data_control.rs`, and `protocols/idle_inhibit.rs`; unrelated
advanced protocols remain unchanged.

## Testing strategy

Tests are written red-first for each behavioral slice. Model-level tests cover
independent generations, source replacement/cancellation, stale source and
offer rejection, MIME bounds/deduplication, source reuse, and wrap-safe focus
epochs. Protocol tests cover wire ordering and FD transfer for PRIMARY,
ext-data-control, and cross-protocol interoperability. Existing normal
clipboard and DnD tests remain mandatory regressions.

Idle tests cover exact surface ownership, mapped/unmapped/minimized/restored
state, subsurface root visibility, client teardown, multiple inhibitors,
idempotent reconciliation, and occlusion behavior.

Capability/global tests are updated only after protocol tests pass, and verify
each global is advertised once at the intended version. Documentation states
the actual guarantees and explicitly leaves XWayland selection bridging out of
scope.

## Verification

On the current Windows machine, run the available Rust formatting, check,
clippy, unit/integration test, source-layout, and diff checks. Linux-only
Wayland-session smoke checks (`wayland-info`, PRIMARY clients,
ext-data-control clipboard manager, and idle-inhibit visibility transitions)
will be reported as pending for execution on the user's Linux environment.
