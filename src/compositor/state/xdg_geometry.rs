use super::*;
use wayland_protocols::xdg::shell::server::xdg_surface;

impl CompositorState {
    pub(in crate::compositor) fn clear_xdg_window_geometry_state(&mut self, surface_id: u32) {
        self.committed_explicit_xdg_window_geometries
            .remove(&surface_id);
        self.pending_xdg_window_geometry_requests
            .remove(&surface_id);
        self.surface_tree_xdg_geometry_publications
            .remove(&surface_id);
        self.surface_tree_pending_resize_completions
            .retain(|(pending_surface_id, _)| *pending_surface_id != surface_id);
        self.surface_tree_pending_window_open_animations
            .retain(|pending_surface_id| *pending_surface_id != surface_id);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn set_test_effective_xdg_window_geometry(
        &mut self,
        xdg_surface_id: u32,
        geometry: XdgWindowGeometry,
    ) {
        self.xdg_surface_lifecycles
            .entry(xdg_surface_id)
            .or_default();
        self.committed_explicit_xdg_window_geometries.insert(
            xdg_surface_id,
            CommittedExplicitXdgWindowGeometry::Effective {
                requested: geometry,
                effective: geometry,
                commit_sequence: 0,
            },
        );
    }

    #[cfg(test)]
    pub(in crate::compositor) fn committed_explicit_effective_xdg_geometry(
        &self,
        xdg_surface_id: u32,
    ) -> Option<XdgWindowGeometry> {
        match self
            .committed_explicit_xdg_window_geometries
            .get(&xdg_surface_id)?
        {
            CommittedExplicitXdgWindowGeometry::Effective { effective, .. } => Some(*effective),
            CommittedExplicitXdgWindowGeometry::AwaitingBounds { .. }
            | CommittedExplicitXdgWindowGeometry::Invalid { .. } => None,
        }
    }

    pub(in crate::compositor) fn committed_xdg_geometry_is_invalid(
        &self,
        xdg_surface_id: u32,
    ) -> bool {
        matches!(
            self.committed_explicit_xdg_window_geometries
                .get(&xdg_surface_id),
            Some(CommittedExplicitXdgWindowGeometry::Invalid { .. })
        )
    }

    pub(in crate::compositor) fn committed_xdg_surface_tree_bounds(
        &self,
        xdg_surface_id: u32,
    ) -> Option<XdgWindowGeometry> {
        let mut stack = vec![(xdg_surface_id, 0_i64, 0_i64)];
        let mut visited = HashSet::new();
        let mut children_by_parent = HashMap::<u32, Vec<u32>>::new();
        for (surface_id, lifecycle) in &self.surface_role_lifecycles {
            if let Some(LiveRoleInstance::Subsurface { parent_id }) = lifecycle.live_instance {
                children_by_parent
                    .entry(parent_id)
                    .or_default()
                    .push(*surface_id);
            }
        }

        let (mut min_x, mut min_y, mut max_x, mut max_y) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);

        while let Some((surface_id, origin_x, origin_y)) = stack.pop() {
            if !visited.insert(surface_id) {
                continue;
            }

            let size = self
                .renderable_surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
                .map(|surface| (surface.width, surface.height))
                .or_else(|| {
                    let owner_root = self.root_surface_id_for_surface(surface_id);
                    self.toplevel_window_state(owner_root)
                        .and_then(|window| window.minimized_surface(surface_id))
                        .map(|surface| (surface.width, surface.height))
                });

            if let Some((width, height)) = size
                && width > 0
                && height > 0
            {
                let right = origin_x.checked_add(i64::from(width))?;
                let bottom = origin_y.checked_add(i64::from(height))?;
                min_x = min_x.min(origin_x);
                min_y = min_y.min(origin_y);
                max_x = max_x.max(right);
                max_y = max_y.max(bottom);
            }

            for child_id in children_by_parent.get(&surface_id).into_iter().flatten() {
                let placement = self.surface_placement(*child_id);
                if placement.parent_surface_id != Some(surface_id) {
                    continue;
                }
                stack.push((
                    *child_id,
                    origin_x.checked_add(i64::from(placement.local_x))?,
                    origin_y.checked_add(i64::from(placement.local_y))?,
                ));
            }
        }

        if min_x == i64::MAX || max_x <= min_x || max_y <= min_y {
            return None;
        }

        Some(XdgWindowGeometry::new(
            i32::try_from(min_x).ok()?,
            i32::try_from(min_y).ok()?,
            i32::try_from(max_x.checked_sub(min_x)?).ok()?,
            i32::try_from(max_y.checked_sub(min_y)?).ok()?,
        ))
    }

