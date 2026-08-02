# Typhon Control Plane

Typhon exposes one local control socket for each Wayland instance:

```text
$XDG_RUNTIME_DIR/astrea/typhon/<instance>/control.sock
```

The instance is the Typhon Wayland socket name. The `astrea`, `typhon`, and
instance directories are created with mode `0700`; the socket is created with
mode `0600`. `XDG_RUNTIME_DIR` must be an absolute directory owned by the
compositor user and must not be group- or world-writable.

The native runtime owns the listener and every client registration through its
generation-safe epoll reactor. Socket creation, reads, writes, and accepted
connections use nonblocking close-on-exec descriptors. The server authenticates
each accepted stream with `SO_PEERCRED` and requires the peer UID to equal the
compositor effective UID. PID names, executable paths, claimed JSON fields, and
filesystem ownership are not authentication mechanisms.

Protocol version 1 accepts one bounded NDJSON request per connection. Requests
are limited to 64 KiB, responses to 1 MiB, simultaneous clients to 32, and
control operations to 16 per compositor cycle. Partial reads and writes,
hangups, resets, malformed messages, slow peers, and stale reactor tokens are
client-local failures. They do not terminate the compositor. The reactor
continues to service DRM, timer, input, Wayland, explicit-sync, XWayland,
child, KMS-worker, and render-fence sources normally.

When an existing socket is found, Typhon refuses symlinks and non-socket
objects. A same-UID live listener is an address-in-use failure. A same-UID
socket is removed only after a nonblocking probe returns a definitive dead
listener result, and the path is revalidated immediately before removal. On
shutdown Typhon unregisters the listener and all client tokens, closes their
descriptors, and removes only the socket inode created by that server instance;
the parent runtime directories remain.

M2 provides transport and reactor infrastructure only. Requests currently
receive a bounded `invalid_command` response: known command semantics are
installed by M3, and unknown commands are rejected. No compositor snapshots,
cursor changes, wallpaper commands, window actions, or shell protocol are
implemented by this milestone.

For a live socket conflict, inspect the instance-specific path above and stop
the owning Typhon instance before starting another one. An insecure or missing
`XDG_RUNTIME_DIR` is a bootstrap error; Typhon does not chmod or replace the
user's runtime directory.
