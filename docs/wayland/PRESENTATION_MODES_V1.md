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

The native policy is `OBLIVION_ONE_TEARING=off|auto`, defaulting to `off`.
Unknown values also resolve to `off`. There is no force mode. In `auto`, an
Async request is accepted only for a solitary fullscreen presentation after
the compositor has checked the output generation, cursor state, plane use,
explicit synchronization, commit timing, KMS lane, and Async TEST_ONLY
qualification. Ordinary tiled/windowed desktop frames remain VSync.

`OutputTransaction` freezes both `OutputPresentationMode` and DRM content type.
The transaction also forces `ReactiveDouble` pacing for Async so the scheduler
does not use predictive triple buffering for a tearing frame. A mode or content
transition therefore cannot be silently changed after KMS ownership is
transferred.

## KMS contract

Atomic VSync commits use `NONBLOCK | PAGE_FLIP_EVENT`. Atomic Async commits use
those flags plus `PAGE_FLIP_ASYNC`. Async TEST_ONLY uses `TEST_ONLY |
PAGE_FLIP_ASYNC` and never `ALLOW_MODESET`; modesets never use Async. Legacy
page flips have explicit VSync and Async submission modes, with Async enabled
only after the legacy capability is available.

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

VSync hardware feedback is reported as `Kind::Vsync`; Async hardware feedback
is reported as tearing and is never mislabeled as synchronized. Direct Scanout
adds the existing zero-copy flag. A completed Async presentation does not
clear a FIFO barrier. A later valid non-tearing latch or surface teardown is
responsible for retiring that barrier.

Direct Scanout validation includes presentation mode and content type. A
composited Async candidate must be present in the driver’s `IN_FORMATS_ASYNC`
set for the selected framebuffer format/modifier; absence of that exact
qualification keeps the frame on VSync. The TEST_ONLY result is cached only
for the exact output generation, CRTC, primary plane, format/modifier, acquire
strategy, cursor state, and content type that were tested.

## Qualification boundary

The implementation does not enable VRR or write `VRR_ENABLED`. Native tearing
is qualified independently through the page-flip capability, exact Atomic
TEST_ONLY contract, generation-aware validation cache, and real-submit failure
fallback. Hardware qualification remains required before changing the default
policy from `off`.
