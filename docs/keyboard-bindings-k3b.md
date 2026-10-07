# Typhon Keyboard K3B — Authoritative XKB Symbolic Bindings

K3B adds layout-aware `KeySym` candidates to K3A's compiled binding engine. It does not add another keyboard-state authority: `XkbKeyboardState::physical_state` in the compositor remains the only authoritative XKB server state.

## Translation boundary

The native runtime asks the compositor for a small, fixed-size translation snapshot before routing a physical keyboard press. The compositor reads its current XKB state and returns numeric keysyms with a compact Shift/Ctrl/Alt/Logo modifier mask. Native input receives only these values; it owns no keymap, XKB state, RMLVO configuration, or Wayland modifier reconstruction.

For each coalesced hardware event, the runtime queries and routes the event, then applies its `NativeInputEffect` to the compositor before routing the next coalesced event. Applying the effect updates `XkbKeyboardState`. Therefore a Shift press followed by `W` in the same materialized batch makes the `W` query observe Shift, while the current key never affects its own press-time translation.

The query is read-only and fails closed. An unavailable keyboard state, invalid or out-of-keymap keycode, `NoSymbol`, or a base level containing zero or multiple symbols produces no candidate for that identity. Physical routing and client behavior continue normally.

## Raw and translated identities

Both identities come from the same authoritative state and use the effective layout for the key.

- **Raw** uses exactly one level-zero keysym from the current layout and retains every active significant shortcut modifier. This supports conventional combinations such as `Shift+W` by matching `w` with `Shift`, and `Shift+=` by matching `equal` with `Shift`.
- **Translated** uses XKB's current-state single-symbol translation. It removes only significant modifiers that xkbcommon reports consumed by producing that symbol. For example, `Shift+=` may be `plus` with no symbolic modifiers; `Alt+Shift+=` may be `plus` with `Alt` remaining.

The significant modifier families are Shift, Ctrl, Alt, and Super/Logo. Their XKB indices are cached from the keymap during initialization and recomputed atomically when a replacement keymap is committed. Missing indices remain absent. CapsLock, NumLock, and ScrollLock may affect translation but never become explicit binding modifier bits. AltGr can affect translation and consumption without adding a Typhon modifier family.

## Binding semantics and arbitration

`BindingInput::KeySym(BindingKeySym)` stores a numeric keysym in the compiled table; matching performs no string parsing or keysym-name lookup. K3B does not convert existing default definitions from `PhysicalKey` to `KeySym`.

Physical candidates use K1's aggregate physical `ModifierMask`. Symbolic candidates use the snapshot's XKB-derived modifiers. One keyboard transition may search at most one physical bucket, one raw `KeySym` bucket, and one translated `KeySym` bucket. The eligible result with the latest definition order wins across all three. Repeat policy and shortcut inhibition are evaluated per candidate, so an ineligible later definition does not shadow an earlier eligible one. A binding found through both symbolic identities executes once.

## Press identity and release

`NativeInputState` keeps a fixed-size symbolic press ledger indexed by logical evdev key code. It captures a snapshot only on K1's aggregate `0→1` transition. A second keyboard's duplicate press cannot replace it; non-final release and non-final source removal preserve it. The final logical release consumes the saved snapshot for release matching and then clears it. Final source removal follows the same route and uses the original press identity.

Ordinary releases do not query XKB. A key pressed under one layout or lock state keeps that identity until the final release, even if the active layout or lock state changes while it is held. Session and VT ownership clears discard the symbolic ledger with K1 pressed ownership, without synthesizing symbolic release actions.

## Repeat

K2 still owns one repeat target by exact `BindingId`, trigger key, and aggregate physical modifier snapshot. A symbolic repeat does not rematch another candidate. When due, after hardware effects have updated authoritative XKB and only when the existing backlog and generation checks allow service, the runtime requests a fresh snapshot for that target's physical trigger code. The exact binding continues only if its `KeySym` and symbolic modifiers still match raw or translated identity. A layout or lock-state change that invalidates it cancels the repeat. Physical repeat targets perform no XKB query.

## Performance and scope

Translation uses cached modifier indices, borrowed level-zero keysym slices, xkbcommon's single-symbol and consumed-modifier APIs, and fixed-size copyable values. Matching uses up to three indexed bucket searches and fixed local candidates. The bridge adds no hot-path strings, heap-backed candidate list, keymap compilation, XKB state clone, lock, timer, thread, channel, or work domain.

K3B adds only the symbolic mechanism. It adds no shortcut parser or configuration UI, runtime binding reload, media/system defaults, XF86 actions, keyboard LEDs, virtual keyboard, text-input or IME support, EIS/libei, or Shell integration. K4A now adds capability-gated XF86 system-action bindings through this symbolic path, while production capabilities remain empty and no executor is present. K4B owns the future session-service capability handshake and action dispatch.
