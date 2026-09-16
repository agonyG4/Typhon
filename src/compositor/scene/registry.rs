use std::collections::{HashMap, HashSet};

use crate::core::{SceneNodeId, SceneNodeIdAllocator};

use super::{
    SceneDomain, SceneDomainAssignment, SceneNodeMetadata, SceneOwner, SceneRole, SceneSource,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SceneRegistryError {
    DuplicateSource,
    Exhausted,
    MissingNode,
    MissingParent,
    SelfParent,
    Cycle,
}

#[derive(Debug, Default)]
pub(crate) struct CanonicalSceneRegistry {
    allocator: SceneNodeIdAllocator,
    nodes: HashMap<SceneNodeId, SceneNodeMetadata>,
    source_index: HashMap<SceneSource, SceneNodeId>,
    children_by_visual_parent: HashMap<SceneNodeId, Vec<SceneNodeId>>,
}

impl CanonicalSceneRegistry {
    pub(crate) fn register(
        &mut self,
        source: SceneSource,
        owner: SceneOwner,
        role: SceneRole,
        domain: SceneDomainAssignment,
    ) -> Result<SceneNodeId, SceneRegistryError> {
        if self.source_index.contains_key(&source) {
            return Err(SceneRegistryError::DuplicateSource);
        }
        let id = self
            .allocator
            .allocate()
            .map_err(|_| SceneRegistryError::Exhausted)?;
        let metadata = SceneNodeMetadata {
            id,
            source,
            owner,
            role,
            domain,
            visual_parent: None,
        };
        self.nodes.insert(id, metadata);
        self.source_index.insert(source, id);
        Ok(id)
    }

    pub(crate) fn node_for_source(&self, source: SceneSource) -> Option<SceneNodeId> {
        self.source_index.get(&source).copied()
    }

    pub(crate) fn metadata(&self, id: SceneNodeId) -> Option<&SceneNodeMetadata> {
        self.nodes.get(&id)
    }

    pub(crate) fn update_metadata(
        &mut self,
        id: SceneNodeId,
        role: SceneRole,
        domain: SceneDomainAssignment,
    ) -> Result<(), SceneRegistryError> {
        let metadata = self
            .nodes
            .get_mut(&id)
            .ok_or(SceneRegistryError::MissingNode)?;
        metadata.role = role;
        metadata.domain = domain;
        Ok(())
    }

    pub(crate) fn set_visual_parent(
        &mut self,
        child: SceneNodeId,
        parent: Option<SceneNodeId>,
    ) -> Result<(), SceneRegistryError> {
        let old_parent = self
            .nodes
            .get(&child)
            .ok_or(SceneRegistryError::MissingNode)?
            .visual_parent;

        if let Some(parent) = parent {
            if !self.nodes.contains_key(&parent) {
                return Err(SceneRegistryError::MissingParent);
            }
            if parent == child {
                return Err(SceneRegistryError::SelfParent);
            }
            let mut cursor = Some(parent);
            let mut visited = HashSet::new();
            while let Some(node) = cursor {
                if node == child {
                    return Err(SceneRegistryError::Cycle);
                }
                if !visited.insert(node) {
                    return Err(SceneRegistryError::Cycle);
                }
                cursor = self
                    .nodes
                    .get(&node)
                    .and_then(|metadata| metadata.visual_parent);
            }
        }

        if old_parent == parent {
            return Ok(());
        }

        if let Some(old_parent) = old_parent {
            self.detach_child_from_parent(old_parent, child);
        }
        if let Some(metadata) = self.nodes.get_mut(&child) {
            metadata.visual_parent = parent;
        }
        if let Some(parent) = parent {
            self.children_by_visual_parent
                .entry(parent)
                .or_default()
                .push(child);
        }
        Ok(())
    }

    pub(crate) fn visual_children(&self, parent: SceneNodeId) -> &[SceneNodeId] {
        self.children_by_visual_parent
            .get(&parent)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn resolve_domain(&self, id: SceneNodeId) -> Option<SceneDomain> {
        let mut current = id;
        let mut fallback = None;
        let mut visited = HashSet::new();

        loop {
            let metadata = self.nodes.get(&current)?;
            match metadata.domain {
                SceneDomainAssignment::Explicit(domain) => return Some(domain),
                SceneDomainAssignment::Inherit {
                    fallback: node_fallback,
                } => {
                    fallback.get_or_insert(node_fallback);
                    let Some(parent) = metadata.visual_parent else {
                        return fallback;
                    };
                    if !visited.insert(current) {
                        return fallback;
                    }
                    current = parent;
                }
            }
        }
    }

    pub(crate) fn remove(
        &mut self,
        id: SceneNodeId,
    ) -> Result<SceneNodeMetadata, SceneRegistryError> {
        let metadata = self
            .nodes
            .remove(&id)
            .ok_or(SceneRegistryError::MissingNode)?;

        if let Some(parent) = metadata.visual_parent {
            self.detach_child_from_parent(parent, id);
        }
        if let Some(children) = self.children_by_visual_parent.remove(&id) {
            for child in children {
                if let Some(child_metadata) = self.nodes.get_mut(&child) {
                    child_metadata.visual_parent = None;
                }
            }
        }
        self.source_index.remove(&metadata.source);
        Ok(metadata)
    }

    fn detach_child_from_parent(&mut self, parent: SceneNodeId, child: SceneNodeId) {
        let mut remove_parent_entry = false;
        if let Some(children) = self.children_by_visual_parent.get_mut(&parent) {
            children.retain(|candidate| *candidate != child);
            remove_parent_entry = children.is_empty();
        }
        if remove_parent_entry {
            self.children_by_visual_parent.remove(&parent);
        }
    }
}
