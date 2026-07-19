//! Incremental XWM startup.
//!
//! The protocol structures and request serializers remain x11rb's.  Only the
//! handshake sequencing, reply bookkeeping, and reactor ownership live here.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    os::fd::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
};

use x11rb::{
    connection::Connection,
    cookie::Cookie,
    protocol::{
        composite,
        composite::ConnectionExt as CompositeConnectionExt,
        randr,
        randr::ConnectionExt as RandrConnectionExt,
        shape,
        shape::ConnectionExt as ShapeConnectionExt,
        sync,
        sync::ConnectionExt as SyncConnectionExt,
        xfixes,
        xfixes::ConnectionExt as XfixesConnectionExt,
        xproto::{self, ConnectionExt as XprotoConnectionExt},
    },
    rust_connection::{RustConnection, Stream},
    wrapper::ConnectionExt as WrapperConnectionExt,
    x11_utils::ExtensionInformation,
};
use x11rb_protocol::{SequenceNumber, connect::Connect};

use super::{
    Xwm, XwmStartupError,
    atoms::{XwmAtomName, XwmAtoms},
    capabilities::XwmCapabilities,
    connection::{ReactorStream, X11Connection},
};
use crate::xwayland::XwaylandGeneration;

const MAX_SETUP_BYTES: usize = 64 * 1024;
const MAX_STARTUP_REPLIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XwmStartupState {
    SocketConnected,
    SetupReceived,
    ExtensionsDiscovered,
    AtomsInterned,
    RootClaimed,
    ExistingWindowsAdopted,
    Running,
}

#[derive(Debug, Clone, Copy)]
enum PendingVersion {
    Composite,
    Xfixes,
    Shape,
    Randr,
    Sync,
}

#[derive(Debug)]
struct SetupReader {
    connect: Connect,
    outbound: Vec<u8>,
    written: usize,
}

#[derive(Debug)]
pub(crate) struct XwmStartup {
    generation: XwaylandGeneration,
    stream: Option<ReactorStream>,
    setup_reader: Option<SetupReader>,
    connection: Option<X11Connection>,
    state: XwmStartupState,
    pending_extensions: BTreeMap<SequenceNumber, &'static str>,
    extensions: HashMap<&'static str, ExtensionInformation>,
    pending_versions: BTreeMap<SequenceNumber, PendingVersion>,
    pending_atoms: BTreeMap<SequenceNumber, XwmAtomName>,
    atoms: Option<XwmAtoms>,
    capabilities: Option<XwmCapabilities>,
    root: Option<u32>,
    supporting_wm_check: Option<u32>,
    pending_tree: Option<SequenceNumber>,
    adopted_windows: Vec<u32>,
    last_error: Option<String>,
}

impl XwmStartup {
    pub(crate) fn new(
        generation: XwaylandGeneration,
        stream: UnixStream,
    ) -> Result<Self, XwmStartupError> {
        let stream = ReactorStream::from_unix_stream(stream)
            .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
        let (connect, outbound) = Connect::with_authorization(Vec::new(), Vec::new());
        if outbound.len() > MAX_SETUP_BYTES {
            return Err(XwmStartupError::Protocol(
                "X11 setup request is oversized".to_owned(),
            ));
        }
        Ok(Self {
            generation,
            stream: Some(stream),
            setup_reader: Some(SetupReader {
                connect,
                outbound,
                written: 0,
            }),
            connection: None,
            state: XwmStartupState::SocketConnected,
            pending_extensions: BTreeMap::new(),
            extensions: HashMap::new(),
            pending_versions: BTreeMap::new(),
            pending_atoms: BTreeMap::new(),
            atoms: None,
            capabilities: None,
            root: None,
            supporting_wm_check: None,
            pending_tree: None,
            adopted_windows: Vec::new(),
            last_error: None,
        })
    }

    pub(crate) fn state(&self) -> XwmStartupState {
        self.state
    }

    pub(crate) fn raw_fd(&self) -> Option<RawFd> {
        self.stream.as_ref().map(AsRawFd::as_raw_fd).or_else(|| {
            self.connection
                .as_ref()
                .map(|connection| connection.stream().as_raw_fd())
        })
    }

