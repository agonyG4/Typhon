# Typhon Core + stable xdg-shell compliance matrix

This is the auditable request/event inventory from the locked XML and dispatch
audit. “XML since” is 1 when the XML has no `since` attribute. The
`classification` column is the Rust dispatch ownership category; `status` is
always exactly one of `Implemented`, `Partial`, `Gap`, or `Not applicable`.
Evidence for every high-risk implemented or partial row is in the evidence
tables below; a gap is deliberately not claimed as compliant.

The Rust-side high-risk request inventory is machine-checked by
`core_xdg_request_contracts_are_classified_and_version_bounded` and lives in
`src/compositor/protocols/versions.rs`. The full request/event inventory below
is retained as the human-auditable source; any future `Gap` row is an explicit
remaining work item and is not claimed as compliant.

Required classifications are exactly: `Implemented`, `ValidatedNoOp`,
`BackendOwned`, `CapabilityRejected`, `ProtocolError`, and
`DestroyedResourceNoFurtherDispatch`.

## Inventory counts

| source | interfaces | requests | events | advertised bound |
|---|---:|---:|---:|---|
| locked core `wayland.xml` through `wl_subcompositor` | 22 | 70 | 61 | per `wl_*` global / resource versions |
| locked stable `xdg-shell.xml` | 5 | 36 | 9 | `xdg_wm_base` v6 |

## Advertised-version consistency inventory

These rows are intentionally compact because extension behavior is outside the
Core/XDG request matrix, but they must remain synchronized with
`src/compositor/protocols/versions.rs`.

| interface | advertised | status |
|---|---:|---|
| `wl_compositor` | 6 | Implemented |
| `wl_subcompositor` | 1 | Implemented |
| `wl_shm` | 2 | Implemented |
| `wl_data_device_manager` | 3 | Partial |
| `wp_viewporter` | 1 | Partial |
| `wp_fractional_scale_manager_v1` | 1 | Partial |
| `wp_presentation` | 2 | Partial |
| `zwlr_layer_shell_v1` | 4 | Partial |
| `wp_color_manager_v1` | 1 | Partial |
| `zwp_relative_pointer_manager_v1` | 1 | Partial |
| `zwp_pointer_constraints_v1` | 1 | Partial |
| `wp_pointer_warp_v1` | 1 | Partial |
| `zwp_idle_inhibit_manager_v1` | 1 | Partial |
| `zwp_primary_selection_device_manager_v1` | 1 | Partial |
| `ext_data_control_manager_v1` | 1 | Partial |
| `zxdg_decoration_manager_v1` | 1 | Partial |
| `zwp_linux_dmabuf_v1` | 4 | Partial |
| `wp_linux_drm_syncobj_manager_v1` | 1 | Partial |
| `wl_drm` | 2 | Partial |
| `xdg_activation_v1` | 1 | Partial |
| `astrea_shortcuts_manager_v1` | 1 | Partial |
| `astrea_shell_control_manager_v1` | 1 | Partial |
| `xdg_wm_base` | 6 | Implemented |
| `wl_output` | 4 | Implemented |
| `wl_seat` | 7 | Implemented |
| `xwayland_shell_v1` | 1 | Implemented for the active private XWayland client |

The current dispatch audit still finds generated-protocol wildcard arms in
some extension handlers. Covered Core/XDG requests have explicit dispatch or
are documented below as validated no-ops; the remaining wildcard inventory is
kept in the source-audit section and is not used to hide a supported request.

## Core request/event inventory

The rows below enumerate every request and event in the locked Core interfaces
covered by this milestone. Rows with a since-version above the advertised
bound are retained as source-audit rows but are not part of Typhon’s supported
contract until the bound is upgraded, which this milestone forbids.

