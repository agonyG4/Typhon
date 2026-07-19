use oblivion_one::compositor::gpu_protocol_capabilities::{
    DmabufProtocolVersion, GpuFormat, GpuGlobal, GpuProtocolCapabilities, GpuProtocolProbe,
    RenderNodeEvidence, RenderNodeKind, inspect_render_node,
};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
};

fn valid_probe() -> GpuProtocolProbe {
    GpuProtocolProbe::valid_for_tests()
}

fn argb_linear() -> GpuFormat {
    GpuFormat::new(0x3432_5241, 0)
}

#[test]
fn missing_render_path_disables_wl_drm_but_keeps_valid_dmabuf() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::missing();
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(capabilities.global_enabled(GpuGlobal::LinuxDmabuf));
    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(!capabilities.wl_drm_disable_reason().is_empty());
}

#[test]
fn empty_render_path_disables_wl_drm() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::empty();
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(
        capabilities
            .wl_drm_disable_reason()
            .contains("empty render-node path")
    );
}

#[test]
fn nonexistent_render_path_disables_wl_drm() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::new(
        Some("/dev/dri/renderD999"),
        RenderNodeKind::Nonexistent,
        None,
        false,
    );
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(
        capabilities
            .wl_drm_disable_reason()
            .contains("does not exist")
    );
}

#[test]
fn regular_file_is_not_a_render_node() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::new(
        Some("/tmp/render-node"),
        RenderNodeKind::RegularFile,
        Some(7),
        true,
    );
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(
        capabilities
            .wl_drm_disable_reason()
            .contains("not a character render node")
    );
}

#[test]
fn inaccessible_render_node_disables_wl_drm() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::new(
        Some("/dev/dri/renderD128"),
        RenderNodeKind::RenderNode,
        Some(7),
        false,
    );
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(
        capabilities
            .wl_drm_disable_reason()
            .contains("cannot be opened")
    );
}

#[test]
fn card_node_is_not_accepted_as_wl_drm_device() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::new(
        Some("/dev/dri/card0"),
        RenderNodeKind::CardNode,
        Some(7),
        true,
    );
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(capabilities.wl_drm_disable_reason().contains("card node"));
}

#[test]
fn matching_render_node_can_enable_wl_drm() {
    let capabilities = GpuProtocolCapabilities::from_probe(valid_probe());

    assert!(capabilities.global_enabled(GpuGlobal::WlDrm));
    assert_eq!(capabilities.wl_drm_device(), Some("/dev/dri/renderD128"));
    assert!(capabilities.wl_drm_prime());
}

#[test]
fn multi_gpu_mismatch_disables_wl_drm_without_disabling_dmabuf() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::new(
        Some("/dev/dri/renderD129"),
        RenderNodeKind::RenderNode,
        Some(9),
        true,
    );
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(capabilities.global_enabled(GpuGlobal::LinuxDmabuf));
    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(
        capabilities
            .wl_drm_disable_reason()
            .contains("does not match")
    );
}

#[test]
fn missing_feedback_downgrades_to_basic_dmabuf_or_none() {
    let mut probe = valid_probe();
    probe.feedback_main_device = None;
    probe.feedback_format_table.clear();
    probe.feedback_tranches_valid = false;
    probe.feedback_has_modifiers = false;

    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::V1);
    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
}

#[test]
fn empty_import_contract_disables_dmabuf() {
    let mut probe = valid_probe();
    probe.importer_formats.clear();
    probe.basic_import_valid = false;

    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::None);
    assert!(!capabilities.global_enabled(GpuGlobal::LinuxDmabuf));
    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
}

#[test]
fn dmabuf_version_prefers_v4_only_with_complete_feedback() {
    let capabilities = GpuProtocolCapabilities::from_probe(valid_probe());

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::V4);
}

#[test]
fn complete_feedback_without_validated_import_is_not_published() {
    let mut probe = valid_probe();
    probe.basic_import_valid = false;

    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::None);
    assert!(!capabilities.global_enabled(GpuGlobal::LinuxDmabuf));
}

#[test]
fn dmabuf_version_downgrades_to_v3_when_v4_feedback_is_missing() {
    let mut probe = valid_probe();
    probe.feedback_main_device = None;
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::V3);
}

#[test]
fn dmabuf_version_downgrades_to_v1_when_modifiers_are_missing() {
    let mut probe = valid_probe();
    probe.feedback_main_device = None;
    probe.feedback_has_modifiers = false;
    probe.feedback_format_table = vec![argb_linear()];
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert_eq!(capabilities.dmabuf_version(), DmabufProtocolVersion::V1);
}

