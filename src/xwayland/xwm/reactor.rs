use std::os::fd::RawFd;

use x11rb::{
    cookie::Cookie,
    protocol::xproto::{self, ConnectionExt as XprotoConnectionExt},
};

use super::{Xwm, XwmError, connection};

impl Xwm {
    pub fn raw_fd(&self) -> RawFd {
        self.raw_fd
    }

    pub(crate) fn wants_writable(&self) -> bool {
        self.connection.stream().wants_writable()
    }

    pub(crate) fn flush_output(&self) -> Result<bool, XwmError> {
        self.connection
            .stream()
            .flush_pending()
            .map_err(|error| XwmError::Connection(x11rb::errors::ConnectionError::IoError(error)))
    }

    pub fn screen_number(&self) -> usize {
        self.screen_number
    }

    pub fn root(&self) -> u32 {
        self.root
    }

    pub fn supporting_wm_check(&self) -> u32 {
        self.supporting_wm_check
    }

    pub fn root_event_mask(&self) -> Option<u32> {
        self.root_event_mask.map(u32::from)
    }

    pub(crate) fn start_root_event_mask_probe(&mut self) -> Result<(), XwmError> {
        if self.root_event_mask_probe.is_some() || self.root_event_mask.is_some() {
            return Ok(());
        }
        let cookie = self
            .connection
            .get_window_attributes(self.root)
            .map_err(XwmError::Connection)?;
        self.root_event_mask_probe = Some(cookie.sequence_number());
        std::mem::forget(cookie);
        Ok(())
    }

    pub(super) fn poll_root_event_mask(&mut self) -> Result<(), XwmError> {
        let Some(sequence) = self.root_event_mask_probe else {
            return Ok(());
        };
        let cookie = Cookie::<connection::X11Connection, xproto::GetWindowAttributesReply>::new(
            &self.connection,
            sequence,
        );
        let reply = match cookie.reply_unchecked() {
            Ok(Some(reply)) => reply,
            Ok(None) => return Err(XwmError::InvalidCommand("malformed root attributes reply")),
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                return Ok(());
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        self.root_event_mask_probe = None;
        let required = xproto::EventMask::SUBSTRUCTURE_REDIRECT
            | xproto::EventMask::SUBSTRUCTURE_NOTIFY
            | xproto::EventMask::PROPERTY_CHANGE
            | xproto::EventMask::FOCUS_CHANGE;
        let observed = u32::from(reply.your_event_mask);
        let valid = observed & u32::from(required) == u32::from(required);
        eprintln!(
            "oblivion-one xwayland: event=xwm_root_event_mask generation={:?} root={} mask=0x{observed:x} required=0x{:x} valid={valid}",
            self.generation,
            self.root,
            u32::from(required),
        );
        self.root_event_mask = Some(reply.your_event_mask);
        if valid {
            Ok(())
        } else {
            Err(XwmError::RootEventMask(observed))
        }
    }

    pub(crate) fn pending_event_count(&self) -> usize {
        self.outgoing_events.len()
    }
}
