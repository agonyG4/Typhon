# Lifecycle Pre-Capture Ownership Design

## Goal

Keep the exact admission-time lifecycle visual and its DMA-BUF release obligations valid until the renderer captures an owned lifecycle texture or the lifecycle chain terminates.

## Existing failure

Fresh admission stores immutable lifecycle metadata and a retained presentation topology, but first-render source resolution still projects from live renderable or minimized surfaces and the live presentation-generation map. XWayland withdrawal removes those live sources and generations before the first render, so the payload survives while its source does not.

## Ownership model

`RetainedLifecyclePayloadStore` remains the single authority keyed by `PresentationRetainedVisualIdentity`. Each value becomes one retained lifecycle entry containing the existing immutable semantic payload and an optional pre-capture lease. The lease owns the snapshot-matched `RenderableSurface` sources and the exact DMA-BUF release obligations transferred from `active_dmabuf_buffers`. Reversal transfers the complete entry under the new presentation identity and preserves its payload ID.

Fresh admission captures the topology, source contents, and source ordering together. It validates each DMA-BUF surface against the exact current buffer ID and release token, rejects conflicts or duplicate tokens, then moves those tokens into the entry only when admission commits. Failure before commit leaves active ownership intact; a partial transfer is rolled back. SHM snapshots need no client release token.

The retained topology gains an exact-source projection that validates frozen surface keys and order, reapplies captured geometry, and never consults live generations. The renderer source provider uses the lease while it exists. Once the renderer reports successful capture for a payload in a rendered frame, Typhon drops the frozen surfaces and transfers held release obligations into that exact compositor frame batch before the existing GPU-release fence is armed.

Lifecycle retirement is centralized at the compositor state boundary. An uncaptured lease is restored to its live owner when the same buffer remains current; otherwise its obligations enter the canonical pending release path. Render fallback is settled only after the existing Atomic EGL/GBM `gl.finish()` safety proof. Shutdown drains remaining lifecycle-held obligations through the existing release machinery. No lifecycle-only fence or success-path stall is introduced.

## Scope and compatibility

This change is limited to Typhon lifecycle source ownership, exact retained projection, compositor frame-batch release transfer, and focused diagnostics/tests. It preserves `LifecycleVisualStore`, Lamp/Squash sampling, reversal identity, settled XWayland restore, partial damage, and Direct Scanout blocking behavior. It does not change Eclipse or ordinary scene visibility.

## Verification

Tests must reproduce admission followed by XWayland association removal before first capture, including live generation removal, buffer replacement, DMA-BUF token transfer, successful capture, fallback, reversal, destruction, SHM, multiple subsurfaces, and shutdown. The renderer integration regression must prove the exact frozen source reaches `LifecycleVisualStore`. Live visual qualification is required before calling the runtime defect fixed.
