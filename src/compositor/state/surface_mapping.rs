use super::*;
use crate::compositor::subsurface::ContentUpdateRef;
use wayland_protocols::wp::viewporter::server::wp_viewport;

pub(super) type SurfaceMappingProjection =
    Result<Option<SurfaceContentMapping>, (SurfaceMappingError, Option<wp_viewport::WpViewport>)>;

#[derive(Debug, Clone, Copy)]
enum EffectiveContentState {
    Retained(BufferSize),
    Attached(BufferSize),
    Empty,
}

impl EffectiveContentState {
    fn buffer_size(self) -> Option<BufferSize> {
        match self {
            Self::Retained(size) | Self::Attached(size) => Some(size),
            Self::Empty => None,
        }
    }
}

#[derive(Debug, Clone)]
struct EffectiveSurfaceMappingState {
    viewport: SurfaceViewportCommit,
    buffer_scale: u32,
    buffer_transform: wl_output::Transform,
    content: EffectiveContentState,
    viewport_error_owner: Option<wp_viewport::WpViewport>,
}

impl EffectiveSurfaceMappingState {
    fn from_surface(
        data: &SurfaceData,
        current: Option<&CurrentSurfaceBuffer>,
    ) -> Result<Self, SurfaceMappingError> {
        Ok(Self {
            viewport: data.viewport_for_change(PendingViewportChange::default()),
            buffer_scale: data.buffer_scale_for_change(None),
            buffer_transform: data.buffer_transform_for_change(None),
            content: current
                .map(CurrentSurfaceBuffer::buffer_size)
                .transpose()?
                .map_or(
                    EffectiveContentState::Empty,
                    EffectiveContentState::Retained,
                ),
            viewport_error_owner: data.committed_viewport_error_owner(),
        })
    }

    fn apply_commit(
        &mut self,
        commit: &CachedSubsurfaceCommit,
    ) -> Result<Option<SurfaceContentMapping>, (SurfaceMappingError, Option<wp_viewport::WpViewport>)>
    {
        let viewport = self.viewport.apply_change(commit.viewport_destination);
        let buffer_scale = commit.buffer_scale.unwrap_or(self.buffer_scale);
        let buffer_transform = commit.buffer_transform.unwrap_or(self.buffer_transform);
        let viewport_error_owner = if commit.viewport_destination.source.is_some() {
            commit.viewport_error_owner.clone()
        } else {
            self.viewport_error_owner.clone()
        };
        let result = match commit.attachment.as_ref() {
            Some(PendingSurfaceAttachment::Buffer(pending)) => {
                let buffer_size = pending
                    .buffer_size()
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                let mapping = pending
                    .content_mapping_for_state(viewport, buffer_scale, buffer_transform)
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                (EffectiveContentState::Attached(buffer_size), Some(mapping))
            }
            Some(PendingSurfaceAttachment::RemoveContent) => {
                viewport
                    .validate_viewport_state_without_buffer()
                    .map_err(|error| (error, viewport_error_owner.clone()))?;
                (EffectiveContentState::Empty, None)
            }
            None => {
                if let Some(buffer_size) = self.content.buffer_size() {
                    viewport
                        .surface_size_for_buffer_size(buffer_size, buffer_scale, buffer_transform)
                        .map_err(|error| (error, viewport_error_owner.clone()))?;
                } else {
                    viewport
                        .validate_viewport_state_without_buffer()
                        .map_err(|error| (error, viewport_error_owner.clone()))?;
                }
                (self.content, None)
            }
        };
        self.viewport = viewport;
        self.buffer_scale = buffer_scale;
        self.buffer_transform = buffer_transform;
        self.content = result.0;
        if commit.viewport_destination.source.is_some() {
            self.viewport_error_owner = commit.viewport_error_owner.clone();
        }
        Ok(result.1)
    }
}

impl CompositorState {
    pub(super) fn derive_surface_mapping_for_commit(
        &self,
        surface_id: u32,
        commit: &CachedSubsurfaceCommit,
    ) -> Option<SurfaceMappingProjection> {
        let surface = self.surface_resource_by_id(surface_id)?;
        let data = surface.data::<SurfaceData>()?;
        let mut effective_state = match EffectiveSurfaceMappingState::from_surface(
            data,
            self.current_surface_buffers.get(&surface_id),
        ) {
            Ok(state) => state,
            Err(error) => return Some(Err((error, data.committed_viewport_error_owner()))),
        };
        let mut references = Vec::new();
        if let Some(predecessor) = commit.lineage.predecessor {
            references.push(predecessor);
        }
        references.extend(commit.lineage.child_dependencies.iter().copied());
        for transaction in self.pending_surface_tree_mapping_prefix(
            self.root_surface_id_for_surface(surface_id),
            &references,
        ) {
            for (pending_surface_id, pending_commit) in &transaction.nodes {
                if *pending_surface_id != surface_id {
                    continue;
                }
                if let Err((error, viewport_error_owner)) =
                    effective_state.apply_commit(pending_commit)
                {
                    debug_assert!(
                        false,
                        "admitted pending surface-tree mapping became invalid while projecting: {error:?}"
                    );
                    return Some(Err((error, viewport_error_owner)));
                }
            }
        }
        Some(effective_state.apply_commit(commit))
    }

