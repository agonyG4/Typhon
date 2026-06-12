use wayland_protocols::xdg::shell::server::xdg_toplevel;

use super::{RenderableSurface, SurfacePlacement};

#[derive(Debug, Clone)]
pub(super) struct WindowState {
    mode: ToplevelMode,
    restore_geometry: Option<WindowGeometry>,
    minimized_surfaces: Vec<RenderableSurface>,
}

impl WindowState {
    pub(super) fn mode(&self) -> ToplevelMode {
        self.mode
    }

    pub(super) fn set_mode(&mut self, mode: ToplevelMode) {
        self.mode = mode;
    }

    pub(super) fn is_minimized(&self) -> bool {
        !self.minimized_surfaces.is_empty()
    }

    pub(super) fn minimize(&mut self, surfaces: Vec<RenderableSurface>) {
        self.minimized_surfaces = surfaces;
    }

    pub(super) fn restore_minimized(&mut self) -> Option<Vec<RenderableSurface>> {
        self.is_minimized()
            .then(|| std::mem::take(&mut self.minimized_surfaces))
    }

    pub(super) fn minimized_root_surface(&self, surface_id: u32) -> Option<&RenderableSurface> {
        self.minimized_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
    }

    pub(super) fn minimized_surface_mut(
        &mut self,
        surface_id: u32,
    ) -> Option<&mut RenderableSurface> {
        self.minimized_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
    }

    pub(super) fn push_minimized_surface(&mut self, surface: RenderableSurface) {
        self.minimized_surfaces.push(surface);
    }

    pub(super) fn capture_restore_geometry(&mut self, geometry: WindowGeometry) {
        if self.mode == ToplevelMode::Floating && self.restore_geometry.is_none() {
            self.restore_geometry = Some(geometry);
        }
    }

    pub(super) fn take_restore_geometry(&mut self) -> Option<WindowGeometry> {
        self.restore_geometry.take()
    }
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            mode: ToplevelMode::Floating,
            restore_geometry: None,
            minimized_surfaces: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToplevelMode {
    Floating,
    Maximized,
    Fullscreen,
}

impl ToplevelMode {
    pub(super) const fn xdg_states(self) -> &'static [xdg_toplevel::State] {
        match self {
            Self::Floating => &[],
            Self::Maximized => &[xdg_toplevel::State::Maximized],
            Self::Fullscreen => &[xdg_toplevel::State::Fullscreen],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WindowGeometry {
    pub(super) placement: SurfacePlacement,
    pub(super) width: u32,
    pub(super) height: u32,
}

impl WindowGeometry {
    pub(super) const fn new(placement: SurfacePlacement, width: u32, height: u32) -> Self {
        Self {
            placement,
            width,
            height,
        }
    }
}

pub(super) fn xdg_toplevel_state_bytes(states: &[xdg_toplevel::State]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(states.len() * std::mem::size_of::<u32>());
    for state in states {
        bytes.extend_from_slice(&(*state as u32).to_ne_bytes());
    }
    bytes
}