| interface | advertised | request/event | XML since | dispatch owner | classification | status |
|---|---:|---|---:|---|---|---|
| `wl_compositor` | 6 | request `create_surface` | 1 | `protocols/core.rs` | Implemented | Implemented |
| `wl_compositor` | 6 | request `create_region` | 1 | `protocols/core.rs` | Implemented | Implemented |
| `wl_compositor` | 6 | request `release` | 5 | `protocols/core.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_shm` | 2 | request `create_pool` | 1 | `protocols/buffers.rs` | ProtocolError | Implemented |
| `wl_shm` | 2 | event `format` | 1 | `protocols/buffers.rs` | Implemented | Implemented |
| `wl_shm` | 2 | request `release` | 1 | `protocols/buffers.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_shm_pool` | n/a | request `create_buffer` | 1 | `protocols/buffers.rs` | ProtocolError | Implemented |
| `wl_shm_pool` | n/a | request `destroy` | 1 | `protocols/buffers.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_shm_pool` | n/a | request `resize` | 1 | `protocols/buffers.rs` | ProtocolError | Implemented |
| `wl_buffer` | n/a | request `destroy` | 1 | `protocols/buffers.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_buffer` | n/a | event `release` | 1 | frame-owned release path | BackendOwned | Implemented |
| `wl_data_offer` | n/a | request `accept` | 1 | `protocols/data_device.rs` | Implemented | Partial |
| `wl_data_offer` | n/a | request `receive` | 1 | `protocols/data_device.rs` | Implemented | Partial |
| `wl_data_offer` | n/a | request `destroy` | 1 | `protocols/data_device.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_data_offer` | n/a | event `offer` | 1 | data-device state | Implemented | Implemented |
| `wl_data_offer` | n/a | request `finish` | 3 | `protocols/data_device.rs` | ProtocolError | Implemented |
| `wl_data_offer` | n/a | request `set_actions` | 3 | `protocols/data_device.rs` | ProtocolError | Implemented |
| `wl_data_offer` | n/a | event `source_actions` | 3 | data-device state | Implemented | Implemented |
| `wl_data_offer` | n/a | event `action` | 3 | data-device state | Implemented | Partial |
| `wl_data_source` | n/a | request `offer` | 1 | `protocols/data_device.rs` | Implemented | Implemented |
| `wl_data_source` | n/a | request `destroy` | 1 | `protocols/data_device.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_data_source` | n/a | event `target` | 1 | data-device state | Implemented | Implemented |
| `wl_data_source` | n/a | event `send` | 1 | clipboard bridge | BackendOwned | Partial |
| `wl_data_source` | n/a | event `cancelled` | 1 | data-device state | Implemented | Implemented |
| `wl_data_source` | n/a | request `set_actions` | 3 | `protocols/data_device.rs` | ProtocolError | Implemented |
| `wl_data_source` | n/a | event `dnd_drop_performed` | 3 | data-device state | Implemented | Implemented |
| `wl_data_source` | n/a | event `dnd_finished` | 3 | data-device state | Implemented | Implemented |
| `wl_data_source` | n/a | event `action` | 3 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | request `start_drag` | 1 | `protocols/data_device.rs` | ProtocolError | Implemented |
| `wl_data_device` | n/a | request `set_selection` | 1 | `protocols/data_device.rs` | Implemented | Partial |
| `wl_data_device` | n/a | event `data_offer` | 1 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | event `enter` | 1 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | event `leave` | 1 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | event `motion` | 1 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | event `drop` | 1 | data-device state | Implemented | Implemented |
| `wl_data_device` | n/a | event `selection` | 1 | selection/clipboard | Implemented | Partial |
| `wl_data_device` | n/a | request `release` | 2 | `protocols/data_device.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_data_device_manager` | 3 | request `create_data_source` | 1 | `protocols/data_device.rs` | Implemented | Implemented |
| `wl_data_device_manager` | 3 | request `get_data_device` | 1 | `protocols/data_device.rs` | Implemented | Implemented |
| `wl_data_device_manager` | 3 | request `release` | 2 | `protocols/data_device.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_surface` | 6 | request `destroy` | 1 | `protocols/core.rs` | ProtocolError | Partial |
| `wl_surface` | 6 | request `attach` | 1 | `protocols/core.rs` | ProtocolError | Implemented |
| `wl_surface` | 6 | request `damage` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_surface` | 6 | request `frame` | 1 | `protocols/core.rs` | Implemented | Implemented |
| `wl_surface` | 6 | request `set_opaque_region` | 1 | `protocols/core.rs` | Implemented | Implemented |
| `wl_surface` | 6 | request `set_input_region` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_surface` | 6 | request `commit` | 1 | `protocols/core.rs` | ProtocolError | Partial |
| `wl_surface` | 6 | event `enter` | 1 | output membership | Implemented | Implemented |
| `wl_surface` | 6 | event `leave` | 1 | output membership | Implemented | Implemented |
| `wl_surface` | 6 | request `set_buffer_transform` | 2 | `protocols/core.rs` | ProtocolError | Implemented |
| `wl_surface` | 6 | request `set_buffer_scale` | 3 | `protocols/core.rs` | ProtocolError | Partial |
| `wl_surface` | 6 | request `damage_buffer` | 4 | `protocols/core.rs` | Implemented | Partial |
| `wl_surface` | 6 | request `offset` | 5 | `protocols/core.rs` | Implemented | Partial |
| `wl_surface` | 6 | event `preferred_buffer_scale` | 6 | output membership | Implemented | Implemented |
| `wl_surface` | 6 | event `preferred_buffer_transform` | 6 | output membership | Implemented | Implemented |
| `wl_surface` | 6 | request `get_release` | 6 | explicit-sync/frame ownership | BackendOwned | Partial |
| `wl_seat` | 7 | event `capabilities` | 1 | `protocols/input.rs` | Implemented | Implemented |
| `wl_seat` | 7 | request `get_pointer` | 1 | `protocols/input.rs` | Implemented | Implemented |
| `wl_seat` | 7 | request `get_keyboard` | 1 | `protocols/input.rs` | Implemented | Implemented |
| `wl_seat` | 7 | request `get_touch` | 1 | `protocols/input.rs` | CapabilityRejected | Implemented |
| `wl_seat` | 7 | event `name` | 2 | `protocols/input.rs` | Implemented | Implemented |
| `wl_seat` | 7 | request `release` | 5 | `protocols/input.rs` | DestroyedResourceNoFurtherDispatch | Partial |
| `wl_pointer` | n/a | request `set_cursor` | 1 | `protocols/input.rs` | ProtocolError | Partial |
| `wl_pointer` | n/a | event `enter` | 1 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `leave` | 1 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `motion` | 1 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `button` | 1 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `axis` | 1 | input state | Implemented | Partial |
| `wl_pointer` | n/a | request `release` | 3 | `protocols/input.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_pointer` | n/a | event `frame` | 5 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `axis_source` | 5 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `axis_stop` | 5 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `axis_discrete` | 5 | input state | Implemented | Partial |
| `wl_pointer` | n/a | event `axis_value120` | 8 | input state | Implemented | Outside advertised seat pointer resource bound unless bound supports it |
| `wl_pointer` | n/a | event `axis_relative_direction` | 9 | input state | Implemented | Outside advertised seat pointer resource bound |
| `wl_keyboard` | n/a | event `keymap` | 1 | input state | Implemented | Partial |
| `wl_keyboard` | n/a | event `enter` | 1 | input state | Implemented | Implemented |
| `wl_keyboard` | n/a | event `leave` | 1 | input state | Implemented | Partial |
| `wl_keyboard` | n/a | event `key` | 1 | input state | Implemented | Partial |
| `wl_keyboard` | n/a | event `modifiers` | 1 | input state | Implemented | Partial |
| `wl_keyboard` | n/a | request `release` | 3 | `protocols/input.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_keyboard` | n/a | event `repeat_info` | 4 | input state | Implemented | Partial |
| `wl_output` | 4 | event `geometry` | 1 | output bindings | Implemented | Implemented |
| `wl_output` | 4 | event `mode` | 1 | output bindings | Implemented | Implemented |
| `wl_output` | 4 | event `done` | 2 | output bindings | Implemented | Partial |
| `wl_output` | 4 | event `scale` | 2 | output bindings | Implemented | Implemented |
| `wl_output` | 4 | request `release` | 3 | `protocols/input.rs` | DestroyedResourceNoFurtherDispatch | Partial |
| `wl_output` | 4 | event `name` | 4 | output bindings | Implemented | Implemented |
| `wl_output` | 4 | event `description` | 4 | output bindings | Implemented | Implemented |
| `wl_region` | n/a | request `destroy` | 1 | `protocols/core.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_region` | n/a | request `add` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_region` | n/a | request `subtract` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_subcompositor` | 1 | request `destroy` | 1 | `protocols/core.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `wl_subcompositor` | 1 | request `get_subsurface` | 1 | `protocols/core.rs` | ProtocolError | Implemented |
| `wl_subsurface` | n/a | request `destroy` | 1 | `protocols/core.rs` | DestroyedResourceNoFurtherDispatch | Partial |
| `wl_subsurface` | n/a | request `set_position` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_subsurface` | n/a | request `place_above` | 1 | `protocols/core.rs` | ProtocolError | Implemented |
| `wl_subsurface` | n/a | request `place_below` | 1 | `protocols/core.rs` | ProtocolError | Implemented |
| `wl_subsurface` | n/a | request `set_sync` | 1 | `protocols/core.rs` | Implemented | Partial |
| `wl_subsurface` | n/a | request `set_desync` | 1 | `protocols/core.rs` | Implemented | Partial |

