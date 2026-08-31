# Typhon Native Input Semantic Epoch and Activation-Time Anchor Closure Plan

> **Execution:** Inline only, as explicitly requested by the user. Do not use
> subagents or the `rtk` wrapper. Commit only task-owned files and preserve
> pre-existing unrelated working-tree changes.

## Objective

Prevent client-side input-semantic changes from reinterpreting a materialized
native input epoch, and resolve locked-pointer activation anchors at the
actual native settlement boundary.

## Global constraints

- Preserve all completed pointer-warp, enter-serial, confinement, v11,
  unlock-settlement, implicit-grab, and native pointer-constraint epoch
  invariants.
- Keep the 256-event native drain budget, retained batch storage, coalescing,
  input-only fast path, and write-side flush batching.
- Do not filter timestamps, drop motion, threshold deltas, add sleeps, add
  application detection, or touch unrelated render/KMS/DMA-BUF/XWayland/
  shell code.
- Use strict RED -> GREEN evidence for the real protocol/resource tests and
  bounded input continuation tests.

## Task 1: RED evidence and diagnostics

1. Add real-server/resource regressions for combined input + Wayland
   readiness, first post-activation motion, mid-batch read-side prevention,
   Wayland-only dispatch, and late-bound activation anchor resolution.
2. Add native backend tests for libinput dispatch-once continuation and exact
   256-event exhaustion where the API permits deterministic injection; retain
   raw-evdev bounded continuation/liveness tests.
3. Run the new tests against the current implementation and record the
   expected failures: a newly created relative resource receives the old
   motion, and activation retains the request-time anchor.
4. Add only gated, lazy pointer diagnostics for semantic epoch identity,
   resource topology timing, batch timestamps/counts, deferred reads,
   transition application boundaries, activation cursor/anchor agreement, and
   continuation state.

## Task 2: stable semantic input phase ordering

1. Change the native cycle seam to distinguish `service_input` from
   `dispatch_wayland_read_side`.
2. Settle older backend work first. If input or a retained backlog is
   serviceable, drain/process that native epoch before client read-side
   dispatch. If no input epoch is materialized, preserve immediate Wayland-only
   dispatch.
3. Remove the in-epoch `tick_with_outcome()` read-side dispatch. Record one
   deferred protocol progression requirement and perform at most one read-side
   dispatch after the epoch and native-input batch end.
4. Add a server semantic-epoch gate so neither direct dispatch nor an API that
   internally dispatches client requests can mutate input topology while the
   epoch is active.
5. Apply queued pointer-constraint backend work after the post-epoch read,
   using current compositor state. Preserve FIFO native request ordering and
   cursor synchronization.

## Task 3: late-bound activation anchor

1. Change `PointerConstraintBackendRequest::ActivateLocked` to carry only the
   backend id and remove `pending_locked_activation_anchors`.
2. Move locked activation-region/focus/anchor resolution to the native
   settlement seam. Revalidate resource lifetime, generation, eligibility,
   region, and ownership there.
3. Pass the resolved anchor to the backend and return that exact anchor to
   compositor activation routing. Store/assert backend and compositor anchor
   agreement before sending `locked`.
4. Clear stale pending ownership safely when deferred eligibility disappears,
   so eligible persistent constraints can follow the existing retry path.

## Task 4: bounded libinput/raw continuation

1. Add a dispatch flag to native input draining. A new semantic epoch calls
   libinput dispatch once; a continuation consumes only the already-dispatched
   internal queue.
2. Use `libinput_next_event_type()` at the budget boundary instead of
   inferring backlog solely from `len == 256`.
3. Keep raw evdev memory bounded and keep client read-side dispatch deferred
   for any raw continuation. Add liveness coverage so continuous input cannot
   permanently starve non-input work.
4. Keep the existing epoch id across continuations and explicitly arm the
   existing immediate continuation deadline.

## Task 5: GREEN and review

1. Rerun targeted causality/anchor/continuation tests.
2. Rerun surrounding pointer, relative-motion, pointer-constraint, native
   input/coalescing, resource-efficiency, unlock-settlement, implicit-grab,
   v11/legacy, and stale-generation suites.
3. Run `cargo fmt --check`, `cargo check --locked --all-targets`,
   `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`,
   and `git diff --check` without `rtk`.
4. Audit all `dispatch_wayland_with_outcome()` and `tick_with_outcome()` call
   sites, all `ActivateLocked` construction/use sites, continuation behavior,
   native/compositor anchor agreement, and unrelated diff scope.
5. Commit in logical scoped slices, never staging the pre-existing DMA-BUF
   changes.

## Acceptance criteria

- Native epochs have immutable input-resource topology, focus/grab meaning,
  and native constraint mode/generation.
- Pre-existing backend work settles before a new epoch; work queued by read
  dispatch or mid-epoch progression settles after it.
- A new relative resource cannot receive older native backlog, while the next
  real sample after activation is delivered exactly.
- Activation uses the current settlement-time pointer position and the same
  anchor reaches backend and compositor routing.
- Libinput dispatch is once per epoch, continuations are bounded and live,
  and exact exhaustion does not strand an epoch.
- Existing pointer and resource-efficiency behavior remains green.
- Sober/Roblox remains an interactive qualification item, not a deterministic
  test claim.
