# Bind cursor feedback to frozen KMS owner

## Goal

Make hardware client cursor presentation feedback follow the exact cursor
transaction frozen into the submitted KMS bundle for composited and Direct
Scanout primary frames, without changing ordinary scene membership or existing
F13 scheduling and delivery policy.

## Tasks

1. Add RED ownership tests for primary/sidecar replacement, sidecar promotion,
   motion-only replacement, content-changing replacement, same-buffer commits,
   submitted ownership, and terminal paths.
2. Generalize the presentation-only obligation narrowly to physical primary
   transactions, keep compatibility-immediate content excluded, and add exact
   transfer/detach ledger operations.
3. Stop sampling worker-replaceable hardware cursor content into the ordinary
   composited/direct FrameBatch. Capture it on the physical cursor owner,
   including sidecars before worker offer.
4. Rebind or settle the embedded obligation when a submitted worker bundle
   names a sidecar cursor, and replace direct sidecar ledger mutation with
   obligation-aware settlement.
5. Complete embedded and sidecar cursor obligations from the same physical
   pageflip evidence, and cover rejection, return, replan, fatal, shutdown,
   generation, and output teardown paths.
6. Run focused RED/GREEN tests followed by fmt, locked check, locked clippy,
   full locked tests, diff checks, and source-layout observation. Make one
   focused commit after verification.

## Constraints

- Preserve worker late cursor replacement and all existing F13 behavior.
- Do not make ordinary primary scene membership late-bindable.
- Do not attach callbacks, FIFO, Commit Timing, damage, release, or render
  generation state to the presentation-only obligation.
- Do not switch branches or touch unrelated worktree changes.