#[test]
fn syncobj_requires_create_and_import_on_selected_device() {
    let mut probe = valid_probe();
    probe.syncobj_timeline_import = false;
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::LinuxDrmSyncobj));
}

#[test]
fn wl_drm_authentication_is_not_claimed_for_render_node_only_contract() {
    let mut probe = valid_probe();
    probe.wl_drm_magic_authentication = false;
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(!capabilities.wl_drm_authentication());
}

#[test]
fn capabilities_explain_each_global_decision() {
    let mut probe = valid_probe();
    probe.render_node = RenderNodeEvidence::missing();
    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    let diagnostics = capabilities.diagnostics();
    assert!(diagnostics.iter().any(|entry| {
        entry.global == GpuGlobal::WlDrm && !entry.enabled && !entry.reason.is_empty()
    }));
    assert!(
        diagnostics
            .iter()
            .any(|entry| { entry.global == GpuGlobal::LinuxDmabuf && entry.enabled })
    );
}

#[test]
fn render_node_inspection_classifies_missing_empty_nonexistent_and_regular_paths() {
    assert_eq!(inspect_render_node(None).kind, RenderNodeKind::Missing);
    assert_eq!(
        inspect_render_node(Some(Path::new(""))).kind,
        RenderNodeKind::Empty
    );

    let nonexistent = std::env::temp_dir().join(format!(
        "typhon-xwayland-render-node-{}-missing",
        std::process::id()
    ));
    let _ = fs::remove_file(&nonexistent);
    assert_eq!(
        inspect_render_node(Some(&nonexistent)).kind,
        RenderNodeKind::Nonexistent
    );

    let regular = std::env::temp_dir().join(format!(
        "typhon-xwayland-render-node-{}-regular",
        std::process::id()
    ));
    fs::write(&regular, b"not a render node").unwrap();
    let evidence = inspect_render_node(Some(&regular));
    assert_eq!(evidence.kind, RenderNodeKind::RegularFile);
    assert!(!evidence.openable || evidence.kind != RenderNodeKind::RenderNode);
    fs::remove_file(regular).unwrap();
}

#[test]
fn wl_drm_requires_a_common_linear_format() {
    let mut probe = valid_probe();
    probe.feedback_format_table = probe
        .feedback_format_table
        .into_iter()
        .map(|format| GpuFormat::new(format.fourcc, 0x100))
        .collect();
    probe.importer_formats = probe.feedback_format_table.clone();

    let capabilities = GpuProtocolCapabilities::from_probe(probe);

    assert!(!capabilities.global_enabled(GpuGlobal::WlDrm));
    assert!(capabilities.wl_drm_formats().is_empty());
}

#[test]
#[ignore = "requires a running native Wayland session and installed Xwayland"]
fn installed_xwayland_validates_displayfd_without_no_glamor_fallback() {
    let Some(wayland_display) = std::env::var_os("WAYLAND_DISPLAY") else {
        return;
    };
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    if Command::new("Xwayland").arg("-version").output().is_err()
        || Command::new("xdpyinfo").arg("-version").output().is_err()
    {
        return;
    }

    let mut xwayland = Command::new("Xwayland")
        .args([
            ":99",
            "-rootless",
            "-terminate",
            "-nolisten",
            "tcp",
            "-displayfd",
            "1",
        ])
        .env("WAYLAND_DISPLAY", &wayland_display)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("XWAYLAND_NO_GLAMOR")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("installed Xwayland should spawn");
    let stdout = xwayland.stdout.take().expect("displayfd stdout pipe");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("displayfd should be readable");
    let display = line.trim();
    assert!(!display.is_empty(), "displayfd payload must be nonempty");
    assert!(display.chars().all(|character| character.is_ascii_digit()));

    let probe = Command::new("xdpyinfo")
        .env("DISPLAY", format!(":{display}"))
        .env("WAYLAND_DISPLAY", &wayland_display)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env_remove("XWAYLAND_NO_GLAMOR")
        .output()
        .expect("xdpyinfo should spawn");
    assert!(
        probe.status.success(),
        "xdpyinfo failed: {}",
        String::from_utf8_lossy(&probe.stderr)
    );

    xwayland.kill().expect("terminate Xwayland probe");
    let _ = xwayland.wait();
}
