use std::path::PathBuf;

use super::{XwaylandConfig, XwaylandMode, XwaylandService, XwaylandStateKind};

#[test]
fn xwayland_mode_parses_only_opt_in_values() {
    assert_eq!(XwaylandMode::parse(None), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("off")), XwaylandMode::Off);
    assert_eq!(XwaylandMode::parse(Some("base")), XwaylandMode::BaseLazy);
    assert_eq!(XwaylandMode::parse(Some("eager")), XwaylandMode::BaseEager);
    assert_eq!(XwaylandMode::parse(Some("host")), XwaylandMode::Off);
}

#[test]
fn off_bootstrap_is_disabled_without_lease_or_process() {
    let service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests(
        XwaylandMode::Off,
        PathBuf::from("Xwayland"),
    ))
    .expect("bootstrap off mode");

    assert_eq!(service.state_kind(), XwaylandStateKind::Disabled);
    assert!(service.app_environment().is_none());
    assert_eq!(service.reactor_registrations().count(), 0);
}

#[test]
fn generation_allocator_returns_distinct_nonzero_values() {
    let mut service = XwaylandService::bootstrap_with_config(XwaylandConfig::for_tests(
        XwaylandMode::BaseLazy,
        PathBuf::from("Xwayland"),
    ))
    .expect("bootstrap base mode");

    let first = service.allocate_generation();
    let second = service.allocate_generation();
    assert_ne!(first, second);
    assert_ne!(first.get(), 0);
    assert_ne!(second.get(), 0);
}
