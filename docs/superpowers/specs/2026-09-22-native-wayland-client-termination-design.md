# Native Wayland client termination investigation and fix

## Goal

Determine whether native Firefox/Zen termination is caused by Typhon issuing a
fatal Wayland protocol error, then fix only protocol or lifecycle violations
that are demonstrated by a reproduced sequence or by a source-backed
regression test. The implementation must remain client-independent and must
preserve fatal validation for genuinely invalid requests.

## Evidence and constraints

The existing xdg-decoration transaction work is already present and its
focused tests pass. The current global advertises xdg-decoration version 1,
although the local `wayland-protocols 0.32.13` XML and generated interfaces
support version 2. Upstream v2 makes creation of a decoration object after a
toplevel has content legal and defines the destroy/recreate behavior needed by
clients that replace the object while a surface is mapped.

The current popup-grab validator accepts only pointer-button serials even
though Typhon records keyboard and touch press serials. This is a narrow
protocol-compatibility defect; ownership, root, seat, serial freshness, and
focus-generation checks must remain unchanged.

The xdg configure ledger is the sole owner of configure acknowledgement
state. Every compositor-emitted `xdg_surface.configure` must enter that ledger
at the same point it is sent. Unknown and already-consumed acknowledgements
remain fatal.

No Firefox-, Zen-, GTK-, OBS-, or app-id-specific behavior is allowed. No
timing workarounds, permissive popup destruction, disabled protocol checks, or
parallel transaction systems are allowed.

## Chosen architecture

### Decoration protocol version

Advertise `zxdg_decoration_manager_v1` at version 2. No crate update is needed:
the checked-in dependency already contains manager and toplevel-decoration
interface version 2.

The existing typed `WindowDecorationState` remains the source of truth. Add a
small lifetime state for a destroyed v2 object:

* destroying the object marks the object absent but does not immediately
  change the visible/applied decoration mode or clear the requested
  preference;
* if the object is recreated before a surface commit, the previous preference
  is retained;
* if a surface commit occurs while the object is absent, the pending destroy is
  finalized through the existing acknowledge-plus-commit transaction and the
  effective preference becomes client-side;
* recreating after that commit starts with client-side semantics until the
  client explicitly requests another mode;
* v1 keeps the existing `unconfigured_buffer` rejection when content already
  exists.

The decoration mode attached to an xdg configure record remains the mode that
is applied only after that configure is acknowledged and the corresponding
surface commit is processed. Object destruction alone must not publish a new
visible mode.

### Configure ledger

Keep `XdgSurfaceLifecycle` as the only acknowledgement ledger. Centralize the
send-and-record operation for the two compositor emission sites (toplevel and
popup), or enforce an equivalent shared helper, so a source/test invariant can
prove that each emitted serial has exactly one lifecycle record. Preserve the
existing rules for unknown, repeated, and consumed serials.

Clear stale acknowledged decoration state on unmap/remap, while preserving the
existing superseded-configure behavior. Add mixed-sequence tests covering
decoration, resize, activation, maximize/fullscreen, popup reposition, remap,
and acknowledgement of the newest configure.

### Popup-grab serials

Split the input-event predicate used by popup grabs from the stricter drag
predicate. Popup grabs accept only `PointerButtonPress`, `KeyboardKeyPress`,
and `TouchDown`. They continue to require the exact serial, owning client,
owning root, seat association, and current focus generation. Pointer-enter,
unknown, stale, cross-client, cross-root, and old-focus serials remain invalid.

Add an integration regression for the Firefox-like keyboard sequence:
mapped toplevel, keyboard press, popup creation, keyboard serial grab, initial
popup configure/ack/commit.

### Fatal diagnostics

Retain one centralized fatal protocol-error emission path. Extend
`ProtocolErrorTrace` records/debug output with the human-readable reason and,
where available from existing client identity data, the peer PID. Map xdg
resource types to precise interface labels (surface, toplevel, popup,
positioner, decoration manager, or decoration object) while retaining the
existing stable categories. Debug formatting is emitted only on fatal paths;
normal protocol traffic does not pay for logging.

### Broader audit

Audit the native protocol handlers reachable from an xdg toplevel, including
wl_surface/subsurface, dmabuf, explicit synchronization, xdg-shell,
xdg-decoration, and input serial paths. Use the existing error trace and
surface/commit tracing. Only implement changes backed by a reproduced fatal
sequence or a focused source-backed regression.

## Test strategy

Add focused regressions before implementation changes:

* decoration v1 pre-map success and post-content rejection;
* decoration v2 post-map creation;
* v2 destroy/recreate before commit retaining the mode;
* v2 destroy/commit/recreate starting client-side;
* repeated preference suppression, configure/ack/commit mode ownership, and
  fullscreen coherence;
* pointer, keyboard, and touch popup grabs;
* invalid enter, unknown, stale, wrong-client, wrong-root, and old-focus
  serials;
* nested popup topmost destruction, reposition, parent teardown, and rapid
  recreate cycles;
* mixed toplevel and popup configure ledger sequences.

Run focused tests first, then the source-layout gate and the complete required
Rust checks with all output directed to `/mnt/Aether/Desktop/GitHub`.

## Alternatives rejected

* Continuing to advertise v1 would make legal v2 client behavior a fatal
  protocol error.
* Accepting arbitrary serials or weakening popup destruction would hide real
  client bugs and violate xdg-shell ownership semantics.
* Adding a second decoration/configure transaction system would risk
  reintroducing the duplicate/ghost-window regression.

## Compatibility risks

Some clients may still behave according to v1 and create decoration objects
too late; they remain correctly rejected when they bind v1. The v2 path must
not apply a decoration preference at object destruction time, because doing so
would create a visible mode transition without the required configure
transaction. The live Firefox/Zen, OBS, and simple native-Wayland checks are
environment-dependent and will be reported separately from automated tests.
