#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolGlobal {
    WlCompositor,
    WlSubcompositor,
    WlDataDeviceManager,
    WlShm,
    WpViewporter,
    WpFractionalScale,
    WpPresentation,
    WpColorManagement,
    WpPointerWarp,
    WpRelativePointer,
    WpPointerConstraints,
    WpIdleInhibit,
    WpPrimarySelection,
    WpFifo,
    WpCommitTiming,
    ExtDataControl,
    XdgDecoration,
    LinuxDmabuf,
    LinuxDrmSyncobj,
    WlDrm,
    XdgWmBase,
    WlOutput,
    WlSeat,
}

impl ProtocolGlobal {
    pub const fn name(self) -> &'static str {
        match self {
            Self::WlCompositor => "wl_compositor",
            Self::WlSubcompositor => "wl_subcompositor",
            Self::WlDataDeviceManager => "wl_data_device_manager",
            Self::WlShm => "wl_shm",
            Self::WpViewporter => "wp_viewporter",
            Self::WpFractionalScale => "wp_fractional_scale_manager_v1",
            Self::WpPresentation => "wp_presentation",
            Self::WpColorManagement => "wp_color_manager_v1",
            Self::WpPointerWarp => "wp_pointer_warp_v1",
            Self::WpRelativePointer => "zwp_relative_pointer_manager_v1",
            Self::WpPointerConstraints => "zwp_pointer_constraints_v1",
            Self::WpIdleInhibit => "zwp_idle_inhibit_manager_v1",
            Self::WpPrimarySelection => "zwp_primary_selection_device_manager_v1",
            Self::WpFifo => "wp_fifo_manager_v1",
            Self::WpCommitTiming => "wp_commit_timing_manager_v1",
            Self::ExtDataControl => "ext_data_control_manager_v1",
            Self::XdgDecoration => "zxdg_decoration_manager_v1",
            Self::LinuxDmabuf => "zwp_linux_dmabuf_v1",
            Self::LinuxDrmSyncobj => "wp_linux_drm_syncobj_manager_v1",
            Self::WlDrm => "wl_drm",
            Self::XdgWmBase => "xdg_wm_base",
            Self::WlOutput => "wl_output",
            Self::WlSeat => "wl_seat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputProtocolCapabilities {
    pub relative_pointer: bool,
    pub pointer_constraints: bool,
    pub pointer_warp: bool,
    pub keyboard_shortcuts_inhibit: bool,
    pub idle_inhibit: bool,
}

impl InputProtocolCapabilities {
    pub const fn desktop_baseline() -> Self {
        Self {
            relative_pointer: false,
            pointer_constraints: false,
            pointer_warp: false,
            keyboard_shortcuts_inhibit: false,
            idle_inhibit: false,
        }
    }

