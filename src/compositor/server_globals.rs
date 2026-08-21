use super::*;
use crate::astrea_shell_auth::server::astrea_shell_auth_manager_v1;
use wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1;
use wayland_protocols::wp::{
    commit_timing::v1::server::wp_commit_timing_manager_v1,
    content_type::v1::server::wp_content_type_manager_v1,
    cursor_shape::v1::server::wp_cursor_shape_manager_v1, fifo::v1::server::wp_fifo_manager_v1,
    keyboard_shortcuts_inhibit::zv1::server::zwp_keyboard_shortcuts_inhibit_manager_v1,
    tearing_control::v1::server::wp_tearing_control_manager_v1,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn register_minimum_globals(
    display: &DisplayHandle,
    gpu_capabilities: &GpuProtocolCapabilities,
    gpu_buffers_enabled: bool,
    input_capabilities: InputProtocolCapabilities,
    selection_capabilities: SelectionProtocolCapabilities,
    renderer_capabilities: RendererProtocolCapabilities,
    frame_pacing_capabilities: FramePacingProtocolCapabilities,
    presentation_capabilities: PresentationProtocolCapabilities,
    xwayland_global_data: XwaylandShellGlobalData,
) {
    debug_assert!(
        versions::all_globals()
            .iter()
            .all(|global| global.version > 0 && !global.interface.is_empty())
    );
    display.create_global::<CompositorState, wl_compositor::WlCompositor, _>(
        versions::WL_COMPOSITOR,
        (),
    );
    display.create_global::<CompositorState, wl_subcompositor::WlSubcompositor, _>(
        versions::WL_SUBCOMPOSITOR,
        (),
    );
    if selection_capabilities.clipboard {
        display.create_global::<CompositorState, wl_data_device_manager::WlDataDeviceManager, _>(
            versions::WL_DATA_DEVICE_MANAGER,
            (),
        );
    }
    display.create_global::<CompositorState, wl_shm::WlShm, _>(versions::WL_SHM, ());
    display.create_global::<CompositorState, wp_viewporter::WpViewporter, _>(
        versions::WP_VIEWPORTER,
        (),
    );
    display.create_global::<
        CompositorState,
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        _,
    >(versions::WP_FRACTIONAL_SCALE_MANAGER_V1, ());
    display.create_global::<CompositorState, wp_presentation::WpPresentation, _>(
        versions::WP_PRESENTATION,
        (),
    );
    display.create_global::<CompositorState, zwlr_layer_shell_v1::ZwlrLayerShellV1, _>(
        versions::ZWLR_LAYER_SHELL_V1,
        (),
    );
    if renderer_capabilities.color_management {
        color::register_color_management_global(display);
    }
    if input_capabilities.relative_pointer {
        display.create_global::<
            CompositorState,
            zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
            _,
        >(versions::ZWP_RELATIVE_POINTER_MANAGER_V1, ());
    }
    if input_capabilities.pointer_constraints {
        display.create_global::<
            CompositorState,
            zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
            _,
        >(versions::ZWP_POINTER_CONSTRAINTS_V1, ());
    }
    if input_capabilities.pointer_warp {
        display.create_global::<CompositorState, wp_pointer_warp_v1::WpPointerWarpV1, _>(
            versions::WP_POINTER_WARP_V1,
            (),
        );
    }
    if input_capabilities.cursor_shape {
        display.create_global::<
            CompositorState,
            wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
            _,
        >(versions::WP_CURSOR_SHAPE_MANAGER_V1, ());
    }
    if input_capabilities.idle_inhibit {
        display.create_global::<
            CompositorState,
            zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
            _,
        >(versions::ZWP_IDLE_INHIBIT_MANAGER_V1, ());
    }
    if input_capabilities.keyboard_shortcuts_inhibit {
        display.create_global::<
            CompositorState,
            zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
            _,
        >(versions::ZWP_KEYBOARD_SHORTCUTS_INHIBIT_MANAGER_V1, ());
    }
    if selection_capabilities.primary_selection {
        display.create_global::<
            CompositorState,
            zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
            _,
        >(versions::ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1, ());
    }
    if selection_capabilities.data_control {
        display.create_global::<
            CompositorState,
            ext_data_control_manager_v1::ExtDataControlManagerV1,
            _,
        >(versions::EXT_DATA_CONTROL_MANAGER_V1, ());
    }
    if frame_pacing_capabilities.fifo {
        display.create_global::<CompositorState, wp_fifo_manager_v1::WpFifoManagerV1, _>(
            versions::WP_FIFO_MANAGER_V1,
            (),
        );
    }
    if frame_pacing_capabilities.commit_timing {
        display.create_global::<
            CompositorState,
            wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
            _,
        >(versions::WP_COMMIT_TIMING_MANAGER_V1, ());
    }
    if presentation_capabilities.tearing_control {
        display.create_global::<
            CompositorState,
            wp_tearing_control_manager_v1::WpTearingControlManagerV1,
            _,
        >(versions::WP_TEARING_CONTROL_MANAGER_V1, ());
    }
    if presentation_capabilities.content_type {
        display.create_global::<
            CompositorState,
            wp_content_type_manager_v1::WpContentTypeManagerV1,
            _,
        >(versions::WP_CONTENT_TYPE_MANAGER_V1, ());
    }
    display
        .create_global::<CompositorState, zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, _>(
            versions::ZXDG_DECORATION_MANAGER_V1,
            (),
        );
    if gpu_buffers_enabled {
        server_gpu_globals::register_gpu_buffer_globals(display, gpu_capabilities);
    }
    display.create_global::<CompositorState, xdg_activation_v1::XdgActivationV1, _>(
        versions::XDG_ACTIVATION_V1,
        (),
    );
    display
        .create_global::<CompositorState, astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, _>(
            versions::ASTREA_SHORTCUTS_MANAGER_V1,
            (),
        );
    display.create_global::<
        CompositorState,
        astrea_shell_auth_manager_v1::AstreaShellAuthManagerV1,
        _,
    >(versions::ASTREA_SHELL_AUTH_MANAGER_V1, ());
    display.create_global::<
        CompositorState,
        astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
        _,
    >(versions::ASTREA_SHELL_CONTROL_MANAGER_V1, ());
    display
        .create_global::<CompositorState, astrea_toplevel_manager_v1::AstreaToplevelManagerV1, _>(
            versions::ASTREA_TOPLEVEL_MANAGER_V1,
            (),
        );
    display.create_global::<CompositorState, xdg_wm_base::XdgWmBase, _>(versions::XDG_WM_BASE, ());
    display.create_global::<CompositorState, wl_output::WlOutput, _>(versions::WL_OUTPUT, ());
    display.create_global::<CompositorState, ext_workspace_manager_v1::ExtWorkspaceManagerV1, _>(
        versions::EXT_WORKSPACE_MANAGER_V1,
        (),
    );
    display.create_global::<CompositorState, wl_seat::WlSeat, _>(versions::WL_SEAT, ());
    display.create_global::<CompositorState, xwayland_shell_v1::XwaylandShellV1, _>(
        versions::XWAYLAND_SHELL_V1,
        xwayland_global_data,
    );
}
