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
    WpRelativePointer,
    WpPointerConstraints,
    WpIdleInhibit,
    WpPrimarySelection,
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
            Self::WpRelativePointer => "zwp_relative_pointer_manager_v1",
            Self::WpPointerConstraints => "zwp_pointer_constraints_v1",
            Self::WpIdleInhibit => "zwp_idle_inhibit_manager_v1",
            Self::WpPrimarySelection => "zwp_primary_selection_device_manager_v1",
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
    pub keyboard_shortcuts_inhibit: bool,
    pub idle_inhibit: bool,
}

impl InputProtocolCapabilities {
    pub const fn desktop_baseline() -> Self {
        Self {
            relative_pointer: false,
            pointer_constraints: false,
            keyboard_shortcuts_inhibit: false,
            idle_inhibit: false,
        }
    }
}

pub const BASE_CLIENT_PROTOCOLS: [ProtocolGlobal; 17] = [
    ProtocolGlobal::WlCompositor,
    ProtocolGlobal::WlSubcompositor,
    ProtocolGlobal::WlDataDeviceManager,
    ProtocolGlobal::WlShm,
    ProtocolGlobal::WpViewporter,
    ProtocolGlobal::WpFractionalScale,
    ProtocolGlobal::WpPresentation,
    ProtocolGlobal::WpColorManagement,
    ProtocolGlobal::WpPrimarySelection,
    ProtocolGlobal::ExtDataControl,
    ProtocolGlobal::XdgDecoration,
    ProtocolGlobal::LinuxDmabuf,
    ProtocolGlobal::LinuxDrmSyncobj,
    ProtocolGlobal::WlDrm,
    ProtocolGlobal::XdgWmBase,
    ProtocolGlobal::WlOutput,
    ProtocolGlobal::WlSeat,
];

pub fn client_protocols_for_capabilities(
    capabilities: InputProtocolCapabilities,
) -> Vec<ProtocolGlobal> {
    let mut protocols = BASE_CLIENT_PROTOCOLS.to_vec();
    let insert_at = protocols
        .iter()
        .position(|protocol| *protocol == ProtocolGlobal::WpPrimarySelection)
        .unwrap_or(protocols.len());
    if capabilities.pointer_constraints {
        protocols.insert(insert_at, ProtocolGlobal::WpPointerConstraints);
    }
    if capabilities.relative_pointer {
        protocols.insert(insert_at, ProtocolGlobal::WpRelativePointer);
    }
    if capabilities.idle_inhibit {
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
                    responsibility: "shared geometry, paths, process plans, and platform contracts",
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
                    responsibility: "nested runner first, then TTY and SDDM lifecycle",
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
}

impl CompositorPlan {
    pub fn new(socket_name: impl Into<String>) -> Self {
        Self {
            socket_name: socket_name.into(),
            architecture: CompositorArchitecture::default(),
        }
    }

    pub const fn uses_external_compositor(&self) -> bool {
        false
    }

    pub fn command_preview(&self) -> String {
        format!("oblivion-one compositor --socket {}", self.socket_name)
    }

    pub fn protocol_names(&self) -> Vec<&'static str> {
        client_protocols_for_capabilities(InputProtocolCapabilities::desktop_baseline())
            .into_iter()
            .map(ProtocolGlobal::name)
            .collect()
    }
}
