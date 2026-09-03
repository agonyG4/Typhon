# Typhon Pointer Constraint Surface Transaction v1 — Closure Report

## Result

The pointer-constraint path now carries a commit-exact surface payload through
subsurface transactions and pairs each selected native routing transition with
the timing evidence from the native action that produced it.

The implementation preserves the existing native input pipeline: no input
thread, polling loop, sleep, mutex ingress queue, motion-value rewrite, or
timestamp reorder was introduced.

## Commit-exact change set

The implementation is split into these commits:

- `23a2d3e` — associate pointer transition timing causally.
- `45e9371` — capture pointer constraint state in surface commits.
- `72eaf2e` — synchronize pointer constraint lifecycle.

The design and execution records are:

- `7564ab9` — pointer constraint surface transaction design.
- `a976ba9` — pointer constraint surface transaction implementation plan.

The implementation boundary is:

1. Native settlement reports a single selected transition together with the
   activation/deactivation timing for that same native action.
2. `wl_surface.commit` captures immutable lifecycle, region, and cursor-hint
   payloads as `CachedSubsurfaceCommit` data.
3. Payload fields use explicit `NoChange` versus explicit region/default and
   concrete hint states, with reducers for merge and replacement.
4. The payload is applied to every tree node, including synchronized
   subsurface children, after ordinary input-region, geometry, and mapping
   state.
5. Native activation and deactivation are permitted only at
   `NativeInputEpoch::constraint_settlement_allowed()`.
6. Pending installs participate in `AlreadyConstrained`; resource creation is
   immediate but native activation waits for the matching commit.
7. Client destruction is immediately dead-resource state, while committed
   effective state remains until its staged removal is published by commit.
   Forced teardown remains immediate.
8. Create-and-destroy before the first commit collapses without activation,
   events, or a ghost warp. Stale deactivation cannot remove a newer current
   constraint.
9. A hint and removal captured in one commit use that commit's hint for
   restoration, while the existing one-shot warp/compositor-driven distinction
   is preserved.

## Protocol and policy boundary

The protocol source of truth is `wl_surface.commit`: pointer-constraint
regions and cursor-position hints are double-buffered surface state and become
current only when the surface commit is processed. Typhon's lifecycle rule is
an explicit policy layered on that protocol behavior: install/removal of a
constraint is synchronized with the corresponding committed surface payload.

Relevant source locations are:

- `src/compositor/protocols/core.rs` — commit capture point.
- `src/compositor/subsurface.rs` — captured payload and merge reducers.
- `src/compositor/state/surface_transactions.rs` and
  `src/compositor/state/subsurfaces.rs` — cached commit propagation.
- `src/compositor/state/pointer_constraints.rs` — staged lifecycle and native
  settlement.
- `src/compositor/state/surface_focus.rs` — current-state reevaluation.
- `src/native_output/input/routing.rs` and
  `src/native_output/runtime/cycle_dispatch.rs` — causal transition timing.

## Tests and verification

Regression coverage was updated for normal lock/confine activation, active
removal, pending one-shot fallback, same-surface rejection, pointer-warp
restore, relative-input helpers, repeated lock/unlock cycles, and disconnect
cleanup. Payload unit tests cover reducer replacement, explicit default region,
install/remove collapse, removal preservation, and hint preservation.

The following commands are the required final checks:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

On this Windows host, the Cargo build/test commands stop in `wayland-sys
v0.31.11` because the native `wayland-server` library and `pkg-config` are not
available. Only the MSVC Rust target is installed, so a Linux-target run was
not available. `cargo fmt --check` reports formatter drift only in unrelated
user-modified frame/pacing/scanout files; the pointer-constraint files are
formatted. `git diff --check` is clean for the task commits.

## Non-claims

This change closes the specified pointer-constraint transaction and timing
association surface. It does not claim to resolve unrelated frame-clock or
native-output work currently present in the worktree, nor does it claim that
every independent input race outside this surface has been redesigned.

