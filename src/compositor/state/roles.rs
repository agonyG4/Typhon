use super::*;
use std::num::NonZeroU64;

use crate::xwayland::{AssociationError, XwaylandAssociationEvent};

/// A role is permanent for the lifetime of its wl_surface.  The associated
/// protocol object is deliberately tracked separately because destroying that
/// object can unmap a surface without making role switching legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum PermanentSurfaceRole {
    XdgToplevel,
    XdgPopup,
    LayerSurface,
    Subsurface,
    Cursor,
    DragIcon,
    Xwayland,
}

impl PermanentSurfaceRole {
    pub(in crate::compositor) const fn label(self) -> &'static str {
        match self {
            Self::XdgToplevel => "xdg_toplevel",
            Self::XdgPopup => "xdg_popup",
            Self::LayerSurface => "layer_surface",
            Self::Subsurface => "subsurface",
            Self::Cursor => "cursor",
            Self::DragIcon => "drag_icon",
            Self::Xwayland => "xwayland",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum LiveRoleInstance {
    XdgToplevel,
    XdgPopup,
    LayerSurface,
    Subsurface { parent_id: u32 },
    Cursor,
    DragIcon,
    Xwayland,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(in crate::compositor) struct SurfaceRoleLifecycle {
    pub(in crate::compositor) permanent: Option<PermanentSurfaceRole>,
    pub(in crate::compositor) live_instance: Option<LiveRoleInstance>,
    pub(in crate::compositor) xdg_association: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct XwaylandSurfaceState {
    pub(in crate::compositor) generation: XwaylandGeneration,
    pub(in crate::compositor) pending_serial: Option<NonZeroU64>,
    pub(in crate::compositor) committed_serial: Option<NonZeroU64>,
    pub(in crate::compositor) association_object_alive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum XwaylandSurfaceCommit {
    None,
    Committed {
        generation: XwaylandGeneration,
        serial: NonZeroU64,
    },
    AlreadyAssociated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum XdgAssociationReservation {
    Fresh,
    Reassociation {
        permanent_role: PermanentSurfaceRole,
    },
}

fn validate_xdg_association_reservation(
    surface_known: bool,
    lifecycle: SurfaceRoleLifecycle,
) -> Result<XdgAssociationReservation, SurfaceRoleError> {
    if !surface_known {
        return Err(SurfaceRoleError::MissingSurface);
    }
    if lifecycle.xdg_association {
        return Err(SurfaceRoleError::XdgAssociationExists);
    }
    if let Some(live_instance) = lifecycle.live_instance {
        return Err(SurfaceRoleError::AlreadyAssigned {
            current: live_instance.surface_role(),
            requested: SurfaceRole::XdgToplevel,
        });
    }
    if let Some(permanent_role) = lifecycle.permanent
        && !matches!(
            permanent_role,
            PermanentSurfaceRole::XdgToplevel | PermanentSurfaceRole::XdgPopup
        )
    {
        return Err(SurfaceRoleError::AlreadyAssigned {
            current: match permanent_role {
                PermanentSurfaceRole::XdgToplevel => SurfaceRole::XdgToplevel,
                PermanentSurfaceRole::XdgPopup => SurfaceRole::XdgPopup,
                PermanentSurfaceRole::LayerSurface => SurfaceRole::LayerSurface,
                PermanentSurfaceRole::Subsurface => SurfaceRole::Subsurface { parent_id: 0 },
                PermanentSurfaceRole::Cursor => SurfaceRole::Cursor,
                PermanentSurfaceRole::DragIcon => SurfaceRole::DragIcon,
                PermanentSurfaceRole::Xwayland => SurfaceRole::Xwayland,
            },
            requested: SurfaceRole::XdgToplevel,
        });
    }
    Ok(match lifecycle.permanent {
        Some(permanent_role) => XdgAssociationReservation::Reassociation { permanent_role },
        None => XdgAssociationReservation::Fresh,
    })
}

fn activate_role_instance(
    lifecycle: &mut SurfaceRoleLifecycle,
    requested: SurfaceRole,
    current: SurfaceRole,
) -> Result<(), SurfaceRoleError> {
    let Some(requested_permanent) = requested.permanent() else {
        return Ok(());
    };
    if let Some(current_permanent) = lifecycle.permanent
        && current_permanent != requested_permanent
    {
        return Err(SurfaceRoleError::AlreadyAssigned { current, requested });
    }
    if lifecycle.live_instance.is_some() {
        if matches!(requested, SurfaceRole::Cursor)
            && lifecycle.live_instance == Some(LiveRoleInstance::Cursor)
        {
            return Ok(());
        }
        return Err(SurfaceRoleError::AlreadyAssigned { current, requested });
    }
    lifecycle.permanent = Some(requested_permanent);
    lifecycle.live_instance = Some(match requested {
        SurfaceRole::XdgToplevel => LiveRoleInstance::XdgToplevel,
        SurfaceRole::XdgPopup => LiveRoleInstance::XdgPopup,
        SurfaceRole::LayerSurface => LiveRoleInstance::LayerSurface,
        SurfaceRole::Subsurface { parent_id } => LiveRoleInstance::Subsurface { parent_id },
        SurfaceRole::Cursor => LiveRoleInstance::Cursor,
        SurfaceRole::DragIcon => LiveRoleInstance::DragIcon,
        SurfaceRole::Xwayland => LiveRoleInstance::Xwayland,
        SurfaceRole::Unassigned => unreachable!(),
    });
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceRole {
    Unassigned,
    XdgToplevel,
    XdgPopup,
    LayerSurface,
    Subsurface { parent_id: u32 },
    Cursor,
    DragIcon,
    Xwayland,
}

impl SurfaceRole {
    pub(in crate::compositor) const fn label(self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::XdgToplevel => "xdg_toplevel",
            Self::XdgPopup => "xdg_popup",
            Self::LayerSurface => "layer_surface",
            Self::Subsurface { .. } => "subsurface",
            Self::Cursor => "cursor",
            Self::DragIcon => "drag_icon",
            Self::Xwayland => "xwayland",
        }
    }

    const fn permanent(self) -> Option<PermanentSurfaceRole> {
        match self {
            Self::Unassigned => None,
            Self::XdgToplevel => Some(PermanentSurfaceRole::XdgToplevel),
            Self::XdgPopup => Some(PermanentSurfaceRole::XdgPopup),
            Self::LayerSurface => Some(PermanentSurfaceRole::LayerSurface),
            Self::Subsurface { .. } => Some(PermanentSurfaceRole::Subsurface),
            Self::Cursor => Some(PermanentSurfaceRole::Cursor),
            Self::DragIcon => Some(PermanentSurfaceRole::DragIcon),
            Self::Xwayland => Some(PermanentSurfaceRole::Xwayland),
        }
    }
}

impl LiveRoleInstance {
    const fn surface_role(self) -> SurfaceRole {
        match self {
            Self::XdgToplevel => SurfaceRole::XdgToplevel,
            Self::XdgPopup => SurfaceRole::XdgPopup,
            Self::LayerSurface => SurfaceRole::LayerSurface,
            Self::Subsurface { parent_id } => SurfaceRole::Subsurface { parent_id },
            Self::Cursor => SurfaceRole::Cursor,
            Self::DragIcon => SurfaceRole::DragIcon,
            Self::Xwayland => SurfaceRole::Xwayland,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_icon_live_role_is_reported_as_drag_icon() {
        assert_eq!(
            LiveRoleInstance::DragIcon.surface_role(),
            SurfaceRole::DragIcon
        );
    }

    #[test]
    fn dormant_xdg_role_reserves_a_same_role_association() {
        let lifecycle = SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::XdgToplevel),
            live_instance: None,
            xdg_association: false,
        };

        assert_eq!(
            validate_xdg_association_reservation(true, lifecycle),
            Ok(XdgAssociationReservation::Reassociation {
                permanent_role: PermanentSurfaceRole::XdgToplevel,
            })
        );
    }

    #[test]
    fn live_role_and_non_xdg_role_cannot_reserve_xdg_association() {
        let live = SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::XdgToplevel),
            live_instance: Some(LiveRoleInstance::XdgToplevel),
            xdg_association: false,
        };
        assert!(matches!(
            validate_xdg_association_reservation(true, live),
            Err(SurfaceRoleError::AlreadyAssigned { .. })
        ));

        let layer = SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::LayerSurface),
            live_instance: None,
            xdg_association: false,
        };
        assert!(matches!(
            validate_xdg_association_reservation(true, layer),
            Err(SurfaceRoleError::AlreadyAssigned { .. })
        ));
    }

    #[test]
    fn same_role_reactivates_but_cross_role_stays_permanent() {
        let mut lifecycle = SurfaceRoleLifecycle {
            permanent: Some(PermanentSurfaceRole::XdgToplevel),
            live_instance: None,
            xdg_association: true,
        };
        assert!(
            activate_role_instance(
                &mut lifecycle,
                SurfaceRole::XdgToplevel,
                SurfaceRole::XdgToplevel,
            )
            .is_ok()
        );
        assert_eq!(lifecycle.live_instance, Some(LiveRoleInstance::XdgToplevel));

        lifecycle.live_instance = None;
        assert!(matches!(
            activate_role_instance(
                &mut lifecycle,
                SurfaceRole::XdgPopup,
                SurfaceRole::XdgToplevel,
            ),
            Err(SurfaceRoleError::AlreadyAssigned { .. })
        ));
        assert_eq!(lifecycle.permanent, Some(PermanentSurfaceRole::XdgToplevel));
    }

    #[test]
    fn destroying_association_preserves_role_and_surface_scrub_removes_lifecycle() {
        let mut state = CompositorState::default();
        state.surface_role_lifecycles.insert(
            7,
            SurfaceRoleLifecycle {
                permanent: Some(PermanentSurfaceRole::XdgPopup),
                live_instance: None,
                xdg_association: true,
            },
        );
        state.destroy_xdg_association(7);
        assert_eq!(
            state.permanent_surface_role(7),
            Some(PermanentSurfaceRole::XdgPopup)
        );
        assert!(!state.xdg_association_exists(7));

        state.scrub_surface_lifecycle(7);
        assert_eq!(
            state.surface_role_lifecycle(7),
            SurfaceRoleLifecycle::default()
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum SurfaceRoleError {
    AlreadyAssigned {
        current: SurfaceRole,
        requested: SurfaceRole,
    },
    MissingSurface,
    MissingParent,
    Cycle,
    XdgAssociationExists,
    MissingXdgAssociation,
}

impl SurfaceRoleError {
    pub(in crate::compositor) fn message(self) -> String {
        match self {
            Self::AlreadyAssigned { current, requested } => format!(
                "wl_surface already has role {} and cannot become {}",
                current.label(),
                requested.label()
            ),
            Self::MissingSurface => "wl_surface is not known to this compositor".to_string(),
            Self::MissingParent => "parent wl_surface is not known to this compositor".to_string(),
            Self::Cycle => "subsurface parent relationship would create a cycle".to_string(),
            Self::XdgAssociationExists => "wl_surface already has an xdg_surface".to_string(),
            Self::MissingXdgAssociation => {
                "xdg_surface has no associated wl_surface lifecycle".to_string()
            }
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn get_xwayland_surface(
        &mut self,
        surface_id: u32,
        generation: XwaylandGeneration,
    ) -> Result<(), SurfaceRoleError> {
        self.assign_surface_role(surface_id, SurfaceRole::Xwayland)?;
        self.xwayland_surface_states.insert(
            surface_id,
            XwaylandSurfaceState {
                generation,
                pending_serial: None,
                committed_serial: None,
                association_object_alive: true,
            },
        );
        Ok(())
    }

    pub(in crate::compositor) fn register_xwayland_surface_resource(
        &mut self,
        surface_id: u32,
        resource: xwayland_surface_v1::XwaylandSurfaceV1,
    ) {
        self.xwayland_surface_resources.insert(surface_id, resource);
    }

    pub(in crate::compositor) fn set_xwayland_pending_serial(
        &mut self,
        surface_id: u32,
        generation: XwaylandGeneration,
        serial: NonZeroU64,
    ) -> Result<(), ()> {
        let Some(state) = self.xwayland_surface_states.get_mut(&surface_id) else {
            return Err(());
        };
        if state.generation != generation || !state.association_object_alive {
            return Err(());
        }
        state.pending_serial = Some(serial);
        Ok(())
    }

    pub(in crate::compositor) fn commit_xwayland_surface_serial(
        &mut self,
        surface_id: u32,
    ) -> Result<XwaylandSurfaceCommit, AssociationError> {
        let Some(state) = self.xwayland_surface_states.get_mut(&surface_id) else {
            return Ok(XwaylandSurfaceCommit::None);
        };
        let Some(serial) = state.pending_serial.take() else {
            return Ok(XwaylandSurfaceCommit::None);
        };
        if state.committed_serial.is_some() {
            return Ok(XwaylandSurfaceCommit::AlreadyAssociated);
        }
        self.xwayland_associations
            .commit_surface_serial(state.generation, serial, surface_id)?;
        state.committed_serial = Some(serial);
        Ok(XwaylandSurfaceCommit::Committed {
            generation: state.generation,
            serial,
        })
    }

    pub(in crate::compositor) fn destroy_xwayland_surface_object(&mut self, surface_id: u32) {
        if let Some(state) = self.xwayland_surface_states.get_mut(&surface_id) {
            state.pending_serial = None;
            state.association_object_alive = false;
        }
        self.xwayland_surface_resources.remove(&surface_id);
        self.deactivate_role_instance_if(surface_id, SurfaceRole::Xwayland);
    }

    pub(in crate::compositor) fn take_xwayland_association_events(
        &mut self,
    ) -> Vec<XwaylandAssociationEvent> {
        self.xwayland_associations.take_events()
    }

    pub(in crate::compositor) fn clear_xwayland_generation(
        &mut self,
        generation: XwaylandGeneration,
    ) {
        self.xwayland_associations.clear_generation(generation);
        self.xwayland_surface_states
            .retain(|_, state| state.generation != generation);
    }

    pub(in crate::compositor) fn surface_role_lifecycle(
        &self,
        surface_id: u32,
    ) -> SurfaceRoleLifecycle {
        self.surface_role_lifecycles
            .get(&surface_id)
            .copied()
            .unwrap_or_default()
    }

    /// Returns the live runtime role. Drag-icon identity remains observable
    /// after its live drag session ends so it cannot be mistaken for an
    /// unassigned surface.
    pub(in crate::compositor) fn surface_role(&self, surface_id: u32) -> SurfaceRole {
        let lifecycle = self
            .surface_role_lifecycles
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        lifecycle
            .live_instance
            .map(LiveRoleInstance::surface_role)
            .or_else(|| {
                matches!(
                    lifecycle.permanent,
                    Some(PermanentSurfaceRole::DragIcon | PermanentSurfaceRole::Xwayland)
                )
                .then_some(match lifecycle.permanent {
                    Some(PermanentSurfaceRole::DragIcon) => SurfaceRole::DragIcon,
                    Some(PermanentSurfaceRole::Xwayland) => SurfaceRole::Xwayland,
                    _ => unreachable!(),
                })
            })
            .unwrap_or(SurfaceRole::Unassigned)
    }

    pub(in crate::compositor) fn permanent_surface_role(
        &self,
        surface_id: u32,
    ) -> Option<PermanentSurfaceRole> {
        self.surface_role_lifecycles
            .get(&surface_id)
            .and_then(|lifecycle| lifecycle.permanent)
    }

    pub(in crate::compositor) fn reserve_xdg_association(
        &mut self,
        surface_id: u32,
    ) -> Result<XdgAssociationReservation, SurfaceRoleError> {
        let lifecycle = self
            .surface_role_lifecycles
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        let reservation = validate_xdg_association_reservation(
            self.surface_resources.contains_key(&surface_id),
            lifecycle,
        )
        .map_err(|error| match error {
            SurfaceRoleError::AlreadyAssigned { .. } => SurfaceRoleError::AlreadyAssigned {
                current: self.surface_role_for_error(surface_id),
                requested: SurfaceRole::XdgToplevel,
            },
            error => error,
        })?;
        self.surface_role_lifecycles
            .entry(surface_id)
            .or_default()
            .xdg_association = true;
        Ok(reservation)
    }

    pub(in crate::compositor) fn destroy_xdg_association(&mut self, surface_id: u32) {
        if let Some(lifecycle) = self.surface_role_lifecycles.get_mut(&surface_id) {
            lifecycle.xdg_association = false;
            if lifecycle.permanent.is_none() && lifecycle.live_instance.is_none() {
                self.surface_role_lifecycles.remove(&surface_id);
            }
        }
    }

    pub(in crate::compositor) fn xdg_association_exists(&self, surface_id: u32) -> bool {
        self.surface_role_lifecycles
            .get(&surface_id)
            .is_some_and(|lifecycle| lifecycle.xdg_association)
    }

    pub(in crate::compositor) fn is_dormant_xdg_cross_role_request(
        &self,
        surface_id: u32,
        requested: SurfaceRole,
    ) -> bool {
        let requested_permanent = requested.permanent();
        let lifecycle = self.surface_role_lifecycle(surface_id);
        lifecycle.live_instance.is_none()
            && matches!(
                lifecycle.permanent,
                Some(PermanentSurfaceRole::XdgToplevel | PermanentSurfaceRole::XdgPopup)
            )
            && lifecycle.permanent != requested_permanent
    }

    pub(in crate::compositor) fn construct_xdg_role(
        &mut self,
        surface_id: u32,
        requested: SurfaceRole,
    ) -> Result<(), SurfaceRoleError> {
        if !matches!(requested, SurfaceRole::XdgToplevel | SurfaceRole::XdgPopup) {
            return Err(SurfaceRoleError::MissingXdgAssociation);
        }
        if !self.xdg_association_exists(surface_id) {
            return Err(SurfaceRoleError::MissingXdgAssociation);
        }
        self.assign_surface_role(surface_id, requested)
    }

    pub(in crate::compositor) fn assign_surface_role(
        &mut self,
        surface_id: u32,
        requested: SurfaceRole,
    ) -> Result<(), SurfaceRoleError> {
        let Some(_requested_permanent) = requested.permanent() else {
            return Ok(());
        };
        if !self.surface_resources.contains_key(&surface_id) {
            return Err(SurfaceRoleError::MissingSurface);
        }

        if let SurfaceRole::Subsurface { parent_id } = requested {
            if !self.surface_resources.contains_key(&parent_id) {
                return Err(SurfaceRoleError::MissingParent);
            }
            if self.subsurface_role_would_cycle(surface_id, parent_id) {
                return Err(SurfaceRoleError::Cycle);
            }
        }

        let current = self.surface_role_for_error(surface_id);
        let lifecycle = self.surface_role_lifecycles.entry(surface_id).or_default();
        activate_role_instance(lifecycle, requested, current)?;
        if surface_tree_debug_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_role surface={surface_id} old={} new={} parent={:?}",
                current.label(),
                requested.label(),
                match requested {
                    SurfaceRole::Subsurface { parent_id } => Some(parent_id),
                    _ => None,
                }
            );
        }
        Ok(())
    }

    pub(in crate::compositor) fn deactivate_role_instance(&mut self, surface_id: u32) {
        if let Some(lifecycle) = self.surface_role_lifecycles.get_mut(&surface_id) {
            lifecycle.live_instance = None;
        }
    }

    pub(in crate::compositor) fn deactivate_role_instance_if(
        &mut self,
        surface_id: u32,
        expected: SurfaceRole,
    ) {
        let expected_permanent = expected.permanent();
        if self
            .surface_role_lifecycles
            .get(&surface_id)
            .is_some_and(|lifecycle| {
                lifecycle.permanent == expected_permanent && lifecycle.live_instance.is_some()
            })
        {
            self.deactivate_role_instance(surface_id);
        }
    }

    pub(in crate::compositor) fn rollback_surface_role_reservation(
        &mut self,
        surface_id: u32,
        requested: SurfaceRole,
    ) {
        let expected_permanent = requested.permanent();
        if self
            .surface_role_lifecycles
            .get(&surface_id)
            .is_some_and(|lifecycle| {
                lifecycle.permanent == expected_permanent
                    && lifecycle.live_instance.is_none()
                    && !lifecycle.xdg_association
            })
        {
            self.surface_role_lifecycles.remove(&surface_id);
        }
    }

    pub(in crate::compositor) fn validate_surface_destroy(&self, surface_id: u32) -> bool {
        self.surface_role_lifecycles
            .get(&surface_id)
            .is_none_or(|lifecycle| lifecycle.live_instance.is_none())
    }

    pub(in crate::compositor) fn scrub_surface_lifecycle(&mut self, surface_id: u32) {
        self.surface_role_lifecycles.remove(&surface_id);
        self.xwayland_surface_states.remove(&surface_id);
        self.xwayland_surface_resources.remove(&surface_id);
        self.xwayland_associations.remove_surface(surface_id);
    }

    fn surface_role_for_error(&self, surface_id: u32) -> SurfaceRole {
        if let Some(live) = self
            .surface_role_lifecycles
            .get(&surface_id)
            .and_then(|lifecycle| lifecycle.live_instance)
        {
            return live.surface_role();
        }
        match self.permanent_surface_role(surface_id) {
            Some(PermanentSurfaceRole::XdgToplevel) => SurfaceRole::XdgToplevel,
            Some(PermanentSurfaceRole::XdgPopup) => SurfaceRole::XdgPopup,
            Some(PermanentSurfaceRole::LayerSurface) => SurfaceRole::LayerSurface,
            Some(PermanentSurfaceRole::Subsurface) => SurfaceRole::Subsurface {
                parent_id: self.subsurface_transactions.parent(surface_id).unwrap_or(0),
            },
            Some(PermanentSurfaceRole::Cursor) => SurfaceRole::Cursor,
            Some(PermanentSurfaceRole::DragIcon) => SurfaceRole::DragIcon,
            Some(PermanentSurfaceRole::Xwayland) => SurfaceRole::Xwayland,
            None => SurfaceRole::Unassigned,
        }
    }

    fn subsurface_role_would_cycle(&self, surface_id: u32, mut parent_id: u32) -> bool {
        let mut visited = HashSet::new();
        while visited.insert(parent_id) {
            if parent_id == surface_id {
                return true;
            }
            parent_id = match self
                .surface_role_lifecycles
                .get(&parent_id)
                .and_then(|lifecycle| lifecycle.live_instance)
            {
                Some(LiveRoleInstance::Subsurface { parent_id }) => parent_id,
                _ => match self.subsurface_transactions.parent(parent_id) {
                    Some(parent_id) => parent_id,
                    None => return false,
                },
            };
        }
        true
    }
}

pub(in crate::compositor) fn surface_tree_debug_enabled() -> bool {
    std::env::var_os("OBLIVION_ONE_DEBUG_SURFACE_TREE").is_some_and(|value| value != "0")
}
