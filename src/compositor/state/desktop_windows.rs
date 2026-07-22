use std::num::NonZeroU64;

use super::*;

impl CompositorState {
    pub(in crate::compositor) fn toplevel_window_id(&self, surface_id: u32) -> Option<WindowId> {
        self.toplevel_surfaces
            .get(&surface_id)
            .map(|toplevel| toplevel.window_id)
    }

    pub(in crate::compositor) fn toplevel_window_state(
        &self,
        surface_id: u32,
    ) -> Option<&WindowState> {
        let window_id = self.toplevel_window_id(surface_id)?;
        self.window(window_id).map(|window| &window.state)
    }

    pub(in crate::compositor) fn toplevel_window_state_mut(
        &mut self,
        surface_id: u32,
    ) -> Option<&mut WindowState> {
        let window_id = self.toplevel_window_id(surface_id)?;
        self.window_mut(window_id).map(|window| &mut window.state)
    }

    pub(in crate::compositor) fn toplevel_window_constraints(
        &self,
        surface_id: u32,
    ) -> WindowConstraints {
        self.toplevel_window_id(surface_id)
            .or_else(|| self.window_id_for_surface(surface_id))
            .and_then(|window_id| self.window(window_id))
            .map(|window| window.constraints)
            .unwrap_or_default()
    }

    pub(in crate::compositor) fn allocate_window_id(&mut self) -> io::Result<WindowId> {
        let value = NonZeroU64::new(self.next_window_id)
            .ok_or_else(|| io::Error::other(DesktopWindowError::WindowIdExhausted))?;
        self.next_window_id = self.next_window_id.checked_add(1).unwrap_or(0);
        Ok(WindowId::new(value))
    }

    pub(in crate::compositor) fn insert_desktop_window(
        &mut self,
        mut window: DesktopWindow,
    ) -> Result<(), DesktopWindowError> {
        if self.desktop_windows.contains_key(&window.id) {
            return Err(DesktopWindowError::DuplicateWindowId);
        }
        if let WindowBackend::X11(handle) = window.backend
            && self.window_by_x11_handle.contains_key(&handle)
        {
            return Err(DesktopWindowError::DuplicateWindowId);
        }
        if self
            .window_by_root_surface
            .contains_key(&window.root_surface_id)
        {
            return Err(DesktopWindowError::DuplicateRootSurface);
        }
        let initial_placement = match window.x11_placement_policy {
            Some(X11PlacementPolicy::CompositorManaged) => {
                Some(self.next_managed_x11_frame_placement())
            }
            Some(
                X11PlacementPolicy::ClientPositioned
                | X11PlacementPolicy::ParentRelative
                | X11PlacementPolicy::OverrideRedirect,
            ) => window.x11_geometry.map(|geometry| geometry.frame.placement),
            None => None,
        };
        if let Some(placement) = initial_placement {
            if let Some(geometry) = window.x11_geometry.as_mut() {
                if window.x11_placement_policy == Some(X11PlacementPolicy::CompositorManaged) {
                    geometry.client.x = placement.local_x;
                    geometry.client.y = placement.local_y;
                }
                geometry.frame.placement = placement;
            }
        }
        let root_surface_id = window.root_surface_id;
        self.window_by_root_surface
            .insert(root_surface_id, window.id);
        if let WindowBackend::X11(handle) = window.backend {
            self.window_by_x11_handle.insert(handle, window.id);
        }
        self.window_stacking.push(window.id);
        self.desktop_windows.insert(window.id, window);
        if let Some(placement) = initial_placement {
            self.set_surface_placement_with_cause(
                root_surface_id,
                placement,
                RenderGenerationCause::WindowMove,
            );
        }
        self.rebuild_x11_transient_relationships();
        self.normalize_window_stacking();
        Ok(())
    }

    fn next_managed_x11_frame_placement(&self) -> SurfacePlacement {
        self.next_managed_x11_frame_placement_excluding(None)
    }

    fn next_managed_x11_frame_placement_excluding(
        &self,
        excluded: Option<WindowId>,
    ) -> SurfacePlacement {
        let ordinal = self
            .desktop_windows
            .values()
            .filter(|window| {
                Some(window.id) != excluded
                    && window.x11_placement_policy == Some(X11PlacementPolicy::CompositorManaged)
            })
            .count();
        let (x, y) = crate::compositor::render::cascaded_root_position(ordinal);
        SurfacePlacement::absolute_root_at(x, y)
    }

    pub(in crate::compositor) fn remove_desktop_window(
        &mut self,
        id: WindowId,
    ) -> Option<DesktopWindow> {
        let window = self.desktop_windows.remove(&id)?;
        if self.window_by_root_surface.get(&window.root_surface_id) == Some(&id) {
            self.window_by_root_surface.remove(&window.root_surface_id);
        }
        if let WindowBackend::X11(handle) = window.backend {
            self.window_by_x11_handle.remove(&handle);
        }
        self.window_stacking.retain(|window_id| *window_id != id);
        self.rebuild_x11_transient_relationships();
        Some(window)
    }

