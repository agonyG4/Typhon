use std::num::NonZeroU64;
use std::os::unix::net::UnixStream;

use super::{
    X11WindowHandle,
    atoms::{XwmAtomName, XwmAtoms},
    window::X11WindowRegistry,
};
use crate::xwayland::XwaylandGeneration;

#[test]
fn advertised_atoms_are_a_unique_implemented_subset() {
    let names = XwmAtoms::advertised_names();
    assert!(!names.is_empty());
    for (index, name) in names.iter().enumerate() {
        assert!(XwmAtomName::ALL.contains(name));
        assert!(!names[..index].contains(name));
    }
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
