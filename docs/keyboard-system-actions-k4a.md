# Typhon Keyboard K4A — Typed System Actions

K4A gives common hardware and media shortcuts a typed meaning. It ends at an action intent: Typhon does not perform the audio, media, brightness, or input-device operation.

## Ownership and action vocabulary

Typhon owns physical input, authoritative XKB translation, shortcut arbitration, and the typed action intent. A future Astrea session service owns audio, MPRIS, brightness, and input-device policy. The Shell owns presentation such as an on-screen display.

`AstreaSystemAction` is a compact enum in `src/system_action.rs`:

- Audio: `OutputVolumeUp`, `OutputVolumeDown`, `ToggleOutputMute`, `ToggleMicrophoneMute`.
- Media: `MediaPlayPause`, `MediaPlay`, `MediaPause`, `MediaStop`, `MediaNext`, `MediaPrevious`, `MediaRewind`, `MediaFastForward`.
- Brightness: `DisplayBrightnessUp`, `DisplayBrightnessDown`, `KeyboardBrightnessUp`, `KeyboardBrightnessDown`, `ToggleKeyboardBacklight`.
- Input-device policy: `ToggleTouchpad`.

The vocabulary intentionally excludes power and session lifecycle actions, screenshots, launchers, workspace actions, and other Shell or compositor behavior. Actions are enum values, not strings, commands, or Shell shortcut protocol messages.

## Capabilities and eligibility

`AstreaSystemActionCapabilities` is a fixed-width bit mask. Each action has a distinct bit, and a compile-time width assertion plus unit tests protect the representation. A capability says that a live executor understands an action; it does not promise that the operation will succeed on the current hardware or that a media player is active.

K4A production constructors use `EMPTY`. Thus the compiled XF86 defaults are ineligible until K4B1 connects a validated session-service peer and completes the capability handshake. A candidate is eligible only when its normal trigger, repeat, and inhibition rules pass and, for a system action, the manager's current capability mask contains that action. K4B1 replaces this mask on service capability updates and clears it on disconnect; it does not rebuild the compiled binding table.

Capability is an eligibility filter, not a shadow. If a later system-action binding is unsupported, matching continues to an earlier eligible physical or symbolic binding. If nothing else matches, the event takes the normal K1/K3B client-forwarding path. The unsupported action is not consumed as a no-op.

The compiled binding table remains immutable. Compilation derives a small `Always` or `SystemAction(action)` availability tag from each cold action definition. The matcher checks this tag and, only for system actions, one capability bit. K3A's indexed lookup and global latest-eligible-definition ordering remain in force across physical, raw KeySym, and translated KeySym candidates.

## XF86 defaults

The defaults use K3B `BindingInput::KeySym` values from xkbcommon and have no Linux physical-key fallback. Each is a reserved, unmodified press binding with `InhibitionPolicy::Bypass`:

| XF86 keysym | Typed action | Repeat |
| --- | --- | --- |
| `XF86AudioRaiseVolume` | `OutputVolumeUp` | Enabled |
| `XF86AudioLowerVolume` | `OutputVolumeDown` | Enabled |
| `XF86AudioMute` | `ToggleOutputMute` | Disabled |
| `XF86AudioMicMute` | `ToggleMicrophoneMute` | Disabled |
| `XF86AudioPlay` | `MediaPlayPause` | Disabled |
| `XF86AudioPause` | `MediaPause` | Disabled |
| `XF86AudioStop` | `MediaStop` | Disabled |
| `XF86AudioNext` | `MediaNext` | Disabled |
| `XF86AudioPrev` | `MediaPrevious` | Disabled |
| `XF86AudioRewind` | `MediaRewind` | Disabled |
| `XF86AudioForward` | `MediaFastForward` | Disabled |
| `XF86MonBrightnessUp` | `DisplayBrightnessUp` | Enabled |
| `XF86MonBrightnessDown` | `DisplayBrightnessDown` | Enabled |
| `XF86KbdBrightnessUp` | `KeyboardBrightnessUp` | Enabled |
| `XF86KbdBrightnessDown` | `KeyboardBrightnessDown` | Enabled |
| `XF86KbdLightOnOff` | `ToggleKeyboardBacklight` | Disabled |
| `XF86TouchpadToggle` | `ToggleTouchpad` | Disabled |

Step adjustments repeat through K2. Toggles and media transport actions do not. The defaults bypass ordinary shortcut inhibition, but capability gating still applies independently. K4A does not define fine-step modifier variants.

## K3B and K2 integration

XF86 keys use K3B's authoritative pre-update XKB snapshot and the same raw/translated `KeySym` candidate lookup as other symbolic bindings. If both identities find the same binding, it is considered once. System actions do not get a separate lookup tier or priority.

K2 retains the exact `BindingId` selected at press time. Repeat validation checks that exact binding's capability, physical held-key/modifier ownership, inhibition policy, and fresh symbolic identity when the trigger is a `KeySym`; it does not rematch or retarget. Physical repeat retains its physical modifier snapshot.

## No executor in K4A/K4B1

K4A defined the typed invocation without dispatching it. K4B1 now carries an eligible invocation to the session service over a nonblocking local socket, but it still does not execute the action. Before a validated peer completes Welcome, capabilities are empty and the XF86 defaults fall through normally. Afterward, only advertised actions are eligible and are sent as typed intents. Typhon makes no D-Bus call, spawns no command, sends no Shell shortcut, publishes no action over Wayland, and requests no rendering because of the system action.

Unsupported actions do not fall back to `wpctl`, `pactl`, `playerctl`, `brightnessctl`, or another command. They remain ineligible and normal binding/client fallback continues. K4A also adds no PipeWire, PulseAudio, WirePlumber, MPRIS, brightness writes, touchpad mutation, power actions, OSD, keyboard LEDs, virtual keyboard, IME, or Shell integration.

The hot path adds one compact availability check and, for a system action, one bit test. It adds no action-name strings, heap allocation, lock, channel, thread, service proxy, or backend lookup.

## K4B boundary

K4B1 adds the nonblocking transport and dynamic capability lifecycle for typed `AstreaSystemAction` values. It does not execute them. K4B2 may add the `astrea-sessiond` executors for audio backends, MPRIS, display and keyboard brightness, and input-device policy. Typhon continues to own shortcut intent only; it does not perform those operations.