    pub(in crate::compositor) fn effective_xdg_window_geometry(
        &self,
        xdg_surface_id: u32,
    ) -> Option<EffectiveXdgWindowGeometry> {
        // A synchronized surface-tree transaction is one logical publication.
        // Keep semantic reads on its pre-transaction geometry until every
        // committed root and child state has been installed.
        if self.surface_tree_generation.is_some()
            && let Some(publication) = self
                .surface_tree_xdg_geometry_publications
                .get(&xdg_surface_id)
        {
            return publication.before;
        }

        match self
            .committed_explicit_xdg_window_geometries
            .get(&xdg_surface_id)
        {
            Some(CommittedExplicitXdgWindowGeometry::Effective { effective, .. }) => {
                Some(EffectiveXdgWindowGeometry {
                    geometry: *effective,
                    source: EffectiveXdgWindowGeometrySource::ExplicitEffective,
                })
            }
            Some(
                CommittedExplicitXdgWindowGeometry::AwaitingBounds { .. }
                | CommittedExplicitXdgWindowGeometry::Invalid { .. },
            ) => None,
            None => self
                .committed_xdg_surface_tree_bounds(xdg_surface_id)
                .map(|geometry| EffectiveXdgWindowGeometry {
                    geometry,
                    source: EffectiveXdgWindowGeometrySource::ImplicitSurfaceTree,
                }),
        }
    }

    fn xdg_surface_tree_root_for_surface(&self, surface_id: u32) -> Option<u32> {
        let mut root_surface_id = surface_id;
        let mut visited = HashSet::new();
        while visited.insert(root_surface_id) {
            match self
                .surface_role_lifecycles
                .get(&root_surface_id)
                .and_then(|lifecycle| lifecycle.live_instance)
            {
                Some(LiveRoleInstance::Subsurface { parent_id }) => {
                    root_surface_id = parent_id;
                }
                _ => break,
            }
        }
        self.xdg_surface_lifecycles
            .contains_key(&root_surface_id)
            .then_some(root_surface_id)
    }

    pub(in crate::compositor) fn capture_xdg_geometry_for_topology_mutation(
        &self,
        surface_id: u32,
    ) -> Option<(u32, Option<EffectiveXdgWindowGeometry>)> {
        let root_surface_id = self.xdg_surface_tree_root_for_surface(surface_id)?;
        Some((
            root_surface_id,
            self.effective_xdg_window_geometry(root_surface_id),
        ))
    }

    pub(in crate::compositor) fn publish_xdg_geometry_after_topology_mutation(
        &mut self,
        root_surface_id: u32,
        before: Option<EffectiveXdgWindowGeometry>,
    ) -> bool {
        self.resolve_awaiting_xdg_window_geometry(root_surface_id);
        let after = self.effective_xdg_window_geometry(root_surface_id);
        let geometry_changed =
            before.map(|geometry| geometry.geometry) != after.map(|geometry| geometry.geometry);
        self.publish_xdg_geometry_transition(root_surface_id, before, after, None, None);
        geometry_changed
    }

    pub(in crate::compositor) fn capture_xdg_geometry_before_surface_commit(
        &mut self,
        surface_id: u32,
    ) -> Option<(u32, Option<EffectiveXdgWindowGeometry>)> {
        let root_surface_id = self.xdg_surface_tree_root_for_surface(surface_id)?;
        let before = self.effective_xdg_window_geometry(root_surface_id);
        if self.surface_tree_generation.is_some() {
            self.surface_tree_xdg_geometry_publications
                .entry(root_surface_id)
                .or_insert(PendingXdgGeometryPublication {
                    before,
                    latest_request: None,
                    latest_commit_sequence: None,
                    latest_root_commit_sequence: None,
                });
        }
        Some((root_surface_id, before))
    }

