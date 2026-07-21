//! Small test-only X11 connection helper shared by native compatibility tests.

use std::{collections::HashMap, error::Error, io, os::unix::net::UnixStream, path::Path};

use x11rb::{
    connection::Connection,
    protocol::xproto::{
        Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageData, ClientMessageEvent,
        ConnectionExt, CreateWindowAux, EventMask, PropMode, Window, WindowClass,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as WrapperConnectionExt,
};

pub type X11Result<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub struct RegressionX11Client {
    pub connection: RustConnection,
    pub root: Window,
    atoms: HashMap<&'static str, Atom>,
}

impl RegressionX11Client {
    pub fn connect(display: Option<&str>) -> X11Result<Self> {
        let (connection, screen) = RustConnection::connect(display)?;
        let root = connection.setup().roots[screen].root;
        let mut atoms = HashMap::new();
        for name in [
            "UTF8_STRING",
            "WM_TRANSIENT_FOR",
            "_NET_WM_NAME",
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_NORMAL",
            "_NET_WM_WINDOW_TYPE_POPUP_MENU",
            "_NET_WM_MOVERESIZE",
            "_XWAYLAND_ALLOW_COMMITS",
        ] {
            let atom = connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom;
            atoms.insert(name, atom);
        }
        Ok(Self {
            connection,
            root,
            atoms,
        })
    }

    pub fn create_window(&self, override_redirect: bool) -> X11Result<Window> {
        let screen = &self.connection.setup().roots[0];
        let window = self.connection.generate_id()?;
        self.connection.create_window(
            screen.root_depth,
            window,
            self.root,
            120,
            120,
            640,
            480,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(u32::from(override_redirect))
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        )?;
        Ok(window)
    }

    pub fn set_override_redirect(&self, window: Window, override_redirect: bool) -> X11Result<()> {
        self.connection.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().override_redirect(u32::from(override_redirect)),
        )?;
        Ok(())
    }

    pub fn set_name(&self, window: Window, name: &[u8]) -> X11Result<()> {
        self.connection.change_property8(
            PropMode::REPLACE,
            window,
            self.atom("_NET_WM_NAME"),
            self.atom("UTF8_STRING"),
            name,
        )?;
        self.connection.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            name,
        )?;
        Ok(())
    }

    pub fn set_class(&self, window: Window, instance: &[u8], class: &[u8]) -> X11Result<()> {
        let mut value = Vec::with_capacity(instance.len() + class.len() + 2);
        value.extend_from_slice(instance);
        value.push(0);
        value.extend_from_slice(class);
        value.push(0);
        self.connection.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            &value,
        )?;
        Ok(())
    }

    pub fn set_window_type(&self, window: Window, popup: bool) -> X11Result<()> {
        let atom = if popup {
            self.atom("_NET_WM_WINDOW_TYPE_POPUP_MENU")
        } else {
            self.atom("_NET_WM_WINDOW_TYPE_NORMAL")
        };
        self.connection.change_property32(
            PropMode::REPLACE,
            window,
            self.atom("_NET_WM_WINDOW_TYPE"),
            AtomEnum::ATOM,
            &[atom],
        )?;
        Ok(())
    }

    pub fn set_transient_for(&self, window: Window, parent: Window) -> X11Result<()> {
        self.connection.change_property32(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_TRANSIENT_FOR,
            AtomEnum::WINDOW,
            &[parent],
        )?;
        Ok(())
    }

    pub fn set_allow_commits(&self, window: Window, allowed: bool) -> X11Result<()> {
        self.connection.change_property32(
            PropMode::REPLACE,
            window,
            self.atom("_XWAYLAND_ALLOW_COMMITS"),
            AtomEnum::CARDINAL,
            &[u32::from(allowed)],
        )?;
        Ok(())
    }

    pub fn send_moveresize(
        &self,
        window: Window,
        root_x: i32,
        root_y: i32,
        direction: u32,
        button: u32,
    ) -> X11Result<()> {
        self.connection.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            ClientMessageEvent::new(
                32,
                window,
                self.atom("_NET_WM_MOVERESIZE"),
                ClientMessageData::from([root_x as u32, root_y as u32, direction, button, 1]),
            ),
        )?;
        Ok(())
    }

    pub fn map(&self, window: Window) -> X11Result<()> {
        self.connection.map_window(window)?;
        Ok(())
    }

    pub fn unmap(&self, window: Window) -> X11Result<()> {
        self.connection.unmap_window(window)?;
        Ok(())
    }

    pub fn destroy(&self, window: Window) -> X11Result<()> {
        self.connection.destroy_window(window)?;
        Ok(())
    }

    pub fn flush(&self) -> X11Result<()> {
        self.connection.flush()?;
        Ok(())
    }

    fn atom(&self, name: &'static str) -> Atom {
        self.atoms[name]
    }
}

#[allow(dead_code)]
pub fn connect_filesystem_socket(path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(path)
}
