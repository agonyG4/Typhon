# Typhon Keyboard K3A

K3A compiles Typhon's physical keyboard and pointer binding definitions into an immutable, ID-based table. It removes action-payload cloning and full binding scans from the event and repeat paths while preserving the current shortcut behavior. K3B now extends this same compiled table with layout-aware `KeySym` candidates.

## Scope and boundary

K3A itself matches Linux physical evdev key codes and pointer button codes. K3B adds symbolic `KeySym` matching and consumed-modifier handling through read-only snapshots from the compositor's existing XKB state. The compositor's keyboard module remains the authority for XKB state, keymaps, layouts, variants, options, and lock state; native input owns no second `xkb::State`.

## Definitions and compiled table

Cold `BindingSpec` values own the binding definition and any heap-backed action input. Compilation validates and assigns stable-in-table `BindingId` and `BindingActionId` values, retains each definition's ordinal, classifies Alt-Tab sequence effects, and builds the immutable `CompiledBindingTable`.

Compiled bindings contain only match metadata, policies, definition order, sequence metadata, and a `BindingActionId`. They contain no `String` or `Vec<String>`. IDs are compact, copyable internal handles with private representation; lookups check catalog/table bounds before returning an entry. IDs are not exposed through IPC.

## Immutable action catalog

`AstreaBindingManager` owns an immutable catalog for the compiled table's lifetime. The catalog holds cold action payloads such as launch argv and shortcut namespace/name strings. It also stores the Alt-Tab commit shortcut once. Event execution borrows a catalog entry; it does not clone the payload to match or construct an effect.

Session-command environment variables are read and validated while the manager is built. The resulting command is stored in the catalog, so the binding table snapshots those environment values at construction. Missing configuration leaves the binding consuming its shortcut while producing no launch.

## Lookup and precedence

The table sorts exact lookup keys made from trigger, physical key, `KeySym`, or pointer button and modifier mask. Each key maps to a compact candidate range ordered by descending definition ordinal. Matching binary-searches each eligible representation's key and checks only those ranges.

This preserves reverse-definition precedence. The first eligible candidate wins, but an ineligible later candidate does not shadow an earlier eligible one. Inhibition and repeat policy remain candidate eligibility rules; duplicate keys remain valid.

## Action invocation and publication

Binding matches are copyable values containing `BindingId`, `BindingActionId`, phase, repeat policy, and inhibition policy. Effects carry compact action invocations. State-sensitive actions such as pointer move and resize are converted immediately using the current native cursor position. Launch and shortcut payloads remain catalog-owned until the side-effect boundary, where launch argv may be cloned once for process ownership and shortcut strings are borrowed for protocol publication.

Shortcut publication remains protocol-first, followed by the existing fallback behavior, phase handling, and telemetry. The fallback decision is not moved into the matcher.

## Alt-Tab and repeat identity

The compiler marks the exact `astrea-shell` `alt_tab_next` and `alt_tab_previous` definitions with sequence metadata. The hot path updates Alt-Tab sequence state from that enum instead of inspecting strings. Releasing the final Alt modifier returns an invocation for the catalog's prebuilt `alt_tab_commit` entry, preserving one commit per active sequence without allocating shortcut strings.

K2 repeat stores the `BindingId` selected by the initial press. Each due tick checks that exact compiled binding, logical aggregate key state, modifiers, inhibition, and repeat policy. It does not run generic candidate lookup or switch to a lower-priority binding. The existing deadline, generation, hardware-before-repeat, backlog, and no-catch-up rules remain unchanged.

## Performance model

Normal lookup performs a small key construction, binary search, and a short candidate-range check. Matching and effect construction allocate no strings or command vectors, compare no shortcut names, read no session-command environment variables, and scan no full binding list. Pointer bindings use the same compiled IDs and catalog as keyboard bindings.
