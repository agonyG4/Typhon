# Typhon System Action Transport — K4B1

K4B1 carries typed K4A system-action intents from Typhon to a future Astrea
session service. Typhon remains the authority for keyboard ownership, XKB
translation, and shortcut arbitration. It does not execute audio, media,
brightness, touchpad, or presentation operations.

## Ownership and endpoint

Typhon is the local socket server. The future `astrea-sessiond` is the client
and owns discovery, reconnect, backoff, and its own service lifecycle. Typhon
does not start or supervise that service and does not poll for it.

Each compositor instance exposes:

```text
$XDG_RUNTIME_DIR/astrea/typhon/<instance>/system-actions.sock
```

The instance name is the same identity used by the native control endpoint.
The path setup reuses its runtime-directory security checks: the runtime path
must be absolute and owned by the effective UID, unsafe writable ancestors are
rejected, Astrea/Typhon directories are private to that UID, and the socket is
created with mode `0600`. Existing socket paths are removed only after their
type, owner, mode, and inode identity have been checked. Unix path length is
validated before bind.

The endpoint uses `AF_UNIX`, `SOCK_SEQPACKET`, `SOCK_NONBLOCK`, and
`SOCK_CLOEXEC`. Packet boundaries make the small protocol messages atomic and
remove stream framing state. The native reactor services the socket without
blocking input processing.

After `accept4`, Typhon checks `SO_PEERCRED` and accepts only a peer whose UID
matches `geteuid()`. At most one peer can own or negotiate the executor slot.
A second peer receives a best-effort `Reject(Busy)` and is closed. An
incomplete handshake releases the slot after two seconds using the existing
shared absolute-deadline/timerfd authority.

## Protocol v1

The fixed packet size is 32 bytes. The codec writes fields explicitly in
little-endian order; it never serializes Rust enum layout or names. Packets
start with the `ASTY` magic and a message kind. Reserved fields must be zero.
Decode rejects wrong sizes, magic, kinds, versions, action codes, and zero
occurrence counts without panicking.

| Message | Direction | Payload |
| --- | --- | --- |
| `Hello` | session service → Typhon | major, minor, advertised capability bits |
| `Welcome` | Typhon → session service | negotiated version and accepted known capability bits |
| `CapabilitiesChanged` | session service → Typhon | replacement capability bits |
| `Action` | Typhon → session service | sequence, stable action code, occurrence count |
| `Reject` | Typhon → session service | reason code |

Protocol major is 1 and minor is 0. A major mismatch is rejected. For a
matching major, Typhon negotiates the lower minor. Unknown future capability
bits are truncated, so they cannot enable unknown bindings. Action wire codes
are explicit stable protocol values and do not depend on enum declaration
order. Reject reason codes used by Typhon are 1 for major mismatch, 2 for a
busy executor slot, and 3 for a protocol violation.

Typhon masks the Hello capabilities to actions it knows, then sends Welcome.
The binding manager is the sole active capability authority and receives
exactly that accepted mask only after the full Welcome packet has been
successfully sent. The transport keeps only a handshake-pending mask until
then; it does not maintain a second active eligibility mask. If Welcome would
block, its packet remains pending and `EPOLLOUT` is enabled until it is sent. Malformed
handshakes, version mismatch, EOF, HUP, timeout, and terminal I/O errors close
the peer and leave capabilities empty.

## Dynamic capabilities and input ordering

The peer's `CapabilitiesChanged` mask replaces the previous mask; it is not
an add/remove delta. Typhon applies it before routing physical keyboard input
and before servicing a due K2 repeat in the same native cycle. This ensures a
same-cycle disconnect makes an XF86 binding ineligible before the key is
matched. Capabilities do not rebuild the immutable compiled binding table.

`NativeInputState` updates `AstreaBindingManager` eligibility. If the exact
active repeat binding is a system action whose capability was removed, the
repeat is cancelled and its deadline disappears. Adding an unrelated
capability leaves the repeat intact. The matcher does not rematch or retarget
the repeat.

The K1/K3 forwarding ledger preserves ownership across capability changes. A
key press forwarded to a client remains client-owned through release even if a
capability appears while it is held. A press consumed as a system action stays
compositor-owned through release if the capability disappears. Typhon never
invents a client press or release to reconcile the change.

Transport activity is its own native work domain. A transport-only socket
wake services the peer, updates capabilities or flushes output, rearms the
shared deadline, and returns to the reactor. It does not dispatch Wayland
reads, request rendering, notify user activity, or publish through the Astrea
shortcut protocol. The peer remains serviced while native output/session
ownership is inactive.

## Bounded outbound delivery

Actions are fire-and-forget intents. The compositor does not wait for an
acknowledgement or for an executor result. With an empty queue Typhon attempts
one nonblocking `send` with `MSG_NOSIGNAL`. `EAGAIN` admits the action to a
fixed 32-record queue and enables `EPOLLOUT`; the peer normally watches only
readability and terminal events. `EPOLLOUT` is removed as soon as Welcome and
queued actions are drained.

At most 16 inbound packets, four accepts, and 16 outbound action packets are
processed per native cycle. The reactor is level-triggered, so unread packets
and remaining output stay ready for a later cycle without a polling timer.
Output records coalesce only adjacent identical volume or brightness step
actions. The record carries an occurrence count. Toggles and media transport
actions stay as individual ordered records, and coalescing never crosses an
action barrier.

If a queue is full, sequence space is exhausted, reactor interest cannot be
updated, or a terminal send error occurs, Typhon disconnects that peer,
drops queued intents, clears capabilities, and cancels any affected repeat.
Actions are never replayed after a later connection. A capability withdrawal
also removes queued actions that the service no longer supports, preserving
the order of retained records.

## Lifecycle and diagnostics

Listener setup is optional: failure is logged and compositor startup
continues with empty capabilities. Peer HUP, malformed input, handshake
timeout, queue overflow, and send failures use one disconnect path that
unregisters the reactor token, closes the FD, drops queued output, clears
capabilities, and releases the handshake deadline. Shutdown clears
capabilities and repeat ownership, unregisters and closes the peer/listener,
drops queued actions, and removes the socket only if it still has the inode
Typhon created.

Transport counters track accepts, rejections, handshake outcomes,
disconnects, capability changes, sent action records and occurrences,
coalescing, would-block sends, queue overflow, and protocol errors. A compact
transport snapshot reports whether a peer is connected/ready and its outbound
queue depth. Runtime diagnostics pair that with the accepted capability count
from the input binding manager. No per-repeat action logging is required.

The steady-state path uses one reactor FD for an idle connected peer. There is
no reconnect polling, second timerfd, thread, async runtime, D-Bus call,
filesystem access, or unbounded backlog. A successful action delivery uses a
fixed packet and nonblocking send.

## K4B2 boundary

K4B1 adds no executor. A future `astrea-sessiond` will connect and reconnect,
advertise capabilities, receive typed actions, and route them to audio,
MPRIS, display brightness, keyboard brightness, and input-device policy
executors. It will own actual service/backend failures and may use the
project's session-bus infrastructure outside Typhon's input path.

The future state/OSD flow is direct from `astrea-sessiond` to the Shell. Typhon
sends intent only; it does not relay executor state or render an OSD. K4B1
adds no PipeWire, PulseAudio, WirePlumber, MPRIS, brightness, touchpad,
power/session, Shell, or OSD implementation.
