#![allow(
    dead_code,
    missing_docs,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_unsafe,
    unused_variables,
    clippy::all
)]

pub mod server {
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/wayland-drm.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("./protocols/wayland-drm.xml");
}

#[cfg(test)]
pub mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/wayland-drm.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("./protocols/wayland-drm.xml");
}