    pub(in crate::compositor) fn detach_x11_surface(&mut self, surface_id: u32) -> bool {
        let Some(window_id) = self.window_by_root_surface.get(&surface_id).copied() else {
            return false;
        };
        let should_detach = self.window(window_id).is_some_and(|window| {
            matches!(window.backend, WindowBackend::X11(_))
                && window.x11_surface_id == Some(surface_id)
        });
        if !should_detach {
            return false;
        }
        self.window_by_root_surface.remove(&surface_id);
        if let Some(window) = self.window_mut(window_id) {
            window.x11_surface_id = None;
        }
        true
    }

    pub(in crate::compositor) fn attach_x11_surface(
        &mut self,
        handle: X11WindowHandle,
        surface_id: u32,
    ) -> Result<Option<u32>, X11SurfaceAttachmentError> {
        let window_id = self
            .window_by_x11_handle
            .get(&handle)
            .copied()
            .ok_or(X11SurfaceAttachmentError::UnknownWindow)?;
        if let Some(owner) = self.window_by_root_surface.get(&surface_id)
            && *owner != window_id
        {
            return Err(X11SurfaceAttachmentError::DuplicateSurface);
        }
        let old_surface_id = self
            .window(window_id)
            .and_then(|window| window.x11_surface_id);
        let replacement_placement = self
            .window(window_id)
            .and_then(|window| window.x11_geometry)
            .map(|geometry| geometry.frame.placement);
        if old_surface_id == Some(surface_id) {
            return Ok(None);
        }
        if let Some(old_surface_id) = old_surface_id
            && self.window_by_root_surface.get(&old_surface_id) == Some(&window_id)
        {
            self.window_by_root_surface.remove(&old_surface_id);
        }
        self.window_by_root_surface.insert(surface_id, window_id);
        let window = self
            .window_mut(window_id)
            .ok_or(X11SurfaceAttachmentError::UnknownWindow)?;
        window.root_surface_id = surface_id;
        window.x11_surface_id = Some(surface_id);
        if let Some(placement) = replacement_placement {
            self.set_surface_placement_with_cause(
                surface_id,
                placement,
                RenderGenerationCause::WindowMove,
            );
        }
        Ok(old_surface_id)
    }

    pub(in crate::compositor) fn can_attach_x11_surface(
        &self,
        handle: X11WindowHandle,
        surface_id: u32,
    ) -> bool {
        let Some(window_id) = self.window_by_x11_handle.get(&handle).copied() else {
            return false;
        };
        if self
            .window_by_root_surface
            .get(&surface_id)
            .is_some_and(|owner| *owner != window_id)
        {
            return false;
        }
        self.surface_role(surface_id) == SurfaceRole::Xwayland
            && self
                .xwayland
                .surface_states
                .get(&surface_id)
                .is_some_and(|state| {
                    state.generation == handle.generation()
                        && state.committed_serial.is_some()
                        && state.association_object_alive
                })
            && self.surface_resource_by_id(surface_id).is_some()
    }

