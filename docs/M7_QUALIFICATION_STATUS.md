# AstreaOS M7 Qualification Status

This ledger separates deterministic repository qualification from the deferred
native TTY/DRM qualification. `DEFERRED` is not a passing status.

| Milestone | Implementation | Deterministic | Native |
| --- | --- | --- | --- |
| M7-A | PASS | PASS | DEFERRED |
| M7-B | PASS | PASS | DEFERRED |
| M7-C | NOT RUN | NOT RUN | DEFERRED |
| M7-D | NOT RUN | NOT RUN | DEFERRED |

## M7-A

- Typhon HEAD: `df4971958fa9d163d7496425c32f41013f5f7c16`
- Deterministic focused and static gates passed after the final M7-A test
  commits.
- The serial full suite had one isolated, unchanged XWayland startup failure
  under the current host environment; this is recorded as a pre-existing test
  environment issue, not a production regression.
- Native Firefox/Kitty TTY/DRM qualification is deferred.

## M7-B

- Typhon starting HEAD: `83f41594a5196f73900d1e1010918e6455e71e15`
- Typhon deterministic closure HEAD: `57e4908c9eaeaa0d53752db3f35834995a0250b1`
- Protocol XML: `protocols/astrea-toplevel-management-v1.xml`
- Protocol XML SHA-256: `0dd3449fda60b1ed183e330e1589093f3d4f8086be117d9ca4baa81bd6bd47e7`
- Deterministic focused action, lifecycle, managed-X11 protocol-path,
  central-primitive, v1 version-rejection, protocol-contract, clippy, format,
  check, source-layout, serial full-suite, and diff-check validation: PASS.
- Source-layout counts: `windows.rs` 1465 lines, `toplevel_publication.rs` 1487
  lines, `toplevel_actions.rs` 228 lines, and `state/window_actions.rs` 90
  lines; all remain below the existing 1500-line limit.
- Locked serial full-suite result: 1538 passed, 1 ignored, 0 failed, using
  `XDG_RUNTIME_DIR=/run/user/1000` and user-owned `TMPDIR=/tmp/t` so Unix-domain
  test paths stay within `SUN_LEN`.
- Managed-X11 v2 coverage exercises exact activate, minimize, restore, and
  close requests through the Wayland manager path and verifies the existing
  XWM close command; XDG coverage exercises the same four actions and manager
  completion ordering.
- Native qualification: deferred until the integrated M7 gate.

## M7-C

- Eclipse client, AltTab, Dock, build matrix, tests, and copied XML hash:
  pending.
- Native qualification: deferred until the integrated M7 gate.

## M7-D

- Unified shell host, IPC, lifecycle, resource measurements, and stress
  results: pending.
- Native qualification: deferred until the integrated M7 gate.

The final M7 result must not be reported as complete until integrated native
qualification passes. The required interim status is:

```text
M7-A IMPLEMENTATION COMPLETE
M7-B IMPLEMENTATION COMPLETE
DETERMINISTIC QUALIFICATION: PASS
REAL NATIVE QUALIFICATION: DEFERRED
```
