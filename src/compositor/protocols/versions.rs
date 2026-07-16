#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlobalAdvertisement {
    pub(crate) interface: &'static str,
    pub(crate) version: u32,
}

impl GlobalAdvertisement {
    pub(crate) const fn new(interface: &'static str, version: u32) -> Self {
        Self { interface, version }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RequestClassification {
    Implemented,
    ValidatedNoOp,
    BackendOwned,
    CapabilityRejected,
    ProtocolError,
    DestroyedResourceNoFurtherDispatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct RequestContract {
    pub(crate) interface: &'static str,
    pub(crate) request: &'static str,
    pub(crate) since: u32,
    pub(crate) classification: RequestClassification,
    pub(crate) test: &'static str,
}

macro_rules! contract {
    ($interface:literal, $request:literal, $since:expr, $classification:ident, $test:literal) => {
        RequestContract {
            interface: $interface,
            request: $request,
            since: $since,
            classification: RequestClassification::$classification,
            test: $test,
        }
    };
}

/// Machine-checked request inventory for the Core/XDG requests whose
/// behavior is part of this milestone. Resource requests inherit the bound
/// version of their creating global. The documentation matrix remains the
/// complete human-readable inventory; this table prevents the high-risk
/// requests from disappearing into an unclassified wildcard arm.
#[allow(dead_code)]
pub(crate) const CORE_XDG_REQUEST_CONTRACTS: &[RequestContract] = &[
    contract!(
        "wl_compositor",
        "create_surface",
        1,
        Implemented,
        "core_surface"
    ),
    contract!(
        "wl_compositor",
        "create_region",
        1,
        Implemented,
        "core_surface"
    ),
    contract!(
        "wl_compositor",
        "release",
        5,
        DestroyedResourceNoFurtherDispatch,
        "core_surface"
    ),
    contract!("wl_shm", "create_pool", 1, ProtocolError, "shm_compliance"),
    contract!(
        "wl_shm",
        "release",
        1,
        DestroyedResourceNoFurtherDispatch,
        "shm_compliance"
    ),
    contract!("wl_surface", "destroy", 1, ProtocolError, "role_lifecycle"),
    contract!(
        "wl_surface",
        "attach",
        1,
        ProtocolError,
        "core_surface_compliance"
    ),
    contract!("wl_surface", "damage", 1, Implemented, "surface_frames"),
    contract!("wl_surface", "frame", 1, Implemented, "surface_frames"),
    contract!(
        "wl_surface",
        "set_opaque_region",
        1,
        Implemented,
        "core_surface_compliance"
    ),
    contract!(
        "wl_surface",
        "set_input_region",
        1,
        Implemented,
        "core_surface_compliance"
    ),
    contract!(
        "wl_surface",
        "commit",
        1,
        ProtocolError,
        "core_surface_compliance"
    ),
    contract!(
        "wl_surface",
        "set_buffer_transform",
        2,
        ProtocolError,
        "core_surface_compliance"
    ),
    contract!(
        "wl_surface",
        "set_buffer_scale",
        3,
        ProtocolError,
        "core_surface_compliance"
    ),
    contract!(
        "wl_surface",
        "damage_buffer",
        4,
        Implemented,
        "surface_frames"
    ),
    contract!(
        "wl_surface",
        "offset",
        5,
        Implemented,
        "core_surface_compliance"
    ),
    contract!("wl_seat", "get_pointer", 1, Implemented, "input_serial"),
    contract!("wl_seat", "get_keyboard", 1, Implemented, "input_serial"),
    contract!(
        "wl_seat",
        "get_touch",
        1,
        CapabilityRejected,
        "protocol_error"
    ),
    contract!(
        "wl_seat",
        "release",
        5,
        DestroyedResourceNoFurtherDispatch,
        "input_serial"
    ),
    contract!("wl_pointer", "set_cursor", 1, ProtocolError, "input_serial"),
    contract!(
        "wl_pointer",
        "release",
        3,
        DestroyedResourceNoFurtherDispatch,
        "input_serial"
    ),
    contract!(
        "wl_keyboard",
        "release",
        3,
        DestroyedResourceNoFurtherDispatch,
        "input_serial"
    ),
    contract!(
        "wl_output",
        "release",
        3,
        DestroyedResourceNoFurtherDispatch,
        "client_lifecycle"
    ),
    contract!(
        "wl_subcompositor",
        "get_subsurface",
        1,
        ProtocolError,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subcompositor",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subsurface",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "role_lifecycle"
    ),
    contract!(
        "wl_subsurface",
        "set_position",
        1,
        Implemented,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subsurface",
        "place_above",
        1,
        ProtocolError,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subsurface",
        "place_below",
        1,
        ProtocolError,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subsurface",
        "set_sync",
        1,
        Implemented,
        "subsurface_compliance"
    ),
    contract!(
        "wl_subsurface",
        "set_desync",
        1,
        Implemented,
        "subsurface_compliance"
    ),
    contract!(
        "wl_data_device_manager",
        "create_data_source",
        1,
        Implemented,
        "data_device"
    ),
    contract!(
        "wl_data_device_manager",
        "get_data_device",
        1,
        Implemented,
        "data_device"
    ),
    contract!(
        "wl_data_device_manager",
        "release",
        2,
        DestroyedResourceNoFurtherDispatch,
        "data_device"
    ),
    contract!(
        "wl_data_device",
        "start_drag",
        1,
        ProtocolError,
        "data_device"
    ),
    contract!(
        "wl_data_device",
        "set_selection",
        1,
        ProtocolError,
        "data_device"
    ),
    contract!(
        "wl_data_device",
        "release",
        2,
        DestroyedResourceNoFurtherDispatch,
        "data_device"
    ),
    contract!("wl_data_source", "offer", 1, ProtocolError, "data_device"),
    contract!(
        "wl_data_source",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "data_device"
    ),
    contract!(
        "wl_data_source",
        "set_actions",
        3,
        ProtocolError,
        "data_device"
    ),
    contract!("wl_data_offer", "accept", 1, ProtocolError, "data_device"),
    contract!("wl_data_offer", "receive", 1, BackendOwned, "data_device"),
    contract!(
        "wl_data_offer",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "data_device"
    ),
    contract!("wl_data_offer", "finish", 3, ProtocolError, "data_device"),
    contract!(
        "wl_data_offer",
        "set_actions",
        3,
        ProtocolError,
        "data_device"
    ),
    contract!("xdg_wm_base", "destroy", 1, ProtocolError, "xdg_compliance"),
    contract!(
        "xdg_wm_base",
        "create_positioner",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_wm_base",
        "get_xdg_surface",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!("xdg_wm_base", "pong", 1, ValidatedNoOp, "xdg_compliance"),
    contract!(
        "xdg_positioner",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_size",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_anchor_rect",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_anchor",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_gravity",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_constraint_adjustment",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_offset",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_reactive",
        3,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_parent_size",
        3,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_positioner",
        "set_parent_configure",
        3,
        Implemented,
        "xdg_compliance"
    ),
    contract!("xdg_surface", "destroy", 1, ProtocolError, "xdg_compliance"),
    contract!(
        "xdg_surface",
        "get_toplevel",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_surface",
        "get_popup",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_surface",
        "set_window_geometry",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_surface",
        "ack_configure",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "destroy",
        1,
        DestroyedResourceNoFurtherDispatch,
        "role_lifecycle"
    ),
    contract!(
        "xdg_toplevel",
        "set_parent",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_title",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_app_id",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "show_window_menu",
        1,
        ValidatedNoOp,
        "xdg_compliance"
    ),
    contract!("xdg_toplevel", "move", 1, ProtocolError, "input_serial"),
    contract!("xdg_toplevel", "resize", 1, ProtocolError, "input_serial"),
    contract!(
        "xdg_toplevel",
        "set_max_size",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_min_size",
        1,
        ProtocolError,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_maximized",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "unset_maximized",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_fullscreen",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "unset_fullscreen",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!(
        "xdg_toplevel",
        "set_minimized",
        1,
        Implemented,
        "xdg_compliance"
    ),
    contract!("xdg_popup", "destroy", 1, ProtocolError, "xdg_compliance"),
    contract!("xdg_popup", "grab", 1, ProtocolError, "input_serial"),
    contract!(
        "xdg_popup",
        "reposition",
        3,
        ProtocolError,
        "xdg_compliance"
    ),
];

pub(crate) const WL_COMPOSITOR: u32 = 6;
pub(crate) const WL_SUBCOMPOSITOR: u32 = 1;
pub(crate) const WL_SHM: u32 = 2;
pub(crate) const WL_DATA_DEVICE_MANAGER: u32 = 3;
pub(crate) const WP_VIEWPORTER: u32 = 1;
pub(crate) const WP_FRACTIONAL_SCALE_MANAGER_V1: u32 = 1;
pub(crate) const WP_PRESENTATION: u32 = 2;
pub(crate) const ZWLR_LAYER_SHELL_V1: u32 = 4;
pub(crate) const WP_COLOR_MANAGER_V1: u32 = 1;
pub(crate) const ZWP_RELATIVE_POINTER_MANAGER_V1: u32 = 1;
pub(crate) const ZWP_POINTER_CONSTRAINTS_V1: u32 = 1;
pub(crate) const WP_POINTER_WARP_V1: u32 = 1;
pub(crate) const ZWP_IDLE_INHIBIT_MANAGER_V1: u32 = 1;
pub(crate) const ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1: u32 = 1;
pub(crate) const EXT_DATA_CONTROL_MANAGER_V1: u32 = 1;
pub(crate) const ZXDG_DECORATION_MANAGER_V1: u32 = 1;
pub(crate) const ZWP_LINUX_DMABUF_V1: u32 = 4;
pub(crate) const WP_LINUX_DRM_SYNCOBJ_MANAGER_V1: u32 = 1;
pub(crate) const WL_DRM: u32 = 2;
pub(crate) const XDG_ACTIVATION_V1: u32 = 1;
pub(crate) const ASTREA_SHORTCUTS_MANAGER_V1: u32 = 1;
pub(crate) const ASTREA_SHELL_CONTROL_MANAGER_V1: u32 = 1;
pub(crate) const XDG_WM_BASE: u32 = 6;
pub(crate) const WL_OUTPUT: u32 = 4;
pub(crate) const WL_SEAT: u32 = 7;
pub(crate) const XWAYLAND_SHELL_V1: u32 = crate::xwayland::XWAYLAND_SHELL_V1_VERSION;

pub(crate) const ALL_GLOBALS: &[GlobalAdvertisement] = &[
    GlobalAdvertisement::new("wl_compositor", WL_COMPOSITOR),
    GlobalAdvertisement::new("wl_subcompositor", WL_SUBCOMPOSITOR),
    GlobalAdvertisement::new("wl_shm", WL_SHM),
    GlobalAdvertisement::new("wl_data_device_manager", WL_DATA_DEVICE_MANAGER),
    GlobalAdvertisement::new("wp_viewporter", WP_VIEWPORTER),
    GlobalAdvertisement::new(
        "wp_fractional_scale_manager_v1",
        WP_FRACTIONAL_SCALE_MANAGER_V1,
    ),
    GlobalAdvertisement::new("wp_presentation", WP_PRESENTATION),
    GlobalAdvertisement::new("zwlr_layer_shell_v1", ZWLR_LAYER_SHELL_V1),
    GlobalAdvertisement::new("wp_color_manager_v1", WP_COLOR_MANAGER_V1),
    GlobalAdvertisement::new(
        "zwp_relative_pointer_manager_v1",
        ZWP_RELATIVE_POINTER_MANAGER_V1,
    ),
    GlobalAdvertisement::new("zwp_pointer_constraints_v1", ZWP_POINTER_CONSTRAINTS_V1),
    GlobalAdvertisement::new("wp_pointer_warp_v1", WP_POINTER_WARP_V1),
    GlobalAdvertisement::new("zwp_idle_inhibit_manager_v1", ZWP_IDLE_INHIBIT_MANAGER_V1),
    GlobalAdvertisement::new(
        "zwp_primary_selection_device_manager_v1",
        ZWP_PRIMARY_SELECTION_DEVICE_MANAGER_V1,
    ),
    GlobalAdvertisement::new("ext_data_control_manager_v1", EXT_DATA_CONTROL_MANAGER_V1),
    GlobalAdvertisement::new("zxdg_decoration_manager_v1", ZXDG_DECORATION_MANAGER_V1),
    GlobalAdvertisement::new("zwp_linux_dmabuf_v1", ZWP_LINUX_DMABUF_V1),
    GlobalAdvertisement::new(
        "wp_linux_drm_syncobj_manager_v1",
        WP_LINUX_DRM_SYNCOBJ_MANAGER_V1,
    ),
    GlobalAdvertisement::new("wl_drm", WL_DRM),
    GlobalAdvertisement::new("xdg_activation_v1", XDG_ACTIVATION_V1),
    GlobalAdvertisement::new("astrea_shortcuts_manager_v1", ASTREA_SHORTCUTS_MANAGER_V1),
    GlobalAdvertisement::new(
        "astrea_shell_control_manager_v1",
        ASTREA_SHELL_CONTROL_MANAGER_V1,
    ),
    GlobalAdvertisement::new("xdg_wm_base", XDG_WM_BASE),
    GlobalAdvertisement::new("wl_output", WL_OUTPUT),
    GlobalAdvertisement::new("wl_seat", WL_SEAT),
    GlobalAdvertisement::new("xwayland_shell_v1", XWAYLAND_SHELL_V1),
];

pub(crate) const fn all_globals() -> &'static [GlobalAdvertisement] {
    ALL_GLOBALS
}
