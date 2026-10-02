# Typhon Presentation Modes v1

Typhon implements presentation metadata as part of the Wayland surface commit
transaction. The protocol-facing state and the output-facing decision are
separate on purpose:

```text
client request
    -> pending wl_surface metadata
    -> wl_surface.commit capture
    -> synchronized-subtree latch
    -> sampled fullscreen-tree policy
    -> frozen OutputTransaction mode/content
    -> KMS TEST_ONLY (when required)
    -> one real page-flip submission
```

## Protocol state

`wp_tearing_control_manager_v1` creates at most one
`wp_tearing_control_v1` object for a surface. Its `set_presentation_hint`
request is double buffered: `vsync` and `async` are pending until the next
surface commit. Destroying the object reverts only the pending hint; the
currently latched hint remains valid until a later commit. A surface destroy
retires the associated metadata and makes later protocol requests inert.

`wp_content_type_manager_v1` and `wp_content_type_v1` follow the same ownership
and double-buffering rules. The protocol values map to connector Content Type
values as follows:

Presentation hint and Content Type are persistent committed surface state.
Capturing a surface commit does not reset the next pending baseline to the
currently active metadata. Only an explicit protocol mutation changes that
pending value. A compositor-local pending mutation generation records whether a
newer explicit request arrived after an older commit was captured, so activating
that older commit cannot overwrite the newer pending request.

| Wayland value | DRM value |
| --- | --- |
| `none` | `Graphics` |
| `photo` | `Photo` |
| `video` | `Cinema` |
| `game` | `Game` |

Content Type is output metadata. `game` does not request tearing by itself.
Hints and content type are sampled from the latched synchronized surface tree,
so a child surface can contribute an `async` request only while it is visible
in the sampled tree.

## Policy and effective mode

The native tearing policy is `OBLIVION_ONE_TEARING=off|auto`, defaulting to
`off`. Unknown values also resolve to `off`. Adaptive Sync has its separate
compatibility setting `OBLIVION_ONE_VRR=off|auto|on`, defaulting to `auto`.
These settings describe policy; neither one is proof that KMS accepted a
request or that a monitor varied its refresh rate.

The output transaction freezes one of four effective modes:

| Mode | VRR requested | Async page flip | Presentation domain | Phase 1 pacing |
| --- | --- | --- | --- | --- |
| `Vsync` | no | no | `FixedVsync` | existing selection |
| `AdaptiveSync` | yes | no | `VrrWindow` | `ReactiveDouble` |
| `Async` | no | yes | `AsyncImmediate` | `ReactiveDouble` |
| `AdaptiveAsync` | yes | yes | `VrrWindow` | `ReactiveDouble` |

Async remains independently qualified by the tearing policy, surface hint,
fullscreen state, cursor and plane state, synchronization readiness, commit
timing, KMS lane, format support, and exact TEST_ONLY result. Adaptive Sync is
qualified independently: `Off` never requests it, `Auto` requires the
compositor's solitary-fullscreen candidate, and `On` requests it whenever the
atomic output path and both DRM properties are capable. Both policies remain
subject to exact KMS qualification and safe transaction state. Content Type
such as `Game` does not activate VRR.

`OutputPresentationMode` is the transaction's single presentation authority;
DRM content type remains separate metadata. All three non-VSync modes force
`ReactiveDouble` in Phase 1, so Adaptive Sync does not enter Predictive Triple
render-ahead. A mode may be replaced only before the transaction transfers to
KMS ownership.

## KMS contract

Atomic `Vsync` and `AdaptiveSync` commits use `NONBLOCK | PAGE_FLIP_EVENT`.
`Async` and `AdaptiveAsync` add `PAGE_FLIP_ASYNC`; VRR by itself never requests
tearing. TEST_ONLY and real requests program the same Content Type and
`VRR_ENABLED` values. Adaptive Async TEST_ONLY uses `TEST_ONLY |
PAGE_FLIP_ASYNC`; all steady-state presentation commits omit `ALLOW_MODESET`.
Legacy KMS may qualify Async through its existing path, but it is never treated
as VRR capable.

Atomic VRR capability requires connector `vrr_capable` to exist and be nonzero,
CRTC `VRR_ENABLED` to exist, and Typhon to use its Atomic backend. The live DRM
atomic connector property is authoritative; sysfs is diagnostic only. The
initial output state explicitly disables VRR while the discovery snapshot
retains the original CRTC value for exact shutdown/session restore. A VRR
transition that fails the no-modeset TEST_ONLY check falls back before submit;
Typhon does not add `ALLOW_MODESET` to a normal presentation commit. Any future
modeset transition needs a separate full-state, TEST_ONLY-validated path.

For composited Async, render-fence readiness is checked nonblocking from the
event loop before submission and the primary `IN_FENCE_FD` is omitted. VSync
retains the existing explicit-sync path. Async cursor-only or cursor-mutating
submissions are rejected, and a visible or transitioning cursor blocks Async
eligibility.

The atomic connector Content Type property is optional. Its absence does not
disable the protocol, but no property programming is emitted. When present,
the initial value is captured in `AtomicPipelineSnapshot` and restored during
shutdown/recovery. Async is not combined with an unrelated connector metadata
transition.

## Feedback and FIFO

VSync and Adaptive Sync feedback retain `Kind::Vsync`; `Async` and
`AdaptiveAsync` feedback are tearing and do not set that flag. Variable-refresh
feedback reports `refresh = 0`, since the next physical interval is not known.
Direct Scanout adds the existing zero-copy flag. A completed tearing
presentation does not clear a FIFO barrier. A later valid non-tearing latch or
surface teardown is responsible for retiring that barrier.

Direct Scanout validation includes presentation mode and content type. A
composited Async candidate must be present in the driver’s `IN_FORMATS_ASYNC`
set for the selected framebuffer format/modifier; absence of that exact
qualification keeps the frame on VSync. The TEST_ONLY result is cached only
for the exact output generation, CRTC, primary plane, format/modifier, acquire
strategy, cursor state, and content type that were tested.

## Phase 1 qualification boundary

Phase 1 carries the four-mode transaction state through policy, exact KMS
validation/submission, matching pageflip confirmation, feedback, and recovery.
Confirmation means the pageflip completed for a transaction that requested
`VRR_ENABLED`; it does not establish that the monitor physically varied on
that frame. Direct Scanout and composited validation include the presentation
mode, so their proofs cannot alias across modes or output generations.

The existing fixed-refresh `PresentationDeadlinePlanner` and Predictive O1
physical opportunity model remain unchanged. Adaptive presentations use
conservative `ReactiveDouble` pacing. Phase 2 owns phase-free `VrrWindow`
scheduling, VRR range/min-refresh handling, overlay coalescing, cursor timing
optimization, anti-flicker cadence ownership, and VRR-specific late rendering.
