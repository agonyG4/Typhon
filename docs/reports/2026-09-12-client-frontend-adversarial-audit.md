# Typhon client-facing compositor frontend audit

## Executive assessment

Typhon has strong building blocks: explicit buffer ownership, generation-qualified X11 identities, incremental XWM startup/property handling, transactional surface publication, typed presentation ownership, and extensive lifecycle tests. The largest defects are not missing abstractions inside those building blocks. They are disagreements between them about what a commit owns, what an event confirms, what remains queued, and what was actually displayed.

The most urgent findings are an XWM byte-stream ordering defect, an XSync event-type mismatch that prevents normal resize acknowledgement, an unbounded synchronized-subsurface cache, and transaction supersession that can discard unrelated latched content. Other source-proven defects affect commit timing, double-buffered role state, bufferless mapping changes, viewport merging, transformed damage, layer focus, and presentation attribution. X11↔Wayland clipboard/PRIMARY/XDND support is a disconnected foundation, not a functioning bridge in this revision.

No P0 is established. No product fixes are included. A standalone diagnostic file reproduces the transport ordering defect and two transfer-state failures against included, unmodified production source.

## Baseline and confidence

Source baseline: `8fb50945ad1849a7d97b34ae8b0deae9ebcd8d32`, inspected September 12, 2026. The working tree changed during the initial investigation; material findings were rechecked after the frontend edits finished. Locations below refer to this final source revision, not the initial `0636bd5ff5963fdb97bb0c75c36b56908ebad2c8`. Existing deleted research documents were not treated as source evidence.

Evidence classes:

- **Proven defect — probe:** executable reproduction against production implementation.
- **Proven defect — source:** a concrete input sequence contradicts the executed branches or protocol contract; no live application reproduction is claimed.
- **Strongly supported risk:** control flow supports the failure, but an end-to-end reproduction is still required before choosing the final repair.
- **Optimization opportunity:** unnecessary work is established; deployment-specific CPU/frame-time improvement is not measured.
- **Speculative idea:** insufficient evidence to recommend implementation; excluded from ranked findings.

Severity is impact and urgency, not confidence: P1 warrants early correction because it can freeze clients, lose committed content, break major interoperability, or exhaust compositor resources; P2 is material but narrower correctness, compatibility, performance, or resource impact; P3 is lower-priority improvement. Resource claims distinguish bounded individual queues from bounded aggregate ownership.

The scope includes protocol handlers, internal state machines, compositor adapters, scene/frame ownership, native event integration, and relevant tests. This is not a proof that every line or every hardware/backend combination is correct. There was no interactive GPU/KMS application session, hardware explicit-sync stress test, or live X11↔Wayland selection interoperability run. Performance estimates are derived work counts/bytes, not benchmark results.

## Ranked highest-confidence defects

| Rank | Finding | Severity | Evidence | Primary consequence |
|---|---|---|---|---|
| 1 | F02: queued X11 bytes can be overtaken | P1 | Probe | Corrupted XWM request stream; generation-wide failure |
| 2 | F01: XSync alarms dispatched as counter notifications | P1 | Source | Resize/content stalls until the 10-second fallback |
| 3 | F03: paced synchronized cache has no entry bound | P1 | Source | Client-controlled compositor memory/FD retention |
| 4 | F05: first timed transaction bypasses timing admission | P1 | Source | Content presented before its not-before constraint |
| 5 | F06: delayed publication consumes newer uncommitted role state | P1 | Source | Atomicity, geometry, stacking, and focus divergence |
| 6 | F11: X11↔Wayland selection and XDND bridge is not connected | P1 | Source | Cross-protocol copy/paste and DND unavailable |
| 7 | F07–F09: mapping-state and damage inconsistencies | P2 | Source | Stale/cropped/incorrectly transformed frames |
| 8 | F10: layer repaint changes activation order | P2 | Source | Focus/restacking changes caused by content repaint |
| 9 | F13: culled surfaces can receive presented feedback | P2 | Source | False presentation attribution; hidden-client pacing |
| 10 | F12: transfer pump strands EOF and later chunks | P2 | Probe; dormant adapter | Bridge would hang once connected |

F04 is separately high priority: it is a strongly supported cross-subsystem loss-of-state risk and should receive a reproduction before broad transaction changes.

## Correctness, lifecycle, and compatibility findings

### F01 — XSync resize creates alarms but handles the wrong notification

**P1 · correctness / compatibility / latency · Proven defect — source.**

**Locations:** `src/xwayland/xwm/commands.rs:898–942`; `src/xwayland/xwm/events.rs:333–354`; `src/xwayland/xwm/mod.rs:706–714`; `src/xwayland/xwm/resize_sync.rs:7`; `src/xwayland/xwm/resize_runtime.rs:48–105`.

