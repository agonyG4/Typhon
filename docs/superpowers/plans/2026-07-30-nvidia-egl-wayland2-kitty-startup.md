# NVIDIA egl-wayland2 Kitty Startup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Typhon start NVIDIA EGL Wayland clients safely by default and make the native Kitty qualification report the observed result accurately.

**Architecture:** Keep the existing topology-gated DMA-BUF feedback normalization as the only compatibility mechanism, but select its `Auto` policy when no environment override exists. Keep explicit `off` as rollback. Correct the qualification helper's observation predicate without changing compositor protocol or scanout eligibility.

**Tech Stack:** Rust, Bash, Cargo integration tests, native Wayland/KMS qualification.

## Global Constraints

- Automatic normalization must activate only for NVIDIA, DMA-BUF v4+, differing node identities, and a proven shared physical GPU.
- Explicit `OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT=off` must preserve strict feedback.
- Do not disable explicit synchronization or `egl-wayland2`.
- Do not change direct-scanout eligibility.
- Preserve unrelated worktree deletions and qualification artifacts.

---

### Task 1: Safe Default Policy

**Files:**
- Modify: `src/native_output/scanout/feedback_policy.rs`
- Modify: `bin/start-oblivion-one`
- Test: `src/native_output/scanout/feedback_policy.rs`
- Test: `tests/start_launcher.rs`

**Interfaces:**
- Consumes: `NvidiaEglWayland2CompatPolicy::{Auto, Off, Force}` and `from_env() -> Self`.
- Produces: an absent environment override resolves to `Auto`; explicit values keep their existing behavior.

- [ ] **Step 1: Write failing policy and launcher tests**

Add a unit test guarded by `crate::native_output::ASTREA_ENV_LOCK` that removes
`OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT`, calls `from_env()`, restores the
previous value, and asserts `Auto`. Add an integration test that runs
`bin/start-oblivion-one --help` and asserts the compatibility line contains
`auto (default)` and an explicit `off` rollback description.

- [ ] **Step 2: Run tests and confirm the expected failures**

Run:

```bash
env XDG_RUNTIME_DIR=/tmp/typhon-kitty-tests cargo test --locked --lib compatibility_policy_defaults_to_auto_when_unset -- --nocapture
env XDG_RUNTIME_DIR=/tmp/typhon-kitty-tests cargo test --locked --test start_launcher start_launcher_documents_safe_nvidia_egl_wayland2_default -- --nocapture
```

Expected: the policy test reports `left: Off, right: Auto`; the launcher test
fails because help still says `off (default)`.

- [ ] **Step 3: Implement the minimal policy and help change**

Change the absent-value fallback in `from_env()` from `Self::Off` to
`Self::Auto`. Leave invalid explicit values falling back to `Off`. Change the
launcher help line to state `auto (default)` and `off` as rollback.

- [ ] **Step 4: Run the focused tests**

Run the two commands from Step 2. Expected: both pass.

- [ ] **Step 5: Commit the policy increment**

```bash
git add src/native_output/scanout/feedback_policy.rs bin/start-oblivion-one tests/start_launcher.rs
git commit -m "fix(dmabuf): enable safe NVIDIA feedback compatibility"
```

### Task 2: Accurate Kitty Qualification

**Files:**
- Modify: `bin/qualify-nvidia-egl-wayland2`
- Test: `tests/start_launcher.rs`

**Interfaces:**
- Consumes: compositor performance log lines `app.first_toplevel ...` and `app.toplevel app_id=kitty ...`.
- Produces: the qualification proceeds when either the brokered first-toplevel metric or the external Kitty toplevel metric is present.

- [ ] **Step 1: Write a failing script-contract test**

Read `bin/qualify-nvidia-egl-wayland2` from `tests/start_launcher.rs` and assert
that its toplevel predicate contains both `app\\.first_toplevel` and
`app\\.toplevel app_id=kitty`. Keep the assertion scoped to the qualifier
script so unrelated performance logs cannot satisfy it.

- [ ] **Step 2: Run the test and confirm the expected failure**

Run:

```bash
env XDG_RUNTIME_DIR=/tmp/typhon-kitty-tests cargo test --locked --test start_launcher nvidia_egl_wayland2_qualifier_accepts_external_kitty_toplevel -- --nocapture
```

Expected: failure because the script contains only `app\\.first_toplevel`.

- [ ] **Step 3: Implement the minimal predicate**

Replace the single-pattern AWK expression with one that marks success for
either `app.first_toplevel` or `app.toplevel` carrying `app_id=kitty`. Do not
relax the crash-text, coredump, or client-liveness checks.

- [ ] **Step 4: Run focused and launcher tests**

Run:

```bash
env XDG_RUNTIME_DIR=/tmp/typhon-kitty-tests cargo test --locked --test start_launcher -- --nocapture
```

Expected: all launcher tests pass.

- [ ] **Step 5: Commit the qualifier increment**

```bash
git add bin/qualify-nvidia-egl-wayland2 tests/start_launcher.rs
git commit -m "fix(qualification): recognize external Kitty toplevel"
```

### Task 3: Full Verification and Native Reproduction

**Files:**
- Verify: `src/native_output/scanout/feedback_policy.rs`
- Verify: `bin/start-oblivion-one`
- Verify: `bin/qualify-nvidia-egl-wayland2`
- Verify: `tests/start_launcher.rs`

**Interfaces:**
- Consumes: the default policy and corrected qualifier from Tasks 1 and 2.
- Produces: a release binary and native evidence that default/auto avoids the NVIDIA abort while explicit off still reproduces it.

- [ ] **Step 1: Format and run the complete test suite**

Run:

```bash
cargo fmt --check
env XDG_RUNTIME_DIR=/tmp/typhon-kitty-tests cargo test --locked -- --test-threads=1
```

Expected: formatter exits zero and the complete suite reports zero failures.

- [ ] **Step 2: Build the release binary**

Run:

```bash
cargo build --locked --release
```

Expected: exit zero and a fresh `target/release/oblivion-one`.

- [ ] **Step 3: Run native qualification from the active TTY seat**

Run:

```bash
env OBLIVION_ONE_EGL_WAYLAND2_OBSERVATION_SECONDS=10 \
  OBLIVION_ONE_EGL_WAYLAND2_LOG_DIR=/tmp/typhon-kitty-final \
  ./bin/qualify-nvidia-egl-wayland2
```

Expected: the helper observes Kitty's toplevel, Kitty remains alive for ten
seconds, no coredump exists for its PID, and the compatibility diagnostic says
`compat_requested=auto compat_effective=same-device-normalization`.

- [ ] **Step 4: Verify explicit rollback reproduces the original boundary**

Start the same native compositor configuration with
`OBLIVION_ONE_NVIDIA_EGL_WAYLAND2_COMPAT=off`, wait for
`native scanout active`, and launch `kitty --config NONE`. Expected: status
134 with `libnvidia-egl-wayland2.so.1` in the coredump. This confirms the
regression is isolated to the feedback policy.

- [ ] **Step 5: Review repository scope**

Run:

```bash
git status --short
git diff HEAD~2..HEAD --stat
git log --oneline -4
```

Expected: only the design, plan, policy, launcher help, qualifier, and focused
tests are part of the new commits; pre-existing user deletions and artifacts
remain untouched.
