# Typhon Locked Cursor Visual Reveal Qualification v1 — Design

## Scope

This design qualifies the remaining Sober/Roblox symptom in the current Typhon
checkout: the cursor appears to teleport visually when an RMB locked pointer is
released. The starting `HEAD` is
`2c4bb7e6c908c10d6d19312b17902eaf7cc0f2af`.

The solved camera jump, physical input backlog, locked-pointer logical warp,
pointer-constraint surface transaction, and activation-anchor fallback issues
are out of scope. The implementation must not reopen them.

## Ownership invariant

The qualification follows one causal chain:

```text
pointer authority
    -> unlock and optional valid client warp
    -> reveal authority
    -> cursor source and client surface geometry
    -> NativeAtomicCursor desired revision
    -> plane policy and frozen owner
    -> exact KMS/software presentation
    -> first visible cursor result
```

For every reveal, the first visible state must correspond to the latest
authoritative reveal state, subject only to explicit output transform and
clipping conversion. A hidden old state must never become visible for one frame
before the current state replaces it.

## Existing architecture to preserve

Typhon already owns cursor state through
`NativeAtomicCursor::{desired, submitted, current}`, desired/submitted epochs,
`CursorRevisionTracker`, `CursorDeltaClass`, plane assignments, worker-owned
submissions, frozen owners, framebuffer pins, pageflip promotion,
`PresentedCursorState`, and capability validation. This work extends and
validates that ownership; it does not add a second cursor transaction system.

The existing `PositionOnly` path remains eligible for low-latency movement only
when the visible hardware representation and all visual/capability inputs are
unchanged. Hidden-to-visible, image, hotspot, source, visibility, and
hardware/software transitions remain coherent visual-state publications.

## Observability

Add the opt-in environment variable `TYPHON_CURSOR_PRESENTATION_TRACE=1`.
Tracing is a lazy, causal event stream using the existing constraint identity
(`constraint_id` and generation) as its correlation key. The disabled path does
not read clocks, format fields, allocate strings, alter scheduling, or emit
logs. A narrow reveal context bridges the compositor reveal lifecycle to the
native presentation lifecycle without introducing a global transaction ID.

The stream covers:

1. `unlock_reveal_begin`, backend settlement, client warp observation, and
   `unlock_reveal_finalize` including positions, origin, dispatch epoch, and
   visibility request.
2. Resolved cursor source and client cursor surface/buffer/commit geometry,
   including logical position, surface offset, hotspot, and resolved output
   position.
3. Native desired/current/submitted/queued/pending state with epochs,
   revisions, visibility, position, image identity, and source identity.
4. Plane-policy inputs and outputs: deliveries, delta class, cursor/primary
   actions, test policy, revision, and presented state.
5. Frozen ownership and worker sidecar replacement, including transaction and
   pageflip identity, framebuffer pins, source/capability keys, and final
   cursor assignment.
6. The cursor-specific KMS payload and the pageflip/presented state, followed
   by exactly one `first_visible_cursor_presentation` event per reveal.

The presentation trace is intentionally separate from the existing pointer
timing trace. The earlier `LockedDeactivated` line proves the pointer boundary;
this trace proves what happens after the cursor is still hidden.

## Tests

Use deterministic existing cursor and worker test hooks. Add sequence-sensitive
tests for:

- hidden P0 -> hidden P1 -> visible, with the first visible state at P1;
- unchanged pointer hide/show;
- hidden image/hotspot/position changes as one coherent state;
- client cursor source and surface-generation changes while hidden;
- valid post-unlock client warp exactly once;
- no-warp preservation of current logical position;
- stale worker revisions, sidecars, and primary commits in flight;
- hidden/hardware/software delivery transitions and no double representation;
- first-visible matching the authoritative reveal state.

Existing pointer-constraint, cursor, plane-policy, KMS worker, and pageflip
promotion tests remain regression guards.

## Qualification and correction gate

First implement and validate the full trace and focused tests. The user then
performs the existing hardware/software A/B qualification with
`OBLIVION_ONE_CURSOR=hardware` and `OBLIVION_ONE_CURSOR=software`; Sober is not
automated. Only the first evidence-backed divergence is corrected. No delay,
synthetic motion, Sober-specific branch, activation-anchor fallback, or
speculative generation filtering is permitted.

If desired state is correct but an older state is newly visible, repair the
existing revalidation/supersession/bundle/sidecar ownership edge. If source or
surface geometry is first wrong, repair that source owner. If both modes
reproduce, investigate the shared reveal/source/visibility path before KMS.

## Report

Create a companion English Markdown report in `docs/superpowers/specs/` with
starting and ending `HEAD`, KWin analysis, the complete evidence chain, the
first divergence and correction (or an explicit no-correction result), focused
and global verification, and manual A/B results when supplied by the user.

