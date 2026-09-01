# Typhon Native Pointer Routing Transition Latency Closure v2

## Goal

Make transition-local input-service timing truthful and give newly ready native
input a bounded opportunity to run at major cycle-tail boundaries after a real
pointer-routing transition. Preserve semantic epochs, exact motion, native
wake authority, and all accepted pointer behavior.

## Evidence and decision

The current implementation has a single nonblocking input check immediately
after settlement. That closes only the case where input is already readable at
the check. It cannot observe input arriving later in the same broad cycle.
The supplied traces prove accumulation but, because `TYPHON_POINTER_DEBUG=1`
uses synchronous per-event output and the launcher uses `tee`, they do not yet
attribute the compositor wall-time owner.

The first implementation slice therefore makes timing observer-neutral. The
production scheduler change is a bounded transition guard, not a timer,
polling loop, sleep, unconditional continuation, or input thread.

## Timing truth

Each real backend routing action starts one fixed-capacity transition record at
`routing_transition_committed_at_ns`. The observer records actual input-service
entry, separate libinput dispatch and queue-drain/materialization spans,
Wayland read, cursor synchronization, cycle return, reactor wake, bounded
checkpoint count/index, and the first non-empty post-transition native batch.
There is no synthesized split between activation resolution and backend
activation. A completed record is one with a real post-transition batch; empty
bookkeeping attempts do not complete it. Incomplete records are bounded and a
replacement increments a bounded superseded-observation counter.

## Input ingress

`NativeInputBackend` exposes two explicit operations:

```text
begin_semantic_epoch()
drain_epoch_chunk_into(batch)
```

Libinput dispatch occurs only in `begin_semantic_epoch`, once per semantic
epoch. Continuations drain the already-dispatched queue only. Raw evdev keeps
its existing bounded drain semantics and has no libinput dispatch span.

## Transition guard

The guard is armed only by a completed locked/confined activation or
deactivation. It performs at most four statically bounded input-readiness
checkpoints:

1. immediately after the transition,
2. before XWayland/client-scene tail work when that work will run,
3. before acquire/prepare when that work will run,
4. immediately before render/presentation/KMS when that work will run.

At a checkpoint, an input-only epoll peek either does nothing or runs exactly
one real input microturn through the existing dispatch seam. The peek is
non-consuming and recognizes only healthy readable input (`EPOLLIN`), while
terminal input flags remain on the normal reactor lifecycle path. The guard
disarms after the first real fresh service, cycle completion, invalidation, or
replacement. It never recursively arms another guard and never yields in a
loop.

An active semantic epoch remains authoritative: a guard cannot insert a
Wayland/topology boundary inside it. A backlog continuation may continue the
same epoch without a second libinput dispatch or client read-side dispatch.

## Cycle-state ownership

The fresh microturn merges only input-dispatch effects that are additive or
monotonic (`accepted`, timing/counters, repaint skips, redraw, shutdown). It
does not overwrite pageflip, frame, presentation, or unrelated wake-snapshot
state. Fresh service is recorded as real input even when it arrived after the
original wake snapshot.

## Rejected alternatives

- Logging-only changes do not address a production tail gap; they are required
  for truthful qualification but are not a scheduler fix.
- An unconditional reactor continuation can wake on Typhon's continuation
  eventfd before new input arrives and therefore does not create the desired
  window.
- A dedicated input thread separates acquisition but cannot deliver Wayland
  relative events while the compositor thread is blocked and would complicate
  epoch ownership.
- Cursor/KMS or renderer/presentation changes are only appropriate if the
  corrected timing identifies one of those owners as the blocking phase.

## Non-goals

No motion dropping, clamping, timestamp reinterpretation, coalescing change,
application special case, permanent timer, busy wait, or compositor redesign.