    pub(crate) fn wants_writable(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(ReactorStream::wants_writable)
            || self
                .connection
                .as_ref()
                .is_some_and(|connection| connection.stream().wants_writable())
    }

    pub(crate) fn flush_output(&self) -> io::Result<bool> {
        if let Some(stream) = self.stream.as_ref() {
            return stream.flush_pending();
        }
        if let Some(connection) = self.connection.as_ref() {
            return connection.stream().flush_pending();
        }
        Ok(false)
    }

    pub(crate) fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Progress by one bounded batch. `Ok(None)` means the caller should wait
    /// for another reactor edge; it never means that the native loop should
    /// wait synchronously.
    pub(crate) fn progress(&mut self) -> Result<Option<Xwm>, XwmStartupError> {
        let result = self.progress_inner();
        if let Err(error) = &result {
            self.last_error = Some(error.to_string());
        }
        result
    }

    fn progress_inner(&mut self) -> Result<Option<Xwm>, XwmStartupError> {
        match self.state {
            XwmStartupState::SocketConnected => {
                if !self.drive_setup()? {
                    return Ok(None);
                }
                self.state = XwmStartupState::SetupReceived;
                self.begin_extensions()?;
                Ok(None)
            }
            XwmStartupState::SetupReceived => {
                if !self.complete_extensions()? {
                    return Ok(None);
                }
                self.state = XwmStartupState::ExtensionsDiscovered;
                self.begin_versions()?;
                Ok(None)
            }
            XwmStartupState::ExtensionsDiscovered => {
                if !self.complete_versions()? {
                    return Ok(None);
                }
                self.begin_atoms()?;
                Ok(None)
            }
            XwmStartupState::AtomsInterned => {
                if !self.pending_atoms.is_empty() && !self.complete_atoms()? {
                    return Ok(None);
                }
                let connection = self.connection.as_ref().ok_or_else(|| {
                    XwmStartupError::Protocol("XWM connection disappeared".to_owned())
                })?;
                let root = connection
                    .setup()
                    .roots
                    .first()
                    .ok_or(XwmStartupError::InvalidScreen)?
                    .root;
                let atoms = self
                    .atoms
                    .as_ref()
                    .ok_or_else(|| XwmStartupError::Protocol("XWM atoms disappeared".to_owned()))?;
                let capabilities = self.capabilities.as_ref().ok_or_else(|| {
                    XwmStartupError::Protocol("XWM capabilities disappeared".to_owned())
                })?;
                let supporting = setup_root(connection, root, atoms, capabilities)?;
                self.root = Some(root);
                self.supporting_wm_check = Some(supporting);
                connection
                    .flush()
                    .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
                let cookie = connection
                    .query_tree(root)
                    .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
                self.pending_tree = Some(cookie.sequence_number());
                std::mem::forget(cookie);
                self.state = XwmStartupState::RootClaimed;
                Ok(None)
            }
            XwmStartupState::RootClaimed => {
                let Some(sequence) = self.pending_tree else {
                    return Err(XwmStartupError::Protocol(
                        "missing QueryTree reply".to_owned(),
                    ));
                };
                let Some(connection) = self.connection.as_ref() else {
                    return Err(XwmStartupError::Protocol(
                        "XWM connection disappeared".to_owned(),
                    ));
                };
                let cookie =
                    Cookie::<X11Connection, xproto::QueryTreeReply>::new(connection, sequence);
                let reply = match cookie.reply_unchecked() {
                    Ok(Some(reply)) => reply,
                    Ok(None) => {
                        return Err(XwmStartupError::Protocol(
                            "malformed QueryTree reply".to_owned(),
                        ));
                    }
                    Err(x11rb::errors::ConnectionError::IoError(error))
                        if error.kind() == io::ErrorKind::WouldBlock =>
                    {
                        return Ok(None);
                    }
                    Err(error) => return Err(XwmStartupError::Protocol(error.to_string())),
                };
                self.adopted_windows = reply
                    .children
                    .iter()
                    .copied()
                    .take(MAX_STARTUP_REPLIES)
                    .collect();
                self.pending_tree = None;
                self.state = XwmStartupState::ExistingWindowsAdopted;
                Ok(self.finish())
            }
            XwmStartupState::ExistingWindowsAdopted => Ok(self.finish()),
            XwmStartupState::Running => Ok(None),
        }
    }