    pub(in crate::compositor) fn publish_xdg_geometry_after_surface_commit(
        &mut self,
        root_surface_id: u32,
        committed_surface_id: u32,
        before: Option<EffectiveXdgWindowGeometry>,
        request: Option<XdgWindowGeometry>,
        commit_sequence: u64,
    ) -> bool {
        if self.surface_tree_generation.is_some() {
            let publication = self
                .surface_tree_xdg_geometry_publications
                .entry(root_surface_id)
                .or_insert(PendingXdgGeometryPublication {
                    before,
                    latest_request: None,
                    latest_commit_sequence: None,
                    latest_root_commit_sequence: None,
                });
            if request.is_some()
                && publication
                    .latest_request
                    .is_none_or(|(_, previous_sequence)| commit_sequence >= previous_sequence)
            {
                publication.latest_request = request.map(|request| (request, commit_sequence));
            }
            publication.latest_commit_sequence = Some(
                publication
                    .latest_commit_sequence
                    .unwrap_or_default()
                    .max(commit_sequence),
            );
            if committed_surface_id == root_surface_id {
                publication.latest_root_commit_sequence = Some(
                    publication
                        .latest_root_commit_sequence
                        .unwrap_or_default()
                        .max(commit_sequence),
                );
            }
            return false;
        }

        self.resolve_committed_xdg_window_geometry_request(
            root_surface_id,
            request,
            commit_sequence,
        );
        self.resolve_awaiting_xdg_window_geometry(root_surface_id);
        let after = self.effective_xdg_window_geometry(root_surface_id);
        let root_commit_sequence =
            (committed_surface_id == root_surface_id).then_some(commit_sequence);
        let geometry_changed =
            before.map(|geometry| geometry.geometry) != after.map(|geometry| geometry.geometry);
        self.publish_xdg_geometry_transition(
            root_surface_id,
            before,
            after,
            root_commit_sequence,
            Some(commit_sequence),
        );
        if !geometry_changed
            && let Some(commit_sequence) = root_commit_sequence
            && self
                .toplevel_visual_geometries
                .get(&root_surface_id)
                .is_some_and(|visual| visual.xdg_mode_transition_fence.is_some())
            && self.toplevel_surfaces.contains_key(&root_surface_id)
        {
            self.update_toplevel_visual_render_assignment_after_root_commit(
                root_surface_id,
                SurfaceCommitSequence(commit_sequence),
            );
        }
        geometry_changed
    }

    pub(in crate::compositor) fn finish_surface_tree_xdg_geometry_publication(&mut self) {
        let mut publications = std::mem::take(&mut self.surface_tree_xdg_geometry_publications)
            .into_iter()
            .collect::<Vec<_>>();
        publications.sort_by_key(|(surface_id, _)| *surface_id);

        for (root_surface_id, publication) in publications {
            if let Some((request, commit_sequence)) = publication.latest_request {
                self.resolve_committed_xdg_window_geometry_request(
                    root_surface_id,
                    Some(request),
                    commit_sequence,
                );
            }
            self.resolve_awaiting_xdg_window_geometry(root_surface_id);
            let after = self.effective_xdg_window_geometry(root_surface_id);
            let geometry_changed = publication.before.map(|geometry| geometry.geometry)
                != after.map(|geometry| geometry.geometry);
            self.publish_xdg_geometry_transition(
                root_surface_id,
                publication.before,
                after,
                publication.latest_root_commit_sequence,
                publication.latest_commit_sequence,
            );
            if !geometry_changed
                && publication.latest_root_commit_sequence.is_some()
                && self
                    .toplevel_visual_geometries
                    .get(&root_surface_id)
                    .is_some_and(|visual| visual.xdg_mode_transition_fence.is_some())
                && let Some(commit_sequence) = publication.latest_root_commit_sequence
                && self.toplevel_surfaces.contains_key(&root_surface_id)
            {
                self.update_toplevel_visual_render_assignment_after_root_commit(
                    root_surface_id,
                    SurfaceCommitSequence(commit_sequence),
                );
            }
        }
    }

