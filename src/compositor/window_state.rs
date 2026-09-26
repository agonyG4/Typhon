use std::collections::HashSet;

use wayland_protocols::xdg::shell::server::xdg_toplevel;

use super::{RenderableSurface, SurfacePlacement, XdgWindowGeometry};

#[derive(Debug, Clone)]
pub(crate) struct WindowState {
    mode: ToplevelMode,
    normal_restore_target: Option<NormalRestoreTarget>,
    minimized_surfaces: Vec<RenderableSurface>,
    minimized: bool,
}

impl WindowState {
    pub(super) fn mode(&self) -> ToplevelMode {
        self.mode
    }

    pub(super) fn set_mode(&mut self, mode: ToplevelMode) {
        self.mode = mode;
    }

    pub(super) fn is_minimized(&self) -> bool {
        self.minimized
    }

    #[cfg(test)]
    pub(super) fn restore_geometry(&self) -> Option<WindowGeometry> {
        self.normal_restore_target.and_then(|target| match target {
            NormalRestoreTarget::Known(geometry) => Some(geometry),
            NormalRestoreTarget::UnknownSize { .. } => None,
        })
    }

    pub(super) fn normal_restore_target(&self) -> Option<NormalRestoreTarget> {
        self.normal_restore_target
    }

    pub(super) fn minimize(&mut self, surfaces: Vec<RenderableSurface>) {
        self.minimized = true;
        self.minimized_surfaces = surfaces;
    }

    pub(super) fn restore_minimized(&mut self) -> Option<Vec<RenderableSurface>> {
        self.is_minimized().then(|| {
            self.minimized = false;
            std::mem::take(&mut self.minimized_surfaces)
        })
    }

    pub(super) fn mark_minimized_without_surfaces(&mut self) {
        self.minimized = true;
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

    pub(super) fn minimized_surface(&self, surface_id: u32) -> Option<&RenderableSurface> {
        self.minimized_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
    }

    pub(super) fn push_minimized_surface(&mut self, surface: RenderableSurface) {
        self.minimized = true;
        self.minimized_surfaces.push(surface);
    }

    pub(super) fn remove_minimized_surface_ids(&mut self, surface_ids: &HashSet<u32>) {
        self.minimized_surfaces
            .retain(|surface| !surface_ids.contains(&surface.surface_id));
    }

    pub(super) fn minimized_surfaces_len(&self) -> usize {
        self.minimized_surfaces.len()
    }

    pub(super) fn minimized_surfaces(&self) -> &[RenderableSurface] {
        &self.minimized_surfaces
    }

    pub(super) fn capture_restore_geometry(&mut self, geometry: WindowGeometry) {
        if self.mode == ToplevelMode::Normal && self.normal_restore_target.is_none() {
            self.normal_restore_target = Some(NormalRestoreTarget::from_geometry(geometry));
        }
    }

    pub(super) fn take_restore_geometry(&mut self) -> Option<WindowGeometry> {
        self.normal_restore_target
            .take()
            .and_then(|target| match target {
                NormalRestoreTarget::Known(geometry) => Some(geometry),
                NormalRestoreTarget::UnknownSize { .. } => None,
            })
    }

    pub(super) fn take_normal_restore_target(&mut self) -> Option<NormalRestoreTarget> {
        self.normal_restore_target.take()
    }
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            mode: ToplevelMode::Normal,
            normal_restore_target: None,
            minimized_surfaces: Vec::new(),
            minimized: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NormalRestoreTarget {
    Known(WindowGeometry),
    UnknownSize { placement: SurfacePlacement },
}

impl NormalRestoreTarget {
    pub(crate) fn from_geometry(geometry: WindowGeometry) -> Self {
        if geometry.width > 0 && geometry.height > 0 {
            Self::Known(geometry)
        } else {
            Self::UnknownSize {
                placement: geometry.placement,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToplevelMode {
    Normal,
    Maximized,
    Fullscreen,
}

impl ToplevelMode {
    pub(super) const fn xdg_states(self) -> &'static [xdg_toplevel::State] {
        match self {
            Self::Normal => &[],
            Self::Maximized => &[xdg_toplevel::State::Maximized],
            Self::Fullscreen => &[xdg_toplevel::State::Fullscreen],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowGeometry {
    pub(super) placement: SurfacePlacement,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToplevelConfigureSize {
    Suggested { width: u32, height: u32 },
    Unspecified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NormalRestoreGeometrySource {
    ExplicitPersistent,
    ImplicitSurfaceTree,
}

impl NormalRestoreGeometrySource {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::ExplicitPersistent => "explicit_persistent",
            Self::ImplicitSurfaceTree => "implicit_surface_tree",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NormalRestoreGeometryObservation {
    pub(crate) geometry: XdgWindowGeometry,
    pub(crate) source: NormalRestoreGeometrySource,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_modes_publish_only_their_xdg_state() {
        assert_eq!(ToplevelMode::Normal.xdg_states(), &[]);
        assert_eq!(
            ToplevelMode::Maximized.xdg_states(),
            &[xdg_toplevel::State::Maximized]
        );
        assert_eq!(
            ToplevelMode::Fullscreen.xdg_states(),
            &[xdg_toplevel::State::Fullscreen]
        );
    }

    #[test]
    fn restore_geometry_is_captured_only_from_normal_protocol_mode() {
        let normal_geometry = WindowGeometry::new(SurfacePlacement::root_at(1, 2), 300, 200);
        let other_geometry = WindowGeometry::new(SurfacePlacement::root_at(3, 4), 500, 400);
        let mut state = WindowState::default();

        state.set_mode(ToplevelMode::Maximized);
        state.capture_restore_geometry(other_geometry);
        assert_eq!(state.take_restore_geometry(), None);

        state.set_mode(ToplevelMode::Normal);
        state.capture_restore_geometry(normal_geometry);
        assert_eq!(
            state.normal_restore_target(),
            Some(NormalRestoreTarget::Known(normal_geometry))
        );
        assert_eq!(state.take_restore_geometry(), Some(normal_geometry));
    }

    #[test]
    fn zero_sized_normal_geometry_is_not_a_known_restore_geometry() {
        let mut state = WindowState::default();
        let unknown_size = WindowGeometry::new(SurfacePlacement::root_at(72, 72), 0, 0);

        state.capture_restore_geometry(unknown_size);

        assert_eq!(state.restore_geometry(), None);
        assert_eq!(
            state.normal_restore_target(),
            Some(NormalRestoreTarget::UnknownSize {
                placement: unknown_size.placement,
            })
        );
    }

    #[test]
    fn known_normal_restore_target_survives_later_maximized_geometry() {
        let normal = WindowGeometry::new(SurfacePlacement::root_at(72, 72), 300, 200);
        let maximized = WindowGeometry::new(SurfacePlacement::root_at(0, 45), 1920, 955);
        let mut state = WindowState::default();

        state.capture_restore_geometry(normal);
        state.set_mode(ToplevelMode::Maximized);
        state.capture_restore_geometry(maximized);

        assert_eq!(
            state.normal_restore_target(),
            Some(NormalRestoreTarget::Known(normal))
        );
    }
}
