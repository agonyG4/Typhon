#![allow(clippy::all)]
#![allow(dead_code)]
#![allow(unused_imports)]

pub mod server {
    use wayland_server;
    use wayland_server::protocol::*;

    pub mod __interfaces {
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/astrea-toplevel-management-v1.xml");
    }

    use self::__interfaces::*;
    wayland_scanner::generate_server_code!("./protocols/astrea-toplevel-management-v1.xml");
}

pub mod client {
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/astrea-toplevel-management-v1.xml");
    }

    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("./protocols/astrea-toplevel-management-v1.xml");
}
