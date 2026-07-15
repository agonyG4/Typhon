use super::super::protocols::versions::{self, GlobalAdvertisement};
use super::super::state_data::{CoreComplianceMetrics, UnhandledRequestClass};
use super::*;

use wayland_client::Proxy;

#[test]
fn advertised_global_versions_are_centralized() {
    assert_eq!(versions::WL_COMPOSITOR, 6);
    assert_eq!(versions::WL_SUBCOMPOSITOR, 1);
    assert_eq!(versions::WL_SHM, 2);
    assert_eq!(versions::WL_DATA_DEVICE_MANAGER, 3);
    assert_eq!(versions::XDG_WM_BASE, 6);
    assert_eq!(versions::WL_OUTPUT, 4);
    assert_eq!(versions::WL_SEAT, 7);

    let globals = versions::all_globals();
    assert!(globals.contains(&GlobalAdvertisement::new("wl_compositor", 6)));
    assert!(globals.contains(&GlobalAdvertisement::new("wl_subcompositor", 1)));
    assert!(globals.contains(&GlobalAdvertisement::new("wl_shm", 2)));
    assert!(globals.contains(&GlobalAdvertisement::new("wl_data_device_manager", 3)));
    assert!(globals.contains(&GlobalAdvertisement::new("xdg_wm_base", 6)));
    assert!(globals.contains(&GlobalAdvertisement::new("wl_output", 4)));
    assert!(globals.contains(&GlobalAdvertisement::new("wl_seat", 7)));

    let mut sorted = globals.to_vec();
    sorted.sort_by_key(|global| global.interface);
    sorted.dedup_by_key(|global| global.interface);
    assert_eq!(sorted.len(), globals.len());
}

#[test]
fn compliance_matrix_advertised_versions_match_manifest() {
    let matrix = include_str!("../../../docs/wayland/CORE_COMPLIANCE_MATRIX.md");
    let manifest = include_str!("../../../docs/wayland/PROTOCOL_SOURCE_MANIFEST.md");
    for global in versions::all_globals() {
        let row = format!("| `{}` | {} |", global.interface, global.version);
        assert!(
            matrix.contains(&row),
            "missing or mismatched matrix row: {row}"
        );
        assert!(
            manifest.contains(&row),
            "missing or mismatched source-manifest row: {row}"
        );
    }
}

#[test]
fn target_globals_bind_at_every_advertised_version() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();

    for version in 1..=versions::WL_COMPOSITOR {
        let compositor: client_wl_compositor::WlCompositor =
            globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(compositor.version(), version);
    }
    for version in 1..=versions::WL_SUBCOMPOSITOR {
        let subcompositor: client_wl_subcompositor::WlSubcompositor =
            globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(subcompositor.version(), version);
    }
    for version in 1..=versions::WL_SHM {
        let shm: client_wl_shm::WlShm = globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(shm.version(), version);
    }
    for version in 1..=versions::WL_DATA_DEVICE_MANAGER {
        let manager: client_wl_data_device_manager::WlDataDeviceManager =
            globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(manager.version(), version);
    }
    for version in 1..=versions::XDG_WM_BASE {
        let wm_base: client_xdg_wm_base::XdgWmBase =
            globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(wm_base.version(), version);
    }
    for version in 1..=versions::WL_OUTPUT {
        let output: client_wl_output::WlOutput = globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(output.version(), version);
    }
    for version in 1..=versions::WL_SEAT {
        let seat: client_wl_seat::WlSeat = globals.bind(&qh, version..=version, ()).unwrap();
        assert_eq!(seat.version(), version);
    }

    connection.flush().unwrap();
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state).unwrap();

    let server = stop_controllable_test_server(commands, server_thread);
    assert!(server.state.surface_resources.is_empty());
    assert_eq!(server.state.compliance_metrics.protocol_errors_total, 0);
}

#[test]
fn core_xdg_request_contracts_are_classified_and_version_bounded() {
    let contracts = versions::CORE_XDG_REQUEST_CONTRACTS;
    assert!(!contracts.is_empty());

    for contract in contracts {
        let advertised = match contract.interface {
            "wl_compositor" | "wl_surface" | "wl_region" => versions::WL_COMPOSITOR,
            "wl_shm" | "wl_shm_pool" => versions::WL_SHM,
            "wl_data_device_manager" | "wl_data_device" | "wl_data_source" | "wl_data_offer" => {
                versions::WL_DATA_DEVICE_MANAGER
            }
            "wl_seat" | "wl_pointer" | "wl_keyboard" => versions::WL_SEAT,
            "wl_output" => versions::WL_OUTPUT,
            "wl_subcompositor" | "wl_subsurface" => versions::WL_SUBCOMPOSITOR,
            "xdg_wm_base" | "xdg_positioner" | "xdg_surface" | "xdg_toplevel" | "xdg_popup" => {
                versions::XDG_WM_BASE
            }
            _ => panic!(
                "unmapped request-contract interface: {}",
                contract.interface
            ),
        };
        assert!(contract.since <= advertised, "{contract:?}");
        assert!(!contract.test.is_empty(), "{contract:?}");
        assert!(!contract.interface.is_empty(), "{contract:?}");
        assert!(!contract.request.is_empty(), "{contract:?}");
    }

    for (index, contract) in contracts.iter().enumerate() {
        assert!(
            contracts[index + 1..]
                .iter()
                .all(|other| (other.interface, other.request)
                    != (contract.interface, contract.request)),
            "duplicate request contract: {contract:?}"
        );
    }
}

#[test]
fn unhandled_request_classification_is_explicit() {
    let mut metrics = CoreComplianceMetrics::default();

    metrics.note_unhandled_request(
        "wl_surface",
        6,
        UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
    );
    assert_eq!(metrics.supported_request_unhandled_total, 0);

    metrics.note_unhandled_request(
        "wl_surface",
        6,
        UnhandledRequestClass::SupportedButUnhandled,
    );
    assert_eq!(metrics.supported_request_unhandled_total, 1);

    metrics.note_dnd_duplicate_terminal_attempt();
    assert_eq!(metrics.dnd_duplicate_terminal_attempts, 1);
}

#[test]
fn every_dispatch_fallback_has_an_explicit_unhandled_classification() {
    let sources = [
        ("core", include_str!("../protocols/core.rs")),
        ("buffers", include_str!("../protocols/buffers.rs")),
        ("data_device", include_str!("../protocols/data_device.rs")),
        ("input", include_str!("../protocols/input.rs")),
        ("xdg", include_str!("../protocols/xdg.rs")),
        ("viewport", include_str!("../protocols/viewport.rs")),
        ("advanced", include_str!("../protocols/advanced.rs")),
        ("syncobj", include_str!("../protocols/syncobj.rs")),
        ("activation", include_str!("../protocols/activation.rs")),
        ("layer_shell", include_str!("../protocols/layer_shell.rs")),
        ("presentation", include_str!("../protocols/presentation.rs")),
    ];
    for (name, source) in sources {
        let mut lines = source.lines();
        while let Some(line) = lines.next() {
            if !line.contains("note_unhandled_request(") {
                continue;
            }
            let mut call = String::from(line);
            for next in lines.by_ref().take(8) {
                call.push_str(next);
                if next.contains(");") {
                    break;
                }
            }
            assert!(
                call.contains("UnhandledRequestClass::"),
                "dispatch fallback in {name} does not name an unhandled-request class: {call}"
            );
        }
    }

    let matrix = include_str!("../../../docs/wayland/CORE_COMPLIANCE_MATRIX.md");
    assert!(!matrix.lines().any(|line| line.contains("| Gap |")));
}