    fn drive_setup(&mut self) -> Result<bool, XwmStartupError> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| XwmStartupError::Protocol("XWM setup stream disappeared".to_owned()))?;
        let reader = self
            .setup_reader
            .as_mut()
            .ok_or_else(|| XwmStartupError::Protocol("XWM setup reader disappeared".to_owned()))?;
        if reader.written < reader.outbound.len() {
            match stream.write(&reader.outbound[reader.written..], &mut Vec::new()) {
                Ok(written) => reader.written = reader.written.saturating_add(written),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(XwmStartupError::Protocol(error.to_string())),
            }
        }
        if reader.written != reader.outbound.len() {
            return Ok(false);
        }
        let setup_complete = match stream.read(reader.connect.buffer(), &mut Vec::new()) {
            Ok(0) => {
                return Err(XwmStartupError::Protocol(
                    "XWM closed during setup".to_owned(),
                ));
            }
            Ok(read) => reader.connect.advance(read),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(XwmStartupError::Protocol(error.to_string())),
        };
        if setup_complete {
            let reader = self.setup_reader.take().ok_or_else(|| {
                XwmStartupError::Protocol("XWM setup reader disappeared".to_owned())
            })?;
            let setup = reader
                .connect
                .into_setup()
                .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
            if setup.roots.is_empty() {
                return Err(XwmStartupError::InvalidScreen);
            }
            let stream = self.stream.take().ok_or_else(|| {
                XwmStartupError::Protocol("XWM setup stream disappeared".to_owned())
            })?;
            let connection = RustConnection::for_connected_stream(stream, setup)
                .map_err(XwmStartupError::Connection)?;
            self.connection = Some(X11Connection::new(connection, HashMap::new()));
            return Ok(true);
        }
        Ok(false)
    }

    fn begin_extensions(&mut self) -> Result<(), XwmStartupError> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| XwmStartupError::Protocol("missing XWM connection".to_owned()))?;
        for name in [
            composite::X11_EXTENSION_NAME,
            xfixes::X11_EXTENSION_NAME,
            shape::X11_EXTENSION_NAME,
            randr::X11_EXTENSION_NAME,
            sync::X11_EXTENSION_NAME,
        ] {
            let cookie = connection
                .query_extension(name.as_bytes())
                .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
            let sequence = cookie.sequence_number();
            std::mem::forget(cookie);
            self.pending_extensions.insert(sequence, name);
        }
        connection
            .flush()
            .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
        Ok(())
    }

    fn complete_extensions(&mut self) -> Result<bool, XwmStartupError> {
        let Some(connection) = self.connection.as_ref() else {
            return Err(XwmStartupError::Protocol(
                "missing XWM connection".to_owned(),
            ));
        };
        let sequences = self.pending_extensions.keys().copied().collect::<Vec<_>>();
        for sequence in sequences {
            let Some(name) = self.pending_extensions.get(&sequence).copied() else {
                continue;
            };
            let cookie =
                Cookie::<X11Connection, xproto::QueryExtensionReply>::new(connection, sequence);
            let reply = match cookie.reply_unchecked() {
                Ok(Some(reply)) => reply,
                Ok(None) => {
                    return Err(XwmStartupError::Protocol(
                        "malformed QueryExtension reply".to_owned(),
                    ));
                }
                Err(x11rb::errors::ConnectionError::IoError(error))
                    if error.kind() == io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(error) => return Err(XwmStartupError::Protocol(error.to_string())),
            };
            self.pending_extensions.remove(&sequence);
            if !reply.present {
                if name == composite::X11_EXTENSION_NAME {
                    return Err(XwmStartupError::MissingRequiredExtension(name));
                }
                continue;
            }
            self.extensions.insert(
                name,
                ExtensionInformation {
                    major_opcode: reply.major_opcode,
                    first_event: reply.first_event,
                    first_error: reply.first_error,
                },
            );
        }
        if !self.pending_extensions.is_empty() {
            return Ok(false);
        }
        self.connection
            .as_mut()
            .expect("connection retained")
            .set_extensions(self.extensions.clone());
        Ok(true)
    }

    fn begin_versions(&mut self) -> Result<(), XwmStartupError> {
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| XwmStartupError::Protocol("missing XWM connection".to_owned()))?;
        if self.extensions.contains_key(composite::X11_EXTENSION_NAME) {
            let cookie = connection
                .composite_query_version(0, 4)
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            self.pending_versions
                .insert(cookie.sequence_number(), PendingVersion::Composite);
            std::mem::forget(cookie);
        }
        if self.extensions.contains_key(xfixes::X11_EXTENSION_NAME) {
            let cookie = connection
                .xfixes_query_version(5, 0)
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            self.pending_versions
                .insert(cookie.sequence_number(), PendingVersion::Xfixes);
            std::mem::forget(cookie);
        }
        if self.extensions.contains_key(shape::X11_EXTENSION_NAME) {
            let cookie = connection
                .shape_query_version()
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            self.pending_versions
                .insert(cookie.sequence_number(), PendingVersion::Shape);
            std::mem::forget(cookie);
        }
        if self.extensions.contains_key(randr::X11_EXTENSION_NAME) {
            let cookie = connection
                .randr_query_version(1, 5)
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            self.pending_versions
                .insert(cookie.sequence_number(), PendingVersion::Randr);
            std::mem::forget(cookie);
        }
        if self.extensions.contains_key(sync::X11_EXTENSION_NAME) {
            let cookie = connection
                .sync_initialize(3, 1)
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            self.pending_versions
                .insert(cookie.sequence_number(), PendingVersion::Sync);
            std::mem::forget(cookie);
        }
        connection
            .flush()
            .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
        Ok(())
    }

    fn complete_versions(&mut self) -> Result<bool, XwmStartupError> {
        let Some(connection) = self.connection.as_ref() else {
            return Err(XwmStartupError::Protocol(
                "missing XWM connection".to_owned(),
            ));
        };
        let sequences = self.pending_versions.keys().copied().collect::<Vec<_>>();
        let mut composite_ok = self.extensions.contains_key(composite::X11_EXTENSION_NAME);
        let mut xfixes_ok = self.extensions.contains_key(xfixes::X11_EXTENSION_NAME);
        let mut shape_ok = self.extensions.contains_key(shape::X11_EXTENSION_NAME);
        let mut randr_ok = self.extensions.contains_key(randr::X11_EXTENSION_NAME);
        let mut sync_ok = self.extensions.contains_key(sync::X11_EXTENSION_NAME);
        for sequence in sequences {
            let Some(kind) = self.pending_versions.get(&sequence).copied() else {
                continue;
            };
            let result = match kind {
                PendingVersion::Composite => {
                    Cookie::<X11Connection, composite::QueryVersionReply>::new(connection, sequence)
                        .reply_unchecked()
                        .map(|reply| {
                            reply.map(|r| (u64::from(r.major_version), u64::from(r.minor_version)))
                        })
                }
                PendingVersion::Xfixes => {
                    Cookie::<X11Connection, xfixes::QueryVersionReply>::new(connection, sequence)
                        .reply_unchecked()
                        .map(|reply| {
                            reply.map(|r| (u64::from(r.major_version), u64::from(r.minor_version)))
                        })
                }
                PendingVersion::Shape => {
                    Cookie::<X11Connection, shape::QueryVersionReply>::new(connection, sequence)
                        .reply_unchecked()
                        .map(|reply| {
                            reply.map(|r| (u64::from(r.major_version), u64::from(r.minor_version)))
                        })
                }
                PendingVersion::Randr => {
                    Cookie::<X11Connection, randr::QueryVersionReply>::new(connection, sequence)
                        .reply_unchecked()
                        .map(|reply| {
                            reply.map(|r| (u64::from(r.major_version), u64::from(r.minor_version)))
                        })
                }
                PendingVersion::Sync => {
                    Cookie::<X11Connection, sync::InitializeReply>::new(connection, sequence)
                        .reply_unchecked()
                        .map(|reply| {
                            reply.map(|r| (u64::from(r.major_version), u64::from(r.minor_version)))
                        })
                }
            };
            let version = match result {
                Ok(Some(version)) => version,
                Ok(None) => {
                    return Err(XwmStartupError::Protocol(
                        "malformed extension version reply".to_owned(),
                    ));
                }
                Err(x11rb::errors::ConnectionError::IoError(error))
                    if error.kind() == io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(_) => {
                    match kind {
                        PendingVersion::Composite => composite_ok = false,
                        PendingVersion::Xfixes => xfixes_ok = false,
                        PendingVersion::Shape => shape_ok = false,
                        PendingVersion::Randr => randr_ok = false,
                        PendingVersion::Sync => sync_ok = false,
                    }
                    self.pending_versions.remove(&sequence);
                    continue;
                }
            };
            let required = match kind {
                PendingVersion::Composite => (0, 4),
                PendingVersion::Xfixes => (5, 0),
                PendingVersion::Shape => (1, 1),
                PendingVersion::Randr => (1, 5),
                PendingVersion::Sync => (3, 1),
            };
            if version < required {
                if matches!(kind, PendingVersion::Composite) {
                    return Err(XwmStartupError::Protocol(
                        "Composite negotiated below the required 0.4 contract".to_owned(),
                    ));
                }
                match kind {
                    PendingVersion::Xfixes => xfixes_ok = false,
                    PendingVersion::Shape => shape_ok = false,
                    PendingVersion::Randr => randr_ok = false,
                    PendingVersion::Sync => sync_ok = false,
                    PendingVersion::Composite => unreachable!(),
                }
            }
            self.pending_versions.remove(&sequence);
        }
        if !self.pending_versions.is_empty() {
            return Ok(false);
        }
        self.capabilities = Some(XwmCapabilities {
            composite: composite_ok,
            xfixes: xfixes_ok,
            shape: shape_ok,
            randr: randr_ok,
            sync: sync_ok,
        });
        self.begin_atoms()?;
        self.state = XwmStartupState::AtomsInterned;
        Ok(false)
    }

    fn begin_atoms(&mut self) -> Result<(), XwmStartupError> {
        if !self.pending_atoms.is_empty() || self.atoms.is_some() {
            return Ok(());
        }
        let connection = self
            .connection
            .as_ref()
            .ok_or_else(|| XwmStartupError::Protocol("missing XWM connection".to_owned()))?;
        for name in XwmAtomName::ALL {
            let cookie = connection
                .intern_atom(false, name.as_bytes())
                .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
            let sequence = cookie.sequence_number();
            std::mem::forget(cookie);
            self.pending_atoms.insert(sequence, *name);
        }
        connection
            .flush()
            .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
        Ok(())
    }

    fn complete_atoms(&mut self) -> Result<bool, XwmStartupError> {
        let Some(connection) = self.connection.as_ref() else {
            return Err(XwmStartupError::Protocol(
                "missing XWM connection".to_owned(),
            ));
        };
        let sequences = self.pending_atoms.keys().copied().collect::<Vec<_>>();
        let mut values = self
            .atoms
            .take()
            .map(|atoms| atoms.into_values())
            .unwrap_or_default();
        for sequence in sequences {
            let Some(name) = self.pending_atoms.get(&sequence).copied() else {
                continue;
            };
            let cookie =
                Cookie::<X11Connection, xproto::InternAtomReply>::new(connection, sequence);
            let reply = match cookie.reply_unchecked() {
                Ok(Some(reply)) => reply,
                Ok(None) => {
                    return Err(XwmStartupError::Protocol(
                        "malformed InternAtom reply".to_owned(),
                    ));
                }
                Err(x11rb::errors::ConnectionError::IoError(error))
                    if error.kind() == io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(error) => return Err(XwmStartupError::Protocol(error.to_string())),
            };
            self.pending_atoms.remove(&sequence);
            values.insert(name, reply.atom);
        }
        if !self.pending_atoms.is_empty() {
            self.atoms = Some(XwmAtoms::from_values(values));
            return Ok(false);
        }
        self.atoms = Some(XwmAtoms::from_values(values));
        self.state = XwmStartupState::AtomsInterned;
        Ok(true)
    }

    fn finish(&mut self) -> Option<Xwm> {
        let connection = self.connection.take()?;
        let atoms = self.atoms.take()?;
        let capabilities = self.capabilities.take()?;
        let root = self.root?;
        let supporting_wm_check = self.supporting_wm_check?;
        let raw_fd = connection.stream().as_raw_fd();
        let mut xwm = Xwm {
            generation: self.generation,
            connection,
            adoption: Default::default(),
            screen_number: 0,
            root,
            atoms,
            capabilities,
            windows: super::window::X11WindowRegistry::default(),
            outgoing_events: Default::default(),
            association: super::association::SurfaceAssociationJoin::default(),
            resize_sync: super::resize_sync::ResizeSyncTracker::default(),
            sync_alarms: Default::default(),
            sync_handles_by_counter: Default::default(),
            next_resize_counter_values: Default::default(),
            shapes: Default::default(),
            data_bridge: Default::default(),
            pending_properties: Default::default(),
            deferred_properties: Default::default(),
            property_metrics: Default::default(),
            buffer_ready_surfaces: Default::default(),
            supporting_wm_check,
            raw_fd,
        };
        for xid in self.adopted_windows.drain(..) {
            let handle = crate::xwayland::X11WindowHandle::new(self.generation, xid);
            let _ = xwm.observe_window(handle);
        }
        self.state = XwmStartupState::Running;
        Some(xwm)
    }
}

