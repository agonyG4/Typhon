# M7-B Deterministic Closure and Source-Layout Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (\`- [ ]\`) syntax for tracking. This task is executed inline because the user explicitly prohibited subagents.

**Goal:** Complete deterministic M7-B qualification by extracting action ownership into focused modules, preserving the frozen protocol contract, and passing every required repository gate.

**Architecture:** Keep \`toplevel_publication.rs\` responsible for v1 enumeration, snapshots, and resource lifecycle. Move manager-owned v2 token admission, completion, and cleanup into \`toplevel_actions.rs\`. Move the small exact-action outcome wrappers into \`state/window_actions.rs\`, while leaving existing activation, focus, stacking, and X11 backend policy in their current owners.

**Tech Stack:** Rust, Smithay/Wayland server and client test harnesses, Cargo locked validation, repository source-layout checker.

## Global Constraints

- Work directly on the existing \`main\` branch in \`/home/agony/GitHub/Typhon\`.
- Do not create branches or worktrees, rewrite history, amend, squash, reset, clean, modify Eclipse, or begin M7-C.
- Keep \`protocols/astrea-toplevel-management-v1.xml\` byte-identical with SHA-256 \`0dd3449fda60b1ed183e330e1589093f3d4f8086be117d9ca4baa81bd6bd47e7\`.
- Preserve v1 read-only compatibility, exact \`WindowId\` targeting, manager-owned completion, synchronous action execution, manager-scoped 64-entry token admission, token reuse, teardown cleanup, and close/action independence.
- Do not add artificial asynchronous queues, sleeps, delayed production actions, duplicate X11/focus/stacking policy, or source-layout allowlists/limit changes.
- All new Markdown is English and every new \`unsafe\` block requires a local \`// SAFETY:\` explanation.

---

### Task 1: Lock the baseline and protocol identity

**Files:**
- Inspect: \`protocols/astrea-toplevel-management-v1.xml\`
- Inspect: \`src/compositor/protocols/toplevel_management.rs\`
- Inspect: \`src/compositor/toplevel_publication.rs\`

- [ ] Record branch, HEAD, status, XML SHA, source counts, and the known source-layout failure.
- [ ] Confirm protocol version 2, manager \`action_done\`, and handle \`activate\`, \`minimize\`, \`restore\`, and \`close\`; do not edit the XML.

Run:

~~~
git branch --show-current
git rev-parse HEAD
git status --short --branch
sha256sum protocols/astrea-toplevel-management-v1.xml
./bin/check-source-layout
~~~

### Task 2: Add or adjust narrow regression coverage before refactoring

**Files:**
- Modify: \`src/compositor/toplevel_publication_tests.rs\`
- Modify: \`src/compositor/tests/toplevel_management.rs\`
- Inspect: \`src/compositor/state/desktop_window_tests.rs\`

- [ ] Preserve and extend the tracker assertions for duplicate admission, the 64-entry bound, release/reuse, manager disconnect cleanup, and stale-generation rejection.
- [ ] Determine whether generated Wayland dispatch rejects since-2 requests bound at version 1. If it does, document and test the observable no-mutation result rather than adding another version mechanism.
- [ ] Run the narrow tests and confirm any new assertion fails for the intended reason before production edits.

Run:

~~~
TMPDIR=/run/user/1000 cargo test --locked toplevel_management -- --test-threads=1
TMPDIR=/run/user/1000 cargo test --locked toplevel_publication -- --test-threads=1
~~~

### Task 3: Extract manager-owned Astrea action state

**Files:**
- Create: \`src/compositor/toplevel_actions.rs\`
- Modify: \`src/compositor/mod.rs\`
- Modify: \`src/compositor/toplevel_publication.rs\`
- Modify: \`src/compositor/protocols/toplevel_management.rs\` only for imports/call sites

- [ ] Move \`AstreaActionToken\`, \`AstreaToplevelAction\`, \`PendingAstreaAction\`, \`AstreaActionBeginError\`, \`AstreaActionTracker\`, \`AstreaActionPreparationError\`, and \`AstreaPreparedAction\` into the action module.
- [ ] Move manager action preparation/completion helpers there while preserving manager/client identity, token duplicate/bound admission, exact handle/resource/\`WindowId\` resolution, reservation, primitive, completion, and release ordering.
- [ ] Keep tracker cleanup on manager failure, destruction, client disconnect, and dead-resource pruning; expose only the narrow publication/action boundary.
- [ ] Leave v1 snapshot, revision, enumeration, and handle lifecycle in \`toplevel_publication.rs\`.
- [ ] Run format, compile, and focused action tests.

### Task 4: Extract the exact action outcome layer

**Files:**
- Create: \`src/compositor/state/window_actions.rs\`
- Modify: \`src/compositor/state/mod.rs\`
- Modify: \`src/compositor/state/windows.rs\`
- Modify: \`src/compositor/protocols/toplevel_management.rs\`
- Modify: \`src/compositor/state/desktop_window_tests.rs\` only for imports

- [ ] Move \`WindowActionOutcome\` and only the exact activate/minimize/restore/graceful-close outcome wrappers.
- [ ] Keep activation/focus policy, family-aware stacking, X11 synchronization, and existing central primitives in their current owners; the activate wrapper calls existing \`activate_desktop_window\`.
- [ ] Keep protocol execution synchronous and manager-owned with only accepted/no_change/unavailable results.
- [ ] Run format, compile, exact primitive tests, and M7-A focus tests.

### Task 5: Complete protocol-level closure coverage

**Files:**
- Modify: \`src/compositor/tests/toplevel_management.rs\`
- Modify: \`src/compositor/toplevel_publication_tests.rs\` only where protocol-level testing is impossible without artificial pending actions

- [ ] Cover v1 read-only behavior, authorization/version rejection, XDG exact actions, stale targets, close/action_done ordering, manager teardown, duplicate tokens, 64-entry bound, and token reuse.
- [ ] Use existing production Xwayland/XWM infrastructure for managed-X11 protocol coverage if available. Otherwise retain existing exact managed-X11 primitive tests and record the lack of a live fixture explicitly; do not invent native infrastructure.
- [ ] Run the focused protocol, contract, and primitive tests serially.

### Task 6: Run all deterministic gates, inspect ownership, update the ledger, and commit

**Files:**
- Modify: \`docs/M7_QUALIFICATION_STATUS.md\`
- Inspect: all files changed by Tasks 3–5

- [ ] Run every required gate with a short valid runtime path: \`cargo fmt --check\`, \`cargo check --locked --all-targets\`, \`cargo clippy --locked --all-targets -- -D warnings\`, \`cargo test --locked -- --test-threads=1\`, \`./bin/check-source-layout\`, and \`git diff --check\`.
- [ ] Report actual counts for \`windows.rs\`, \`toplevel_publication.rs\`, and the new action modules; verify all are under 1500 lines.
- [ ] Recompute the XML SHA and require the original value.
- [ ] Search for \`action_done\`, pending action state, \`WindowActionOutcome\`, action primitives, and \`resource.version()\`; confirm one clear owner and no new unsafe code or sleeps.
- [ ] Set M7-B Implementation PASS, Deterministic PASS only after every gate passes, Native DEFERRED; keep M7-C/D unstarted.
- [ ] Commit closure changes in focused, bisectable commits without amending or rewriting earlier history.
