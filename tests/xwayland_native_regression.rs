#[path = "support/x11_client.rs"]
mod x11_client;

use x11_client::RegressionX11Client;

/// Send a native X11 request sequence to an existing display.
///
/// This is a request-generation smoke test, not an end-to-end compositor
/// assertion. Behavioral coverage for popup admission, resize presentation,
/// and remap association ordering lives in the deterministic XWayland tests.
#[test]
fn opt_in_x11_client_generates_popup_resize_and_remap_requests() {
    if std::env::var_os("TYPHON_XWAYLAND_NATIVE_TESTS").as_deref() != Some("1".as_ref()) {
        return;
    }
    let client = RegressionX11Client::connect(None).expect("connect to the opt-in X11 display");
    let parent = client
        .create_window(false)
        .expect("create managed regression window");
    client
        .set_name(parent, b"Typhon XWayland regression parent")
        .expect("set parent title");
    client
        .set_class(parent, b"typhon-regression", b"TyphonRegression")
        .expect("set parent class");
    client
        .set_window_type(parent, false)
        .expect("set normal parent type");
    client.map(parent).expect("map parent");

    let popup = client
        .create_window(false)
        .expect("create popup regression window");
    client
        .set_window_type(popup, false)
        .expect("set provisional managed popup type");
    client
        .set_transient_for(popup, parent)
        .expect("set popup transient parent");
    client
        .set_override_redirect(popup, true)
        .expect("flip popup override-redirect before map");
    client
        .set_window_type(popup, true)
        .expect("set popup menu type after create scan");
    client.map(popup).expect("map popup");

    client
        .send_moveresize(parent, 180, 180, 8, 1)
        .expect("send moveresize request");
    client
        .set_allow_commits(parent, false)
        .expect("block parent commits");
    client
        .set_allow_commits(parent, true)
        .expect("release parent commits");
    client.unmap(popup).expect("unmap popup");
    client.map(popup).expect("remap popup");
    client.destroy(popup).expect("destroy popup");
    client.destroy(parent).expect("destroy parent");
    client.flush().expect("flush regression sequence");
}