    pub(super) fn validate_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &[(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<(), (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>)> {
        self.derive_surface_tree_surface_state(
            root_surface_id,
            nodes,
            external_content_update_dependencies,
        )
        .map(|_| ())
    }

    pub(in crate::compositor) fn prepare_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &mut [(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<(), (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>)> {
        let mappings = self.derive_surface_tree_surface_state(
            root_surface_id,
            nodes,
            external_content_update_dependencies,
        )?;
        for ((_, commit), mapping) in nodes.iter_mut().zip(mappings) {
            if let Some(mapping) = mapping {
                let Some(PendingSurfaceAttachment::Buffer(pending)) = commit.attachment.as_mut()
                else {
                    debug_assert!(false, "mapping derived without a pending buffer");
                    continue;
                };
                pending.apply_content_mapping(mapping);
            }
        }
        Ok(())
    }

    fn derive_surface_tree_surface_state(
        &self,
        root_surface_id: u32,
        nodes: &[(u32, CachedSubsurfaceCommit)],
        external_content_update_dependencies: &[ContentUpdateRef],
    ) -> Result<
        Vec<Option<SurfaceContentMapping>>,
        (u32, SurfaceMappingError, Option<wp_viewport::WpViewport>),
    > {
        let mut effective_states = HashMap::new();
        let mut references = external_content_update_dependencies.to_vec();
        for (surface_id, commit) in nodes {
            if !effective_states.contains_key(surface_id) {
                let initial = match self.surface_resource_by_id(*surface_id) {
                    Some(surface) => match surface.data::<SurfaceData>() {
                        Some(data) => EffectiveSurfaceMappingState::from_surface(
                            data,
                            self.current_surface_buffers.get(surface_id),
                        ),
                        None => Err(SurfaceMappingError::InvalidBufferSize),
                    },
                    None => Err(SurfaceMappingError::InvalidBufferSize),
                };
                let Ok(initial) = initial else {
                    return Err((
                        *surface_id,
                        SurfaceMappingError::InvalidBufferSize,
                        commit.viewport_error_owner.clone(),
                    ));
                };
                effective_states.insert(*surface_id, initial);
            }
            if let Some(predecessor) = commit.lineage.predecessor {
                references.push(predecessor);
            }
            references.extend(commit.lineage.child_dependencies.iter().copied());
        }
        for transaction in self.pending_surface_tree_mapping_prefix(root_surface_id, &references) {
            for (surface_id, commit) in &transaction.nodes {
                let Some(effective_state) = effective_states.get_mut(surface_id) else {
                    continue;
                };
                if let Err((error, viewport_error_owner)) = effective_state.apply_commit(commit) {
                    debug_assert!(
                        false,
                        "admitted pending surface-tree mapping became invalid while projecting: {error:?}"
                    );
                    return Err((*surface_id, error, viewport_error_owner));
                }
            }
        }
        let mut mappings = Vec::with_capacity(nodes.len());
        for (surface_id, commit) in nodes {
            let mapping = effective_states
                .get_mut(surface_id)
                .expect("effective state inserted above")
                .apply_commit(commit);
            match mapping {
                Ok(mapping) => mappings.push(mapping),
                Err((error, viewport_error_owner)) => {
                    return Err((*surface_id, error, viewport_error_owner));
                }
            }
        }
        Ok(mappings)
    }

    fn pending_surface_tree_mapping_prefix<'a>(
        &'a self,
        root_surface_id: u32,
        candidate_references: &[ContentUpdateRef],
    ) -> Vec<&'a PendingSurfaceTreeTransaction> {
        let transactions = self
            .surface_transactions
            .pending_trees()
            .collect::<Vec<_>>();
        let mut selected = vec![false; transactions.len()];
        for (index, transaction) in transactions.iter().enumerate() {
            if transaction.root_surface_id == root_surface_id {
                selected[index] = true;
            }
        }

        let mut references = Vec::new();
        for reference in candidate_references {
            add_unique_content_update_ref(&mut references, *reference);
        }

        let mut next_reference = 0;
        loop {
            while let Some(reference) = references.get(next_reference).copied() {
                next_reference += 1;
                if let Some(index) = transactions.iter().position(|transaction| {
                    transaction_covers_content_update_ref(transaction, reference)
                }) {
                    selected[index] = true;
                }
            }

            let mut changed = false;
            for index in 0..transactions.len() {
                if !selected[index] {
                    continue;
                }
                let transaction = &transactions[index];
                for prior_index in 0..index {
                    if transactions[prior_index].root_surface_id == transaction.root_surface_id
                        && !selected[prior_index]
                    {
                        selected[prior_index] = true;
                        changed = true;
                    }
                }
                for dependency in &transaction.external_content_update_dependencies {
                    changed |= add_unique_content_update_ref(&mut references, *dependency);
                }
                for (_, commit) in &transaction.nodes {
                    if let Some(predecessor) = commit.lineage.predecessor {
                        changed |= add_unique_content_update_ref(&mut references, predecessor);
                    }
                    for dependency in &commit.lineage.child_dependencies {
                        changed |= add_unique_content_update_ref(&mut references, *dependency);
                    }
                }
            }
            if !changed && next_reference >= references.len() {
                break;
            }
        }

        let selected_indices = selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect::<Vec<_>>();
        if selected_indices.len() < 2 {
            return selected_indices
                .into_iter()
                .map(|index| transactions[index])
                .collect();
        }

        let mut indegree = vec![0usize; transactions.len()];
        let mut successors = vec![Vec::new(); transactions.len()];
        let mut add_edge = |predecessor: usize, dependent: usize| {
            if predecessor == dependent || successors[predecessor].contains(&dependent) {
                return;
            }
            successors[predecessor].push(dependent);
            indegree[dependent] = indegree[dependent].saturating_add(1);
        };
        for dependent in selected_indices.iter().copied() {
            for predecessor in selected_indices.iter().copied() {
                if predecessor < dependent
                    && transactions[predecessor].root_surface_id
                        == transactions[dependent].root_surface_id
                {
                    add_edge(predecessor, dependent);
                }
            }
            let transaction = &transactions[dependent];
            let mut transaction_references = Vec::new();
            transaction_references.extend(
                transaction
                    .external_content_update_dependencies
                    .iter()
                    .copied(),
            );
            for (_, commit) in &transaction.nodes {
                if let Some(predecessor) = commit.lineage.predecessor {
                    transaction_references.push(predecessor);
                }
                transaction_references.extend(commit.lineage.child_dependencies.iter().copied());
            }
            for reference in transaction_references {
                if let Some(predecessor) = selected_indices.iter().copied().find(|index| {
                    transaction_covers_content_update_ref(transactions[*index], reference)
                }) {
                    add_edge(predecessor, dependent);
                }
            }
        }

        let mut emitted = vec![false; transactions.len()];
        let mut order = Vec::with_capacity(selected_indices.len());
        for _ in 0..selected_indices.len() {
            let Some(next) = selected_indices
                .iter()
                .copied()
                .find(|index| !emitted[*index] && indegree[*index] == 0)
            else {
                debug_assert!(false, "surface-tree pending transaction ordering cycle");
                return selected_indices
                    .into_iter()
                    .map(|index| transactions[index])
                    .collect();
            };
            emitted[next] = true;
            order.push(next);
            for successor in &successors[next] {
                indegree[*successor] = indegree[*successor].saturating_sub(1);
            }
        }
        order.into_iter().map(|index| transactions[index]).collect()
    }

    pub(in crate::compositor) fn post_surface_mapping_error(
        &mut self,
        surface_id: u32,
        error: SurfaceMappingError,
        viewport_error_owner: Option<wp_viewport::WpViewport>,
    ) {
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return;
        };
        let Some(client) = surface.client() else {
            return;
        };
        // A cached viewport error belongs to the resource that authored the
        // cached source state. If that object has since been destroyed, its
        // error cannot be reassigned to a newer viewport or to wl_surface.
        let viewport = viewport_error_owner.filter(Resource::is_alive);
        match (viewport, error) {
            (Some(viewport), SurfaceMappingError::ViewportSourceNonIntegralWithoutDestination) => {
                self.post_protocol_error(
                    &client,
                    &viewport,
                    wp_viewport::Error::BadSize,
                    "viewport source width and height must be integral when destination is unset",
                )
            }
            (Some(viewport), SurfaceMappingError::ViewportSourceOutOfBounds) => {
                self.post_protocol_error(
                    &client,
                    &viewport,
                    wp_viewport::Error::OutOfBuffer,
                    "viewport source rectangle is outside the buffer",
                );
            }
            (None, SurfaceMappingError::ViewportSourceNonIntegralWithoutDestination)
            | (None, SurfaceMappingError::ViewportSourceOutOfBounds) => {}
            (_, SurfaceMappingError::InvalidBufferSize)
            | (_, SurfaceMappingError::BufferScaleNotIntegral) => self.post_protocol_error(
                &client,
                &surface,
                wl_surface::Error::InvalidSize,
                "surface buffer mapping is invalid",
            ),
        }
    }
}
