//! Bounded, generation-bound X11 property discovery.
//!
//! Requests are queued as X11 cookies and consumed only from the XWM reactor
//! drain.  The compositor never asks this module to wait for a reply.

use x11rb::{
    connection::{DiscardMode, RequestConnection, RequestKind, SequenceNumber},
    cookie::Cookie,
    protocol::xproto::{self, AtomEnum, ConnectionExt as XprotoConnectionExt},
};

use super::{
    X11PublishedState, X11WindowHandle, Xwm, XwmError,
    atoms::XwmAtomName,
    window::{X11PropertySnapshot, X11WindowRecord, X11WindowType},
};

pub(crate) const MAX_TEXT_PROPERTY_BYTES: usize = 64 * 1024;
const MAX_PENDING_PROPERTIES: usize = 256;
const MAX_PENDING_PROPERTIES_PER_WINDOW: usize = 32;
const MAX_PROPERTY_ITEMS_TEXT: u32 = (MAX_TEXT_PROPERTY_BYTES / 4) as u32;
const MAX_PROPERTY_ITEMS_SCALAR: u32 = 64;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PropertyMetrics {
    pub(crate) requested: u64,
    pub(crate) completed: u64,
    pub(crate) coalesced: u64,
    pub(crate) rejected: u64,
    pub(crate) stale: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PropertyKind {
    NetWmName,
    WmName,
    WmClass,
    NetWmPid,
    NetWmWindowType,
    WmTransientFor,
    WmNormalHints,
    WmHints,
    WmProtocols,
    NetWmSyncRequestCounter,
    NetWmState,
    MotifWmHints,
}

impl PropertyKind {
    const ALL: [Self; 12] = [
        Self::NetWmName,
        Self::WmName,
        Self::WmClass,
        Self::NetWmPid,
        Self::NetWmWindowType,
        Self::WmTransientFor,
        Self::WmNormalHints,
        Self::WmHints,
        Self::WmProtocols,
        Self::NetWmSyncRequestCounter,
        Self::NetWmState,
        Self::MotifWmHints,
    ];

    const fn bit(self) -> u16 {
        match self {
            Self::NetWmName => 1 << 0,
            Self::WmName => 1 << 1,
            Self::WmClass => 1 << 2,
            Self::NetWmPid => 1 << 3,
            Self::NetWmWindowType => 1 << 4,
            Self::WmTransientFor => 1 << 5,
            Self::WmNormalHints => 1 << 6,
            Self::WmHints => 1 << 7,
            Self::WmProtocols => 1 << 8,
            Self::NetWmSyncRequestCounter => 1 << 9,
            Self::NetWmState => 1 << 10,
            Self::MotifWmHints => 1 << 11,
        }
    }

    fn atom(self, xwm: &Xwm) -> u32 {
        xwm.atoms.get(match self {
            Self::NetWmName => XwmAtomName::NetWmName,
            Self::WmName => XwmAtomName::WmName,
            Self::WmClass => XwmAtomName::WmClass,
            Self::NetWmPid => XwmAtomName::NetWmPid,
            Self::NetWmWindowType => XwmAtomName::NetWmWindowType,
            Self::WmTransientFor => XwmAtomName::WmTransientFor,
            Self::WmNormalHints => XwmAtomName::WmNormalHints,
            Self::WmHints => XwmAtomName::WmHints,
            Self::WmProtocols => XwmAtomName::WmProtocols,
            Self::NetWmSyncRequestCounter => XwmAtomName::NetWmSyncRequestCounter,
            Self::NetWmState => XwmAtomName::NetWmState,
            Self::MotifWmHints => XwmAtomName::MotifWmHints,
        })
    }

    const fn max_items(self) -> u32 {
        match self {
            Self::NetWmName | Self::WmName | Self::WmClass => MAX_PROPERTY_ITEMS_TEXT,
            _ => MAX_PROPERTY_ITEMS_SCALAR,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingProperty {
    pub(crate) sequence: SequenceNumber,
    pub(crate) handle: X11WindowHandle,
    pub(crate) kind: PropertyKind,
    pub(crate) epoch: u64,
}

#[derive(Debug, Clone, PartialEq)]
enum ParsedProperty {
    Text(Option<String>),
    AppId(Option<String>),
    Pid(Option<u32>),
    WindowType(Option<X11WindowType>),
    TransientFor(Option<u32>),
    Constraints(crate::compositor::WindowConstraints),
    Protocols {
        supports_delete: bool,
        supports_take_focus: bool,
    },
    SyncCounter(Option<u64>),
    State(crate::xwayland::xwm::X11PublishedState),
    Hints {
        accepts_input: Option<bool>,
    },
    MotifHints,
}

pub(crate) fn begin_initial(xwm: &mut Xwm, handle: X11WindowHandle) -> Result<(), XwmError> {
    begin_refresh(xwm, handle, true)
}

pub(crate) fn begin_refresh(
    xwm: &mut Xwm,
    handle: X11WindowHandle,
    initial: bool,
) -> Result<(), XwmError> {
    if handle.generation() != xwm.generation {
        return Err(XwmError::StaleGeneration);
    }

    let already_pending = xwm
        .pending_properties
        .values()
        .any(|pending| pending.handle == handle);
    if already_pending && !initial {
        xwm.property_metrics.coalesced = xwm.property_metrics.coalesced.saturating_add(1);
        return Ok(());
    }
    cancel(xwm, handle);

    let Some(record) = xwm.windows.get_mut(handle) else {
        return Ok(());
    };

    record.property_epoch = record.property_epoch.saturating_add(1);
    let epoch = record.property_epoch;
    record.properties = X11PropertySnapshot::default();
    record.resolved_properties = 0;
    record.properties_ready = false;

    for kind in PropertyKind::ALL {
        if pending_for_window(xwm, handle) >= MAX_PENDING_PROPERTIES_PER_WINDOW
            || xwm.pending_properties.len() >= MAX_PENDING_PROPERTIES
        {
            xwm.property_metrics.rejected = xwm.property_metrics.rejected.saturating_add(1);
            resolve_fallback(xwm, handle, kind, epoch);
            continue;
        }
        request(xwm, handle, kind, epoch)?;
    }
    Ok(())
}

pub(crate) fn cancel(xwm: &mut Xwm, handle: X11WindowHandle) {
    let sequences = xwm
        .pending_properties
        .iter()
        .filter_map(|(sequence, pending)| (pending.handle == handle).then_some(*sequence))
        .collect::<Vec<_>>();
    for sequence in sequences {
        xwm.connection.discard_reply(
            sequence,
            RequestKind::HasResponse,
            DiscardMode::DiscardReply,
        );
        xwm.pending_properties.remove(&sequence);
    }
}

pub(crate) fn cancel_generation(xwm: &mut Xwm, generation: super::XwaylandGeneration) {
    let handles = xwm
        .pending_properties
        .values()
        .filter_map(|pending| (pending.handle.generation() == generation).then_some(pending.handle))
        .collect::<std::collections::HashSet<_>>();
    for handle in handles {
        cancel(xwm, handle);
    }
}

pub(crate) fn poll_replies(xwm: &mut Xwm, budget: usize) -> Result<usize, XwmError> {
    let sequences = xwm
        .pending_properties
        .keys()
        .copied()
        .take(budget)
        .collect::<Vec<_>>();
    let mut completed = 0;
    for sequence in sequences {
        let Some(pending) = xwm.pending_properties.remove(&sequence) else {
            continue;
        };
        debug_assert_eq!(pending.sequence, sequence);
        let cookie = Cookie::<_, xproto::GetPropertyReply>::new(&xwm.connection, sequence);
        let reply = match cookie.reply_unchecked() {
            Ok(reply) => reply,
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                xwm.pending_properties.insert(sequence, pending);
                continue;
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        let Some(reply) = reply else {
            resolve_fallback(xwm, pending.handle, pending.kind, pending.epoch);
            xwm.property_metrics.completed = xwm.property_metrics.completed.saturating_add(1);
            completed += 1;
            continue;
        };
        let parsed = parse(pending.kind, &reply, xwm);
        if parsed.is_none() {
            xwm.property_metrics.rejected = xwm.property_metrics.rejected.saturating_add(1);
        }
        let parsed = parsed.or_else(|| Some(fallback_for(pending.kind)));
        apply(xwm, pending, parsed);
        xwm.emit_ready_if_complete(pending.handle)?;
        xwm.property_metrics.completed = xwm.property_metrics.completed.saturating_add(1);
        completed += 1;
    }
    Ok(completed)
}

pub(crate) fn socket_has_input(fd: std::os::fd::RawFd) -> bool {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` points to one initialized entry and the timeout is zero.
    unsafe { libc::poll(&mut pollfd, 1, 0) > 0 && pollfd.revents & libc::POLLIN != 0 }
}

fn request(
    xwm: &mut Xwm,
    handle: X11WindowHandle,
    kind: PropertyKind,
    epoch: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .get_property(
            false,
            handle.xid(),
            kind.atom(xwm),
            AtomEnum::ANY,
            0,
            kind.max_items(),
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    // A cookie owns the pending-reply discard behavior.  The sequence is
    // retained explicitly and reconstructed only when the reactor drains it.
    std::mem::forget(cookie);
    xwm.pending_properties.insert(
        sequence,
        PendingProperty {
            sequence,
            handle,
            kind,
            epoch,
        },
    );
    xwm.property_metrics.requested = xwm.property_metrics.requested.saturating_add(1);
    Ok(())
}

fn pending_for_window(xwm: &Xwm, handle: X11WindowHandle) -> usize {
    xwm.pending_properties
        .values()
        .filter(|pending| pending.handle == handle)
        .count()
}

fn resolve_fallback(xwm: &mut Xwm, handle: X11WindowHandle, kind: PropertyKind, epoch: u64) {
    let Some(record) = xwm.windows.get_mut(handle) else {
        return;
    };
    if record.property_epoch != epoch {
        return;
    }
    apply_parsed(record, handle, kind, fallback_for(kind));
    if matches!(kind, PropertyKind::NetWmName | PropertyKind::WmName) {
        record.properties.title = record
            .properties
            .net_wm_name
            .clone()
            .or_else(|| record.properties.wm_name.clone());
    }
    record.resolved_properties |= kind.bit();
    record.properties_ready = record.resolved_properties == all_mask();
}

fn apply(xwm: &mut Xwm, pending: PendingProperty, parsed: Option<ParsedProperty>) {
    let Some(record) = xwm.windows.get_mut(pending.handle) else {
        return;
    };
    if record.property_epoch != pending.epoch {
        xwm.property_metrics.stale = xwm.property_metrics.stale.saturating_add(1);
        return;
    }
    let was_admitted = record.snapshot.is_some();
    if let Some(parsed) = parsed {
        apply_parsed(record, pending.handle, pending.kind, parsed);
    }
    record.resolved_properties |= pending.kind.bit();
    if pending.kind == PropertyKind::NetWmName || pending.kind == PropertyKind::WmName {
        record.properties.title = record
            .properties
            .net_wm_name
            .clone()
            .or_else(|| record.properties.wm_name.clone());
    }
    record.properties_ready = record.resolved_properties == all_mask();
    if was_admitted {
        let metadata_delta = match pending.kind {
            PropertyKind::NetWmName | PropertyKind::WmName => Some(super::X11MetadataDelta::Title(
                record.properties.title.clone(),
            )),
            PropertyKind::WmClass => Some(super::X11MetadataDelta::AppId(
                record.properties.app_id.clone(),
            )),
            PropertyKind::NetWmPid => Some(super::X11MetadataDelta::Pid(record.properties.pid)),
            PropertyKind::WmNormalHints => Some(super::X11MetadataDelta::Constraints(
                record.properties.constraints,
            )),
            PropertyKind::WmTransientFor => Some(super::X11MetadataDelta::TransientFor(
                record.properties.transient_for,
            )),
            PropertyKind::WmProtocols => Some(super::X11MetadataDelta::Protocols {
                supports_delete: record.properties.supports_delete,
                supports_take_focus: record.properties.supports_take_focus,
            }),
            _ => None,
        };
        if let Some(snapshot) = record.snapshot.as_mut() {
            snapshot.metadata.title = record.properties.title.clone();
            snapshot.metadata.app_id = record.properties.app_id.clone();
            snapshot.metadata.pid = record.properties.pid;
            snapshot.constraints = record.properties.constraints;
            snapshot.transient_for = record.properties.transient_for;
            snapshot.supports_delete = record.properties.supports_delete;
            snapshot.supports_take_focus = record.properties.supports_take_focus;
            snapshot.sync_counter = record.properties.sync_counter;
            snapshot.state = record.properties.state;
        }
        if let Some(delta) = metadata_delta {
            xwm.outgoing_events
                .push_back(super::XwmEvent::MetadataChanged {
                    window: pending.handle,
                    delta,
                });
        }
    }
}

fn apply_parsed(
    record: &mut X11WindowRecord,
    handle: X11WindowHandle,
    kind: PropertyKind,
    parsed: ParsedProperty,
) {
    match parsed {
        ParsedProperty::Text(value) => match kind {
            PropertyKind::NetWmName => record.properties.net_wm_name = value,
            PropertyKind::WmName => record.properties.wm_name = value,
            _ => {}
        },
        ParsedProperty::AppId(value) => record.properties.app_id = value,
        ParsedProperty::Pid(value) => record.properties.pid = value,
        ParsedProperty::WindowType(value) => record.properties.window_type = value,
        ParsedProperty::TransientFor(value) => {
            record.properties.transient_for =
                value.map(|xid| X11WindowHandle::new(handle.generation(), xid));
        }
        ParsedProperty::Constraints(value) => record.properties.constraints = value,
        ParsedProperty::Protocols {
            supports_delete,
            supports_take_focus,
        } => {
            record.properties.supports_delete = supports_delete;
            record.properties.supports_take_focus = supports_take_focus;
        }
        ParsedProperty::SyncCounter(value) => record.properties.sync_counter = value,
        ParsedProperty::State(value) => record.properties.state = value,
        ParsedProperty::Hints { accepts_input } => record.properties.accepts_input = accepts_input,
        ParsedProperty::MotifHints => {}
    }
}

fn fallback_for(kind: PropertyKind) -> ParsedProperty {
    match kind {
        PropertyKind::NetWmName | PropertyKind::WmName => ParsedProperty::Text(None),
        PropertyKind::WmClass => ParsedProperty::AppId(None),
        PropertyKind::NetWmPid => ParsedProperty::Pid(None),
        PropertyKind::NetWmWindowType => ParsedProperty::WindowType(None),
        PropertyKind::WmTransientFor => ParsedProperty::TransientFor(None),
        PropertyKind::WmNormalHints => ParsedProperty::Constraints(Default::default()),
        PropertyKind::WmHints => ParsedProperty::Hints {
            accepts_input: None,
        },
        PropertyKind::WmProtocols => ParsedProperty::Protocols {
            supports_delete: false,
            supports_take_focus: false,
        },
        PropertyKind::NetWmSyncRequestCounter => ParsedProperty::SyncCounter(None),
        PropertyKind::NetWmState => ParsedProperty::State(Default::default()),
        PropertyKind::MotifWmHints => ParsedProperty::MotifHints,
    }
}

fn parse(
    kind: PropertyKind,
    reply: &xproto::GetPropertyReply,
    xwm: &Xwm,
) -> Option<ParsedProperty> {
    if reply.type_ == x11rb::NONE || reply.value.is_empty() {
        return Some(fallback_for(kind));
    }
    if reply.bytes_after != 0 {
        return None;
    }
    match kind {
        PropertyKind::NetWmName | PropertyKind::WmName => {
            (reply.format == 8).then(|| ParsedProperty::Text(parse_text(&reply.value)))
        }
        PropertyKind::WmClass => {
            (reply.format == 8).then(|| ParsedProperty::AppId(parse_app_id(&reply.value)))
        }
        PropertyKind::NetWmPid => parse_u32s(reply)
            .and_then(|values| (values.len() == 1).then(|| ParsedProperty::Pid(Some(values[0])))),
        PropertyKind::NetWmWindowType => parse_window_type(reply, xwm),
        PropertyKind::WmTransientFor => parse_u32s(reply).and_then(|values| {
            (values.len() == 1).then(|| ParsedProperty::TransientFor(Some(values[0])))
        }),
        PropertyKind::WmNormalHints => parse_normal_hints(reply),
        PropertyKind::WmHints => parse_wm_hints(reply),
        PropertyKind::WmProtocols => parse_protocols(reply, xwm),
        PropertyKind::NetWmSyncRequestCounter => parse_u32s(reply).and_then(|values| {
            (values.len() == 1).then(|| ParsedProperty::SyncCounter(Some(u64::from(values[0]))))
        }),
        PropertyKind::NetWmState => parse_state(reply, xwm),
        PropertyKind::MotifWmHints => (reply.format == 32).then_some(ParsedProperty::MotifHints),
    }
}

fn parse_window_type(reply: &xproto::GetPropertyReply, xwm: &Xwm) -> Option<ParsedProperty> {
    let values = parse_u32s(reply)?;
    let atom = *values.first()?;
    let window_type = if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeNormal) {
        X11WindowType::Normal
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeDialog) {
        X11WindowType::Dialog
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeUtility) {
        X11WindowType::Utility
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeMenu) {
        X11WindowType::Menu
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypePopupMenu) {
        X11WindowType::PopupMenu
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeDropdownMenu) {
        X11WindowType::DropdownMenu
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeTooltip) {
        X11WindowType::Tooltip
    } else if atom == xwm.atoms.get(XwmAtomName::NetWmWindowTypeNotification) {
        X11WindowType::Notification
    } else {
        X11WindowType::Other(atom)
    };
    Some(ParsedProperty::WindowType(Some(window_type)))
}

fn parse_wm_hints(reply: &xproto::GetPropertyReply) -> Option<ParsedProperty> {
    let values = parse_u32s(reply)?;
    let flags = *values.first()?;
    Some(ParsedProperty::Hints {
        accepts_input: (flags & 1 != 0).then(|| values.get(1).copied().unwrap_or(0) != 0),
    })
}

fn parse_text(value: &[u8]) -> Option<String> {
    if value.is_empty() || value.len() > MAX_TEXT_PROPERTY_BYTES {
        return None;
    }
    let value = value.split(|byte| *byte == 0).next().unwrap_or_default();
    (!value.is_empty()).then(|| String::from_utf8_lossy(value).into_owned())
}

fn parse_app_id(value: &[u8]) -> Option<String> {
    if value.len() > MAX_TEXT_PROPERTY_BYTES {
        return None;
    }
    let fields = value
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    fields
        .get(1)
        .or_else(|| fields.first())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .filter(|value| !value.is_empty())
}

fn parse_u32s(reply: &xproto::GetPropertyReply) -> Option<Vec<u32>> {
    if reply.format != 32 || !reply.value.len().is_multiple_of(4) {
        return None;
    }
    Some(
        reply
            .value
            .chunks_exact(4)
            .map(|chunk| u32::from_ne_bytes(chunk.try_into().expect("chunk is four bytes")))
            .collect(),
    )
}

fn parse_normal_hints(reply: &xproto::GetPropertyReply) -> Option<ParsedProperty> {
    let values = parse_u32s(reply)?;
    if values.len() < 18 {
        return None;
    }
    let mut constraints = crate::compositor::WindowConstraints::default();
    if values[0] & (1 << 4) != 0 {
        constraints.min_width = nonzero(values[5]);
        constraints.min_height = nonzero(values[6]);
    }
    if values[0] & (1 << 5) != 0 {
        constraints.max_width = nonzero(values[7]);
        constraints.max_height = nonzero(values[8]);
    }
    if values[0] & (1 << 6) != 0 {
        constraints.width_increment = nonzero(values[9]);
        constraints.height_increment = nonzero(values[10]);
    }
    if values[0] & (1 << 7) != 0 {
        constraints.min_aspect = ratio(values[11], values[12]);
        constraints.max_aspect = ratio(values[13], values[14]);
    }
    if values[0] & (1 << 8) != 0 {
        constraints.base_width = nonzero(values[15]);
        constraints.base_height = nonzero(values[16]);
    }
    Some(ParsedProperty::Constraints(constraints))
}

fn nonzero(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn ratio(numerator: u32, denominator: u32) -> Option<f64> {
    (denominator != 0).then_some(f64::from(numerator) / f64::from(denominator))
}

fn parse_protocols(reply: &xproto::GetPropertyReply, xwm: &Xwm) -> Option<ParsedProperty> {
    let values = parse_u32s(reply)?;
    Some(ParsedProperty::Protocols {
        supports_delete: values.contains(&xwm.atoms.get(XwmAtomName::WmDeleteWindow)),
        supports_take_focus: values.contains(&xwm.atoms.get(XwmAtomName::WmTakeFocus)),
    })
}

fn parse_state(reply: &xproto::GetPropertyReply, xwm: &Xwm) -> Option<ParsedProperty> {
    let values = parse_u32s(reply)?;
    Some(ParsedProperty::State(X11PublishedState {
        fullscreen: values.contains(&xwm.atoms.get(XwmAtomName::NetWmStateFullscreen)),
        maximized: values.contains(&xwm.atoms.get(XwmAtomName::NetWmStateMaximizedVert))
            || values.contains(&xwm.atoms.get(XwmAtomName::NetWmStateMaximizedHorz)),
        hidden: values.contains(&xwm.atoms.get(XwmAtomName::NetWmStateHidden)),
        activated: false,
    }))
}

fn all_mask() -> u16 {
    PropertyKind::ALL
        .iter()
        .fold(0, |mask, kind| mask | kind.bit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_mask_covers_every_initial_property() {
        assert_eq!(all_mask().count_ones(), PropertyKind::ALL.len() as u32);
    }

    #[test]
    fn text_and_class_properties_are_bounded_and_lossy() {
        assert_eq!(parse_text("títle".as_bytes()), Some("títle".to_owned()));
        assert_eq!(parse_app_id(b"instance\0QtApp\0"), Some("QtApp".to_owned()));
        assert!(parse_text(&vec![b'x'; MAX_TEXT_PROPERTY_BYTES + 1]).is_none());
    }

    #[test]
    fn normal_hints_parse_constraints_and_aspect() {
        let mut values = [0u32; 18];
        values[0] = (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8);
        values[5] = 640;
        values[6] = 480;
        values[7] = 1920;
        values[8] = 1080;
        values[9] = 8;
        values[10] = 6;
        values[11] = 4;
        values[12] = 3;
        values[13] = 16;
        values[14] = 9;
        values[15] = 640;
        values[16] = 480;
        let reply = xproto::GetPropertyReply {
            format: 32,
            sequence: 0,
            length: 18,
            type_: 1,
            bytes_after: 0,
            value_len: 18,
            value: values
                .iter()
                .flat_map(|value| value.to_ne_bytes())
                .collect(),
        };
        let ParsedProperty::Constraints(constraints) = parse_normal_hints(&reply).unwrap() else {
            panic!("expected constraints")
        };
        assert_eq!(constraints.min_width, Some(640));
        assert_eq!(constraints.width_increment, Some(8));
        assert_eq!(constraints.min_aspect, Some(4.0 / 3.0));
    }

    #[test]
    fn wm_hints_parse_input_flag_without_blocking_defaults() {
        let values = [1u32, 1u32];
        let reply = xproto::GetPropertyReply {
            format: 32,
            sequence: 0,
            length: 2,
            type_: 1,
            bytes_after: 0,
            value_len: 2,
            value: values
                .iter()
                .flat_map(|value| value.to_ne_bytes())
                .collect(),
        };
        assert_eq!(
            parse_wm_hints(&reply),
            Some(ParsedProperty::Hints {
                accepts_input: Some(true)
            })
        );
    }
}
