use std::io;

use super::super::adoption;
use super::{XwmStartup, XwmStartupError};
use x11rb::{
    cookie::Cookie,
    protocol::xproto::{self, ConnectionExt as XprotoConnectionExt, MapState, WindowClass},
};

const MAX_ADOPTION_IN_FLIGHT: usize = 64;

#[derive(Debug, Clone, Copy)]
pub(super) enum PendingAdoptionKind {
    Attributes,
    Geometry,
}

pub(super) enum AdoptionReply {
    Attributes(xproto::GetWindowAttributesReply),
    Geometry(xproto::GetGeometryReply),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PendingAdoptionReply {
    pub(super) xid: u32,
    pub(super) kind: PendingAdoptionKind,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AdoptionCandidate {
    pub(super) xid: u32,
    pub(super) kind: Option<crate::compositor::DesktopWindowKind>,
    pub(super) geometry: Option<super::super::X11Geometry>,
    pub(super) mapped: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AdoptedWindow {
    pub(super) xid: u32,
    pub(super) kind: crate::compositor::DesktopWindowKind,
    pub(super) geometry: super::super::X11Geometry,
    pub(super) mapped: bool,
}

impl XwmStartup {
    pub(super) fn start_adoption_batch(&mut self) -> Result<(), XwmStartupError> {
        let mut queued = false;
        let available = MAX_ADOPTION_IN_FLIGHT.saturating_sub(self.adoption_candidates.len());
        for xid in adoption::take_batch(&mut self.adoption_queue, available) {
            if self.adoption_candidates.contains_key(&xid) {
                continue;
            }
            self.adoption_candidates.insert(
                xid,
                AdoptionCandidate {
                    xid,
                    kind: None,
                    geometry: None,
                    mapped: false,
                },
            );
            let attributes = {
                let cookie = self
                    .connection()?
                    .get_window_attributes(xid)
                    .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
                let sequence = cookie.sequence_number();
                std::mem::forget(cookie);
                sequence
            };
            let geometry = {
                let cookie = self
                    .connection()?
                    .get_geometry(xid)
                    .map_err(|error| XwmStartupError::Protocol(error.to_string()))?;
                let sequence = cookie.sequence_number();
                std::mem::forget(cookie);
                sequence
            };
            self.pending_adoption.insert(
                attributes,
                PendingAdoptionReply {
                    xid,
                    kind: PendingAdoptionKind::Attributes,
                },
            );
            self.pending_adoption.insert(
                geometry,
                PendingAdoptionReply {
                    xid,
                    kind: PendingAdoptionKind::Geometry,
                },
            );
            queued = true;
        }
        if queued {
            self.flush_connection()?;
        }
        Ok(())
    }

    pub(super) fn complete_adoption(&mut self) -> Result<bool, XwmStartupError> {
        let sequences = self.pending_adoption.keys().copied().collect::<Vec<_>>();
        for sequence in sequences {
            let Some(pending) = self.pending_adoption.get(&sequence).copied() else {
                continue;
            };
            let result = match pending.kind {
                PendingAdoptionKind::Attributes => {
                    let result = {
                        let cookie = Cookie::<
                            super::super::connection::X11Connection,
                            xproto::GetWindowAttributesReply,
                        >::new(self.connection()?, sequence);
                        cookie.reply_unchecked()
                    };
                    result.map(|reply| reply.map(AdoptionReply::Attributes))
                }
                PendingAdoptionKind::Geometry => {
                    let result = {
                        let cookie = Cookie::<
                            super::super::connection::X11Connection,
                            xproto::GetGeometryReply,
                        >::new(self.connection()?, sequence);
                        cookie.reply_unchecked()
                    };
                    result.map(|reply| reply.map(AdoptionReply::Geometry))
                }
            };
            match result {
                Ok(Some(reply)) => {
                    self.pending_adoption.remove(&sequence);
                    self.apply_adoption_reply(pending.xid, reply)?;
                }
                Ok(None) => {
                    self.pending_adoption.remove(&sequence);
                    self.adoption_candidates.remove(&pending.xid);
                    self.drain_adoption_errors()?;
                }
                Err(x11rb::errors::ConnectionError::IoError(error))
                    if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    return Err(XwmStartupError::Protocol(format!(
                        "existing-window adoption connection failed: {error}"
                    )));
                }
            }
        }
        self.finalize_adoption_candidates();
        self.start_adoption_batch()?;
        Ok(self.pending_adoption.is_empty()
            && self.adoption_queue.is_empty()
            && self.adoption_candidates.is_empty())
    }

    fn apply_adoption_reply(
        &mut self,
        xid: u32,
        reply: AdoptionReply,
    ) -> Result<(), XwmStartupError> {
        match reply {
            AdoptionReply::Attributes(attributes) => {
                if attributes.class != WindowClass::INPUT_OUTPUT {
                    self.adoption_candidates.remove(&xid);
                    return Ok(());
                }
                let Some(candidate) = self.adoption_candidates.get_mut(&xid) else {
                    return Ok(());
                };
                candidate.kind = Some(if attributes.override_redirect {
                    crate::compositor::DesktopWindowKind::OverrideRedirect
                } else {
                    crate::compositor::DesktopWindowKind::Managed
                });
                candidate.mapped = attributes.map_state != MapState::UNMAPPED;
            }
            AdoptionReply::Geometry(geometry) => {
                if geometry.width == 0 || geometry.height == 0 {
                    self.adoption_candidates.remove(&xid);
                    return Ok(());
                }
                let Some(candidate) = self.adoption_candidates.get_mut(&xid) else {
                    return Ok(());
                };
                candidate.geometry = Some(super::super::X11Geometry {
                    x: i32::from(geometry.x),
                    y: i32::from(geometry.y),
                    width: u32::from(geometry.width),
                    height: u32::from(geometry.height),
                });
            }
        }
        Ok(())
    }

    fn finalize_adoption_candidates(&mut self) {
        let ready = self
            .adoption_candidates
            .values()
            .filter_map(|candidate| {
                Some(AdoptedWindow {
                    xid: candidate.xid,
                    kind: candidate.kind?,
                    geometry: candidate.geometry?,
                    mapped: candidate.mapped,
                })
            })
            .collect::<Vec<_>>();
        for adopted in ready {
            self.adoption_candidates.remove(&adopted.xid);
            self.adopted_windows.push(adopted);
        }
    }

    fn drain_adoption_errors(&mut self) -> Result<(), XwmStartupError> {
        loop {
            let Some((raw, sequence)) = self
                .connection()?
                .poll_new_raw_event_with_sequence()
                .map_err(|error| XwmStartupError::Protocol(error.to_string()))?
            else {
                return Ok(());
            };
            if raw.first().copied() != Some(0) {
                self.connection()?.defer_raw_event((raw, sequence));
            }
        }
    }
}