## Stable xdg-shell request/event inventory

| interface | advertised | request/event | XML since | dispatch owner | classification | status |
|---|---:|---|---:|---|---|---|
| `xdg_wm_base` | 6 | request `destroy` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_wm_base` | 6 | request `create_positioner` | 1 | `protocols/xdg.rs` | Implemented | Implemented |
| `xdg_wm_base` | 6 | request `get_xdg_surface` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_wm_base` | 6 | request `pong` | 1 | `protocols/xdg.rs` | ValidatedNoOp | Implemented |
| `xdg_wm_base` | 6 | event `ping` | 1 | xdg lifecycle | Implemented | Not applicable |
| `xdg_positioner` | n/a | request `destroy` | 1 | `protocols/xdg.rs` | DestroyedResourceNoFurtherDispatch | Implemented |
| `xdg_positioner` | n/a | request `set_size` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_positioner` | n/a | request `set_anchor_rect` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_positioner` | n/a | request `set_anchor` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_positioner` | n/a | request `set_gravity` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_positioner` | n/a | request `set_constraint_adjustment` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_positioner` | n/a | request `set_offset` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_positioner` | n/a | request `set_reactive` | 3 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_positioner` | n/a | request `set_parent_size` | 3 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_positioner` | n/a | request `set_parent_configure` | 3 | `protocols/xdg.rs` | Implemented | Implemented |
| `xdg_surface` | n/a | request `destroy` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_surface` | n/a | request `get_toplevel` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_surface` | n/a | request `get_popup` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_surface` | n/a | request `set_window_geometry` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_surface` | n/a | request `ack_configure` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_surface` | n/a | event `configure` | 1 | xdg lifecycle | Implemented | Partial |
| `xdg_toplevel` | n/a | request `destroy` | 1 | `protocols/xdg.rs` | DestroyedResourceNoFurtherDispatch | Partial |
| `xdg_toplevel` | n/a | request `set_parent` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_toplevel` | n/a | request `set_title` | 1 | `protocols/xdg.rs` | Implemented | Implemented |
| `xdg_toplevel` | n/a | request `set_app_id` | 1 | `protocols/xdg.rs` | Implemented | Implemented |
| `xdg_toplevel` | n/a | request `show_window_menu` | 1 | `protocols/xdg.rs` | ValidatedNoOp | Implemented |
| `xdg_toplevel` | n/a | request `move` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_toplevel` | n/a | request `resize` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_toplevel` | n/a | request `set_max_size` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_toplevel` | n/a | request `set_min_size` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_toplevel` | n/a | request `set_maximized` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_toplevel` | n/a | request `unset_maximized` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_toplevel` | n/a | request `set_fullscreen` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_toplevel` | n/a | request `unset_fullscreen` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_toplevel` | n/a | request `set_minimized` | 1 | `protocols/xdg.rs` | Implemented | Partial |
| `xdg_toplevel` | n/a | event `configure` | 1 | xdg lifecycle | Implemented | Partial |
| `xdg_toplevel` | n/a | event `close` | 1 | window lifecycle | Implemented | Partial |
| `xdg_toplevel` | n/a | event `configure_bounds` | 4 | xdg lifecycle | Implemented | Not applicable |
| `xdg_toplevel` | n/a | event `wm_capabilities` | 5 | xdg lifecycle | Implemented | Implemented |
| `xdg_popup` | n/a | request `destroy` | 1 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_popup` | n/a | request `grab` | 1 | `protocols/xdg.rs` | ProtocolError | Partial |
| `xdg_popup` | n/a | event `configure` | 1 | xdg lifecycle | Implemented | Partial |
| `xdg_popup` | n/a | event `popup_done` | 1 | xdg lifecycle | Implemented | Partial |
| `xdg_popup` | n/a | request `reposition` | 3 | `protocols/xdg.rs` | ProtocolError | Implemented |
| `xdg_popup` | n/a | event `repositioned` | 3 | xdg lifecycle | Implemented | Implemented |

## XDG same-role reassociation extension

Typhon preserves permanent `wl_surface` roles and rejects role switching. For
compatibility with real clients and KWin-observed behavior, a destroyed XDG
association may be recreated only when the new role equals the permanent XDG
role and the surface is dormant and content-free. The recreated association
starts a fresh configure lifecycle; no prior role-instance state is restored.
When an XDG role object is destroyed, unpublished explicit-sync commits and
surface-tree transactions owned by that live role are retired before the role
instance is deactivated; published current/pageflip-owned buffers are left to
the normal unmap/presentation lifecycle.

## High-risk evidence

Each row below gives the owning source, a concrete test, the invalid-input
error (or `none` when the XML defines no error), the bound, and the remaining
limitation. The status in this table uses the same four-value vocabulary as
the inventory.

| request/event | owner | test | invalid error | bound | status | limitation |
|---|---|---|---|---:|---|---|
| `wl_surface.attach` | `protocols/core.rs` | `attach_nonzero_offset_v5_posts_invalid_offset` | `wl_surface.invalid_offset` | 6 | Implemented | v4 retains legacy offsets |
| `wl_surface.set_opaque_region` | `protocols/core.rs` | `opaque_region_is_copied_and_double_buffered` | none | 6 | Implemented | culling optimization only |
| `wl_surface.set_buffer_transform` | `protocols/core.rs` | `invalid_transform_posts_invalid_transform` | `wl_surface.invalid_transform` | 6 | Implemented | renderer transform is client-buffer state |
| `wl_surface.set_buffer_scale` | `protocols/core.rs` | `invalid_scale_is_a_wire_error_and_does_not_disconnect_another_client` | `wl_surface.invalid_scale` | 6 | Implemented | client-buffer scale is validated before publication |
| `wl_surface.commit` | `state/surface_transactions.rs` | `wayland_client_surface_commit_creates_renderable_shm_snapshot`, `output_membership_reconciles_leave_and_remap_enter` | `wl_surface.invalid_size` | 6 | Implemented | field ownership is audited in `SURFACE_PENDING_STATE_AUDIT.md` |
| `wl_surface.enter/leave` | `state/output_membership.rs` | `output_membership_reconciles_leave_and_remap_enter`, `output_membership_reconciles_move_outside_without_duplicate_enter`, `output_production_model_runs_10_000_operations` | none | 6 | Implemented | geometric overlap and same-client output resources are reconciled; the production-backed model covers resource ownership, effective subsurface mapping, and teardown |
| `wl_surface.preferred_buffer_scale/transform` | `state/output_membership.rs` | `preferred_buffer_events_are_version_gated_and_deduplicated`, `preferred_buffer_events_emit_changed_values_and_default_returns_once`, `output_production_model_runs_10_000_operations` | none | 6 | Implemented | default events are suppressed until a preference changes and the production-backed model checks deduplication/version gating |
| `wl_seat.get_touch` | `protocols/input.rs` | `get_touch_without_advertised_touch_capability_is_a_wire_error` | `wl_seat.missing_capability` | 7 | Implemented | touch remains intentionally unadvertised |
| `wl_subcompositor.get_subsurface` | `protocols/core.rs` | `invalid_subsurface_sibling_is_a_wire_error_and_does_not_disconnect_another_client` | `wl_subcompositor.bad_surface` | 1 | Implemented | tree policy remains single-output |
| `wl_shm.create_pool` | `protocols/buffers.rs` | `invalid_shm_pool_size_is_a_wire_error_and_does_not_disconnect_another_client` | `wl_shm.invalid_fd` / locked size mapping | 2 | Implemented | request-time FD usability depends on host FD |
| `wl_shm_pool.create_buffer` | `protocols/buffers.rs` | `wl_shm_pool_resize_growth_enables_buffer_above_initial_size` | `wl_shm.invalid_format`, `invalid_stride`, `invalid_fd` | 2 | Implemented | only advertised formats are accepted |
| `wl_data_source.set_actions` | `protocols/data_device.rs` | `sourced_wire_drag_ask_resolves_to_copy_before_finished`, `v3_start_drag_without_set_actions_is_a_wire_protocol_error`, `dnd_production_state_seeded_model_runs_100_000_transitions` | `wl_data_source.invalid_action_mask`, `invalid_source` | 3 | Implemented | modifier-driven action overrides remain optional policy |
| `wl_data_offer.set_actions` | `protocols/data_device.rs` | `sourced_wire_drag_ask_resolves_to_copy_before_finished`, `dnd_production_state_seeded_model_runs_100_000_transitions` | `wl_data_offer.invalid_action_mask`, `invalid_action` | 3 | Implemented | modifier-driven action overrides remain optional policy; duplicate unchanged actions are suppressed |
| `wl_data_offer.finish` | `protocols/data_device.rs`, `state/data_device.rs` | `sourced_wire_drag_ask_resolves_to_copy_before_finished`, `sourced_wire_drag_target_disconnect_after_drop_cancels_once`, `sourced_wire_drag_offer_destroy_after_drop_cancels_once`, `dnd_production_state_seeded_model_runs_100_000_transitions` | `wl_data_offer.invalid_finish` | 3 | Implemented | native toolkit coverage remains required; deterministic normal, ASK, destruction, and disconnect paths are covered |
| `wl_data_device.start_drag` | `protocols/data_device.rs`, `state/data_device.rs` | `sourced_wire_drag_target_disconnect_before_drop_cancels_once`, `sourced_wire_drag_target_disconnect_while_ask_is_unresolved_cancels_once`, `source_less_wire_drag_with_icon_reserves_a_permanent_drag_icon_role`, `dnd_production_state_seeded_model_runs_100_000_transitions` | `wl_data_device.used_source`, `wl_data_device.role`, `wl_data_source.invalid_source` | 3 | Implemented | source-less, icon, source-use, post-drop cancellation, and teardown transitions are covered; native toolkit validation remains required |
| `wl_data_device.set_selection` | `protocols/data_device.rs` | `v3_source_set_actions_then_selection_is_a_wire_protocol_error`, clipboard selection tests | `wl_data_device.used_source` | 3 | Partial | focus/serial policy remains shared with clipboard bridge |
| `xdg_wm_base.get_xdg_surface` | `protocols/xdg.rs` | `sober_style_toplevel_reassociation_on_same_wl_surface_is_supported`, `sober_style_popup_reassociation_on_same_wl_surface_is_supported`, `xdg_toplevel_role_destroy_retires_unpublished_explicit_sync_work`, `xdg_popup_role_destroy_retires_unpublished_explicit_sync_work`, `duplicate_xdg_association_on_same_wl_surface_is_rejected`, `dormant_xdg_toplevel_reassociation_to_popup_is_rejected`, `dormant_xdg_popup_reassociation_to_toplevel_is_rejected`, `pending_surface_content_rejects_xdg_association_and_preserves_healthy_client`, `committed_surface_content_rejects_xdg_association_and_preserves_healthy_client`, `xdg_role_after_layer_surface_is_rejected`, `xdg_role_after_subsurface_is_rejected_and_healthy_client_survives`, `xdg_role_after_cursor_surface_is_rejected_and_healthy_client_survives`, `source_less_wire_drag_with_icon_reserves_a_permanent_drag_icon_role`, `xdg_buffer_before_initial_configure_is_a_wire_error` | `xdg_wm_base.role`, `xdg_wm_base.invalid_surface_state`, `xdg_surface.already_constructed` | 6 | Implemented | same-role reassociation is a deliberate interoperability extension; association cleanup is shared with client teardown and stale unpublished work is rejected defensively |
| `xdg_wm_base.destroy` | `protocols/xdg.rs` | `wm_base_destroy_with_live_xdg_surfaces_posts_defunct_surfaces` | `xdg_wm_base.defunct_surfaces` | 6 | Implemented | none |
| `xdg_positioner` completeness | `popup.rs` / `protocols/xdg.rs` | `incomplete_positioner_is_not_usable_for_reposition` | `xdg_wm_base.invalid_positioner`, `xdg_positioner.invalid_input` | 6 | Implemented | parent serial is policy metadata per XML |
| `xdg_surface.ack_configure` | `state/xdg_lifecycle.rs` | `unknown_configure_acknowledgement_is_rejected` | `xdg_surface.invalid_serial` | 6 | Implemented | configure role-state application remains in window state |
| `xdg_surface` buffer gate | `state/xdg_lifecycle.rs` | `mapped_surface_accepts_existing_valid_content_with_outstanding_configure` | `xdg_surface.unconfigured_buffer` | 6 | Implemented | remap requires a fresh handshake |
| `xdg_toplevel.wm_capabilities` | `state/windows.rs` | `xdg_toplevel_v5_receives_capabilities_before_initial_configure` | none | 5-6 | Implemented | only maximize/fullscreen are advertised |
| `xdg_popup.reposition` | `protocols/xdg.rs` | `wayland_client_xdg_popup_reposition_sends_repositioned_and_reconfigures` | `xdg_wm_base.invalid_positioner` | 3-6 | Implemented | latest-request policy is deterministic |
| `wl_pointer` axis metadata | `native_output/input/routing.rs`, `state/input_dispatch.rs` | `libinput_v120_conversion_uses_logical_steps_and_signed_remainders`, `libinput_v120_conversion_keeps_devices_and_axes_independent`, `libinput_v120_conversion_does_not_create_discrete_finger_or_continuous_steps`, `wl_pointer_v4_receives_only_legacy_axis_events`, `wayland_client_receives_pointer_axis_from_native_input_bridge` | none | 4-7 | Partial | logical v120 conversion and version/order semantics are deterministic; hardware-specific source/stop combinations still require native validation |
| compliance request fallback | `state_data.rs`, `protocols/*.rs` | `unhandled_request_classification_is_explicit`, `core_xdg_request_contracts_are_classified_and_version_bounded` | none | advertised bounds | Implemented | future/generated fallbacks are classified explicitly; a supported fallback increments `supported_request_unhandled_total` |

`configure_bounds` is `Not applicable`: Typhon deliberately does not
advertise a bounds preference, so it does not send this optional event.
`wm_capabilities` is `Implemented` for v5 and v6 and is version-gated for
v4 resources.

## Deterministic model evidence

| model | owner | fixed seed | required run | assertions |
|---|---|---:|---:|---|
| output membership | `tests/output_model.rs` + `state/output_membership.rs` | `0x4f55_5450_5554_3130` | 10,000 operations | real production registration/reconciliation/mapping/teardown versus independent geometry, same-client resources, enter/leave deduplication, preferred-event gating |
| DnD lifecycle | `tests/data_device.rs` + `state/data_device.rs` | `0xdad5_0000_0042` | 100,000 transitions | real production session/offer/source transitions versus independent source-use, action intersection, ASK resolution, terminal exactly-once behavior, duplicate-terminal counting |

The models are deterministic regression evidence, not a substitute for the
wire-level tests or native toolkit validation. Both test names are required by
the automated acceptance gate and print their seed and operation index on a
failure.

## Required errors to verify against generated enums

The final matrix/test layer must confirm exact enum owner, spelling, numeric
code, and bound-version behavior from the locked generated interfaces before
using each error:

`wl_surface.invalid_scale`, `wl_surface.invalid_transform`,
`wl_surface.invalid_size`, `wl_surface.invalid_offset`,
`wl_surface.defunct_role_object`; `wl_subcompositor.bad_surface`,
`wl_subcompositor.bad_parent`; `wl_shm.invalid_format`,
`wl_shm.invalid_stride`, `wl_shm.invalid_fd`; `wl_seat.missing_capability`;
`wl_data_source.invalid_action_mask`, `wl_data_source.invalid_source`;
`wl_data_device.role`, `wl_data_device.used_source`;
`wl_data_offer.invalid_finish`, `invalid_action_mask`, `invalid_action`,
`invalid_offer`; `xdg_wm_base.role`, `defunct_surfaces`,
`not_the_topmost_popup`, `invalid_popup_parent`, `invalid_surface_state`,
`invalid_positioner`; `xdg_positioner.invalid_input`;
`xdg_surface.not_constructed`, `already_constructed`, `unconfigured_buffer`,
`invalid_serial`, `invalid_size`, `defunct_role_object`;
`xdg_toplevel.invalid_resize_edge`, `invalid_parent`, `invalid_size`; and
`xdg_popup.invalid_grab`.

## Current intentional no-ops

`xdg_wm_base.pong` is a validated no-op because the XML defines no unsolicited
or unmatched-pong error and Typhon does not emit pings in this path. The
optional `xdg_toplevel.configure_bounds` event is not sent because Typhon has
no bounds preference to announce. Neither behavior is a wildcard fallthrough.

## Current checkpoint status

The following behavior is implemented and covered by real-client tests since
the initial inventory was written:

- central advertised versions and per-version global binding;
- wire-level client-isolated errors for invalid scale, transform, attach
  offset, SHM pool size, invalid subsurface sibling, missing touch capability,
  invalid data-source actions, XDG unconfigured buffer, invalid XDG serial,
  and live-role destruction order;
- permanent role reservation separate from live role instances;
- XDG construction/initial empty commit/configure/ack mapping gates and an
  ordered configure ledger;
- request-time SHM format, stride, checked final-row bounds, and pool-growth
  validation;
- typed input serial records, pressed-key keyboard enter state, monotonic
  timestamps, and purpose-specific validators;
- copied/double-buffered input and opaque regions, buffer transform/scale
  validation, and version-gated preferred buffer scale/transform announcements;
- independent XDG initial-handshake and outstanding-configure state, ordered
  ACK retirement, v5/v6 `wm_capabilities` ordering, complete-positioner checks,
  and source-less drag-icon role reservation;
- synchronized subsurface transaction publication and client teardown scrub.

The remaining partial rows are intentionally visible. They identify behavior
that still depends on compositor policy, an external backend, or native
interoperability evidence rather than an untested production transition. The
production-backed DnD model and wire tests cover source/target destruction,
disconnect, post-drop cancellation, ASK resolution, and terminal exactly-once
ownership. Output leave/remap and preferred events are behaviorally covered;
the pending-state audit documents why their split request storage is not
itself a protocol failure.
