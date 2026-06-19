# DRM Presentation Metadata Design

Date: 2026-06-19

## Objective

Make the kernel legacy pageflip event the authoritative timestamp, sequence,
and completion token source for native asynchronous `wp_presentation`
feedback. Timers and compositor counters must never synthesize native DRM
completion metadata.

## Architecture

Add `src/native/drm.rs` as the boundary around the legacy DRM event ABI. It
will provide:

- a typed `DrmPresentationEvent` containing the original `u32` seconds,
  microseconds, sequence, CRTC ID, and `u64` user-data token;
- length-validated parsing of complete DRM event buffers;
- DRM timestamp clock capability detection;
- legacy pageflip submission with a caller-provided token.

The parser will use the `repr(C)` DRM UAPI structures supplied by `drm-sys`
and unaligned reads only after validating the declared event length. Unknown
events will be skipped by their validated length. Zero, undersized, oversized,
or truncated event lengths will return a structured parse error.

Linux documents that a DRM FD read returns only complete events. The runtime
will therefore parse each read independently, while looping until `EAGAIN` and
retrying `EINTR`. A parser test may still inject truncated input and must receive
an error without an out-of-bounds read.

## Clock Policy

Native setup will query `DRM_CAP_TIMESTAMP_MONOTONIC` before dispatching any
Wayland clients:

- capability value `1`: advertise `CLOCK_MONOTONIC`;
- capability value `0`: advertise `CLOCK_REALTIME`;
- capability query failure: return a clear native startup error rather than
  advertise an uncertain clock domain.

The selected clock ID is stored in compositor state and sent when a client
binds `wp_presentation`. DRM timestamps are never translated between clock
domains. Compositor receive time is sampled separately in the same selected
domain only for optional latency diagnostics.

## Submission And Matching

`NativePageFlipState` will replace its boolean with a pending submission record
and a wrapping token allocator. Tokens are nonzero `u64` values and are passed
as DRM pageflip user data through a local legacy pageflip ioctl wrapper.

Submission reserves one token before the ioctl. Failure cancels only that
reservation. Completion consumes pending state only when the event token
matches. A mismatched or stale event does not promote buffers, clear scheduler
state, or call `finish_frame`. Duplicate events after a successful completion
are stale and cannot finish twice.

The scheduler stores the same pending token. `note_page_flip_completion`
requires a matching token before leaving pageflip-pending state. Sequence wrap
does not participate in matching.

## Frame Completion

Introduce a typed compositor presentation payload containing:

- clock domain;
- seconds and nanoseconds;
- protocol sequence;
- conservative presentation flags;
- metadata origin.

Native DRM completion converts `tv_usec` to nanoseconds with checked validation
that microseconds are below `1_000_000`. Kernel sequence is delivered as a
finite-width value with protocol sequence high `0` and low equal to the kernel
`u32`; wrap is preserved naturally and no unbounded monotonic sequence is
invented.

`OwnCompositorServer::finish_frame` receives this payload and forwards it to
pending presentation feedback while preserving existing buffer-release and
frame-callback ordering. Native synchronized legacy pageflips report `VSYNC`
only. `HW_CLOCK`, `HW_COMPLETION`, and `ZERO_COPY` are omitted because the
current path does not establish those semantics conservatively.

Immediate and protocol-only completion use an explicitly non-DRM payload with
the advertised compositor clock, sequence zero, and no hardware flags. They do
not construct `DrmPresentationEvent` values.

## Watchdog And Errors

The watchdog remains failure detection. Its final nonblocking drain may produce
a real matching `DrmPresentationEvent`, which follows normal completion. If it
does not, the runtime logs and returns the existing fatal error without sending
presentation feedback.

Malformed input returns an I/O error. Unknown valid events are ignored. Token
mismatch, stale, and duplicate events are counted and logged only through the
existing performance mode to avoid unconditional frame-volume output.

## Testing

Tests will be added in this order:

1. DRM parser cases for valid, multiple, unknown, malformed, boundary timestamp,
   maximum sequence, and token preservation.
2. Timestamp conversion and seconds high/low protocol encoding.
3. Pending-token lifecycle, mismatch, duplicate, stale event, wrap, watchdog,
   and immediate completion behavior.
4. Wayland feedback tests asserting injected timestamp, sequence, clock ID,
   flags, and exactly-once delivery.
5. Existing native scheduler, pageflip, buffer, explicit-sync, resize, cursor,
   and pointer-lock regressions plus all workspace quality gates.

Hardware validation will be reported only if a native DRM session is actually
available and exercised.

## Non-Goals

This design does not add atomic KMS, VRR, direct scanout, planes, hotplug,
multi-output, eventfd-backed syncobj waits, renderer changes, or visual changes.
