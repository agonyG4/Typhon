# Wayland DnD Routing Ownership Through Cancellation

## Problem

Native Wayland `start_drag` withdraws normal pointer focus and routes the physical pointer grab exclusively to DnD. The current routing predicate derives that ownership from `ActiveDrag`, so cancelling the canonical DnD session while the initiating button remains held also releases pointer-routing ownership. The physical grab then sends normal pointer events without a matching pointer enter.

## Design

Keep `ActiveDrag` as the sole DnD session authority. Add a small typed routing owner to `ImplicitPointerGrab`, initialized to normal routing and changed to native Wayland DnD only when a pointer-driven Wayland drag establishes the existing handoff. That marker lasts exactly as long as the physical grab.

Use the marker to suppress ordinary pointer, axis, relative-motion, reposition, and focus-refresh delivery. Keep a separate live-session check for DnD target updates so cancellation cannot resume target updates. Additional buttons update only physical held-button bookkeeping while this grab owns routing; they create no normal pointer serial or focus state. The existing all-buttons terminal condition ends the grab, clears its marker with the grab, and recomputes normal focus after termination. XWayland drag startup never sets this native marker.

## Regression coverage

Add a native Wayland wire test that starts a sourced drag using a real press serial, destroys its entered offer while the initiating button remains held, then verifies no pointer motion, axis, relative motion, DnD target update, or ordinary button release leaks before physical termination. It also verifies one source cancellation, no later DnD drop, and pointer-enter restoration after release. Reuse pointer-constraint support to prove lock ownership remains withdrawn until terminal release. Keep the existing sourced, source-less, drag-icon, and XWayland regressions intact.

## Acceptance

The new regression fails on the current implementation for normal pointer events leaking after canonical cancellation. The smallest ownership fix passes it and the existing DnD/XWayland regressions. Run the task's focused and full Cargo verification with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`, then review and commit only task files.