pub(crate) fn setup_root<C: Connection>(
    connection: &C,
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
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    connection
        .composite_redirect_subwindows(root, composite::Redirect::MANUAL)
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    let screen = connection
        .setup()
        .roots
        .first()
        .ok_or(XwmStartupError::InvalidScreen)?;
    let supporting_wm_check = connection
        .generate_id()
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
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
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(XwmAtomName::NetSupportingWmCheck),
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            atoms.get(XwmAtomName::NetSupportingWmCheck),
            xproto::AtomEnum::WINDOW,
            &[supporting_wm_check],
        )
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    connection
        .change_property8(
            xproto::PropMode::REPLACE,
            supporting_wm_check,
            atoms.get(XwmAtomName::NetWmName),
            atoms.get(XwmAtomName::Utf8String),
            b"Typhon",
        )
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    let supported = XwmAtoms::advertised_names()
        .iter()
        .map(|name| atoms.get(*name))
        .collect::<Vec<_>>();
    connection
        .change_property32(
            xproto::PropMode::REPLACE,
            root,
            atoms.get(XwmAtomName::NetSupported),
            xproto::AtomEnum::ATOM,
            &supported,
        )
        .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    for (atom, ty) in [
        (XwmAtomName::NetActiveWindow, xproto::AtomEnum::WINDOW),
        (XwmAtomName::NetClientList, xproto::AtomEnum::WINDOW),
        (XwmAtomName::NetClientListStacking, xproto::AtomEnum::WINDOW),
    ] {
        connection
            .change_property32(xproto::PropMode::REPLACE, root, atoms.get(atom), ty, &[])
            .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    }
    let width = u32::from(screen.width_in_pixels);
    let height = u32::from(screen.height_in_pixels);
    for (atom, values) in [
        (XwmAtomName::NetNumberOfDesktops, vec![1]),
        (XwmAtomName::NetCurrentDesktop, vec![0]),
        (XwmAtomName::NetDesktopGeometry, vec![width, height]),
        (XwmAtomName::NetDesktopViewport, vec![0, 0]),
        (XwmAtomName::NetWorkarea, vec![0, 0, width, height]),
    ] {
        connection
            .change_property32(
                xproto::PropMode::REPLACE,
                root,
                atoms.get(atom),
                xproto::AtomEnum::CARDINAL,
                &values,
            )
            .map_err(|e| XwmStartupError::Protocol(e.to_string()))?;
    }
    Ok(supporting_wm_check)
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU64, os::unix::net::UnixStream, time::Instant};

    use super::*;

    #[test]
    fn setup_progress_never_waits_for_the_server() {
        let (stream, _peer) = UnixStream::pair().expect("socket pair");
        let generation = XwaylandGeneration::new(NonZeroU64::new(1).expect("nonzero"));
        let mut startup = XwmStartup::new(generation, stream).expect("startup driver");
        let started = Instant::now();
        assert!(startup.progress().expect("pending setup").is_none());
        assert!(started.elapsed().as_millis() < 100);
        assert_eq!(startup.state(), XwmStartupState::SocketConnected);
    }
}
