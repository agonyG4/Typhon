# Phase 1 Plan: Truthful GPU Protocol Publication

Date: 2026-07-17

Design: docs/superpowers/specs/2026-07-17-xwayland-production-hardening-design.md

## Goal

Make GPU protocol publication derive from one immutable, native-bootstrap
GpuProtocolCapabilities record. Publish zwp_linux_dmabuf_v1,
wp_linux_drm_syncobj_manager_v1, and wl_drm independently and only when the
complete advertised contract is implemented and tested.

The phase must preserve the existing displayfd readiness fix, private client
authorization, xwayland-shell-v1 association, deferred XWayland lease teardown,
and generation boundaries. It does not change XWM startup, window mapping,
ICCCM/EWMH, selections, or Xdnd.

## Contract

The immutable record will expose:

- selected KMS device and dmabuf/import device identity;
- canonical render-node path and character-device/openability evidence;
- dmabuf publication version: none, v1, v3, or v4;
- syncobj timeline support for the selected dmabuf/render device;
- wl_drm enablement, device path, PRIME support, and disable reasons;
- validated format/modifier evidence and structured diagnostics.

wl_drm requires a canonical character render node, successful O_RDWR
openability, matching device identity with the dmabuf importer, PRIME support,
and a bind-time nonempty device event. Its Authenticate request is rejected
unless real DRM-magic authentication is available for the selected master.

dmabuf v4 requires a valid nonzero main device, nonempty valid format table,
valid tranche indices, and importer support for the advertised pairs. v3
requires valid modifier events and the importer. v1 requires validated basic
import. Otherwise the global is omitted.

Syncobj is enabled only when the selected dmabuf/render device itself supports
timeline creation and import. No global device scan may select it.

## Files

Implementation:

- src/compositor/gpu_protocol_capabilities.rs: immutable record, probe
  evidence, contract evaluator, diagnostics, and test fixtures.
- src/compositor/mod.rs: module registration and internal exports.
- src/compositor/server.rs: store the record and register only the selected
  globals; make native enablement accept the record.
- src/compositor/state/surfaces.rs: remove broad default device selection and
  apply the immutable feedback/device fields.
- src/compositor/dmabuf.rs: validate feedback/table/tranche data, emit only
  truthful legacy events, and gate wl_drm capabilities.
- src/compositor/protocols/globals.rs: make dmabuf bind behavior follow the
  selected protocol version.
- src/compositor/protocols/buffers.rs: reject unsupported wl_drm
  authentication and invalid feedback/import contracts.
- src/native_output/runtime/bootstrap.rs: create the record after the native
  scanout/importer is selected and before GPU globals are enabled.
- src/native_output/output/bootstrap.rs: expose only the selected
  card/render-node evidence required by the native probe.
- src/syncobj.rs: make selected-device timeline validation reusable without
  global device search.
- src/compositor/tests/lifecycle.rs: update registry expectations and test
  independent global publication.
- src/compositor/tests/protocol_buffers.rs: test valid and invalid dmabuf
  feedback and wl_drm authentication behavior.
- src/native_output/tests/output.rs: retain and extend same-device render-node
  selection tests.
- tests/xwayland_gpu_startup.rs: integration tests for capability evidence and
  installed-XWayland displayfd/GPU startup gating.

## Ordered tasks

### 1. Add failing contract tests

First add unit tests for the evaluator covering:

1. missing render path;
2. empty render path;
3. nonexistent path;
4. regular file;
5. inaccessible render node;
6. KMS card path without render node;
7. valid character render node;
8. render node/device mismatch;
9. dmabuf main-device mismatch;
10. missing or empty v4 feedback;
11. invalid format-table/tranche evidence;
12. v4, v3, v1, and no-contract selection;
13. syncobj support on the selected device versus unrelated global devices;
14. wl_drm enablement and explicit disable reasons;
15. wl_drm never acknowledges unsupported authentication.

Add registry tests proving each global is independently absent or present from
the record. Add a real installed-XWayland test that is skipped with a recorded
reason when no native DRM session or XWayland binary is available; when enabled,
it must require managed Running, displayfd readiness, successful xdpyinfo,
and no XWAYLAND_NO_GLAMOR.

Run the focused tests and observe the new tests fail for the current broad
registration behavior.

### 2. Implement the immutable capability record

Implement GpuProtocolCapabilities and a separate probe-evidence constructor.
Keep filesystem and DRM probing at native bootstrap; keep the evaluator pure
and deterministic for tests. Store disable reasons as stable structured values,
not only log strings.

Ensure the record is created once after the actual native scanout/importer is
selected. The record carries the selected device identity through to compositor
state and global registration.

### 3. Bind global registration to the record

Replace boolean GPU registration with record-based registration:

- dmabuf global version is the record's complete supported version;
- syncobj global exists only when the record says the selected device supports
  timeline create/import;
- wl_drm global exists only when its full contract is true;
- a bind sends a nonempty device event before any optional event;
- unsupported Authenticate requests produce the protocol error supported by the
  generated wl_drm interface, never unconditional authenticated.

Keep test-only synthetic contracts explicit and isolated from native bootstrap.
Production paths with no validated record publish no GPU globals. Do not
re-enable wl_drm merely because a card node exists.

### 4. Validate feedback and import symmetry

Update dmabuf v4 feedback generation so main device, format table, tranche
indices, target devices, and done are all valid for the selected record.
Update legacy v1/v3 format/modifier events to advertise only pairs accepted by
the importer. Remove the hard-coded default device search from production
publication.

Tie syncobj state to the same selected device used for dmabuf import/export.
Keep software/CPU scanout able to run with GPU globals omitted.

### 5. Run the phase validation

Run targeted GPU/global tests first, then:

~~~text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
~~~

Verify the worktree contains only intended Phase 1 changes.

### 6. Run the native checkpoint

On the real native DRM/NVIDIA session, run eager startup and record:

- managed XWayland state reaches Running;
- displayfd readiness is observed and validated;
- xdpyinfo succeeds while XWayland remains alive;
- the registry and diagnostics show whether wl_drm is omitted or enabled;
- no launch uses XWAYLAND_NO_GLAMOR;
- no unrelated syncobj device was selected.

If hardware or the installed XWayland binary is unavailable, stop at this
checkpoint and report the exact missing environment. Do not mark Phase 1
complete and do not begin Phase 2.

### 7. Commit

After all automated tests and the native checkpoint pass, commit exactly:

~~~text
fix(xwayland): advertise only usable GPU protocol globals
~~~

The commit must not include Phase 2 or later XWM, mapping, ICCCM/EWMH, socket,
selection, DnD, RandR, cursor, or compatibility-matrix changes.