    pub(in crate::compositor) fn insert_x11_window(
        &mut self,
        snapshot: crate::xwayland::xwm::X11WindowSnapshot,
    ) -> Result<WindowId, X11WindowAdmissionError> {
        let generation = snapshot.handle.generation();
        if self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != generation)
        {
            return Err(X11WindowAdmissionError::StaleGeneration);
        }
        if !matches!(
            self.surface_role(snapshot.surface_id),
            SurfaceRole::Xwayland
        ) {
            return Err(X11WindowAdmissionError::SurfaceNotXwayland);
        }
        if self
            .xwayland
            .surface_states
            .get(&snapshot.surface_id)
            .is_none_or(|state| state.generation != generation || state.committed_serial.is_none())
        {
            return Err(X11WindowAdmissionError::SurfaceNotAssociated);
        }
        if self.window_by_x11_handle.contains_key(&snapshot.handle) {
            return Err(X11WindowAdmissionError::DuplicateX11Window);
        }
        if self
            .window_by_root_surface
            .contains_key(&snapshot.surface_id)
        {
            return Err(X11WindowAdmissionError::DuplicateRootSurface);
        }
        let window_id = self
            .allocate_window_id()
            .map_err(|_| X11WindowAdmissionError::WindowIdExhausted)?;
        let handle = snapshot.handle;
        let geometry = snapshot.geometry;
        let requested_state = snapshot.state;
        self.insert_desktop_window(DesktopWindow::new_x11(window_id, snapshot))
            .map_err(|_| X11WindowAdmissionError::DuplicateRootSurface)?;
        let initial_geometry = self
            .window(window_id)
            .and_then(|window| window.x11_geometry)
            .map(|state| crate::xwayland::xwm::X11Geometry {
                x: state.frame.placement.local_x,
                y: state.frame.placement.local_y,
                ..geometry
            })
            .unwrap_or(geometry);
        let _ = self.set_x11_geometry(handle, initial_geometry);
        self.rebuild_x11_transient_relationships();
        self.apply_initial_x11_state(handle, requested_state, geometry);
        Ok(window_id)
    }

    pub(in crate::compositor) fn apply_initial_x11_state(
        &mut self,
        handle: X11WindowHandle,
        state: crate::xwayland::xwm::X11PublishedState,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let root_surface_id = self.window(window_id).map(|window| window.root_surface_id);
        let Some(root_surface_id) = root_surface_id else {
            return false;
        };
        let placement_policy = self
            .window(window_id)
            .and_then(|window| window.x11_placement_policy);
        let current_placement = self.surface_placement(root_surface_id);
        let floating_placement = match placement_policy {
            Some(X11PlacementPolicy::CompositorManaged) => current_placement,
            Some(
                X11PlacementPolicy::ClientPositioned
                | X11PlacementPolicy::ParentRelative
                | X11PlacementPolicy::OverrideRedirect,
            )
            | None => SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
        };
        let restore_geometry = WindowGeometry::new(
            floating_placement,
            geometry.width.max(1),
            geometry.height.max(1),
        );
        let mode = if state.fullscreen {
            ToplevelMode::Fullscreen
        } else if state.maximized {
            ToplevelMode::Maximized
        } else {
            ToplevelMode::Floating
        };
        if let Some(window) = self.window_mut(window_id) {
            if mode != ToplevelMode::Floating {
                window.state.capture_restore_geometry(restore_geometry);
            }
            window.state.set_mode(mode);
        }
        let target_geometry = self.window_geometry_for_mode(mode);
        self.set_surface_placement_with_cause(
            root_surface_id,
            if mode == ToplevelMode::Floating {
                floating_placement
            } else {
                target_geometry.placement
            },
            RenderGenerationCause::WindowMode,
        );
        if let Some(window) = self.window_mut(window_id)
            && let Some(x11_geometry) = window.x11_geometry.as_mut()
        {
            x11_geometry.frame = if mode == ToplevelMode::Floating {
                restore_geometry
            } else {
                target_geometry
            };
        }
        if state.hidden {
            if !self.minimize_desktop_window(window_id)
                && let Some(window) = self.window_mut(window_id)
            {
                window.state.mark_minimized_without_surfaces();
            }
        } else if self
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized())
        {
            self.restore_minimized_desktop_window(window_id);
        }
        if mode == ToplevelMode::Fullscreen && !state.hidden {
            self.set_fullscreen_presentation_owner(root_surface_id);
        } else {
            self.clear_fullscreen_presentation_owner(root_surface_id);
        }
        if mode != ToplevelMode::Floating {
            self.queue_backend_configure(window_id, target_geometry, mode, false);
        }
        self.queue_backend_state(window_id);
        true
    }

    pub(in crate::compositor) fn window_id_for_x11_handle(
        &self,
        handle: X11WindowHandle,
    ) -> Option<WindowId> {
        self.window_by_x11_handle.get(&handle).copied()
    }

    pub(in crate::compositor) fn apply_x11_metadata_delta(
        &mut self,
        handle: X11WindowHandle,
        delta: crate::xwayland::xwm::X11MetadataDelta,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let transient_handle = match &delta {
            crate::xwayland::xwm::X11MetadataDelta::TransientFor(parent) => *parent,
            _ => None,
        };
        let accepted_transient_handle = transient_handle
            .filter(|parent| !self.x11_transient_would_cycle(window_id, Some(*parent)));
        let transient_parent_id =
            accepted_transient_handle.and_then(|parent| self.window_id_for_x11_handle(parent));
        let old_policy = self
            .window(window_id)
            .and_then(|window| window.x11_placement_policy);
        let Some(window) = self.window_mut(window_id) else {
            return false;
        };
        match delta {
            crate::xwayland::xwm::X11MetadataDelta::Title(title) => {
                window.metadata.title = title;
            }
            crate::xwayland::xwm::X11MetadataDelta::AppId(app_id) => {
                window.metadata.app_id = app_id;
            }
            crate::xwayland::xwm::X11MetadataDelta::Pid(pid) => {
                window.metadata.pid = pid;
            }
            crate::xwayland::xwm::X11MetadataDelta::Constraints(constraints) => {
                window.constraints = constraints;
            }
            crate::xwayland::xwm::X11MetadataDelta::TransientFor(_) => {
                window.relationships.transient_for = transient_parent_id;
                window.x11_transient_for = accepted_transient_handle;
                window.x11_role = Some(crate::compositor::desktop_window::classify_x11_role(
                    window.kind,
                    &window.x11_window_types,
                    accepted_transient_handle.is_some(),
                    window.kind == DesktopWindowKind::OverrideRedirect,
                ));
                window.x11_placement_policy = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_placement_policy);
                window.stack_layer = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_stack_layer)
                    .unwrap_or(DesktopStackLayer::Normal);
            }
            crate::xwayland::xwm::X11MetadataDelta::WindowTypes(window_types) => {
                window.x11_window_types = window_types;
                window.x11_role = Some(crate::compositor::desktop_window::classify_x11_role(
                    window.kind,
                    &window.x11_window_types,
                    window.x11_transient_for.is_some(),
                    window.kind == DesktopWindowKind::OverrideRedirect,
                ));
                window.x11_placement_policy = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_placement_policy);
                window.stack_layer = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_stack_layer)
                    .unwrap_or(DesktopStackLayer::Normal);
            }
            crate::xwayland::xwm::X11MetadataDelta::Kind(kind) => {
                window.kind = kind;
                window.x11_role = Some(crate::compositor::desktop_window::classify_x11_role(
                    window.kind,
                    &window.x11_window_types,
                    window.x11_transient_for.is_some(),
                    window.kind == DesktopWindowKind::OverrideRedirect,
                ));
                window.x11_placement_policy = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_placement_policy);
                window.stack_layer = window
                    .x11_role
                    .map(crate::compositor::desktop_window::x11_stack_layer)
                    .unwrap_or(DesktopStackLayer::Normal);
            }
            crate::xwayland::xwm::X11MetadataDelta::AcceptsInput(accepts_input) => {
                window.x11_accepts_input = accepts_input;
            }
            crate::xwayland::xwm::X11MetadataDelta::Protocols { .. } => {}
        }
        self.rebuild_x11_transient_relationships();
        let new_policy = self
            .window(window_id)
            .and_then(|window| window.x11_placement_policy);
        if old_policy != new_policy {
            self.migrate_x11_placement_policy(window_id, old_policy, new_policy);
        }
        true
    }

    fn migrate_x11_placement_policy(
        &mut self,
        window_id: WindowId,
        old_policy: Option<X11PlacementPolicy>,
        new_policy: Option<X11PlacementPolicy>,
    ) {
        let Some((surface_id, client, current_frame)) = self.window(window_id).map(|window| {
            (
                window.root_surface_id,
                window.x11_geometry.map(|geometry| geometry.client),
                window.x11_geometry.map(|geometry| geometry.frame),
            )
        }) else {
            return;
        };
        let Some(client) = client else {
            return;
        };
        let placement = match new_policy {
            Some(X11PlacementPolicy::CompositorManaged) => {
                if old_policy == Some(X11PlacementPolicy::CompositorManaged) {
                    current_frame
                        .map(|geometry| geometry.placement)
                        .unwrap_or_else(|| {
                            self.next_managed_x11_frame_placement_excluding(Some(window_id))
                        })
                } else {
                    self.next_managed_x11_frame_placement_excluding(Some(window_id))
                }
            }
            Some(
                X11PlacementPolicy::ClientPositioned
                | X11PlacementPolicy::ParentRelative
                | X11PlacementPolicy::OverrideRedirect,
            )
            | None => SurfacePlacement::absolute_root_at(client.x, client.y),
        };
        if let Some(window) = self.window_mut(window_id)
            && let Some(geometry) = window.x11_geometry.as_mut()
        {
            if new_policy == Some(X11PlacementPolicy::CompositorManaged) {
                geometry.client.x = placement.local_x;
                geometry.client.y = placement.local_y;
            }
            geometry.frame =
                WindowGeometry::new(placement, client.width.max(1), client.height.max(1));
        }
        self.set_surface_placement_with_cause(
            surface_id,
            placement,
            RenderGenerationCause::WindowMove,
        );
    }

    fn x11_transient_would_cycle(
        &self,
        child_id: WindowId,
        parent_handle: Option<crate::xwayland::X11WindowHandle>,
    ) -> bool {
        let Some(mut current) =
            parent_handle.and_then(|handle| self.window_id_for_x11_handle(handle))
        else {
            return false;
        };
        let mut seen = std::collections::HashSet::new();
        while seen.insert(current) {
            if current == child_id {
                return true;
            }
            let Some(parent) = self
                .window(current)
                .and_then(|window| window.relationships.transient_for)
            else {
                return false;
            };
            current = parent;
        }
        true
    }

    fn rebuild_x11_transient_relationships(&mut self) {
        let mut handles_by_id = std::collections::HashMap::new();
        for window in self.desktop_windows.values() {
            let WindowBackend::X11(handle) = window.backend else {
                continue;
            };
            handles_by_id.insert(handle, window.id);
        }
        let requested = self
            .desktop_windows
            .values()
            .filter_map(|window| {
                matches!(window.backend, WindowBackend::X11(_)).then_some((
                    window.id,
                    window
                        .x11_transient_for
                        .and_then(|parent| handles_by_id.get(&parent).copied()),
                ))
            })
            .collect::<std::collections::HashMap<_, _>>();

        // Resolve parent handles against the complete admission set, then
        // reject any edge that would leave a cycle in the relationship graph.
        let mut accepted = requested
            .iter()
            .map(|(id, parent)| (*id, *parent))
            .collect::<std::collections::HashMap<_, _>>();
        let mut rejected = std::collections::HashSet::new();
        for id in requested.keys().copied().collect::<Vec<_>>() {
            if relationship_cycle(id, &accepted) {
                accepted.insert(id, None);
                rejected.insert(id);
            }
        }
        for (id, parent) in accepted {
            if let Some(window) = self.window_mut(id) {
                window.relationships.transient_for = parent;
                if rejected.contains(&id)
                    && window
                        .x11_transient_for
                        .and_then(|handle| handles_by_id.get(&handle).copied())
                        .is_some()
                {
                    window.x11_transient_for = None;
                }
            }
        }

        let roots = self
            .desktop_windows
            .values()
            .filter_map(|window| {
                matches!(window.backend, WindowBackend::X11(_))
                    .then_some(window.id)
                    .filter(|id| {
                        self.window(*id)
                            .and_then(|candidate| candidate.relationships.transient_for)
                            .is_none()
                    })
            })
            .collect::<Vec<_>>();
        for root in roots {
            self.reorder_x11_family(root);
        }
        self.normalize_window_stacking();
    }

    fn normalize_window_stacking(&mut self) {
        let layers = self
            .desktop_windows
            .values()
            .map(|window| (window.id, window.stack_layer))
            .collect::<std::collections::HashMap<_, _>>();
        self.window_stacking.sort_by_key(|window_id| {
            layers
                .get(window_id)
                .copied()
                .unwrap_or(DesktopStackLayer::Normal)
        });
        self.reorder_renderable_surfaces_by_window_stack();
    }

    pub(in crate::compositor) fn filter_x11_geometry(
        &self,
        handle: X11WindowHandle,
        requested: crate::xwayland::xwm::X11Geometry,
    ) -> Option<crate::xwayland::xwm::X11Geometry> {
        let window_id = self.window_id_for_x11_handle(handle)?;
        let window = self.window(window_id)?;
        let width = requested
            .width
            .max(1)
            .max(window.constraints.min_width.unwrap_or(1))
            .min(window.constraints.max_width.unwrap_or(u32::MAX));
        let height = requested
            .height
            .max(1)
            .max(window.constraints.min_height.unwrap_or(1))
            .min(window.constraints.max_height.unwrap_or(u32::MAX));
        Some(crate::xwayland::xwm::X11Geometry {
            width,
            height,
            ..requested
        })
    }

    pub(in crate::compositor) fn set_x11_geometry(
        &mut self,
        handle: X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some(filtered) = self.filter_x11_geometry(handle, geometry) else {
            return false;
        };
        let Some((root_surface_id, placement_policy)) = self
            .window(window_id)
            .map(|window| (window.root_surface_id, window.x11_placement_policy))
        else {
            return false;
        };
        let placement = match placement_policy {
            Some(X11PlacementPolicy::CompositorManaged) => {
                SurfacePlacement::absolute_root_at(filtered.x, filtered.y)
            }
            Some(
                X11PlacementPolicy::ClientPositioned
                | X11PlacementPolicy::ParentRelative
                | X11PlacementPolicy::OverrideRedirect,
            )
            | None => SurfacePlacement::absolute_root_at(filtered.x, filtered.y),
        };
        if let Some(window) = self.window_mut(window_id)
            && let Some(x11_geometry) = window.x11_geometry.as_mut()
        {
            x11_geometry.client = if placement_policy == Some(X11PlacementPolicy::CompositorManaged)
            {
                crate::xwayland::xwm::X11Geometry {
                    x: placement.local_x,
                    y: placement.local_y,
                    ..filtered
                }
            } else {
                filtered
            };
            x11_geometry.frame =
                WindowGeometry::new(placement, filtered.width.max(1), filtered.height.max(1));
        }
        self.set_surface_placement_with_cause(
            root_surface_id,
            placement,
            RenderGenerationCause::WindowMove,
        );
        true
    }

    pub(in crate::compositor) fn reconcile_x11_configure_notify(
        &mut self,
        handle: X11WindowHandle,
        geometry: crate::xwayland::xwm::X11Geometry,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let Some((root_surface_id, placement_policy, persisted_placement)) =
            self.window(window_id).map(|window| {
                (
                    window.root_surface_id,
                    window.x11_placement_policy,
                    window.x11_geometry.map(|geometry| geometry.frame.placement),
                )
            })
        else {
            return false;
        };
        let placement = match placement_policy {
            Some(X11PlacementPolicy::CompositorManaged) => {
                persisted_placement.unwrap_or_else(|| self.next_managed_x11_frame_placement())
            }
            Some(
                X11PlacementPolicy::ClientPositioned
                | X11PlacementPolicy::ParentRelative
                | X11PlacementPolicy::OverrideRedirect,
            )
            | None => SurfacePlacement::absolute_root_at(geometry.x, geometry.y),
        };
        let frame = WindowGeometry::new(placement, geometry.width.max(1), geometry.height.max(1));
        if let Some(window) = self.window_mut(window_id)
            && let Some(x11_geometry) = window.x11_geometry.as_mut()
        {
            x11_geometry.client = if placement_policy == Some(X11PlacementPolicy::CompositorManaged)
            {
                crate::xwayland::xwm::X11Geometry {
                    x: placement.local_x,
                    y: placement.local_y,
                    ..geometry
                }
            } else {
                geometry
            };
            x11_geometry.frame = frame;
        }
        if self.active_toplevel_resizes.contains_key(&root_surface_id) {
            return false;
        }
        let changed = self.surface_placement(root_surface_id) != placement
            || self
                .current_visual_root_window_geometry(root_surface_id)
                .is_some_and(|current| {
                    current.width != geometry.width || current.height != geometry.height
                });
        self.set_surface_placement_with_cause(
            root_surface_id,
            placement,
            RenderGenerationCause::WindowMove,
        );
        if let Some(visual) = self.toplevel_visual_geometries.get_mut(&root_surface_id) {
            visual.placement = placement;
            visual.width = geometry.width;
            visual.height = geometry.height;
            self.update_toplevel_visual_render_assignment(root_surface_id);
        }
        let child_surfaces = self
            .renderable_surfaces
            .iter()
            .filter_map(|surface| {
                (self.root_surface_id_for_surface(surface.surface_id) == root_surface_id)
                    .then_some(surface.surface_id)
            })
            .collect::<Vec<_>>();
        for surface_id in child_surfaces {
            if let Some(surface) = self
                .renderable_surfaces
                .iter_mut()
                .find(|surface| surface.surface_id == surface_id)
            {
                surface.placement = placement;
            }
        }
        if changed {
            self.advance_render_generation(RenderGenerationCause::WindowMove);
        }
        changed
    }

    pub(in crate::compositor) fn x11_authoritative_geometry(
        &self,
        handle: X11WindowHandle,
    ) -> Option<crate::xwayland::xwm::X11Geometry> {
        let window_id = self.window_id_for_x11_handle(handle)?;
        let window = self.window(window_id)?;
        window.x11_geometry.map(|geometry| geometry.client)
    }

    pub(in crate::compositor) fn apply_x11_published_state(
        &mut self,
        handle: X11WindowHandle,
        state: crate::xwayland::xwm::X11PublishedState,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let mode = if state.fullscreen {
            ToplevelMode::Fullscreen
        } else if state.maximized {
            ToplevelMode::Maximized
        } else {
            ToplevelMode::Floating
        };
        let Some(root_surface_id) = self.window(window_id).map(|window| window.root_surface_id)
        else {
            return false;
        };
        let minimized = self
            .window(window_id)
            .is_some_and(|window| window.state.is_minimized());
        if let Some(window) = self.window_mut(window_id) {
            window.state.set_mode(mode);
        }
        if state.hidden && !minimized {
            let _ = self.minimize_desktop_window(window_id);
        } else if !state.hidden && minimized {
            let _ = self.restore_minimized_desktop_window(window_id);
        }
        let placement = match mode {
            ToplevelMode::Fullscreen => self.fullscreen_window_geometry().placement,
            ToplevelMode::Maximized => self.maximized_window_geometry().placement,
            ToplevelMode::Floating => self.surface_placement(root_surface_id),
        };
        self.set_surface_placement_with_cause(
            root_surface_id,
            placement,
            RenderGenerationCause::WindowMode,
        );
        true
    }

    pub(in crate::compositor) fn apply_x11_state_request(
        &mut self,
        handle: X11WindowHandle,
        request: crate::xwayland::xwm::X11StateRequest,
    ) -> Option<crate::xwayland::xwm::X11PublishedState> {
        let window_id = self.window_id_for_x11_handle(handle)?;
        let window = self.window(window_id)?;
        let mut state = crate::xwayland::xwm::X11PublishedState {
            fullscreen: window.state.mode() == ToplevelMode::Fullscreen,
            maximized: window.state.mode() == ToplevelMode::Maximized,
            hidden: window.state.is_minimized(),
            activated: self.focused_window_id == Some(window_id),
        };
        let mut maximized_horizontal = state.maximized;
        let mut maximized_vertical = state.maximized;
        for atom in [request.first, request.second].into_iter().flatten() {
            let value = match atom {
                crate::xwayland::xwm::X11StateAtom::Fullscreen => &mut state.fullscreen,
                crate::xwayland::xwm::X11StateAtom::MaximizedHorizontal => {
                    &mut maximized_horizontal
                }
                crate::xwayland::xwm::X11StateAtom::MaximizedVertical => &mut maximized_vertical,
                crate::xwayland::xwm::X11StateAtom::Hidden => &mut state.hidden,
            };
            *value = crate::xwayland::xwm::ewmh::apply_state_action(*value, request.action);
        }
        state.maximized = crate::xwayland::xwm::ewmh::aggregate_maximize(
            maximized_horizontal,
            maximized_vertical,
        );
        self.apply_x11_published_state(handle, state);
        Some(state)
    }

    pub(in crate::compositor) fn raise_window_id(&mut self, id: WindowId) -> bool {
        if !self.desktop_windows.contains_key(&id) {
            return false;
        }
        let x11_window = self
            .window(id)
            .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)));
        let raised = self.x11_subtree_order(id);
        if raised.is_empty() {
            return false;
        }
        self.window_stacking
            .retain(|window_id| !raised.contains(window_id));
        self.window_stacking.extend(raised);
        self.normalize_window_stacking();
        if x11_window {
            self.backend_commands.push(
                crate::compositor::window_backend::WindowBackendCommand::Restack { window: id },
            );
        }
        true
    }

    pub(in crate::compositor) fn apply_x11_stack_request(
        &mut self,
        handle: X11WindowHandle,
        sibling: Option<X11WindowHandle>,
        mode: crate::xwayland::xwm::X11StackMode,
    ) -> bool {
        let Some(window_id) = self.window_id_for_x11_handle(handle) else {
            return false;
        };
        let sibling_id = sibling.and_then(|handle| self.window_id_for_x11_handle(handle));
        let original = self.window_stacking.clone();
        let raised = self.x11_subtree_order(window_id);
        let raised_set = raised
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        let Some(sibling_id) = sibling_id.filter(|id| !raised_set.contains(id)) else {
            if sibling.is_some() {
                return false;
            }
            if !matches!(
                mode,
                crate::xwayland::xwm::X11StackMode::Above
                    | crate::xwayland::xwm::X11StackMode::Below
            ) {
                return false;
            }
            self.window_stacking
                .retain(|candidate| !raised_set.contains(candidate));
            if matches!(mode, crate::xwayland::xwm::X11StackMode::Above) {
                self.window_stacking.extend(raised);
            } else {
                let mut reordered = raised;
                reordered.extend(std::mem::take(&mut self.window_stacking));
                self.window_stacking = reordered;
            }
            self.rebuild_x11_transient_relationships();
            return self.window_stacking != original;
        };
        let original_window_index = original
            .iter()
            .position(|candidate| *candidate == window_id)
            .unwrap_or(0);
        let sibling_index = original
            .iter()
            .position(|candidate| *candidate == sibling_id)
            .unwrap_or(0);
        let effective_mode = match mode {
            crate::xwayland::xwm::X11StackMode::TopIf => (original_window_index < sibling_index)
                .then_some(crate::xwayland::xwm::X11StackMode::Above),
            crate::xwayland::xwm::X11StackMode::BottomIf => (original_window_index > sibling_index)
                .then_some(crate::xwayland::xwm::X11StackMode::Below),
            crate::xwayland::xwm::X11StackMode::Opposite => {
                Some(if original_window_index < sibling_index {
                    crate::xwayland::xwm::X11StackMode::Above
                } else {
                    crate::xwayland::xwm::X11StackMode::Below
                })
            }
            mode => Some(mode),
        };
        let Some(effective_mode) = effective_mode else {
            return false;
        };
        self.window_stacking
            .retain(|candidate| !raised_set.contains(candidate));
        let sibling_index = self
            .window_stacking
            .iter()
            .position(|candidate| *candidate == sibling_id)
            .unwrap_or(self.window_stacking.len());
        let insertion = matches!(effective_mode, crate::xwayland::xwm::X11StackMode::Above)
            .then_some(sibling_index.saturating_add(1))
            .unwrap_or(sibling_index);
        self.window_stacking.splice(insertion..insertion, raised);
        self.rebuild_x11_transient_relationships();
        self.window_stacking != original
    }

    fn reorder_x11_family(&mut self, root: WindowId) -> bool {
        let family = self.x11_subtree_order(root);
        if family.len() < 2 {
            return false;
        }
        let original_stack = self.window_stacking.clone();
        let mut ordered = family;
        ordered.sort_by_key(|id| {
            let mut depth = 0usize;
            let mut current = *id;
            let mut seen = std::collections::HashSet::new();
            while current != root && seen.insert(current) {
                let Some(parent) = self
                    .window(current)
                    .and_then(|window| window.relationships.transient_for)
                else {
                    break;
                };
                current = parent;
                depth += 1;
            }
            (
                depth,
                self.window_stacking
                    .iter()
                    .position(|candidate| candidate == id),
            )
        });
        for child in ordered.into_iter().filter(|id| *id != root) {
            let Some(parent) = self
                .window(child)
                .and_then(|window| window.relationships.transient_for)
            else {
                continue;
            };
            let Some(parent_index) = self.window_stacking.iter().position(|id| *id == parent)
            else {
                continue;
            };
            let Some(child_index) = self.window_stacking.iter().position(|id| *id == child) else {
                continue;
            };
            if child_index < parent_index {
                let child = self.window_stacking.remove(child_index);
                let parent_index = self
                    .window_stacking
                    .iter()
                    .position(|id| *id == parent)
                    .unwrap_or(self.window_stacking.len());
                self.window_stacking.insert(parent_index + 1, child);
            }
        }
        let stack_changed = self.window_stacking != original_stack;
        if stack_changed {
            self.reorder_renderable_surfaces_by_window_stack();
        }
        stack_changed
    }

    pub(in crate::compositor) fn raise_x11_family_for_surface(&mut self, surface_id: u32) -> bool {
        let Some(id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        self.raise_window_id(id)
    }

    fn x11_subtree_order(&self, id: WindowId) -> Vec<WindowId> {
        let members = self
            .desktop_windows
            .values()
            .filter(|window| {
                let mut current = window.id;
                let mut seen = std::collections::HashSet::new();
                while seen.insert(current) {
                    if current == id {
                        return true;
                    }
                    let Some(parent) = self
                        .window(current)
                        .and_then(|candidate| candidate.relationships.transient_for)
                    else {
                        break;
                    };
                    current = parent;
                }
                false
            })
            .map(|window| window.id)
            .collect::<std::collections::HashSet<_>>();
        self.window_stacking
            .iter()
            .copied()
            .filter(|candidate| members.contains(candidate))
            .collect()
    }

    pub(in crate::compositor) fn x11_client_lists(
        &self,
    ) -> (
        Vec<crate::xwayland::X11WindowHandle>,
        Vec<crate::xwayland::X11WindowHandle>,
    ) {
        let mut client_list = self
            .desktop_windows
            .values()
            .filter(|window| {
                window.kind == DesktopWindowKind::Managed && window.is_normal_x11_role()
            })
            .filter_map(|window| match window.backend {
                WindowBackend::X11(handle) => Some((window.id, handle)),
                WindowBackend::Xdg(_) => None,
            })
            .collect::<Vec<_>>();
        client_list.sort_by_key(|(id, _)| *id);
        let client_list = client_list
            .iter()
            .map(|(_, handle)| *handle)
            .collect::<Vec<_>>();
        let stacking = self
            .window_stacking
            .iter()
            .filter_map(|id| self.window(*id))
            .filter(|window| {
                window.kind == DesktopWindowKind::Managed && window.is_normal_x11_role()
            })
            .filter_map(|window| match window.backend {
                WindowBackend::X11(handle) => Some(handle),
                WindowBackend::Xdg(_) => None,
            })
            .collect::<Vec<_>>();
        (client_list, stacking)
    }

    pub(in crate::compositor) fn queue_backend_configure(
        &mut self,
        window_id: WindowId,
        geometry: WindowGeometry,
        mode: ToplevelMode,
        resizing: bool,
    ) {
        self.backend_commands.push(
            crate::compositor::window_backend::WindowBackendCommand::Configure {
                window: window_id,
                geometry,
                mode,
                resizing,
            },
        );
    }

    pub(in crate::compositor) fn queue_backend_finalize_resize(
        &mut self,
        window_id: WindowId,
        geometry: WindowGeometry,
        mode: ToplevelMode,
    ) {
        self.backend_commands.push(
            crate::compositor::window_backend::WindowBackendCommand::FinalizeResize {
                window: window_id,
                geometry,
                mode,
            },
        );
    }

    pub(in crate::compositor) fn queue_backend_activation(
        &mut self,
        window_id: WindowId,
        activated: bool,
    ) {
        if self
            .window(window_id)
            .is_some_and(|window| matches!(window.backend, WindowBackend::X11(_)))
        {
            self.backend_commands.push(
                crate::compositor::window_backend::WindowBackendCommand::SetActivated {
                    window: window_id,
                    activated,
                },
            );
        }
    }

    pub(in crate::compositor) fn update_desktop_focus_window(
        &mut self,
        new_surface_id: u32,
        changed: bool,
    ) -> Option<WindowId> {
        let old_window_id = self.focused_window_id;
        let new_window_id =
            self.window_id_for_surface(self.root_surface_id_for_surface(new_surface_id));
        if changed || old_window_id != new_window_id {
            if let Some(window_id) = old_window_id {
                self.queue_backend_activation(window_id, false);
            }
            if let Some(window_id) = new_window_id {
                self.queue_backend_activation(window_id, true);
            }
        }
        new_window_id
    }

    pub(in crate::compositor) fn queue_backend_state(&mut self, window_id: WindowId) {
        let Some(window) = self.window(window_id) else {
            return;
        };
        if matches!(window.backend, WindowBackend::X11(_)) {
            self.backend_commands.push(
                crate::compositor::window_backend::WindowBackendCommand::PublishState {
                    window: window_id,
                    mode: window.state.mode(),
                    minimized: window.state.is_minimized(),
                    activated: self.focused_window_id == Some(window_id),
                },
            );
        }
    }

    pub(in crate::compositor) fn take_backend_commands(
        &mut self,
    ) -> Vec<crate::compositor::window_backend::WindowBackendCommand> {
        std::mem::take(&mut self.backend_commands)
    }

    pub(in crate::compositor) fn window_id_for_surface(&self, surface_id: u32) -> Option<WindowId> {
        self.window_by_root_surface.get(&surface_id).copied()
    }

    pub(in crate::compositor) fn window(&self, id: WindowId) -> Option<&DesktopWindow> {
        self.desktop_windows.get(&id)
    }

    pub(in crate::compositor) fn window_mut(&mut self, id: WindowId) -> Option<&mut DesktopWindow> {
        self.desktop_windows.get_mut(&id)
    }
}

fn relationship_cycle(
    child: WindowId,
    parents: &std::collections::HashMap<WindowId, Option<WindowId>>,
) -> bool {
    let mut current = child;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(current) {
        let Some(Some(parent)) = parents.get(&current) else {
            return false;
        };
        current = *parent;
        if current == child {
            return true;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum X11SurfaceAttachmentError {
    UnknownWindow,
    DuplicateSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum X11WindowAdmissionError {
    StaleGeneration,
    SurfaceNotXwayland,
    SurfaceNotAssociated,
    DuplicateX11Window,
    DuplicateRootSurface,
    WindowIdExhausted,
}
