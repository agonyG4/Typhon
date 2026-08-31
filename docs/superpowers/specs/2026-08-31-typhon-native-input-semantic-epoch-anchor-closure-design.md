# Typhon Native Input Semantic Epoch and Activation-Time Anchor Closure

## Status

Focused follow-up to the native pointer-constraint causality closure. This
document supersedes the earlier phase description that allowed new Wayland
input topology to be read before a serviceable native backlog was drained.

## Evidence

The clean Sober trace showed that the previous constraint-generation epoch
fix removed the former first-locked relative spikes: the first locked sample
had a median of about 4.8 pixels, a maximum of 12.5 pixels, and no sample over
20 pixels. It also showed a separate deterministic relationship in 28 of 28
lock activations: the pointer position at activation minus the queued anchor
was exactly the complete coalesced motion processed between the lock request
and native activation. The largest stale-anchor distance was about 227.8
pixels; the intervening epoch had a median of 26.5 raw events and a median
timestamp span of about 27 ms.

That relationship proves stale request-time state, but the trace alone does
not prove that every large physical sample consists of pre-request hardware
events. A genuine post-activation backlog and input-service latency remain
falsifiable hypotheses. Epoch and timestamp diagnostics therefore remain
diagnostics, not motion-filtering authority.

## Semantic epoch invariant

Once a native input semantic epoch begins, its effective Wayland input-resource
topology, focus/grab interpretation, and native pointer-constraint
mode/generation are immutable until the epoch ends. Client requests observed
after the epoch begins can affect only a later semantic epoch.

The implementation does not copy all compositor state. It establishes the
invariant by phase ordering and a server-side read-side gate:

```text
settle older native constraint work
  -> if native input/backlog is serviceable, begin one semantic epoch
  -> dispatch libinput once (or consume its already-dispatched queue)
  -> drain/coalesce/process the bounded batch
  -> defer any native-only client progression requirement
  -> end the epoch and flush the existing native-input write batch
  -> perform at most one client read-side Wayland dispatch
  -> resolve/apply newly queued native constraint work
```

When no native input or retained backlog is serviceable, a Wayland-only cycle
may dispatch readable clients immediately. No client read-side dispatch is
performed while the semantic epoch gate is active, including through
`tick_with_outcome` or any equivalent API.

## Activation-time anchor

`ActivateLocked` is an intent containing only the current backend id. It does
not contain a request-time coordinate. Immediately before the native backend
activates it, Typhon revalidates the current generation, resources, focus,
client/root eligibility, and constraint region. It then resolves the existing
anchor policy from the current compositor pointer position, which is the
post-input position at the settlement boundary.

The resolved anchor is passed to the backend and returned with the activation
acknowledgement. The compositor's active locked-pointer route stores that same
value. Debug output records the cursor and resolved anchor under
`TYPHON_POINTER_DEBUG`.

If eligibility changed while the intent was deferred, the intent is skipped
and pending ownership is cleared through the existing retry/lifecycle path; a
stale request cannot activate a new generation.

Activation-specific focus reconciliation and the locked event occur only
after the settlement checks and resolved anchor are available. Queueing an
intent alone does not send an activation enter or locked event.

## Bounded input continuation

`NATIVE_INPUT_DRAIN_BUDGET` remains 256. For libinput, one semantic epoch
calls `libinput.dispatch()` once, then consumes the internal queue. At the
budget boundary, `libinput_next_event_type()` distinguishes an exact queue
exhaustion from a remaining event; continuation chunks consume the existing
queue without another dispatch. The same epoch id remains active across every
bounded continuation.

The raw evdev fallback retains bounded storage and its existing nonblocking
drain behavior. A raw continuation also keeps client read-side dispatch
deferred, and its bounded scheduling must still allow other reactor work to
run.

## Preserved behavior

Motion coalescing remains consecutive-motion-only with latest timestamp and
exact accelerated/unaccelerated sums. Button, key, and axis boundaries remain
boundaries. No physical motion is dropped, thresholded, clamped, reset, or
delayed by magnitude. Existing active-lock absolute immutability, exact
relative delivery, confinement, enter-serial authority, same-client warp,
implicit grabs, v11/legacy reposition behavior, unlock settlement, and cursor
write batching remain unchanged.

## Diagnostics and validation

Pointer-only diagnostics report semantic epoch id, effective constraint,
resource/batch counts, timestamps, continuation status, deferred read-side
work, and native transition boundaries. They are lazy and remain behind
`TYPHON_POINTER_DEBUG`; no hot-path logging is added when disabled.

Deterministic tests use real compositor protocol resources for the combined
readiness/resource-topology regression and for late-bound activation state.
Runtime qualification is separate: the compositor is not described as
Sober/Roblox-fixed until a real application run confirms it.

## References

- `2026-08-30-typhon-native-input-pointer-constraint-causality-closure-design.md`
- `2026-08-28-typhon-pointer-reposition-semantics-v2-design.md`
- `2026-08-29-typhon-locked-pointer-unlock-settlement-closure-design.md`
- `2026-08-25-typhon-resource-efficiency-v1-design.md`