    pub const fn native_libinput() -> Self {
        Self {
            relative_pointer: true,
            pointer_constraints: true,
            pointer_warp: true,
            keyboard_shortcuts_inhibit: false,
            idle_inhibit: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionProtocolCapabilities {
    pub clipboard: bool,
    pub primary_selection: bool,
    pub data_control: bool,
}

impl SelectionProtocolCapabilities {
    pub const fn safe_baseline() -> Self {
        Self {
            clipboard: false,
            primary_selection: false,
            data_control: false,
        }
    }

    pub const fn core_clipboard() -> Self {
        Self {
            clipboard: true,
            primary_selection: true,
            data_control: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererProtocolCapabilities {
    pub color_management: bool,
}

/// Protocols which are safe to expose only after their complete frame-pacing
/// path has been qualified.  These are deliberately separate from the base
/// client protocol list so a partial implementation cannot become a public
/// global by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePacingProtocolCapabilities {
    pub fifo: bool,
    pub commit_timing: bool,
}

impl FramePacingProtocolCapabilities {
    pub const fn safe_baseline() -> Self {
        Self {
            fifo: false,
            commit_timing: false,
        }
    }

    pub const fn qualified_native() -> Self {
        Self {
            fifo: true,
            commit_timing: true,
        }
    }
}

impl RendererProtocolCapabilities {
    pub const fn unsupported() -> Self {
        Self {
            color_management: false,
        }
    }
}

pub const BASE_CLIENT_PROTOCOLS: [ProtocolGlobal; 13] = [
    ProtocolGlobal::WlCompositor,
    ProtocolGlobal::WlSubcompositor,
    ProtocolGlobal::WlShm,
    ProtocolGlobal::WpViewporter,
    ProtocolGlobal::WpFractionalScale,
    ProtocolGlobal::WpPresentation,
    ProtocolGlobal::XdgDecoration,
    ProtocolGlobal::LinuxDmabuf,
    ProtocolGlobal::LinuxDrmSyncobj,
    ProtocolGlobal::WlDrm,
    ProtocolGlobal::XdgWmBase,
    ProtocolGlobal::WlOutput,
    ProtocolGlobal::WlSeat,
];

pub fn client_protocols_for_capabilities(
    input_capabilities: InputProtocolCapabilities,
    selection_capabilities: SelectionProtocolCapabilities,
    renderer_capabilities: RendererProtocolCapabilities,
) -> Vec<ProtocolGlobal> {
    client_protocols_for_capabilities_with_frame_pacing(
        input_capabilities,
        selection_capabilities,
        renderer_capabilities,
        FramePacingProtocolCapabilities::safe_baseline(),
    )
}

pub fn client_protocols_for_capabilities_with_frame_pacing(
    input_capabilities: InputProtocolCapabilities,
    selection_capabilities: SelectionProtocolCapabilities,
    renderer_capabilities: RendererProtocolCapabilities,
    frame_pacing_capabilities: FramePacingProtocolCapabilities,
) -> Vec<ProtocolGlobal> {
    let mut protocols = BASE_CLIENT_PROTOCOLS.to_vec();
    let selection_insert_at = protocols
        .iter()
        .position(|protocol| *protocol == ProtocolGlobal::WlShm)
        .unwrap_or(protocols.len());
    if selection_capabilities.data_control {
        protocols.insert(selection_insert_at, ProtocolGlobal::ExtDataControl);
    }
    if selection_capabilities.primary_selection {
        protocols.insert(selection_insert_at, ProtocolGlobal::WpPrimarySelection);
    }
    if selection_capabilities.clipboard {
        protocols.insert(selection_insert_at, ProtocolGlobal::WlDataDeviceManager);
    }

    if frame_pacing_capabilities.commit_timing {
        protocols.insert(selection_insert_at, ProtocolGlobal::WpCommitTiming);
    }
    if frame_pacing_capabilities.fifo {
        protocols.insert(selection_insert_at, ProtocolGlobal::WpFifo);
    }

    if renderer_capabilities.color_management {
        let insert_at = protocols
            .iter()
            .position(|protocol| *protocol == ProtocolGlobal::XdgDecoration)
            .unwrap_or(protocols.len());
        protocols.insert(insert_at, ProtocolGlobal::WpColorManagement);
    }

    let insert_at = protocols
        .iter()
        .position(|protocol| *protocol == ProtocolGlobal::XdgDecoration)
        .unwrap_or(protocols.len());
    if input_capabilities.pointer_constraints {
        protocols.insert(insert_at, ProtocolGlobal::WpPointerConstraints);
    }
    if input_capabilities.pointer_warp {
        protocols.insert(insert_at, ProtocolGlobal::WpPointerWarp);
    }
    if input_capabilities.relative_pointer {
        protocols.insert(insert_at, ProtocolGlobal::WpRelativePointer);
    }
    if input_capabilities.idle_inhibit {
        protocols.insert(insert_at, ProtocolGlobal::WpIdleInhibit);
    }
    protocols
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchitectureLayer {
    pub name: &'static str,
    pub responsibility: &'static str,
    pub status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorArchitecture {
    layers: Vec<ArchitectureLayer>,
}

impl Default for CompositorArchitecture {
    fn default() -> Self {
        Self {
            layers: vec![
                ArchitectureLayer {
                    name: "core",
                    responsibility: "shared geometry, paths, and platform contracts",
                    status: "active",
                },
                ArchitectureLayer {
                    name: "compositor",
                    responsibility: "owned Wayland display, client socket, protocol state, and render/input backends",
                    status: "active",
                },
                ArchitectureLayer {
                    name: "wm",
                    responsibility: "window focus, floating placement, move, resize, maximize, and close policy",
                    status: "active",
                },
                ArchitectureLayer {
                    name: "shell",
                    responsibility: "dock, topbar, launcher, notifications, and desktop surfaces",
                    status: "deferred",
                },
                ArchitectureLayer {
                    name: "session",
                    responsibility: "TTY and SDDM lifecycle",
                    status: "active",
                },
            ],
        }
    }
}

impl CompositorArchitecture {
    pub fn layer_names(&self) -> Vec<&'static str> {
        self.layers.iter().map(|layer| layer.name).collect()
    }

    pub fn layer(&self, name: &str) -> Option<&ArchitectureLayer> {
        self.layers.iter().find(|layer| layer.name == name)
    }

    pub fn layers(&self) -> &[ArchitectureLayer] {
        &self.layers
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositorPlan {
    pub socket_name: String,
    pub architecture: CompositorArchitecture,
    pub frame_pacing_capabilities: FramePacingProtocolCapabilities,
}

impl CompositorPlan {
    pub fn new(socket_name: impl Into<String>) -> Self {
        Self {
            socket_name: socket_name.into(),
            architecture: CompositorArchitecture::default(),
            frame_pacing_capabilities: FramePacingProtocolCapabilities::safe_baseline(),
        }
    }

    pub fn with_frame_pacing_capabilities(
        mut self,
        capabilities: FramePacingProtocolCapabilities,
    ) -> Self {
        self.frame_pacing_capabilities = capabilities;
        self
    }

    pub const fn uses_external_compositor(&self) -> bool {
        false
    }

    pub fn command_preview(&self) -> String {
        format!("oblivion-one compositor --socket {}", self.socket_name)
    }

    pub fn protocol_names(&self) -> Vec<&'static str> {
        client_protocols_for_capabilities_with_frame_pacing(
            InputProtocolCapabilities::desktop_baseline(),
            SelectionProtocolCapabilities::core_clipboard(),
            RendererProtocolCapabilities::unsupported(),
            self.frame_pacing_capabilities,
        )
        .into_iter()
        .map(ProtocolGlobal::name)
        .collect()
    }
}
