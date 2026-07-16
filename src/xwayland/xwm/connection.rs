use std::{
    io,
    os::fd::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use x11rb::utils::RawFdContainer;
use x11rb::{
    connection::Connection,
    protocol::composite::ConnectionExt as CompositeConnectionExt,
    protocol::xproto::ConnectionExt as XprotoConnectionExt,
    protocol::{composite, xproto},
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    wrapper::ConnectionExt,
};

use super::{
    Xwm, XwmStartupError, atoms::XwmAtoms, capabilities::XwmCapabilities, window::X11WindowRegistry,
};
use crate::xwayland::XwaylandGeneration;

#[derive(Debug)]
pub(crate) struct ReactorStream {
    inner: DefaultStream,
    nonblocking_poll: Arc<AtomicBool>,
}

impl ReactorStream {
    fn new(inner: DefaultStream) -> (Self, Arc<AtomicBool>) {
        let nonblocking_poll = Arc::new(AtomicBool::new(false));
        (
            Self {
                inner,
                nonblocking_poll: Arc::clone(&nonblocking_poll),
            },
            nonblocking_poll,
        )
    }
}

impl AsRawFd for ReactorStream {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl Stream for ReactorStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        if !self.nonblocking_poll.load(Ordering::Acquire) {
            return self.inner.poll(mode);
        }
        let mut events = 0;
        if mode.readable() {
            events |= libc::POLLIN;
        }
        if mode.writable() {
            events |= libc::POLLOUT;
        }
        let mut pollfd = libc::pollfd {
            fd: self.as_raw_fd(),
            events,
            revents: 0,
        };
        // SAFETY: `pollfd` is initialized and points to one valid descriptor.
        let result = unsafe { libc::poll(&mut pollfd, 1, 0) };
        if result > 0 {
            Ok(())
        } else if result == 0 {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "XWM socket is not ready",
            ))
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn read(&self, buffer: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(buffer, fds)
    }

    fn write(&self, buffer: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(buffer, fds)
    }

    fn write_vectored(
        &self,
        buffers: &[std::io::IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(buffers, fds)
    }
}

pub(crate) fn connect(
    generation: XwaylandGeneration,
    stream: UnixStream,
) -> Result<Xwm, XwmStartupError> {
    let (stream, _) = DefaultStream::from_unix_stream(stream)
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    let (stream, nonblocking_poll) = ReactorStream::new(stream);
    let raw_fd = stream.as_raw_fd();
    let connection =
        RustConnection::connect_to_stream(stream, 0).map_err(XwmStartupError::Connection)?;
    let screen_number = 0;
    let root = connection
        .setup()
        .roots
        .get(screen_number)
        .ok_or(XwmStartupError::InvalidScreen)?
        .root;
    let capabilities = XwmCapabilities::discover(&connection)?;
    let atoms = XwmAtoms::intern(&connection)?;
    let supporting_wm_check = setup_root(&connection, root, &atoms, &capabilities)?;
    connection
        .flush()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    nonblocking_poll.store(true, Ordering::Release);

    Ok(Xwm {
        generation,
        connection,
        screen_number,
        root,
        atoms,
        capabilities,
        windows: X11WindowRegistry::default(),
        outgoing_events: Default::default(),
        association: super::association::SurfaceAssociationJoin::default(),
        resize_sync: super::resize_sync::ResizeSyncTracker::default(),
        sync_alarms: Default::default(),
        sync_handles_by_counter: Default::default(),
        next_resize_counter_values: Default::default(),
        pending_properties: Default::default(),
        property_metrics: Default::default(),
        buffer_ready_surfaces: Default::default(),
        supporting_wm_check,
        raw_fd,
    })
}

fn setup_root<S: Stream>(
    connection: &RustConnection<S>,
    root: u32,
    atoms: &XwmAtoms,
    capabilities: &XwmCapabilities,
) -> Result<u32, XwmStartupError> {
    if !capabilities.composite
        || !capabilities.xfixes
        || !capabilities.shape
        || !capabilities.randr
        || !capabilities.sync
    {
        return Err(XwmStartupError::Protocol(
            "XWM capability record is incomplete".to_owned(),
        ));
    }

    let event_mask = xproto::EventMask::SUBSTRUCTURE_REDIRECT
        | xproto::EventMask::SUBSTRUCTURE_NOTIFY
        | xproto::EventMask::PROPERTY_CHANGE
        | xproto::EventMask::FOCUS_CHANGE;
    connection
        .change_window_attributes(
            root,
            &xproto::ChangeWindowAttributesAux::new().event_mask(event_mask),
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .composite_redirect_subwindows(root, composite::Redirect::MANUAL)
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let screen = connection
        .setup()
        .roots
        .first()
        .ok_or(XwmStartupError::InvalidScreen)?;
    let supporting_wm_check = connection
        .generate_id()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .create_window(
            screen.root_depth,
            supporting_wm_check,
            root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &xproto::CreateWindowAux::new(),
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
        .check()
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let supporting_atom = atoms.get(super::atoms::XwmAtomName::NetSupportingWmCheck);
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            supporting_atom,
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            supporting_atom,
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property8(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            atoms.get(super::atoms::XwmAtomName::NetWmName),
            atoms.get(super::atoms::XwmAtomName::Utf8String),
            b"Typhon",
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;

    let supported = XwmAtoms::advertised_names()
        .iter()
        .map(|name| atoms.get(*name))
        .collect::<Vec<_>>();
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetSupported),
            xproto::AtomEnum::ATOM,
            &supported,
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetActiveWindow),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetClientList),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(super::atoms::XwmAtomName::NetClientListStacking),
            xproto::AtomEnum::WINDOW,
            &[],
        )
        .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
    Ok(supporting_wm_check)
}
