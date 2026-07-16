#![allow(dead_code)]

use std::{io, num::NonZeroU64};

use crate::xwayland::X11WindowHandle;
use crate::xwayland::xwm::X11WindowSnapshot;

use super::WindowState;

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

#[derive(Debug, Clone)]
pub struct DesktopWindow {
    pub id: WindowId,
    pub root_surface_id: u32,
    pub backend: WindowBackend,
    pub kind: DesktopWindowKind,
    pub metadata: WindowMetadata,
    pub constraints: WindowConstraints,
    pub relationships: WindowRelationships,
    pub state: WindowState,
}

impl DesktopWindow {
    pub(crate) fn new_xdg(id: WindowId, root_surface_id: u32) -> Self {
        Self {
            id,
            root_surface_id,
            backend: WindowBackend::Xdg(XdgWindowHandle::new(root_surface_id)),
            kind: DesktopWindowKind::Managed,
            metadata: WindowMetadata::default(),
            constraints: WindowConstraints::default(),
            relationships: WindowRelationships::default(),
            state: WindowState::default(),
        }
    }

    pub(crate) fn new_x11(id: WindowId, snapshot: X11WindowSnapshot) -> Self {
        Self {
            id,
            root_surface_id: snapshot.surface_id,
            backend: WindowBackend::X11(snapshot.handle),
            kind: snapshot.kind,
            metadata: snapshot.metadata,
            constraints: snapshot.constraints,
            relationships: WindowRelationships::default(),
            state: WindowState::default(),
        }
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
