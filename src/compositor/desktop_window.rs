#![allow(dead_code)]

use std::{io, num::NonZeroU64};

use crate::xwayland::X11WindowHandle;
use crate::xwayland::xwm::{X11Geometry, X11WindowSnapshot, X11WindowType, X11WindowTypes};

use super::{SurfacePlacement, WindowGeometry, WindowState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(NonZeroU64);

impl WindowId {
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopWindowKind {
    Managed,
    OverrideRedirect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11DesktopRole {
    Toplevel,
    Dialog,
    AuxiliaryPopup,
    Notification,
    OverrideRedirect,
    AuxiliarySupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum X11PlacementPolicy {
    CompositorManaged,
    ClientPositioned,
    ParentRelative,
    OverrideRedirect,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WindowMetadata {
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowConstraints {
    pub min_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub base_width: Option<u32>,
    pub base_height: Option<u32>,
    pub width_increment: Option<u32>,
    pub height_increment: Option<u32>,
    pub min_aspect: Option<f64>,
    pub max_aspect: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowRelationships {
    pub parent: Option<WindowId>,
    pub transient_for: Option<WindowId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct XdgWindowHandle {
    pub(crate) root_surface_id: u32,
}

impl XdgWindowHandle {
    pub(crate) const fn new(root_surface_id: u32) -> Self {
        Self { root_surface_id }
    }

    pub(crate) const fn root_surface_id(self) -> u32 {
        self.root_surface_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowBackend {
    Xdg(XdgWindowHandle),
    X11(X11WindowHandle),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct X11GeometryState {
    pub(super) client: X11Geometry,
    pub(super) frame: WindowGeometry,
}

#[derive(Debug, Clone)]
pub struct DesktopWindow {
    pub id: WindowId,
    pub root_surface_id: u32,
    pub backend: WindowBackend,
    pub(crate) x11_surface_id: Option<u32>,
    pub kind: DesktopWindowKind,
    pub x11_role: Option<X11DesktopRole>,
    pub x11_placement_policy: Option<X11PlacementPolicy>,
    pub x11_window_types: X11WindowTypes,
    pub x11_accepts_input: Option<bool>,
    pub x11_transient_for: Option<X11WindowHandle>,
    pub(super) x11_geometry: Option<X11GeometryState>,
    pub metadata: WindowMetadata,
    pub constraints: WindowConstraints,
    pub relationships: WindowRelationships,
    pub state: WindowState,
}

impl DesktopWindow {
    pub(crate) fn is_normal_x11_role(&self) -> bool {
        matches!(
            self.x11_role,
            None | Some(X11DesktopRole::Toplevel | X11DesktopRole::Dialog)
        )
    }

    pub(crate) fn is_auxiliary_x11_role(&self) -> bool {
        matches!(
            self.x11_role,
            Some(
                X11DesktopRole::AuxiliaryPopup
                    | X11DesktopRole::Notification
                    | X11DesktopRole::OverrideRedirect
                    | X11DesktopRole::AuxiliarySupport
            )
        )
    }

    pub(crate) fn new_xdg(id: WindowId, root_surface_id: u32) -> Self {
        Self {
            id,
            root_surface_id,
            backend: WindowBackend::Xdg(XdgWindowHandle::new(root_surface_id)),
            x11_surface_id: None,
            kind: DesktopWindowKind::Managed,
            x11_role: None,
            x11_placement_policy: None,
            x11_window_types: X11WindowTypes::default(),
            x11_accepts_input: None,
            x11_transient_for: None,
            x11_geometry: None,
            metadata: WindowMetadata::default(),
            constraints: WindowConstraints::default(),
            relationships: WindowRelationships::default(),
            state: WindowState::default(),
        }
    }

    pub(crate) fn new_x11(id: WindowId, snapshot: X11WindowSnapshot) -> Self {
        let geometry = snapshot.geometry;
        let role = classify_x11_role(
            snapshot.kind,
            &snapshot.window_types,
            snapshot.transient_for.is_some(),
            snapshot.override_redirect,
        );
        Self {
            id,
            root_surface_id: snapshot.surface_id,
            backend: WindowBackend::X11(snapshot.handle),
            x11_surface_id: Some(snapshot.surface_id),
            kind: snapshot.kind,
            x11_role: Some(role),
            x11_placement_policy: Some(x11_placement_policy(role)),
            x11_window_types: snapshot.window_types,
            x11_accepts_input: snapshot.accepts_input,
            x11_transient_for: snapshot.transient_for,
            x11_geometry: Some(X11GeometryState {
                client: geometry,
                frame: WindowGeometry::new(
                    SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
                    geometry.width.max(1),
                    geometry.height.max(1),
                ),
            }),
            metadata: snapshot.metadata,
            constraints: snapshot.constraints,
            relationships: WindowRelationships::default(),
            state: WindowState::default(),
        }
    }
}

pub(crate) const fn x11_placement_policy(role: X11DesktopRole) -> X11PlacementPolicy {
    match role {
        X11DesktopRole::Toplevel => X11PlacementPolicy::CompositorManaged,
        X11DesktopRole::Dialog => X11PlacementPolicy::ParentRelative,
        X11DesktopRole::AuxiliaryPopup | X11DesktopRole::Notification => {
            X11PlacementPolicy::ClientPositioned
        }
        X11DesktopRole::OverrideRedirect => X11PlacementPolicy::OverrideRedirect,
        X11DesktopRole::AuxiliarySupport => X11PlacementPolicy::ClientPositioned,
    }
}

pub(crate) fn classify_x11_role(
    kind: DesktopWindowKind,
    window_types: &X11WindowTypes,
    transient_for: bool,
    override_redirect: bool,
) -> X11DesktopRole {
    if override_redirect || kind == DesktopWindowKind::OverrideRedirect {
        return X11DesktopRole::OverrideRedirect;
    }
    match (window_types.preferred_supported_type(), transient_for) {
        (Some(X11WindowType::Dialog), true) => X11DesktopRole::Dialog,
        (Some(X11WindowType::Utility), true) => X11DesktopRole::Dialog,
        (
            Some(
                X11WindowType::Menu
                | X11WindowType::PopupMenu
                | X11WindowType::DropdownMenu
                | X11WindowType::Tooltip
                | X11WindowType::Combo,
            ),
            _,
        ) => X11DesktopRole::AuxiliaryPopup,
        (
            Some(
                X11WindowType::Splash
                | X11WindowType::Toolbar
                | X11WindowType::Dnd
                | X11WindowType::Dock
                | X11WindowType::Desktop,
            ),
            _,
        ) => X11DesktopRole::AuxiliarySupport,
        (Some(X11WindowType::Notification), _) => X11DesktopRole::Notification,
        _ => X11DesktopRole::Toplevel,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DesktopWindowError {
    DuplicateWindowId,
    DuplicateRootSurface,
    UnknownWindow,
    WindowIdExhausted,
}

impl std::fmt::Display for DesktopWindowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateWindowId => "desktop window id already exists",
            Self::DuplicateRootSurface => "root surface already belongs to a desktop window",
            Self::UnknownWindow => "desktop window does not exist",
            Self::WindowIdExhausted => "desktop window identity exhausted",
        })
    }
}

impl std::error::Error for DesktopWindowError {}

impl From<DesktopWindowError> for io::Error {
    fn from(error: DesktopWindowError) -> Self {
        io::Error::other(error)
    }
}
