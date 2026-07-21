# Render-Ahead Damage Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent stale window copies by forcing full repair when a predictive render-ahead frame cannot be covered by presented-only damage history.

**Architecture:** Normalize the acquired atomic output slot's buffer age to zero whenever another frame is pending presentation. Reuse the existing buffer-age-zero full-repaint fallback without changing scheduler, swapchain ownership, or surface geometry.

**Tech Stack:** Rust 2024, EGL/GLES repaint planner, atomic KMS output swapchain, Cargo tests.

## Global Constraints

- Preserve partial repaint when no presentation is pending.
- Do not disable predictive triple buffering or change presentation ordering.
- Do not add X11- or Steam-specific rendering rules.
- Keep damage history presentation-authoritative.
- Work directly on `main` under the user's standing approval and commit atomically.

---

### Task 1: Pending-presentation buffer-age fallback

**Files:**
- Modify: `src/egl_renderer/damage.rs`
- Modify: `src/egl_renderer.rs`
- Modify: `src/native_output/scanout/output_slot.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Test: `src/egl_renderer/damage.rs`

**Interfaces:**
- Produces: `render_target_buffer_age(presentation_serial: u64, last_presented_serial: Option<u64>, presentation_pending: bool) -> BufferAge`.
- Changes: `AtomicOutputSlot::buffer_age` gains `presentation_pending: bool`.

- [ ] **Step 1: Write the failing regression**

Add beside the existing `software_buffer_age_uses_output_presentation_serials` test:

```rust
#[test]
fn pending_presentation_invalidates_reused_render_target_age() {
    assert_eq!(render_target_buffer_age(10, Some(8), false), BufferAge::Value(3));
    assert_eq!(render_target_buffer_age(10, Some(8), true), BufferAge::Value(0));
}
```

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test --locked pending_presentation_invalidates_reused_render_target_age
```

Expected: compilation fails because `render_target_buffer_age` is not defined.

- [ ] **Step 3: Implement the pure fallback and export it**

Add after `software_buffer_age` in `src/egl_renderer/damage.rs`:

```rust
pub(crate) fn render_target_buffer_age(
    presentation_serial: u64,
    last_presented_serial: Option<u64>,
    presentation_pending: bool,
) -> BufferAge {
    if presentation_pending {
        BufferAge::Value(0)
    } else {
        software_buffer_age(presentation_serial, last_presented_serial)
    }
}
```

Re-export it from `src/egl_renderer.rs` in the existing `pub(crate) use damage::{...}` list.

- [ ] **Step 4: Route atomic slot age through the fallback**

In `src/native_output/scanout/output_slot.rs`, import `render_target_buffer_age` and change the method to:

```rust
pub(crate) fn buffer_age(
    &self,
    presentation_serial: u64,
    presentation_pending: bool,
) -> BufferAge {
    render_target_buffer_age(
        presentation_serial,
        self.last_presented_serial,
        presentation_pending,
    )
}
```

In `AtomicEglGbmScanout::render_to_slot`, derive both values from the swapchain and pass them to the slot:

```rust
let (presentation_serial, presentation_pending) = self
    .swapchain
    .as_ref()
    .map_or((0, false), |swapchain| {
        (
            swapchain.presentation_serial(),
            swapchain.pending_slot().is_some(),
        )
    });
(slot.gl_framebuffer, slot.buffer_age(presentation_serial, presentation_pending))
```

- [ ] **Step 5: Verify focused and neighboring suites**

Run:

```bash
cargo test --locked pending_presentation_invalidates_reused_render_target_age
cargo test --locked egl_renderer::damage::tests
cargo test --locked native_output::tests::scanout
cargo test --locked native_output::tests::frame
cargo test --locked native_output::tests::output
```

Expected: all commands exit zero.

- [ ] **Step 6: Format, inspect, and commit**

Run:

```bash
cargo fmt --check
git diff --check
git diff -- src/egl_renderer/damage.rs src/egl_renderer.rs src/native_output/scanout/output_slot.rs src/native_output/scanout/atomic_egl_gbm.rs
```

Commit only these four files:

```bash
git add src/egl_renderer/damage.rs src/egl_renderer.rs src/native_output/scanout/output_slot.rs src/native_output/scanout/atomic_egl_gbm.rs
git commit -m "fix(native): fully repair render-ahead output buffers"
```

### Task 2: Full gate and fresh release

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: Task 1's committed implementation.
- Produces: full verification evidence and a fresh native release binary.

- [ ] **Step 1: Run full verification**

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

Expected: all commands exit zero with no failures or denied warnings.

- [ ] **Step 2: Build and identify a clean release**

```bash
cargo clean --release -p oblivion-one
cargo build --locked --release
stat -c '%i' target/release/oblivion-one
sha256sum target/release/oblivion-one
```

Expected: the release build exits zero and differs from the running compositor until restart.

- [ ] **Step 3: Native validation**

After restart, launch Steam and repeat rapid move plus right/bottom and left/top resizing. Verify one normal X11 client, no black helper surface, no stale duplicate copies, and stable final geometry.
