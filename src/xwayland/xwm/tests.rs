use std::num::NonZeroU64;
use std::os::unix::net::UnixStream;

use super::{
    X11WindowHandle,
    atoms::{XwmAtomName, XwmAtoms},
    capabilities::XwmCapabilities,
    window::X11WindowRegistry,
};
use crate::xwayland::XwaylandGeneration;

#[test]
fn advertised_atoms_are_a_unique_implemented_subset() {
    let names = XwmAtoms::advertised_names();
    assert!(!names.is_empty());
    assert!(names.contains(&XwmAtomName::GtkFrameExtents));
    assert!(names.contains(&XwmAtomName::NetWmMoveresize));
    assert!(names.contains(&XwmAtomName::NetMoveResizeWindow));
    for (index, name) in names.iter().enumerate() {
        assert!(XwmAtomName::ALL.contains(name));
        assert!(!names[..index].contains(name));
    }
}

#[test]
fn supported_atoms_apply_capabilities_without_filtering_moveresize() {
    let normal_capabilities = XwmCapabilities {
        composite: true,
        xfixes: true,
        shape: true,
        randr: true,
        sync: true,
    };
    let normal_supported = XwmAtoms::advertised_names_for(normal_capabilities).collect::<Vec<_>>();
    assert!(normal_supported.contains(&XwmAtomName::NetWmMoveresize));
    assert!(normal_supported.contains(&XwmAtomName::NetMoveResizeWindow));
    assert!(normal_supported.contains(&XwmAtomName::NetWmSyncRequest));

    let no_optional_extensions = XwmCapabilities {
        composite: true,
        xfixes: false,
        shape: false,
        randr: false,
        sync: false,
    };
    let filtered_supported =
        XwmAtoms::advertised_names_for(no_optional_extensions).collect::<Vec<_>>();
    assert!(filtered_supported.contains(&XwmAtomName::NetWmMoveresize));
    assert!(filtered_supported.contains(&XwmAtomName::NetMoveResizeWindow));
    assert!(!filtered_supported.contains(&XwmAtomName::NetWmSyncRequest));
}

#[test]
fn x11_window_registry_is_generation_bound() {
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero"));
    let newer_generation = XwaylandGeneration::new(NonZeroU64::new(2).expect("nonzero"));
    let first = X11WindowHandle::new(generation, 10);
    let second = X11WindowHandle::new(newer_generation, 10);
    let mut registry = X11WindowRegistry::default();

    assert!(registry.insert_observed(first));
    assert!(!registry.insert_observed(first));
    assert!(registry.contains(first));
    registry.clear_generation(generation);
    assert!(!registry.contains(first));
    assert!(!registry.contains(second));
}

#[test]
fn xwm_connect_consumes_the_wm_stream_on_setup_failure() {
    let (stream, peer) = UnixStream::pair().expect("socket pair");
    drop(peer);
    let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero"));

    assert!(super::Xwm::connect(generation, stream).is_err());
}