    fn resolve_committed_xdg_window_geometry_request(
        &mut self,
        xdg_surface_id: u32,
        request: Option<XdgWindowGeometry>,
        commit_sequence: u64,
    ) {
        if let Some(requested) = request {
            self.committed_explicit_xdg_window_geometries.insert(
                xdg_surface_id,
                CommittedExplicitXdgWindowGeometry::AwaitingBounds {
                    requested,
                    commit_sequence,
                },
            );
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: event=xdg_window_geometry_commit_latched surface={xdg_surface_id} commit_sequence={commit_sequence} requested={},{},{},{}",
                    requested.x, requested.y, requested.width, requested.height,
                );
            }
        }
    }

    fn resolve_awaiting_xdg_window_geometry(&mut self, xdg_surface_id: u32) {
        let Some(CommittedExplicitXdgWindowGeometry::AwaitingBounds {
            requested,
            commit_sequence,
        }) = self
            .committed_explicit_xdg_window_geometries
            .get(&xdg_surface_id)
            .copied()
        else {
            return;
        };

        let Some(tree_bounds) = self.committed_xdg_surface_tree_bounds(xdg_surface_id) else {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: event=xdg_window_geometry_awaiting_bounds surface={xdg_surface_id} commit_sequence={commit_sequence} requested={},{},{},{}",
                    requested.x, requested.y, requested.width, requested.height,
                );
            }
            return;
        };

        let Some(effective) = intersect_xdg_window_geometry(requested, tree_bounds) else {
            self.committed_explicit_xdg_window_geometries.insert(
                xdg_surface_id,
                CommittedExplicitXdgWindowGeometry::Invalid {
                    requested,
                    commit_sequence,
                },
            );
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: event=xdg_window_geometry_invalid surface={xdg_surface_id} commit_sequence={commit_sequence} requested={},{},{},{} tree_bounds={},{},{},{}",
                    requested.x,
                    requested.y,
                    requested.width,
                    requested.height,
                    tree_bounds.x,
                    tree_bounds.y,
                    tree_bounds.width,
                    tree_bounds.height,
                );
            }
            if let Some(resource) = self.xdg_surface_resources.get(&xdg_surface_id).cloned()
                && let Some(client) = resource.client()
            {
                self.post_protocol_error(
                    &client,
                    &resource,
                    xdg_surface::Error::InvalidSize,
                    format!(
                        "xdg_surface window geometry ({},{},{},{}) does not intersect committed surface-tree bounds ({},{},{},{})",
                        requested.x,
                        requested.y,
                        requested.width,
                        requested.height,
                        tree_bounds.x,
                        tree_bounds.y,
                        tree_bounds.width,
                        tree_bounds.height,
                    ),
                );
            }
            return;
        };

        self.committed_explicit_xdg_window_geometries.insert(
            xdg_surface_id,
            CommittedExplicitXdgWindowGeometry::Effective {
                requested,
                effective,
                commit_sequence,
            },
        );
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: event=xdg_window_geometry_effective surface={xdg_surface_id} source=explicit_effective commit_sequence={commit_sequence} requested={},{},{},{} tree_bounds={},{},{},{} effective={},{},{},{}",
                requested.x,
                requested.y,
                requested.width,
                requested.height,
                tree_bounds.x,
                tree_bounds.y,
                tree_bounds.width,
                tree_bounds.height,
                effective.x,
                effective.y,
                effective.width,
                effective.height,
            );
        }
    }

    fn publish_xdg_geometry_transition(
        &mut self,
        root_surface_id: u32,
        before: Option<EffectiveXdgWindowGeometry>,
        after: Option<EffectiveXdgWindowGeometry>,
        root_commit_sequence: Option<u64>,
        commit_sequence: Option<u64>,
    ) {
        if before.map(|geometry| geometry.geometry) == after.map(|geometry| geometry.geometry) {
            return;
        }

        if compositor_debug_surface_logging_enabled() {
            let previous = before.map(|geometry| geometry.geometry);
            let source = after.map(|geometry| match geometry.source {
                EffectiveXdgWindowGeometrySource::ExplicitEffective => "explicit_effective",
                EffectiveXdgWindowGeometrySource::ImplicitSurfaceTree => "implicit_surface_tree",
            });
            let geometry = after.map(|geometry| geometry.geometry);
            eprintln!(
                "oblivion-one compositor: event=xdg_window_geometry_effective surface={root_surface_id} source={} commit_sequence={commit_sequence:?} previous={:?} effective={:?}",
                source.unwrap_or("unavailable"),
                previous.map(|geometry| (geometry.x, geometry.y, geometry.width, geometry.height)),
                geometry.map(|geometry| (geometry.x, geometry.y, geometry.width, geometry.height)),
            );
        }

        if self.toplevel_surfaces.contains_key(&root_surface_id) {
            if self
                .toplevel_visual_geometries
                .get(&root_surface_id)
                .is_some_and(|visual| visual.xdg_mode_transition_fence.is_some())
                && let Some(commit_sequence) = root_commit_sequence
            {
                self.update_toplevel_visual_render_assignment_after_root_commit(
                    root_surface_id,
                    SurfaceCommitSequence(commit_sequence),
                );
            } else {
                self.update_toplevel_visual_render_assignment(root_surface_id);
            }
        }

        let before_origin = before
            .map(|geometry| (geometry.geometry.x, geometry.geometry.y))
            .unwrap_or_default();
        let after_origin = after
            .map(|geometry| (geometry.geometry.x, geometry.geometry.y))
            .unwrap_or_default();
        let origin_delta = (
            after_origin.0.saturating_sub(before_origin.0),
            after_origin.1.saturating_sub(before_origin.1),
        );
        if self.popup_surfaces.contains_key(&root_surface_id) {
            self.rebase_popup_surface_placement_for_xdg_geometry_change(
                root_surface_id,
                (0, 0),
                origin_delta,
            );
        }
        if let Some(popup) = self.popup_surfaces.get(&root_surface_id)
            && popup.positioner.reactive
            && self.xdg_surface_is_configured(root_surface_id)
        {
            self.configure_popup_surface(root_surface_id, popup.positioner, None);
        }

        let mut child_popups = self
            .popup_surfaces
            .iter()
            .filter_map(|(popup_surface_id, popup)| {
                (popup.parent_surface_id == Some(root_surface_id))
                    .then_some((*popup_surface_id, popup.positioner))
            })
            .collect::<Vec<_>>();
        child_popups.sort_by_key(|(popup_surface_id, _)| *popup_surface_id);
        for (popup_surface_id, positioner) in child_popups {
            self.rebase_popup_surface_placement_for_xdg_geometry_change(
                popup_surface_id,
                origin_delta,
                (0, 0),
            );
            if positioner.reactive && self.xdg_surface_is_configured(popup_surface_id) {
                self.configure_popup_surface(popup_surface_id, positioner, None);
            }
        }

        self.refresh_active_scene_surface_tree(root_surface_id);
        self.reconcile_surface_tree_output_memberships(root_surface_id);
        let pointer_hit_generation_before_publication = self.pointer_hit_generation;
        self.refresh_pointer_focus_after_geometry_change(
            true,
            pointer_hit_generation_before_publication,
        );
    }
}

