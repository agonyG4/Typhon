use super::*;

#[derive(Debug, Default)]
pub(in crate::compositor) struct PointerConstraintRuntimeState {
    pub(in crate::compositor::state::pointer_constraints) constraints:
        HashMap<u64, PointerConstraint>,
    pub(in crate::compositor::state::pointer_constraints) pending_pointer_constraint_surface_states:
        HashMap<u32, CapturedPointerConstraintSurfaceState>,
    pub(in crate::compositor::state::pointer_constraints) next_internal_pointer_constraint_id: u64,
    pub(in crate::compositor::state::pointer_constraints) next_pointer_constraint_generation: u64,
    pub(in crate::compositor::state::pointer_constraints) active_locked_pointer_routing:
        Option<ActiveLockedPointerRouting>,
    pub(in crate::compositor::state::pointer_constraints) active_confined_pointer_routing:
        Option<ActiveConfinedPointerRouting>,
    pub(in crate::compositor::state::pointer_constraints) dispatch_epoch: u64,
    pub(in crate::compositor::state::pointer_constraints) active_backend_constraint:
        Option<PointerConstraintBackendId>,
    pub(in crate::compositor::state::pointer_constraints) pending_backend_constraint:
        Option<PointerConstraintBackendId>,
    pub(in crate::compositor::state::pointer_constraints) pending_locked_pointer_reveal:
        Option<PendingLockedPointerReveal>,
    pub(in crate::compositor::state::pointer_constraints) last_cursor_reveal_authority:
        Option<CursorRevealAuthority>,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct ActiveLockedPointerRouting {
    pub(in crate::compositor) constraint_id: u64,
    pub(in crate::compositor) generation: u64,
    pub(in crate::compositor) pointer: wl_pointer::WlPointer,
    pub(in crate::compositor) surface: wl_surface::WlSurface,
    pub(in crate::compositor) surface_x: f64,
    pub(in crate::compositor) surface_y: f64,
    pub(in crate::compositor) activation_anchor: OutputPosition,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct ActiveConfinedPointerRouting {
    pub(in crate::compositor) constraint_id: u64,
    pub(in crate::compositor) generation: u64,
    pub(in crate::compositor) pointer: wl_pointer::WlPointer,
    pub(in crate::compositor) surface: wl_surface::WlSurface,
    pub(in crate::compositor) region: OutputRegion,
}

#[derive(Debug, Clone)]
pub(super) struct PendingLockedPointerReveal {
    pub(in crate::compositor::state::pointer_constraints) backend_id: PointerConstraintBackendId,
    pub(in crate::compositor::state::pointer_constraints) pointer: wl_pointer::WlPointer,
    pub(in crate::compositor::state::pointer_constraints) surface: wl_surface::WlSurface,
    pub(in crate::compositor::state::pointer_constraints) fallback_position: Option<OutputPosition>,
    pub(in crate::compositor::state::pointer_constraints) fallback_origin:
        Option<PointerWarpOrigin>,
    pub(in crate::compositor::state::pointer_constraints) backend_restore_settled: bool,
    pub(in crate::compositor::state::pointer_constraints) backend_settled_dispatch_epoch:
        Option<u64>,
    pub(in crate::compositor::state::pointer_constraints) client_warp_position:
        Option<OutputPosition>,
}

#[derive(Debug, Clone)]
pub(super) struct PointerConstraint {
    pub(in crate::compositor::state::pointer_constraints) id: u64,
    pub(in crate::compositor::state::pointer_constraints) generation: u64,
    pub(in crate::compositor::state::pointer_constraints) mode: PointerConstraintMode,
    pub(in crate::compositor::state::pointer_constraints) lifetime: PointerConstraintLifetime,
    pub(in crate::compositor::state::pointer_constraints) surface: wl_surface::WlSurface,
    pub(in crate::compositor::state::pointer_constraints) pointer: wl_pointer::WlPointer,
    pub(in crate::compositor::state::pointer_constraints) locked_resource:
        Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    pub(in crate::compositor::state::pointer_constraints) confined_resource:
        Option<zwp_confined_pointer_v1::ZwpConfinedPointerV1>,
    pub(in crate::compositor::state::pointer_constraints) active: bool,
    pub(in crate::compositor::state::pointer_constraints) backend_pending: bool,
    pub(in crate::compositor::state::pointer_constraints) canceled_backend_activation: bool,
    pub(in crate::compositor::state::pointer_constraints) protocol_resource_alive: bool,
    pub(in crate::compositor::state::pointer_constraints) surface_constraint_pending: bool,
    pub(in crate::compositor::state::pointer_constraints) lifecycle_removal_pending: bool,
    pub(in crate::compositor::state::pointer_constraints) defunct: bool,
    pub(in crate::compositor::state::pointer_constraints) committed: bool,
    pub(in crate::compositor::state::pointer_constraints) committed_region: SurfaceInputRegion,
    pub(in crate::compositor::state::pointer_constraints) committed_cursor_position_hint:
        Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(in crate::compositor) struct PointerConstraintRegistration {
    pub(in crate::compositor) id: u64,
    pub(in crate::compositor) mode: PointerConstraintMode,
    pub(in crate::compositor) lifetime: PointerConstraintLifetime,
    pub(in crate::compositor) surface: wl_surface::WlSurface,
    pub(in crate::compositor) pointer: wl_pointer::WlPointer,
    pub(in crate::compositor) locked_resource: Option<zwp_locked_pointer_v1::ZwpLockedPointerV1>,
    pub(in crate::compositor) confined_resource:
        Option<zwp_confined_pointer_v1::ZwpConfinedPointerV1>,
    pub(in crate::compositor) region: SurfaceInputRegion,
}

impl PointerConstraint {
    pub(super) fn backend_id(&self) -> PointerConstraintBackendId {
        PointerConstraintBackendId {
            constraint_id: self.id,
            generation: self.generation,
        }
    }
}

impl PointerConstraintRuntimeState {
    pub(super) fn allocate_internal_constraint_id(&mut self) -> u64 {
        self.next_internal_pointer_constraint_id = self
            .next_internal_pointer_constraint_id
            .saturating_add(1)
            .max(1);
        self.next_internal_pointer_constraint_id
    }

    pub(super) fn allocate_generation(&mut self) -> u64 {
        self.next_pointer_constraint_generation = self
            .next_pointer_constraint_generation
            .wrapping_add(1)
            .max(1);
        self.next_pointer_constraint_generation
    }

    pub(super) fn constraint_ids_for_surface(&self, surface_id: u32) -> Vec<u64> {
        self.constraints
            .values()
            .filter(|constraint| compositor_surface_id(&constraint.surface) == surface_id)
            .map(|constraint| constraint.id)
            .collect()
    }

    pub(super) fn constraint_ids_for_pointer(&self, pointer: &wl_pointer::WlPointer) -> Vec<u64> {
        self.constraints
            .values()
            .filter(|constraint| same_wayland_resource(&constraint.pointer, pointer))
            .map(|constraint| constraint.id)
            .collect()
    }

    #[cfg(test)]
    pub(super) fn constraint_ids(&self) -> Vec<u64> {
        self.constraints.keys().copied().collect()
    }

    /// Absolute-motion suppression follows backend-effective committed state,
    /// without resource-liveness filtering used by input routing snapshots.
    pub(super) fn locked_surface_for_absolute_motion(&self) -> Option<u32> {
        let backend_id = self.active_backend_constraint?;
        let constraint = self.constraints.get(&backend_id.constraint_id)?;
        (constraint.generation == backend_id.generation
            && constraint.active
            && constraint.mode == PointerConstraintMode::Locked)
            .then(|| compositor_surface_id(&constraint.surface))
    }

    #[cfg(test)]
    pub(super) fn has_pending_locked_reveal(&self) -> bool {
        self.pending_locked_pointer_reveal.is_some()
    }

    pub(super) fn last_cursor_reveal_authority(&self) -> Option<CursorRevealAuthority> {
        self.last_cursor_reveal_authority
    }

    pub(super) fn clear_cursor_reveal_authority_if(&mut self, completed: CursorRevealAuthority) {
        if self.last_cursor_reveal_authority == Some(completed) {
            self.last_cursor_reveal_authority = None;
        }
    }

    #[cfg(test)]
    pub(super) fn active_locked_pointer_anchor(&self) -> Option<OutputPosition> {
        self.active_locked_pointer_routing
            .as_ref()
            .map(|active| active.activation_anchor)
    }

    #[cfg(test)]
    pub(super) fn surface_snapshot(
        &self,
        constraint_id: u64,
    ) -> Option<PointerConstraintRuntimeSnapshot> {
        self.constraints
            .get(&constraint_id)
            .map(|constraint| PointerConstraintRuntimeSnapshot {
                committed: constraint.committed,
                active: constraint.active,
                protocol_resource_alive: constraint.protocol_resource_alive,
                backend_pending: constraint.backend_pending,
                surface_constraint_pending: constraint.surface_constraint_pending,
                lifecycle_removal_pending: constraint.lifecycle_removal_pending,
                defunct: constraint.defunct,
                committed_region: constraint.committed_region.clone(),
                committed_cursor_position_hint: constraint.committed_cursor_position_hint,
            })
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(in crate::compositor) struct PointerConstraintRuntimeSnapshot {
    pub(in crate::compositor) committed: bool,
    pub(in crate::compositor) active: bool,
    pub(in crate::compositor) protocol_resource_alive: bool,
    pub(in crate::compositor) backend_pending: bool,
    pub(in crate::compositor) surface_constraint_pending: bool,
    pub(in crate::compositor) lifecycle_removal_pending: bool,
    pub(in crate::compositor) defunct: bool,
    pub(in crate::compositor) committed_region: SurfaceInputRegion,
    pub(in crate::compositor) committed_cursor_position_hint: Option<(f64, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_ids_saturate_without_reusing_earlier_ids() {
        let mut runtime = PointerConstraintRuntimeState::default();
        assert_eq!(runtime.allocate_internal_constraint_id(), 1);
        assert_eq!(runtime.allocate_internal_constraint_id(), 2);
        runtime.next_internal_pointer_constraint_id = u64::MAX;
        assert_eq!(runtime.allocate_internal_constraint_id(), u64::MAX);
    }

    #[test]
    fn generations_wrap_to_the_next_nonzero_identity() {
        let mut runtime = PointerConstraintRuntimeState::default();
        assert_eq!(runtime.allocate_generation(), 1);
        runtime.next_pointer_constraint_generation = u64::MAX;
        assert_eq!(runtime.allocate_generation(), 1);
    }
}