**Current behavior and evidence:** resize allocates an XSync alarm with events enabled, stores it in `sync_alarms`, sends `_XWAYLAND_ALLOW_COMMITS=false`, and sends the sync request before configure. The reactor only handles `Event::SyncCounterNotify`; `SyncAlarmNotify` falls through the default branch. Alarm threshold notifications and Await counter notifications are different event types in the [X Synchronization Extension protocol](https://xorg.freedesktop.org/archive/current/doc/xextproto/sync.html).

**Failure/scenario:** an otherwise conforming X11 application increments its advertised resize counter. The server's alarm notification is ignored. The acknowledgement transition and commit reenable never execute normally; the timeout is 10,000,000,000 ns. The timeout eventually reenables commits and disables synchronization for that window. The late-ack recovery helper exists, but the missing wire dispatch also prevents the normal alarm from reaching it. This can make an initial interactive resize appear frozen and then fall back to unsynchronized resizing.

**Existing tests:** `events_regression_tests.rs:1505` checks request-before-configure ordering; `:1644` checks counter initialization; `:1844–1868` tests late acknowledgement through `note_resize_sync_ack_for_test`, bypassing event decoding/dispatch. They do not establish wire-level alarm completion.

**Architectural fix:** dispatch alarm notifications through a generation-qualified reverse alarm→window/resize-transaction association. Validate alarm identity, active transaction, and counter progress before acknowledgement. Keep the existing timeout and failure fallback; shortening the timeout is not the repair.

**Regression risks/tests:** stale/destroyed alarms, counter replacement, XID reuse, signed counter conversion, late acknowledgements, and a timeout racing the alarm. Add a real encoded AlarmNotify reactor test and a real X server counter increment test; require commit reenable before deadline and exactly one terminal resize outcome.

### F02 — Backpressured XWM output can reorder the X11 byte stream

**P1 · correctness / compatibility · Proven defect — probe.**

**Locations:** `src/xwayland/xwm/connection.rs:61–104,162–195`; probe `docs/reports/2026-09-12-frontend-audit-probes.rs`, `audit_queued_x11_bytes_must_precede_new_request`.

**Current behavior and evidence:** `write` calls `flush_pending`, ignores its returned pending-output flag, and writes the new request directly to the socket. A partially drained backlog is therefore not an ordering barrier. `flush_pending` also mistakes equality of the old and new first byte for lack of progress; repeated byte values stop draining even after a successful 16 KiB write.

**Failure/scenario:** after a prior short write, 48 KiB of queued `A` bytes precede a new `B`. The probe receives `B` at offset **16,384**, not **49,152**. The injected queue is a valid state of this production transport following backpressure. With actual X11 requests, the same ordering violation can splice requests into earlier payloads, corrupt lengths/opcodes, and terminate or desynchronize the entire XWM connection. No multithreaded race is required. EAGAIN followed by socket availability creates another overtaking window.

**Existing tests:** `connection.rs` tests nonblocking setup, writable-interest removal, EPOLLOUT drain, and HUP behavior. Those do not check byte-exact ordering across a nonempty queue and a subsequent request; six inherited tests pass in the probe harness while this probe fails.

**Architectural fix:** one serialized FIFO authority for socket writes and queued suffixes. While backlog exists, append new accepted bytes behind it. Track progress by byte counts, not byte values. Retain the 1 MiB bound and explicit writable interest. Preserve x11rb's accepted-byte contract, including queue-full/error behavior.

**Regression risks/tests:** partial acceptance, simultaneous flush/write access, capacity errors after a prefix is written, and lost EPOLLOUT interest. Property-test arbitrary short writes/EAGAIN with a reference concatenated stream; test identical bytes, alternating request sizes, and exact capacity boundaries. Once correct, vectored writes can avoid the current flatten/copy allocation, but correctness precedes that optimization.

### F03 — Synchronized paced commits bypass the bounded transaction queue

**P1 · resource usage / correctness / observability · Proven defect — source.**

**Locations:** `src/compositor/state/subsurfaces.rs:145–147,647–780,1037–1092`; `src/compositor/subsurface.rs:647–671,702–706,754–770`.

**Current behavior and evidence:** the main pending-tree queue has an eight-transactions-per-root limit. Effectively synchronized children return into `cache_synchronized_subsurface_commit` before that queue. `cache_commit` merges only when there is exactly one cached commit and neither commit is a pacing boundary. Once a paced entry exists, subsequent entries append to the child's `VecDeque` without an entry/byte/FD bound. `cached_node_count` counts nonempty surfaces, not cached entries. Parent latch drains all entries into one node vector, so the downstream root-slot limit does not bound this memory either.

**Failure/scenario:** one synchronized child commits a FIFO/timing boundary, then repeatedly commits buffers or callback-bearing state without another parent commit. The cache grows for the client's lifetime, retaining resource references and potentially DMA-BUF/sync FDs. Even ordinary unpaced commits stop coalescing after that first boundary. Request-dispatch budgets limit work per wake, not aggregate retained state. The observable cached-node count can stay at one while thousands of entries accumulate.

**Existing tests:** root queue bounds and paced transaction ordering are tested, but those exercise queued trees, not sustained pre-parent child caching. They do not establish a global resource bound.

**Architectural fix:** define the synchronized-cache reduction semantics first, then enforce per-client and compositor-wide entry/byte/FD budgets across cached, pending, and frame-owned stages. Distinguish permissible cumulative state merging from presentation/FIFO boundaries. Do not simply drop callbacks or flatten multiple required boundaries into one eventual display. Exhaustion must fail the responsible client safely, not other clients.

**Regression risks/tests:** premature buffer release, merging away a FIFO wait, changing synchronized-child latch behavior, or disconnecting a normal bursty application. Add 100,000 commits without a parent latch, bounded outstanding FDs/bytes, paced→unpaced sequences, a parent latch after the bound, and a second client whose progress remains unaffected.

### F04 — Replacing one surface attachment can discard an entire latched tree

**P1 · correctness / architecture · Strongly supported risk.**

**Locations:** `src/compositor/state/subsurfaces.rs:95–110,145–147`; `src/compositor/state/surfaces.rs:474–535`; `src/compositor/state/surface_tree_readiness.rs:121–177`.

**Current behavior and evidence:** before deciding whether a new commit belongs in a synchronized child cache, `commit_surface_tree_request` supersedes older pending attachments for that surface. If any node in a nonpaced, unready tree has an older attachment, `supersede_older_pending_attachments_for_surface` releases the **whole transaction**, not only that node. It carries returned callbacks into the incoming surface commit; other nodes' state is not merged into a replacement tree.

**Concrete scenario:** parent P latches children A and B. The transaction waits for A's acquire fence. A submits a newer buffer while still synchronized, without another P commit. The old P/A/B transaction is removed, including P and B's committed updates. Its callbacks move into A's new synchronized cache, which cannot publish without a later parent commit. A client waiting for P's callback before committing P again can stop making progress. Even if it continues, unrelated B content has been lost.

**Existing tests:** supersession tests establish release/callback handling for replaceable attachment work, and readiness tests distinguish paced heads. They do not establish conservation of unrelated nodes when replacement happens before a new parent latch. A full protocol reproduction is required to pin down every callback's observable outcome; the whole-tree removal itself is explicit.

**Architectural fix:** a replacement cannot acquire authority over another surface's latched state merely by matching one node. Maintain cumulative state per surface with explicit parent-latch membership/dependency edges; coalesce only into a valid successor carrying all still-required state. Synchronized child commits must not retroactively cancel an already-latched parent transaction on their own.

**Regression risks/tests:** retained obsolete acquire dependencies, lost progress, double release, and incorrect feedback carry-forward. Add P/A/B tests with delayed A, newer A without P, newer P without B, sibling feedback, child destruction, and independently signaled fences. Check that every callback and release obligation has exactly one terminal owner.

### F05 — The empty-queue admission path ignores commit-timing constraints

**P1 · correctness / latency · Proven defect — source.**

**Locations:** `src/compositor/state/subsurfaces.rs:238–265`; `src/compositor/state/surface_pacing.rs:277–365,367–414`; `src/compositor/state/frame_tests.rs:188–210`.

**Current behavior and evidence:** when there is no existing transaction for a root, admission computes readiness using `surface_tree_parts_ready`, which checks acquire dependencies and FIFO waits only. A future timestamp alone does not make it false, so the transaction publishes immediately. The full `transaction_is_ready` predicate checks timing, but this path does not call it. `apply_captured_surface_pacing` creates an active timing claim only from `commit_timing_readiness`; this immediate path has not created that scheduling evidence.

**Failure/scenario:** a mapped SHM surface with no pending tree sets a timestamp one second in the future and commits new content. There is no acquire dependency or FIFO barrier to force the queue, so the next ordinary output frame can contain it early. This violates the [commit-timing not-before contract](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/staging/commit-timing/commit-timing-v1.xml), not merely ideal frame pacing.

**Existing tests:** `future_commit_timing_is_planning_work_not_scene_prepare_work` constructs a `PendingSurfaceTreeTransaction` directly. It proves behavior after queue insertion, not admission from `wl_surface.commit` into an empty queue.

**Architectural fix:** one readiness/admission predicate for an immutable transaction, including time, dependencies, FIFO, and predecessor ordering. Preserve Typhon's safe early-render planning: rendering before the target can be valid only with an output submission/presentation constraint that survives all later stages.

**Regression risks/tests:** unnecessary stalls, clocks changing, stale planning generations, and bypass through already-signaled DMA-BUF commits. Add real entry-path tests for SHM, signaled DMA-BUF, empty/nonempty queues, timing-only bufferless commits, timer destruction, and a paced commit followed by unpaced content. Assert no presentation before the requested timestamp.

### F06 — Deferred publication reads newer uncommitted parent and role state

**P1 · correctness / architecture · Proven defect — source.**

**Locations:** `src/compositor/subsurface.rs:140–163,728–751`; `src/compositor/state/subsurfaces.rs:1094–1146,1211–1237`; `src/compositor/layer_shell.rs:407–434,436–490,712–746`; `src/compositor/state/surface_commits.rs:631–654`.

**Current behavior and evidence:** `CachedSubsurfaceCommit` captures surface fields, pacing, presentation, and pointer-constraint mutations, but not pending child positions/stacks or layer role state. `publish_surface_tree` calls `apply_pending_subsurface_parent_state` at publication time; that takes the live mutable pending position/stack collections. Layer publication similarly invokes `commit_pending_layer_surface_state`, which reads `role.pending`, and consumes the live pending configure acknowledgement.

**Failure/scenario:** set child position to 10; commit P while an acquire fence blocks its tree; set child position to 20 without committing P; signal the fence. P's earlier transaction publishes position 20. A mapped layer client can similarly commit state A, block publication, then issue uncommitted exclusive-zone/keyboard-interactivity state B. B influences layout or focus when A publishes. These are ordering defects even on a single compositor thread.

**Existing tests:** normal double buffering and layer configure acknowledgement are covered. Tests such as layer-shell exact configure geometry and immediate parent positioning do not insert a publication delay followed by an uncommitted role mutation.

**Architectural fix:** capture parent-latched child placement/order and role-specific committed state at the correct `wl_surface.commit` boundary. Store immutable role/ack snapshots in the transaction. Promotion may validate lifecycle/generation, but must not consume newer pending requests. Preserve the existing explicit resize-snapshot pattern rather than introducing another mutable shadow state.

**Regression risks/tests:** nested subsurface latch rules, role destruction before apply, initial layer configure, and multiple acked-but-not-published configurations. Add no-second-commit tests across position, stacking, exclusive zone, anchor, keyboard mode, and configure ack; inspect scene, input hit testing, and protocol state together.

### F07 — Bufferless commits do not propagate the full visual mapping

**P2 · correctness / compatibility · Proven defect — source.**

**Locations:** `src/compositor/state/surface_transactions.rs:121–129,202–208,293–304`; `src/compositor/state/surface_commits.rs:430–547,894–978`.

**Current behavior and evidence:** publication applies viewport/scale/transform to `SurfaceData`, but `BufferlessSurfaceCommitState` carries only a derived surface size and scale, not viewport source, transform, or offset. `commit_surface_without_buffer` only invokes the visual update for damage or a size/window-geometry change. The damage-only update itself takes transform and viewport source from the old `CurrentSurfaceBuffer` and copies them back into `RenderableSurface`.

**Failure/scenario:** a client keeps its buffer, changes a 180-degree transform or moves a viewport source rectangle without changing destination size, and commits. Protocol state advances; scene sampling retains the previous mapping. Adding damage does not repair the missing transform/source because the update uses old buffer metadata. Removing a viewport also needs an explicit reset, not the absence of a new destination.

**Existing tests:** damage-only resize continuity and geometry-only commits are covered. They do not test every mapping field independently with no new attachment, particularly a changed mapping with unchanged dimensions.

**Architectural fix:** separate immutable buffer payload ownership from complete committed sampling/mapping state. Every commit must produce the effective mapping and compare it against the published mapping independently of whether a buffer is attached. Damage old/new extents as appropriate and derive input geometry from the same snapshot.

**Regression risks/tests:** breaking resize preview continuity, repainting unnecessarily, incorrect viewport reset semantics, and cursor-specific paths. Add retained-buffer tests for all eight transforms, scale-only changes, source-only crop movement, viewport destruction, offset changes, and unchanged-size mappings; compare protocol state, renderer metadata, pixels, and hit coordinates.

### F08 — Cached viewport merging overwrites independent fields

**P2 · correctness · Proven defect — source.**

**Locations:** `src/compositor/subsurface.rs:165–219` (especially `212–214`); `src/compositor/state_data.rs:568–625`; related preparation boundary `src/compositor/state/subsurfaces.rs:503–527`.

**Current behavior and evidence:** `PendingViewportChange` represents independent optional changes, including explicit unsets. `CachedSubsurfaceCommit::merge` replaces the entire viewport delta when either source or destination changes. That is not a field-wise merge. The [viewporter protocol](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/stable/viewporter/viewporter.xml) explicitly treats source and destination as independently double-buffered state.

**Failure/scenario:** a synchronized child commits source rectangle S, then commits destination D, before P latches it. The second delta contains no source change; replacing the delta loses S. Reversing the order loses D. An explicit unset can be lost by the same mechanism. This produces the wrong crop/size despite both requests being valid.

**Existing tests:** damage conversion with a fully specified viewport is tested in `state_data.rs:1087–1137`. That does not exercise sequential independent viewport deltas in the synchronized cache.

**Architectural fix:** merge each field independently while preserving the distinction between unchanged and explicitly unset. Materialize effective state in commit order. Also verify the pending-tree merge boundary: incoming attachments are prepared against published `SurfaceData` before coalescing, so derived buffer metadata must be recomputed or proven consistent with the merged cumulative state.

**Regression risks/tests:** mixing delta and complete-state representations, accidentally retaining a destroyed viewport, or carrying stale size calculations. Property-test equivalence of sequential application and allowed coalescing for source/destination set/unset, scale, transform, and attachment replacement. Treat the related prepared-metadata concern as a required verification, not a separately proven defect here.

### F09 — Surface damage conversion omits transform and crop/scale composition

**P2 · correctness / compatibility · Proven defect — source.**

**Locations:** `src/compositor/protocols/core.rs:157–163`; `src/compositor/state_data.rs:853–889,915–948,1623–1666`; tests `state_data.rs:1040–1137`.

**Current behavior and evidence:** `convert_pending_damage` has no buffer-transform input. With no viewport destination it scales the surface rectangle and ignores a viewport source crop. With a destination it treats source coordinates as buffer-pixel coordinates, rather than coordinates after buffer transform and scale. The required transformation order is defined by [viewporter](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/stable/viewporter/viewporter.xml).

**Failure/scenario:** a 90-degree-transformed SHM buffer updates a small rectangle and sends `wl_surface.damage` in surface coordinates. Typhon maps it to the wrong buffer rectangle. On same-buffer publication the SHM materialization path copies only that incorrect rectangle into the retained snapshot. A later full-screen GPU repaint cannot recover pixels never copied. A source-only crop, or a cropped scale-2 buffer, produces analogous stale regions. Buffer-coordinate `damage_buffer` avoids this particular conversion error.

**Existing tests:** scale one/two, viewport destination, source plus destination, and edge clipping are present. The converter cannot currently express a transform, so those tests do not cover it; source-only crop and transformed/scaled crop composition are also missing from the cited tests.

**Architectural fix:** centralize forward/inverse buffer↔surface mapping and use it for sampling, damage, size validation, and input conversion. Map rectangles with conservative outward rounding and clip in the appropriate coordinate space. A full-damage fallback is safe where an exact inverse is unavailable; silently using the untransformed rectangle is not.

**Regression risks/tests:** fractional crop rounding, flipped transforms, overflow, and applying transform twice in renderer code. Add pixel-reference tests for eight transforms × multiple scales × source-only/source+destination viewports; verify changed pixels are never excluded. Include repeated same-buffer SHM partial updates and untouched-pixel preservation.

### F10 — Every layer buffer publication acts like a new map/activation

**P2 · correctness / performance / latency · Proven defect — source; optimization opportunity.**

**Locations:** `src/compositor/state/surface_commits.rs:1662–1672`; `src/compositor/layer_shell.rs:436–460,493–517,1061–1070`.

**Current behavior and evidence:** every successful layer buffer publication calls `note_layer_surface_buffer_published`. It increments `layer_surface_order`, sets `mapped=true`, replaces `role.order`, arranges layers/stateful windows, may recompute exclusive keyboard focus, restacks the scene, and reconciles idle inhibition. The pre-publication check also arranges/restacks already-mapped layers even when the role state did not change. Exclusive focus picks the maximum `(layer rank, role.order)`.

**Failure/scenario:** two exclusive surfaces occupy the same layer. The older surface repaints; its order becomes newest and it can regain focus. A frequently repainting layer surface can also move within same-layer stacking/reservation order. The intended policy may choose the newest activated surface, but content repaint is not an activation request.

**Cost/hot path:** an animated panel can trigger two layer-arrangement/restacking paths per buffer commit, even with unchanged anchors, size, reservation, and keyboard mode. Work grows with layers and affected scene/windows; equality guards downstream do not eliminate traversal and planning.

**Existing tests:** `src/compositor/tests/layer_shell.rs:499` checks arbitration when overlays map and restore. It does not establish stable activation order during content-only repaint.

**Architectural fix:** separate first map/remap edges from content publication. Only lifecycle/policy changes alter activation order. Use role/layout dirty flags and run arrangement once for meaningful geometry/reservation changes. Keep buffer damage and frame completion independent.

**Regression risks/tests:** missing arrangement after first map, output resize, exclusive-zone change, or unmap. Add two same-layer exclusive clients that repaint alternately; assert focus/order remain stable. Count arrangement passes for 1,000 content-only commits, plus explicit role-change controls. Expected benefit: remove repeated global maintenance from panel frame cadence and eliminate repaint-driven focus transitions; no measured millisecond saving is claimed.

### F11 — X11↔Wayland clipboard, PRIMARY, and XDND are not integrated

**P1 · compatibility / architecture · Proven integration gap — source.**

**Locations:** `src/xwayland/xwm/mod.rs:373,545–553`; `src/xwayland/xwm/startup.rs:1360`; `src/xwayland/xwm/events.rs:333–354,365–473`; `src/xwayland/xwm/data_bridge/`; `src/compositor/state/selection_runtime.rs:349–421`; `tests/xwayland_selection.rs:8–35`; `tests/xwayland_dnd.rs:9–19`.

**Current behavior and evidence:** XWM owns a `DataBridge`, but its production uses are initialization and generation cleanup. The XWM event switch has no selection request/notify/clear or XFixes selection dispatch; client-message normalization has no XDND route. The Wayland selection broker's backends are Wayland clipboard, Wayland primary, data-control, and host clipboard bridge, not X11. The foundation managers are not an alternate live adapter.

**Failure/scenario:** copying from an X11 application into a native Wayland application, the reverse direction, cross-protocol PRIMARY paste, and cross-protocol drag/drop have no completed route. This is **not** a claim that X11→X11 clipboard operations fail: those can remain inside the X server. It is also not a claim that the ordinary Wayland broker is absent. Foundation tests explicitly test managers, not protocol interoperability.

**Existing tests:** the two cited integration-test files instantiate `SelectionBridge`/`DndManager` directly. They cannot detect missing wire dispatch, MIME/atom negotiation, FD pumping, or compositor selection ownership.

**Architectural fix:** add an X11 backend to the existing selection broker, with generation/source/transfer identities and explicit ownership/timestamp rules. Wire XFixes owner changes, SelectionRequest/Notify, TARGETS, INCR property flow, and readiness-driven FD transfer. XDND requires its own negotiated action/target/terminal lifecycle, connected to the existing Wayland drag authority. Keep clipboard, PRIMARY, and drag transfers distinct. Do not introduce a second independent clipboard truth.

**Regression risks/tests:** self-ownership loops, selection theft, stale offers after restart, INCR deadlocks, MIME conversion mistakes, and unauthorized cross-client delivery. Add bidirectional real-protocol tests for small and >64 KiB payloads, slow readers, owner death, owner replacement, restart, PRIMARY, target changes, rejection, and Ask/Move/Copy. F12 must be repaired before wiring its pump into production. KWin and Hyprland comparisons below explain the missing event/transfer boundary, not a mandate to adopt their object layouts.

### F12 — Transfer pump strands EOF and data following EAGAIN

**P2 · correctness / resource usage · Proven defect — probe; currently dormant.**

**Locations:** `src/xwayland/xwm/data_bridge/transfer.rs:71–128`; probe functions `audit_eof_after_data_must_complete_transfer` and `audit_eagain_after_data_must_allow_later_data`.

**Current behavior and evidence:** after a successful read/write, `offset == buffer.len() > 0`. The next EOF or EAGAIN clears the vector without resetting `offset`. With EOF, terminal detection compares the old positive offset with zero and never completes. With EAGAIN, the next pump's read condition is false, so newly arriving bytes are never read.

**Failure/scenario:** transfer `abc`, drain it, then either close the producer or pause until the next nonblocking read returns EAGAIN before sending `def`. Both probes fail. The later-data probe finds the consumer still at WouldBlock. Existing `transfer_uses_nonblocking_bounded_chunks` passes because it does not require post-payload EOF settlement or resumed data after EAGAIN.

**Impact boundary:** F11 means this is not evidence of an already-running production clipboard transfer hanging. It is a concrete defect in the adapter proposed for that role. The manager caps active transfers at 64; these failures can exhaust those slots once integrated. Deadline checks happen when `pump` runs, so an idle deadline also needs explicit reactor ownership.

**Architectural fix:** represent reading, buffered writing, EOF-draining, and terminal states explicitly; keep `0 <= offset <= len`, resetting both coherently. Export desired read/write readiness and a deadline to the event loop. Retain chunk and active-transfer bounds.

**Regression risks/tests:** dropping buffered bytes on EOF, busy EPOLLOUT loops, EINTR behavior, zero writes, and source/sink closure. Add arbitrary partial reads/writes, EAGAIN before/after every chunk, EOF with pending sink data, deadline without FD readiness, cancellation, and generation cleanup. Require exact payload conservation and eventual FD/slot release.

### F13 — Native fullscreen culling and presentation feedback disagree about visibility

**P2 · correctness / latency / resource usage · Proven defect — source.**

**Locations:** `src/compositor/state/workspaces.rs:163–202`; `src/compositor/state/frame_callbacks.rs:286–298`; `src/compositor/state/frames.rs:243–311,364–411,757–803`; `src/compositor/state/fullscreen.rs:609–635`.

**Current behavior and evidence:** feedback visibility is based on active-workspace/nonminimized ownership. The native fullscreen render plan can then remove every surface except the solitary fullscreen tree and allowed overlays. Frame batches still capture the workspace-visible feedback set. Supplying the actually rendered surface-damage set does not filter feedback; completion sends `presented` to every captured live surface with the matching clock.

**Failure/scenario:** a background window on the same workspace commits with presentation feedback while an opaque solitary fullscreen window is being composited. The native list omits the background window, but the fullscreen frame's pageflip completes its feedback as presented. This finding concerns the ordinary composited frame-batch path; the direct-surface presentation path has separate lineage filtering and should not be generalized from it.

**Cost/hot path:** callbacks use workspace visibility too. Callback delivery to an occluded surface is not itself prohibited, but it can sustain animation and SHM/transaction work for content deliberately culled from output. Presentation feedback, unlike callback scheduling, must not claim unseen content was displayed. See the content-update semantics in [presentation-time](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/stable/presentation-time/presentation-time.xml).

**Existing tests:** batch identity/retry/release tests and fullscreen culling tests validate their respective sets separately. No inspected test requires a fullscreen-culled content update to remain unpresented while another tree flips.

**Architectural fix:** attach actual content-update/sample membership to the admitted render frame and settle feedback only for that membership. Keep callback policy separate and explicit. Preserve feedback for a still-current update that may later become visible, or discard when it is superseded/terminal; do not manufacture presentation to drain a queue. Define FIFO forward progress separately from proof of display.

**Regression risks/tests:** wrongly suppressing feedback for unchanged pixels that really were displayed, cursor planes, overlays, scanout, and buffer reuse. Test fullscreen enter/exit with background feedback, two commits while occluded, overlay exceptions, failed render/retry, and direct scanout. Count hidden-client callbacks/commits under fullscreen before selecting a throttling policy.

## Ranked highest-value optimizations

| Rank | Opportunity | Why it merits measurement |
|---|---|---|
| 1 | O01: avoid unconditional full SHM COW copies | Large bytes/commit cost despite partial or empty damage |
| 2 | F10: event-driven layer maintenance | Repaint-triggered global work plus a focus correctness repair |
| 3 | O02: indexed transaction readiness / incremental work accounting | Cubic worst-case drain and repeated global reconstruction |
| 4 | O03: reuse presentation targets and root geometry indices | Quadratic construction in frame-admission/animation queries |
| 5 | O04: lazy disabled trace formatting | Repeated avoidable allocations with tracing off; low-risk boundary |

These rankings reflect potential impact and confidence in redundant work, not measured application speedups. Do not substitute microbenchmarks for end-to-end frame-time and power measurements.

### O01 — Partial SHM updates still clone the entire immutable pixel snapshot

**P2 · performance / resource usage · Optimization opportunity; source-established copy.**

**Locations:** `src/compositor/state_data.rs:1623–1666`; `src/render_backend/buffer.rs:280–307`.

**Work today:** same-buffer/same-size SHM publication clones `previous`, then obtains mutable pixels through `Arc::make_mut`. The original snapshot is still live, so mutation copies the full pixel vector before patching damaged rectangles. Even empty damage obtains mutable pixels before the damage reader decides there is nothing to copy from SHM.

**Why costly/hot:** this is client buffer publication, not setup. A 3840×2160 RGBA snapshot is 33,177,600 bytes. One such full snapshot copy at 60 commits/s is approximately **1.99 GB/s of copied payload**, before SHM patch reads, GPU upload, allocator traffic, or the additional memory-bus read/write accounting. This is an arithmetic illustration, not a measured bandwidth result. It does not apply to DMA-BUF payloads.

**Replacement/invariants:** share the old snapshot for semantically empty updates. For partial updates, consider reclaimable snapshot pools or tiled immutable storage only after profiling. The old snapshot must remain immutable while any render/frame owner can access it, and SHM release must still occur only after safe materialization. Deleting `Arc` or mutating shared pixels is not an acceptable optimization.

**Risk/testing/benefit:** stale render snapshots and early buffer release are the main risks. Existing partial-damage tests check correctness, not bytes cloned. Add allocation/copied-byte instrumentation and release-profile benchmarks for empty, 1%, and full damage with delayed frame retirement. Expected benefit is eliminating a full-buffer allocation/copy on eligible commits; the benefit of partial-update pooling depends on how many immutable snapshots are simultaneously live.

### O02 — Root-head selection and scene-work rebuilding rescan global queues

**P2 · performance / latency · Optimization opportunity.**

**Locations:** `src/compositor/state/surface_tree_readiness.rs:71–119`; `src/compositor/state/scene_work.rs:104–194`; callers include `src/compositor/state/frames.rs:455` and `src/compositor/state/surface_pacing.rs:332`.

**Work today:** each readiness-loop iteration builds root heads by scanning each transaction's earlier prefix. It removes one selected transaction, then rebuilds all heads. With N ready transactions for distinct roots, head construction is O(N²) per iteration and the drain is O(N³) comparisons. The eight-per-root cap does not bound the number of roots. Scene-work reconstruction separately rescans commits, callbacks, feedback, FIFO claims across batches, and timing predecessors.

**Why costly/hot:** many fence completions or application roots can produce a burst on the compositor thread exactly when frame admission should be predictable. Rebuilding a small index is reasonable; repeated full reconstruction and predecessor discovery is unnecessary once root queues and dependency transitions are explicit.

**Replacement/invariants:** per-root ordered deques, a bounded fair ready-root queue, dependency→transaction wake mappings, and incrementally maintained owner counters. Keep the current implementation as a reference model in tests. Preserve root order, FIFO barriers, timing generations, safe supersession, teardown, and owner/workspace visibility; do F03–F06 semantics first.

**Risk/testing/benefit:** stale incremental counters or missed wakes can strand commits; fairness can regress. Add randomized differential tests, duplicate/stale readiness events, many roots, and 10/100/1,000-root release benchmarks. Expected benefit: remove cubic predecessor discovery and much global queue traversal; no guaranteed total-runtime complexity is claimed because publication itself has other work.

### O03 — Presentation-target queries repeatedly rediscover root geometry

**P2 · performance / latency · Optimization opportunity.**

**Locations:** `src/compositor/state/active_scene.rs:76–139,219–224`; `src/compositor/state/fullscreen.rs:609–635`; `src/compositor/state/frames.rs:183–189`.

**Work today:** target construction visits unique roots, then `presentation_rect_for_geometry` finds each root in the active surface slice and counts preceding roots. This is O(R×S), becoming quadratic when roots scale with surfaces. `presentation_animation_has_pending_visible` constructs renderable surfaces, targets, and a new visible-key vector before asking whether the animator has pending visible work.

**Why costly/hot:** this runs from frame/work admission queries, not just animation creation. It can allocate and traverse scene state when no animation exists, and fullscreen filtering can allocate another list. Repetition within one unchanged scene generation buys no newer geometry.

**Replacement/invariants:** early-out when the animator has no pending entries; build root ordinal/geometry indices at scene publication and reuse frame-local `NativeFramePresentationTargets`. Key invalidation on placement, stack/cascade ordinal, owner, output, workspace, and presentation-geometry changes—not merely buffer identity.

**Risk/testing/benefit:** stale target geometry can break hit testing and interrupted resize/animation continuity. Existing geometry/fullscreen tests should be retained; add traversal/allocation counters with 0/1/many animating roots, plus raise, workspace switch, popup ownership, and fullscreen transitions. Expected benefit: avoid target construction on idle animation checks and replace repeated root scans with indexed lookup; magnitude needs many-window profiling.

### O04 — Disabled pacing tracing still formats fields in commit/frame paths

**P2 · performance / resource usage · Optimization opportunity.**

**Locations:** `src/compositor/pacing.rs:8–19,36–45`; `src/compositor/state/surface_commits.rs:453–473`; `src/compositor/state/frames.rs:386–426`; `src/compositor/protocols/core.rs` frame/commit logging call sites.

**Work today:** `client_pacing_log` accepts already-owned strings and checks the trace flag inside the function. Callers format IDs, clients, damage flags, and generations before that check. Multiple such sites run per commit and frame. A bounded asynchronous sink protects enabled logging but does not remove disabled-call preparation.

**Why costly/hot:** formatting/heap work is unnecessary when the event will not be recorded. This is more credible than replacing small Arc clones or changing allocator types without a profile, but is still likely below large SHM copies and layout traversal.

**Replacement/invariants:** lazy field builders or a macro checking the cached enabled flag before evaluating fields, retaining the same bounded sink, drop counters, timestamps, and event names. Typhon's closure-based XWayland trace API already demonstrates the appropriate boundary; no logging subsystem rewrite is needed.

**Risk/testing/benefit:** changing timestamp semantics, skipping accidental side effects in field expressions, or losing forensic fields. Require disabled builders never execute and enabled output remains equivalent; count allocations during a representative no-trace commit workload. Expected benefit: eliminate the listed field allocations/formatting on disabled paths, not all frontend allocations.

## State-authority map

Multiple representations are not inherently a defect. Intent, committed content, a render snapshot, and physically presented state must often differ. The problem is when one representation can overwrite another without a named transition and generation check.

| State | Current authorities/representations | Assessment and desired contract |
|---|---|---|
| Surface sampling/mapping | `SurfaceData`; `CachedSubsurfaceCommit`; prepared `PendingSurfaceBuffer`; `CurrentSurfaceBuffer`; `RenderableSurface` | F07–F09 show divergent partial updates. One complete committed mapping, projected into immutable render metadata. |
| Parent-latched state | `SubsurfaceTransactions` pending positions; pending/committed stack maps; tree transaction nodes | F06: pending state is consumed too late. Parent commit owns a placement/stack snapshot. |
| Surface work ordering | synchronized child cache; pending tree vector; legacy/per-surface explicit-sync queue; readiness scanner | F03–F05: limits, timing, and supersession differ by entry path. One transaction ordering model; adapters preserve its ownership. |
| Layer activation/layout | `role.pending`, `role.committed`, `mapped`, `order`, arranged geometry, WM usable area | F06/F10: commit and map edges are conflated with publication/repaint. Keep role snapshot and lifecycle activation separate. |
| X11 resize geometry | property/window snapshot; configure timeline; resize-sync tracker; compositor desired/current/visual geometry; associated Wayland commit | Necessary intent/observed/content separation. F01 breaks acknowledgement linkage. Name every transition and require association/transaction identity. |
| Keyboard focus | compositor seat/focused window; XWM desired focus; pending focus transition; confirmed server focus; `_NET_ACTIVE_WINDOW` | Deliberate split. Existing FocusTracker does not treat property publication as focus confirmation. Avoid collapsing these into one boolean; retain bounded repair. |
| X11↔Wayland identity | private Wayland client/generation registry; X11 window handle; serial association join; compositor surface/window mapping | Useful layered validation. Keep generation + association serial + commit floor at handoffs; raw XID/surface ID is insufficient. |
| Selection ownership | Wayland selection broker/source keys; nested host bridge; dormant XWM `DataBridge` | F11: missing adapter. Broker is the authority; X11 ownership is a protocol projection with transfer identities. |
| Visibility | workspace membership/nonminimized state; active scene; fullscreen culled native list; frame feedback/callback sets | F13: distinguish eligible-to-run, sampled, displayed, and input-visible. A single ambiguous visibility boolean is inadequate. |
| Buffer release | pending/current content; deferred release obligations; frame batches; GPU-use ownership | Split is necessary and broadly well defended. Preserve release-token identity, not just wl_buffer identity. |
| Output facts | native/Wayland output model; XWayland's server-side RandR; XWM `RandrSnapshot` | XWM snapshot is initialized to a 1×1 default and has no update consumer in inspected source (`startup.rs:1390–1403`, `mod.rs:389–391`). Do not present it as a live output authority. This alone does not prove X server RandR is 1×1: XWayland can derive outputs from Wayland. |
| Presentation | frontend feedback lineage; frame/batch IDs; native KMS completion identity; actual render membership | Frame identity is strong; actual content membership must join it for F13. Pageflip ownership alone is not proof every workspace surface was shown. |

## Most dangerous cross-subsystem interactions

1. **Synchronized cache → acquire wait → supersession → callbacks:** replacing one child can erase another surface's latched content and move the only progress signal into a cache waiting for another parent commit (F04).
2. **Pending role mutation → delayed transaction → WM/input:** a correct fence wait allows later uncommitted placement/keyboard state to affect an older transaction (F06). Thread safety does not prevent this ordering race.
3. **XSync alarm → XWM acknowledgement → XWayland commit gate:** the tracker and timeout are locally coherent, but the wire event never reaches the tracker (F01).
4. **x11rb accepted write → private queue → next request:** library-level request ordering is undermined by the transport hiding unflushed bytes while accepting the next request (F02).
5. **Damage conversion → SHM snapshot → render damage:** later GPU damage cannot repair omitted SHM pixels. This is why damage correctness and snapshot-copy optimization must be considered together (F09/O01).
6. **Workspace visibility → fullscreen culling → presentation/FIFO/callback settlement:** a valid output frame can be associated with content it did not render (F13).
7. **First commit → fast-path publication → native timing planner:** planning tests pass only after insertion into a queue that the live first commit bypasses (F05).
8. **Independent bounded managers → aggregate cache lifetime:** downstream transaction/transfer bounds do not bound upstream synchronized storage or guarantee timeout wakeups (F03/F12).

## Comparisons with KWin and Hyprland/Aquamarine

The useful comparison is ownership at boundaries, not code size, language, or class names. Upstream sources below were inspected on September 12, 2026; moving upstream branches are references, while Typhon's revision above remains the audit baseline.

**Commit transactions and explicit sync.** KWin's transaction entries carry captured surface state and buffer references, predecessor/successor relationships, and fence readiness. `tryApply` evaluates readiness and target time together. This is a stronger model for Typhon's F04–F06 boundaries because the delayed object owns the state it eventually applies, and ordering follows shared surfaces. Typhon should preserve its explicit IDs, safe early-render timing evidence, and release tokens rather than importing KWin's object-lifetime assumptions. [KWin transaction.cpp](https://raw.githubusercontent.com/KDE/kwin/master/src/wayland/transaction.cpp).

**State queues and damage.** Hyprland's surface commit handler enqueues a copied pending state before asynchronous fence/timing scheduling; mapping changes mark appropriate state/damage bits. The relevant lesson is complete captured state and explicit mapping invalidation, not a recommendation to replace Typhon's scene representation. Typhon's immutable SHM snapshots provide ownership guarantees that an alternative memory strategy must retain. [Hyprland core Compositor.cpp](https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/protocols/core/Compositor.cpp).

**Selection and DND.** KWin routes selection/property/client-message events into selection handling and transfers. Hyprland also explicitly routes selection events and XFixes owner notifications in its XWM, with FD transfer handling. These examples establish why a generation-safe `SelectionBridge` model alone cannot provide interoperability: there must be an event→ownership→conversion→transfer→terminal path. For Typhon, integrating an X11 backend into the existing Wayland broker is stronger than creating competing ownership stores. [KWin selection.cpp](https://raw.githubusercontent.com/KDE/kwin/master/src/xwayland/selection.cpp), [Hyprland XWM.cpp](https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/xwayland/XWM.cpp).

**Event-loop/output integration.** Aquamarine's DRM scheduling tracks frame scheduling/in-flight state and removes redundant idle frame work once a real commit owns the frame. This is consistent with preserving Typhon's event-driven native output and typed in-flight ownership, not an argument for polling or moving frontend protocol state into the backend. Feedback membership must remain above the output backend: a pageflip identifies an output frame, not every surface in the workspace. [Aquamarine DRM.cpp](https://raw.githubusercontent.com/hyprwm/aquamarine/main/src/backend/drm/DRM.cpp).

**Resize/focus/client lifecycle.** Typhon's desired-versus-confirmed focus, configure sequence tracking, generation-qualified associations, bounded repairs, and timeout fallbacks are useful strengths. F01 requires a protocol-correct alarm adapter, not a replacement window manager or a shorter timeout. Likewise, another compositor's broad property rereads are not evidence that Typhon should abandon asynchronous property epochs.

## Missing regression, property, stress, and measurement coverage

The individual findings specify targeted tests. The highest-value shared test facilities are:

- **A protocol-to-scene transaction oracle:** generate commit, attach/detach, source/destination set/unset, transform/scale, sync/desync, parent position/stack, timing/FIFO, acquire-ready, and destroy operations. Compare a simple sequential semantic model with all published fields and obligations. Test equivalence only for explicitly permitted coalescing.
- **A terminal-ownership ledger:** for each callback, feedback, buffer release token, acquire watch, X11 transfer, and resize transaction, require exactly one current owner and a valid eventual terminal outcome. Include shutdown/restart at every intermediate state.
- **A wire-level XWM harness:** encode/decode actual SyncAlarmNotify, property replies, focus sequences, SelectionRequest/Notify, XFixes owner events, XDND messages, and short transport writes. Calling tracker helpers directly is insufficient.
- **A three-party subsurface test:** parent plus independently updating siblings, one delayed fence, another later child commit, no further parent commit. This is the critical F04/F06 composition missing from single-root happy paths.
- **A sampled-content presentation oracle:** compare rendered/scanout/cursor-plane content IDs with feedback outcomes through fullscreen culling, workspace change, frame failure, retry, and late pageflip. Keep callback scheduling policy independently testable.
- **Resource isolation stress:** large cached synchronized bursts, many roots, callback floods, slow selection readers, owner death, and generation restart. Assert aggregate entries/bytes/FDs and another client's latency, not just one queue's length.
- **Release-profile work counters:** SHM bytes copied, allocations, layout passes, root lookups, queue comparisons, property requests, flushes, and wakeups per client commit/output frame. Record median/p95/p99 frame service time; test idle, video, animated panels, live resize, and 10/100/1,000 roots.
- **Test-process isolation:** FD-reuse and global trace retention tests can interfere with unrelated parallel tests. Run descriptor-number manipulation and process-global state tests in isolated subprocesses or with an appropriately shared serialization mechanism; do not interpret a flaky infrastructure assertion as proof of a production lifecycle bug.

## Recommended implementation order

1. Land wire/transport reproductions and fix F02 and F01 in small independent changes. Retain generation cleanup and timeout fallback. These have high confidence and immediate client impact.
2. Establish aggregate cache accounting and the F03 exhaustion policy. Add the P/A/B loss-of-state reproduction for F04 before altering coalescing.
3. Define the complete immutable committed-surface/role snapshot and allowed transaction reduction rules. Address F04–F06 together at the architecture level, but land independently testable behavior changes. Do not optimize the queue while its semantics remain split.
4. Repair mapping propagation/merging and coordinate conversion (F07–F09), using a shared mapping oracle. Preserve resize continuity snapshots and safe SHM release.
5. Separate layer map/activation from repaint (F10). This provides both correctness and likely performance benefit with a narrow boundary.
6. Bind frame feedback to actual content membership (F13), including composited, direct-scanout, and cursor paths. Decide hidden callback/FIFO policy explicitly rather than deriving it accidentally from feedback code.
7. Repair F12, then implement the F11 selection/PRIMARY/XDND adapter in stages with real clients and backpressure. Report capabilities honestly until each route works.
8. Measure and pursue O01–O04. Start with empty-damage SHM sharing and lazy trace fields; use measured pressure to justify more invasive snapshot pooling or indexed ready queues. Require end-to-end frame-time and idle-wakeup evidence, not only operation-count improvements.

## Inspected areas without an additional meaningful defect established

These are bounded positive observations, not certification of the whole subsystem:

| Area | Evidence/inspection | Assessment |
|---|---|---|
| DMA-BUF release identity and frame retirement | `src/compositor/state/frames.rs`; `src/compositor/state/frame_tests.rs` current-token, same-buffer/different-release-token, retry, and mismatched-batch tests | Explicit release obligations are not conflated with buffer identity; current-token protection and batch ownership are valuable. No additional early-release defect established in those paths. Hardware GPU-use retirement remains untested here. |
| Frame callbacks and failure handling | `src/compositor/state/frame_callbacks.rs`; `src/compositor/state/frames.rs`; frame tests | Admission versus presentation fallback is intentional; retry/abandonment has explicit settlement. Callback completion at admission is not itself a presentation correctness bug. F13 concerns actual feedback membership. |
| XWayland association/restart | `src/xwayland/association.rs`; `src/xwayland/xwm/association.rs`; `src/xwayland/xwm/mod.rs:545–563`; service and native event-loop tests | Private association, generation-qualified handles, association serials, and stale-unregister defenses are strong. No additional cross-generation association defect established; a broad kill/restart/live-render stress test remains desirable. |
| XWM focus and ConfigureNotify | `src/xwayland/xwm/focus.rs`; `src/xwayland/xwm/events.rs:298–348`; `events_regression_tests.rs` | Desired/pending/confirmed focus, ICCCM focus models, sequence classification, and bounded repair are deliberately distinct. No evidence that EWMH property publication is incorrectly used as focus confirmation. F01 is the independently identified resize fault. |
| Adoption, transients, override-redirect, decoration boundaries | `src/xwayland/xwm/adoption.rs`; `src/xwayland/xwm/window.rs`; property/event regression tests | Readiness gates distinguish observed, mapped, associated, property-ready, and buffer-ready states. Unmap/admission cancellation and override-redirect classification have explicit paths. No further high-confidence defect established; shaped-window compatibility is a declared rectangular fallback. |
| XDG shell and resize state | `src/compositor/state/subsurfaces.rs:112–143`; XDG/window tests; captured resize state in `surface_transactions.rs` | Initial empty commit/configure gating and captured resize metadata are useful. No separate lifecycle defect established beyond the transaction/mapping boundaries reported above. |
| Wayland selection/data-control and DND | `src/compositor/state/selection_runtime.rs:349–421`; data-device tests | Source-key/client/liveness validation and FD handoff avoid a compositor-owned blocking copy in the ordinary broker. Native Wayland operation should not be conflated with absent X11 bridging. |
| Pointer constraints/relative pointer | `src/compositor/tests/input_output/relative_and_constraints.rs`; captured constraint state in `src/compositor/subsurface.rs` | Tests exercise backend activation, stale identities, focused recipients, generation epochs, and destroy/cancel semantics. No additional material input-lifetime defect established in those paths. |
| Idle inhibition and cursor lifecycle | `src/compositor/state/input_dispatch.rs:15–84`; output/keyboard/cursor and constraint tests | Resource/client/liveness and scene effectiveness are checked; hardware/input backend activation is not silently equated with protocol request acceptance. Visibility-policy changes must revisit inhibition; fullscreen occlusion is not certified here. |
| Workspace protocol | `src/compositor/protocols/workspace.rs`; `src/compositor/workspace_protocol.rs:244–312,358–394,440–466` | Client-qualified bounded manager state, queued activation consumed on commit, and no advertisement of unsupported group mutations. Broad workspace-state republishing exists but was not ranked without evidence it is a consequential hot path. |
| Toplevel publication/output membership | `src/compositor/server_toplevel.rs`; `src/compositor/toplevel_publication.rs`; output-model tests | Inspected publication/teardown and per-client output membership boundaries did not yield another high-confidence finding. Single-output policy is not itself a multi-output defect. This is shallower coverage than transaction/XWM boundaries. |
| Native output scheduling/explicit sync | `src/native_output/runtime/cycle_dispatch.rs`; `presentation_cycle.rs`; native event-loop tests | Typed completion identity, explicit readiness, and retry ownership should be preserved. No justification found for replacing event-driven work with periodic polling. GPU/driver-specific timing and cross-device DMA-BUF behavior require hardware validation. |

Additional limitations worth tracking, without promoting them into defects unsupported by runtime evidence:

- XWM's RandR snapshot is a foundation/default object, not proof of published X server geometry. Verify real `xrandr`/EWMH workarea after output mode/scale/reservation changes before designing an output bridge.
- Shape events are deliberately ignored with rectangular fallback (`events.rs:348–352`). Test affected legacy applications and expose the limitation; do not claim full Shape support merely because the extension version was negotiated.
- Drag-icon buffers share the retained unassigned-buffer path (`surface_commits.rs:1645–1646`). The inspected paths did not establish a renderer consuming the drag icon. Verify visible native drag icons before declaring DND visually complete; this was not developed into a separate high-confidence finding.
- Property refresh issues all property kinds and cancels the prior epoch (`properties.rs:176–222`). Creation/map refresh overlap is a candidate for request-count measurement, but compatibility-driven rereads should not be removed based on request count alone. No blanket recommendation to batch or eliminate protocol flushes is justified without preserving ordering and wake ownership.

## Verification and reproducibility

All compilation used the existing repository/target directory. No alternate build tree or source fixes were created.

Standalone probes include production `connection.rs` and `transfer.rs`. Only the transfer module's generation-key wrapper is supplied locally; I/O/state transitions are production code. The transport probe seeds a valid post-short-write queue state so its ordering assertion is deterministic. These diagnostics are deliberately outside Cargo's normal test targets and are expected to fail on this baseline.

```bash
rtk proxy rustc --edition=2024 --test docs/reports/2026-09-12-frontend-audit-probes.rs \
  -L dependency=target/debug/deps \
  --extern x11rb=target/debug/deps/libx11rb-2cb93c2e8c7deab6.rlib \
  --extern x11rb_protocol=target/debug/deps/libx11rb_protocol-15d479eda831b671.rlib \
  --extern libc=target/debug/deps/liblibc-3ca9a8e3c01cf86b.rlib \
  -o target/frontend-audit-probes
rtk proxy target/frontend-audit-probes --test-threads=1 --nocapture
```

The dependency hashes above are the artifacts present in this workspace; another build must substitute its corresponding compiled dependencies.

Observed probe result: **6 inherited tests passed; 3 audit probes failed**. Transport: new byte at 16,384 instead of 49,152. Transfer: EOF failed to terminate; resumed payload was not delivered.

Initial library run during edits: **2,405 passed, 4 failed, 2 ignored**. Failures were in global XWayland trace retention, including follow-on mutex poisoning. Later logs at the updated tree showed **2,409 passed, 1 failed, 2 ignored**, with failures varying between an FD-lifetime test and a stale-XWayland-unregister test. The latter failed at its `dup2` fixture assertion (`src/native/event_loop.rs:1599`), before asserting compositor unregister behavior. These are not promoted to production defects. A transient borrow-check error during concurrent source edits was subsequently absent and is not a final-baseline finding.

Final-baseline serial verification, `rtk proxy cargo test --lib --no-fail-fast -- --test-threads=1`: **2,410 passed, 0 failed, 2 ignored**, exit 0, 201.91 seconds. This does not include every separate Cargo integration-test binary or ignored hardware tests. The passing serial suite does not invalidate the standalone failures; it shows those sequences are not asserted by the existing library tests. Earlier parallel/global-state failures are recorded separately above rather than hidden by this passing run.

Structural verification used project `home-agony-GitHub-Typhon`, with evidence-path checks at graph generations `2026-09-12T03:49:48Z` and `2026-09-12T04:12:21Z`. Relevant result pagination and source fallbacks were used. Graph edges occasionally resolved generic method names incorrectly or missed a direct call, so exact source, not an inferred call graph alone, supports material claims. Coverage metadata reported no skipped source files and two partial parse ranges: `src/native_output/runtime/cycle_dispatch.rs:1464` and `src/native_output/runtime/presentation_cycle.rs:155`; both were read directly and are test-only attributes inside destructuring. Clean coverage metadata is a best-effort signal, not proof of exhaustive parsing or audit coverage. No subagents were used.

## External primary references

- X.Org, [X Synchronization Extension Protocol](https://xorg.freedesktop.org/archive/current/doc/xextproto/sync.html): alarm versus counter event semantics.
- Wayland protocols, [commit-timing-v1](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/staging/commit-timing/commit-timing-v1.xml): not-before and ordering contract.
- Wayland protocols, [viewporter](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/stable/viewporter/viewporter.xml): independent double-buffered crop/scale state and coordinate order.
- Wayland protocols, [presentation-time](https://raw.githubusercontent.com/wayland-mirror/wayland-protocols/main/stable/presentation-time/presentation-time.xml): content-update presentation/discard semantics.
- KDE KWin, [transaction.cpp](https://raw.githubusercontent.com/KDE/kwin/master/src/wayland/transaction.cpp) and [selection.cpp](https://raw.githubusercontent.com/KDE/kwin/master/src/xwayland/selection.cpp): comparison boundaries described above.
- Hyprland, [core Compositor.cpp](https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/protocols/core/Compositor.cpp) and [XWM.cpp](https://raw.githubusercontent.com/hyprwm/Hyprland/main/src/xwayland/XWM.cpp): state capture and selection event integration.
- Aquamarine, [DRM.cpp](https://raw.githubusercontent.com/hyprwm/aquamarine/main/src/backend/drm/DRM.cpp): event-driven output scheduling boundary.
