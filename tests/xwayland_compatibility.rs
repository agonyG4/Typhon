use std::process::Command;

#[test]
fn session_inspector_uses_explicit_values_or_unconnected_status() {
    let source =
        std::fs::read_to_string("bin/check-xwayland-session").expect("session inspector source");
    assert!(!source.contains("reported by Typhon"));
    assert!(source.contains("unsupported/not connected"));
}

#[test]
#[ignore = "requires a native managed Typhon session and installed X11 clients"]
fn installed_x11_compatibility_smoke() {
    let display = std::env::var("DISPLAY").expect("native test requires DISPLAY");
    let xdpyinfo = Command::new("xdpyinfo")
        .env("DISPLAY", &display)
        .status()
        .expect("xdpyinfo must be installed");
    assert!(xdpyinfo.success(), "xdpyinfo failed on {display}");
    let xmessage = Command::new("xmessage")
        .env("DISPLAY", &display)
        .args(["-timeout", "1", "Typhon XWayland smoke"])
        .status()
        .expect("xmessage must be installed");
    assert!(xmessage.success(), "xmessage failed on {display}");
}

#[test]
#[ignore = "requires a native managed Typhon session"]
fn session_inspector_is_standalone() {
    let status = Command::new("./bin/check-xwayland-session")
        .status()
        .expect("session inspector");
    assert!(status.success());
}
