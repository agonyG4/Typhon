# Wayland Keyboard Shortcuts Inhibit v1

Typhon implements `zwp_keyboard_shortcuts_inhibit_manager_v1` and
`zwp_keyboard_shortcuts_inhibitor_v1` for the primary logical seat.

## Capability and advertisement

The global is advertised only when the compositor input capability
`keyboard_shortcuts_inhibit` is enabled. The native libinput profile enables the
capability after the compositor/native-input path is available; the desktop
baseline remains conservative. Clients that bind the global can therefore rely
on the protocol being intentionally enabled rather than merely generated into
the server.

## Ownership and duplicate requests

An inhibitor is owned by the exact `(wl_surface, logical seat)` pair and by the
client that owns both objects. Multiple `wl_seat` resources for the same
logical seat share one logical seat identity. A second inhibitor for the same
pair is rejected with `already_inhibited`. A surface or seat from another
client is never accepted.

The manager may be destroyed without destroying existing inhibitors. Inhibitor
resources are removed on explicit destroy, surface teardown, client teardown,
or stale-resource cleanup.

## Effective state

Registration and effective inhibition are separate states:

1. Registration records the request and starts with policy enabled.
2. Relevance requires a live, currently mapped surface whose exact surface
   resource owns the canonical `wl_keyboard` focus.
3. Effective inhibition is `relevant && policy_enabled`.

`active` is sent once for each false-to-true effective transition. This includes
creation while focused and mapped, focus return, and policy re-enable. No
duplicate `active` is sent while the state remains effective.

Focus loss, unmapping, and destruction update compositor state without sending
`inactive`. `inactive` is reserved for an explicit policy disable while the
surface remains relevant. Relevance is nevertheless removed immediately from
the compositor snapshot, so native input does not wait for a protocol event.

The policy transition is internal and intentionally separate from protocol
registration. The compositor exposes narrow effective snapshots containing only
the effective boolean and a monotonic generation. Native input reconciles that
snapshot before every hardware event.

## Input behavior

When effective inhibition changes from false to true, the native input state:

- cancels stateful shortcut sequences such as Alt-Tab;
- replays held deferred Alt/Super modifier presses exactly once;
- keeps the physical modifier truth intact; and
- does not synthesize releases for client key presses that were consumed while
  the shortcut was inhibited.

When inhibition changes from true to false, already-forwarded client key
ownership is preserved and its eventual release is forwarded normally. This
prevents modifier and key-state imbalance at either edge of the transition.

Only the shortcut binding layer is suppressed. Reserved and bypass bindings,
including emergency and virtual-terminal paths, remain available. Pointer
modifier bindings are suppressed while raw pointer delivery continues. No
XWayland keyboard grab is installed by this feature.

## Surfaces and cleanup

The exact focused surface identity is used; a sibling, parent, proxy, or another
surface resource for the same client cannot activate an inhibitor. XDG toplevels
and mapped layer-shell surfaces participate in relevance. Mapping and unmapping
are reconciled at their lifecycle transition, and focus changes are reconciled
with keyboard enter/leave state.

The compositor keeps bounded counters for registrations, duplicate requests,
effective activations, policy transitions, relevance deactivations, stale
cleanup, and destruction. These counters are diagnostics only and do not alter
the protocol state machine.

## Scope

This implementation is intentionally protocol-neutral at the policy boundary:
the compositor owns effective state, while native input consumes snapshots.
Future keyboard backends can use the same snapshot contract without changing
Wayland resource ownership or focus semantics.
