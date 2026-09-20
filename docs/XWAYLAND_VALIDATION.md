# Typhon XWayland validation

This is the operator checklist for validating popup classification, resize
commit ordering, minimize/restore lifecycle, and `_NET_WM_MOVERESIZE` input
ordering. The deterministic Rust tests are suitable for every build. The
Steam and native-session checks require a running Typhon session with the
relevant applications installed.

## Deterministic checks

Run the focused suites first:

```sh
cargo test --locked xwayland::xwm::
cargo test --locked compositor::tests::xwayland::
cargo test --locked native_output::runtime::xwayland_reactor_tests::
```

The opt-in X11 driver generates the request order used by the native checks:
managed creation, late popup properties, OR reconciliation, moveresize,
commit blocking/release, and unmap/remap.

```sh
TYPHON_XWAYLAND_NATIVE_TESTS=1 \
  cargo test --locked --test xwayland_native_regression -- --nocapture
```

The driver is intentionally opt-in because it mutates the X11 display named
by `DISPLAY`.

This driver is only a request-generation smoke test. It does not attach
Wayland surfaces, deliver XSync acknowledgements, inspect Typhon events,
assert focus or client-list membership, or verify rendered pixels. A passing
run must not be reported as Steam or end-to-end behavioral qualification.
Those assertions are covered by the deterministic tests below and by the
separate native TTY matrix.

## Trace capture

Enable the ordered Typhon trace while reproducing a native failure:

```sh
TYPHON_XWAYLAND_TRACE=1 TYPHON_XWAYLAND_LOG=1 typhon 2>typhon-xwayland.log
```

For each failure, keep the XID and inspect the ordered records for:

- `CreateNotify`, `MapRequest`, `MapNotify`, `ConfigureNotify`,
  `PropertyNotify`, `UnmapNotify`, and `DestroyNotify`;
- property refresh epochs and completion/cancellation;
- the stored and observed OR value, window type, transient parent, and
  lifecycle;
- association serial, surface ID, commit sequence, buffer ID, and buffer size;
- resize ACK observation, commit floor, release, accepted/rejected commit,
  timeout, and presentation;
- moveresize result and rejection reason;
- focus before/after `WindowReady` and metadata repair.

## Interactive resize geometry ownership

Interactive XWayland resize keeps three geometry states separate:

- Visual resize geometry may lead the client.
- Canonical X11 geometry may only advance from XWM progress, including an
  immediate configure or the geometry explicitly selected as timeout fallback.
- Committed buffer content may lag both and is tracked separately.

During an active resize, a client `ConfigureRequest` is answered with canonical
applied geometry. It does not turn the pointer preview into X11 authority.
Intermediate XSync presentations advance canonical geometry while preserving
the latest pointer preview; only a valid final presentation or timeout fallback
can retire that preview.

## Interactive move and resize session matrix

Start Typhon with input and resize tracing enabled:

```sh
TYPHON_XWAYLAND_TRACE=1 TYPHON_XWAYLAND_LOG=1 \
TYPHON_RESIZE_DEBUG=1 TYPHON_POINTER_DEBUG=1 \
  typhon 2>typhon-interaction.log
```

On a native session, exercise a native Wayland/XDG application through move,
slow resize, rapid resize, and release followed immediately by move. Repeat the
same cases with a Flatpak application after determining from the session events
whether that client is native XDG or has an XWayland window; packaging does not
identify its window protocol. For an XWayland application, exercise move, slow
and rapid resize, repeated direction changes, release followed immediately by
move, and maximize/restore where supported. After each interaction settles,
check the trace for a retired interaction, converged canonical and visual
geometry, and no later resize presentation overwriting a move. Record this as
native session evidence separately from deterministic test results.

Classify the sequence before changing code. The useful categories are stale
classification, provisional focus, client withdrawal, WM self-unmap, missing
association/buffer, stale resize commit, missing commit edge, moveresize
rejection, or behavior not correlated with Typhon events.

## Steam qualification

No Steam result is implied by the opt-in X11 smoke test. Run and record the
following matrix on a native TTY session with Steam already configured:

Run the following on a native TTY session with Steam already configured:

1. Open Library 50 times and record every failed or repeated click.
2. Open Help 50 times and record every failed or repeated click.
3. Trigger the previously black popup 50 times.
4. Resize slowly for 30 seconds, then rapidly for 30 seconds.
5. Release during a final rapid resize.
6. Minimize and restore 20 times.

Acceptance targets:

- 50/50 successful Library openings;
- 50/50 successful Help openings;
- zero black popups;
- no black resize flash or stuck preview;
- no black window after minimize/restore;
- no repeated click required.

Repeat the popup and resize checks with Firefox, Zen, Kitty or another X11
terminal, GTK3 X11, Qt6 X11, and Sober/Roblox where available. A Steam-only
pass is not sufficient evidence that the lifecycle or resize state machines
are correct.

## Final repository checks

```sh
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
```

Also perform a release build and a native TTY smoke test. If
`check-source-layout` reports files that were already over the repository
limits before this validation phase, record that as a separate cleanup task;
do not mix source-layout refactoring with popup, resize, lifecycle, or input
fixes.
