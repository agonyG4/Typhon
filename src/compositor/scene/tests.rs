use super::*;
use crate::core::{SceneNodeId, WindowId};

fn surface_source(surface_id: u32) -> (SceneSource, SceneOwner) {
    (
        SceneSource::Surface(surface_id),
        SceneOwner::Surface(surface_id),
    )
}

fn register_surface(
    registry: &mut CanonicalSceneRegistry,
    surface_id: u32,
    role: SceneRole,
    domain: SceneDomainAssignment,
) -> SceneNodeId {
    let (source, owner) = surface_source(surface_id);
    registry
        .register(source, owner, role, domain)
        .expect("surface node registration")
}

#[test]
fn unique_source_registration_returns_a_stable_node_id() {
    let mut registry = CanonicalSceneRegistry::default();
    let first = register_surface(
        &mut registry,
        42,
        SceneRole::UnassignedSurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );

    assert_eq!(
        registry.node_for_source(SceneSource::Surface(42)),
        Some(first)
    );
    assert_eq!(
        registry.node_for_source(SceneSource::Surface(42)),
        Some(first)
    );
}

#[test]
fn duplicate_source_registration_is_rejected() {
    let mut registry = CanonicalSceneRegistry::default();
    let (source, owner) = surface_source(1);
    registry
        .register(
            source,
            owner,
            SceneRole::UnassignedSurface,
            SceneDomainAssignment::Inherit {
                fallback: SceneDomain::Content,
            },
        )
        .expect("initial source registration");

    assert_eq!(
        registry.register(
            source,
            owner,
            SceneRole::ClientSurface,
            SceneDomainAssignment::Explicit(SceneDomain::Content),
        ),
        Err(SceneRegistryError::DuplicateSource)
    );
}

#[test]
fn visual_parent_requires_an_existing_distinct_node() {
    let mut registry = CanonicalSceneRegistry::default();
    let child = register_surface(
        &mut registry,
        1,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    let missing = SceneNodeId::from_raw(999).expect("test node id");

    assert_eq!(
        registry.set_visual_parent(child, Some(missing)),
        Err(SceneRegistryError::MissingParent)
    );
    assert_eq!(
        registry.set_visual_parent(child, Some(child)),
        Err(SceneRegistryError::SelfParent)
    );
    assert_eq!(registry.metadata(child).unwrap().visual_parent, None);
}

#[test]
fn visual_parent_cycles_are_rejected_without_mutating_old_topology() {
    let mut registry = CanonicalSceneRegistry::default();
    let a = register_surface(
        &mut registry,
        1,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    let b = register_surface(
        &mut registry,
        2,
        SceneRole::Subsurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );
    let c = register_surface(
        &mut registry,
        3,
        SceneRole::Subsurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );
    registry
        .set_visual_parent(b, Some(a))
        .expect("first parent edge");
    registry
        .set_visual_parent(c, Some(b))
        .expect("second parent edge");

    assert_eq!(
        registry.set_visual_parent(a, Some(c)),
        Err(SceneRegistryError::Cycle)
    );
    assert_eq!(registry.metadata(a).unwrap().visual_parent, None);
    assert_eq!(registry.metadata(b).unwrap().visual_parent, Some(a));
    assert_eq!(registry.metadata(c).unwrap().visual_parent, Some(b));
    assert_eq!(registry.visual_children(a), &[b]);
    assert_eq!(registry.visual_children(b), &[c]);
}

#[test]
fn reverse_children_index_remains_coherent_across_parent_mutation() {
    let mut registry = CanonicalSceneRegistry::default();
    let first = register_surface(
        &mut registry,
        1,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    let second = register_surface(
        &mut registry,
        2,
        SceneRole::Subsurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );
    let third = register_surface(
        &mut registry,
        3,
        SceneRole::Subsurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );
    registry
        .set_visual_parent(second, Some(first))
        .expect("first child edge");
    registry
        .set_visual_parent(third, Some(first))
        .expect("second child edge");
    registry
        .set_visual_parent(third, Some(second))
        .expect("reparent child");

    assert_eq!(registry.visual_children(first), &[second]);
    assert_eq!(registry.visual_children(second), &[third]);
}

#[test]
fn removing_a_parent_detaches_children_without_destroying_them() {
    let mut registry = CanonicalSceneRegistry::default();
    let parent = register_surface(
        &mut registry,
        1,
        SceneRole::WindowGroup,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    let child = register_surface(
        &mut registry,
        2,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Content,
        },
    );
    registry
        .set_visual_parent(child, Some(parent))
        .expect("child edge");

    registry.remove(parent).expect("remove parent");

    assert!(registry.metadata(parent).is_none());
    assert_eq!(registry.metadata(child).unwrap().visual_parent, None);
    assert!(registry.visual_children(parent).is_empty());
    assert!(registry.remove(parent).is_err());
    assert_eq!(
        registry.node_for_source(SceneSource::Surface(2)),
        Some(child)
    );
    let replacement = register_surface(
        &mut registry,
        3,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    assert_ne!(replacement, parent);
}

#[test]
fn external_identity_namespaces_and_window_nodes_remain_distinct() {
    let mut registry = CanonicalSceneRegistry::default();
    let surface = register_surface(
        &mut registry,
        7,
        SceneRole::ClientSurface,
        SceneDomainAssignment::Explicit(SceneDomain::Content),
    );
    let window = WindowId::from_raw(7).expect("window id");
    let group = registry
        .register(
            SceneSource::WindowGroup(window),
            SceneOwner::Window(window),
            SceneRole::WindowGroup,
            SceneDomainAssignment::Explicit(SceneDomain::Content),
        )
        .expect("window group");
    let decoration = registry
        .register(
            SceneSource::ServerDecoration(window),
            SceneOwner::Window(window),
            SceneRole::ServerDecoration,
            SceneDomainAssignment::Explicit(SceneDomain::Chrome),
        )
        .expect("server decoration");

    assert_ne!(surface, group);
    assert_ne!(group, decoration);
    assert_eq!(
        registry.metadata(group).unwrap().owner,
        SceneOwner::Window(window)
    );
    assert_eq!(
        registry.metadata(decoration).unwrap().owner,
        SceneOwner::Window(window)
    );
}

#[test]
fn inherited_domain_resolves_through_visual_ancestry() {
    let mut registry = CanonicalSceneRegistry::default();
    let window = WindowId::from_raw(1).unwrap();
    let group = registry
        .register(
            SceneSource::WindowGroup(window),
            SceneOwner::Window(window),
            SceneRole::WindowGroup,
            SceneDomainAssignment::Explicit(SceneDomain::Content),
        )
        .expect("window group");
    let child = register_surface(
        &mut registry,
        1,
        SceneRole::PopupSurface,
        SceneDomainAssignment::Inherit {
            fallback: SceneDomain::Chrome,
        },
    );
    registry
        .set_visual_parent(child, Some(group))
        .expect("popup edge");

    assert_eq!(registry.resolve_domain(group), Some(SceneDomain::Content));
    assert_eq!(registry.resolve_domain(child), Some(SceneDomain::Content));
}