fn intersect_xdg_window_geometry(
    requested: XdgWindowGeometry,
    tree_bounds: XdgWindowGeometry,
) -> Option<XdgWindowGeometry> {
    let requested_left = i64::from(requested.x);
    let requested_top = i64::from(requested.y);
    let requested_right = requested_left.checked_add(i64::from(requested.width))?;
    let requested_bottom = requested_top.checked_add(i64::from(requested.height))?;
    let tree_left = i64::from(tree_bounds.x);
    let tree_top = i64::from(tree_bounds.y);
    let tree_right = tree_left.checked_add(i64::from(tree_bounds.width))?;
    let tree_bottom = tree_top.checked_add(i64::from(tree_bounds.height))?;
    let left = requested_left.max(tree_left);
    let top = requested_top.max(tree_top);
    let right = requested_right.min(tree_right);
    let bottom = requested_bottom.min(tree_bottom);
    if right <= left || bottom <= top {
        return None;
    }
    Some(XdgWindowGeometry::new(
        i32::try_from(left).ok()?,
        i32::try_from(top).ok()?,
        i32::try_from(right.checked_sub(left)?).ok()?,
        i32::try_from(bottom.checked_sub(top)?).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_renderable_surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test size"),
                vec![0; width as usize * height as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    fn set_subsurface_parent(state: &mut CompositorState, child: u32, parent: u32) {
        state.surface_role_lifecycles.insert(
            child,
            super::super::roles::SurfaceRoleLifecycle {
                permanent: Some(super::super::roles::PermanentSurfaceRole::Subsurface),
                live_instance: Some(super::super::roles::LiveRoleInstance::Subsurface {
                    parent_id: parent,
                }),
                xdg_association: false,
            },
        );
    }

    #[test]
    fn explicit_window_geometry_is_clamped_to_committed_tree_bounds() {
        assert_eq!(
            intersect_xdg_window_geometry(
                XdgWindowGeometry::new(10, 10, 520, 410),
                XdgWindowGeometry::new(0, 0, 80, 60),
            ),
            Some(XdgWindowGeometry::new(10, 10, 70, 50)),
        );
    }

    #[test]
    fn explicit_window_geometry_with_no_intersection_is_invalid() {
        assert_eq!(
            intersect_xdg_window_geometry(
                XdgWindowGeometry::new(100, 100, 20, 20),
                XdgWindowGeometry::new(0, 0, 80, 60),
            ),
            None,
        );
    }

    #[test]
    fn explicit_geometry_clamps_once_and_freezes_until_a_new_request() {
        let root_id = 10;
        let mut state = CompositorState::default();
        state.append_renderable_surface(test_renderable_surface(root_id, 80, 60));

        let first_request = XdgWindowGeometry::new(10, 10, 520, 410);
        state.resolve_committed_xdg_window_geometry_request(root_id, Some(first_request), 1);
        state.resolve_awaiting_xdg_window_geometry(root_id);
        let first_effective = XdgWindowGeometry::new(10, 10, 70, 50);
        assert_eq!(
            state.effective_xdg_window_geometry(root_id),
            Some(EffectiveXdgWindowGeometry {
                geometry: first_effective,
                source: EffectiveXdgWindowGeometrySource::ExplicitEffective,
            })
        );
        assert_ne!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(first_request),
            "the raw request must never escape as effective geometry"
        );

        let root = state
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == root_id)
            .expect("root surface");
        root.width = 96;
        root.height = 72;
        state.resolve_awaiting_xdg_window_geometry(root_id);
        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(first_effective),
            "tree growth must not recalculate an established explicit geometry"
        );

        let second_request = XdgWindowGeometry::new(5, 6, 500, 400);
        state.resolve_committed_xdg_window_geometry_request(root_id, Some(second_request), 2);
        state.resolve_awaiting_xdg_window_geometry(root_id);
        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(XdgWindowGeometry::new(5, 6, 91, 66)),
            "a new committed request reclamps against the current tree"
        );
    }

    #[test]
    fn awaiting_bounds_keeps_only_the_latest_committed_request() {
        let root_id = 20;
        let mut state = CompositorState::default();
        state.resolve_committed_xdg_window_geometry_request(
            root_id,
            Some(XdgWindowGeometry::new(0, 0, 400, 300)),
            1,
        );
        state.resolve_awaiting_xdg_window_geometry(root_id);
        assert!(state.effective_xdg_window_geometry(root_id).is_none());

        let latest = XdgWindowGeometry::new(10, 5, 300, 200);
        state.resolve_committed_xdg_window_geometry_request(root_id, Some(latest), 2);
        state.resolve_awaiting_xdg_window_geometry(root_id);
        state.append_renderable_surface(test_renderable_surface(root_id, 80, 60));
        state.resolve_awaiting_xdg_window_geometry(root_id);

        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(XdgWindowGeometry::new(10, 5, 70, 55))
        );
        assert!(matches!(
            state.committed_explicit_xdg_window_geometries.get(&root_id),
            Some(CommittedExplicitXdgWindowGeometry::Effective {
                requested,
                commit_sequence: 2,
                ..
            }) if *requested == latest
        ));
    }

    #[test]
    fn implicit_bounds_include_nested_subsurfaces_and_exclude_xdg_popups() {
        let root_id = 30;
        let child_id = 31;
        let grandchild_id = 32;
        let popup_id = 33;
        let mut state = CompositorState::default();
        state.append_renderable_surface(test_renderable_surface(root_id, 400, 300));
        state.append_renderable_surface(test_renderable_surface(child_id, 200, 100));
        state.append_renderable_surface(test_renderable_surface(grandchild_id, 50, 40));
        state.append_renderable_surface(test_renderable_surface(popup_id, 900, 900));
        state.store_surface_placement(child_id, SurfacePlacement::subsurface(root_id, -20, -10));
        state.store_surface_placement(
            grandchild_id,
            SurfacePlacement::subsurface(child_id, -5, 30),
        );
        state.store_surface_placement(
            popup_id,
            SurfacePlacement::subsurface(root_id, 1_000, 1_000),
        );
        set_subsurface_parent(&mut state, child_id, root_id);
        set_subsurface_parent(&mut state, grandchild_id, child_id);
        state.surface_role_lifecycles.insert(
            popup_id,
            super::super::roles::SurfaceRoleLifecycle {
                permanent: Some(super::super::roles::PermanentSurfaceRole::XdgPopup),
                live_instance: Some(super::super::roles::LiveRoleInstance::XdgPopup),
                xdg_association: true,
            },
        );

        assert_eq!(
            state.effective_xdg_window_geometry(root_id),
            Some(EffectiveXdgWindowGeometry {
                geometry: XdgWindowGeometry::new(-25, -10, 425, 310),
                source: EffectiveXdgWindowGeometrySource::ImplicitSurfaceTree,
            })
        );

        state.store_surface_placement(child_id, SurfacePlacement::subsurface(root_id, 25, 15));
        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(XdgWindowGeometry::new(0, 0, 400, 300)),
            "never-explicit geometry follows committed child movement dynamically"
        );
    }

    #[test]
    fn pending_request_is_not_current_effective_geometry() {
        let root_id = 40;
        let mut state = CompositorState::default();
        state.append_renderable_surface(test_renderable_surface(root_id, 100, 80));
        let committed = XdgWindowGeometry::new(5, 6, 40, 30);
        state.set_test_effective_xdg_window_geometry(root_id, committed);
        state
            .pending_xdg_window_geometry_requests
            .insert(root_id, XdgWindowGeometry::new(20, 20, 70, 50));

        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(committed)
        );
    }

    #[test]
    fn synchronized_tree_resolution_uses_final_child_state_as_one_publication() {
        let root_id = 50;
        let child_id = 51;
        let mut state = CompositorState::default();
        state.append_renderable_surface(test_renderable_surface(root_id, 100, 100));
        state.append_renderable_surface(test_renderable_surface(child_id, 20, 20));
        state.xdg_surface_lifecycles.insert(
            root_id,
            super::super::xdg_lifecycle::XdgSurfaceLifecycle::default(),
        );
        state.store_surface_placement(child_id, SurfacePlacement::subsurface(root_id, 80, 80));
        set_subsurface_parent(&mut state, child_id, root_id);
        let before = state
            .effective_xdg_window_geometry(root_id)
            .expect("old implicit geometry");
        let requested = XdgWindowGeometry::new(150, 0, 200, 90);

        state.begin_surface_tree_publication();
        let (captured_root, captured_before) = state
            .capture_xdg_geometry_before_surface_commit(root_id)
            .expect("XDG tree root");
        assert_eq!(captured_root, root_id);
        assert_eq!(captured_before, Some(before));
        state.publish_xdg_geometry_after_surface_commit(
            root_id,
            root_id,
            Some(before),
            Some(requested),
            1,
        );

        state.store_surface_placement(child_id, SurfacePlacement::subsurface(root_id, -20, -10));
        let child = state
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == child_id)
            .expect("synchronized child");
        child.width = 200;
        child.height = 100;

        assert_eq!(state.effective_xdg_window_geometry(root_id), Some(before));
        state.finish_surface_tree_publication();
        assert_eq!(
            state
                .effective_xdg_window_geometry(root_id)
                .map(|value| value.geometry),
            Some(XdgWindowGeometry::new(150, 0, 30, 90))
        );
    }

    #[test]
    fn topology_geometry_publication_does_not_qualify_pending_normal_restore() {
        let root_id = 60;
        let child_id = 61;
        let mut state = CompositorState::new(None);
        let window_id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, root_id))
            .expect("XDG toplevel window");
        let display = wayland_server::Display::<CompositorState>::new().expect("test display");
        let mut display_handle = display.handle();
        let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test socket");
        let client = display_handle
            .insert_client(server_end, std::sync::Arc::new(()))
            .expect("test client");
        let surface =
            state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
        let xdg_surface = client
            .create_resource::<
                wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface,
                XdgSurfaceData,
                CompositorState,
            >(
                &display_handle,
                20,
                XdgSurfaceData {
                    surface: surface.clone(),
                    reservation: XdgAssociationReservation::Fresh,
                },
            )
            .expect("test XDG surface");
        let toplevel = client
            .create_resource::<
                wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
                XdgToplevelData,
                CompositorState,
            >(
                &display_handle,
                21,
                XdgToplevelData { surface },
            )
            .expect("test XDG toplevel");
        state.toplevel_surfaces.insert(
            root_id,
            ToplevelSurface {
                window_id,
                xdg_surface,
                toplevel,
                pending_constraints: None,
                wm_capabilities_sent: false,
            },
        );
        let frame = WindowGeometry::new(SurfacePlacement::absolute_root_at(72, 72), 420, 310);
        let mut root = test_renderable_surface(root_id, 400, 300);
        root.placement = frame.placement;
        state.append_renderable_surface(root);
        state.append_renderable_surface(test_renderable_surface(child_id, 200, 100));
        state.store_surface_placement(root_id, frame.placement);
        state.store_surface_placement(child_id, SurfacePlacement::subsurface(root_id, -20, -10));
        state.xdg_surface_lifecycles.entry(root_id).or_default();
        set_subsurface_parent(&mut state, child_id, root_id);
        state.install_toplevel_visual_geometry(root_id, frame);
        state.install_xdg_mode_transition_response_fence(root_id, frame, 77);
        state.pending_normal_restores.insert(
            root_id,
            PendingNormalRestore {
                root_surface_id: root_id,
                window_id,
                configure_serial: 77,
                restore_placement: frame.placement,
                policy: PendingNormalRestorePolicy::StoredPlacement,
                response_commit_sequence: None,
            },
        );

        let (captured_root, before) = state
            .capture_xdg_geometry_for_topology_mutation(child_id)
            .expect("subsurface belongs to the XDG root");
        assert_eq!(captured_root, root_id);
        assert_eq!(
            before.map(|geometry| geometry.geometry),
            Some(XdgWindowGeometry::new(-20, -10, 420, 310))
        );

        state.retain_renderable_surfaces(|surface| surface.surface_id != child_id);
        state.deactivate_role_instance(child_id);
        state.set_surface_placement(child_id, SurfacePlacement::root());
        assert!(state.publish_xdg_geometry_after_topology_mutation(root_id, before));

        assert_eq!(
            state.pending_normal_restores[&root_id].response_commit_sequence, None,
            "topology mutation is not restore-response commit evidence"
        );
        assert_eq!(
            state.toplevel_visual_geometries[&root_id]
                .xdg_mode_transition_fence
                .and_then(|fence| fence.ack_commit_sequence_floor),
            None,
            "topology mutation does not advance the ACK commit floor"
        );
        assert!(state.pending_normal_restores.contains_key(&root_id));
        drop(client);
        drop(display);
    }
}
