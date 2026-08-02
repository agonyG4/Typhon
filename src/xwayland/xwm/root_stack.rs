use std::{collections::HashSet, io};

use x11rb::{
    connection::Connection,
    cookie::Cookie,
    protocol::xproto::{self, ConnectionExt as XprotoConnectionExt},
};
use x11rb_protocol::SequenceNumber;

use super::{X11WindowLifecycle, Xwm, XwmError, XwmEvent, connection};
use crate::compositor::DesktopWindowKind;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OverrideRedirectStackMetrics {
    pub(crate) queries_issued: u64,
    pub(crate) queries_coalesced: u64,
    pub(crate) replies: u64,
    pub(crate) superseded_replies: u64,
    pub(crate) incomplete_replies: u64,
    pub(crate) snapshots_emitted: u64,
    pub(crate) entries_pruned: u64,
}

#[derive(Debug, Default)]
pub(crate) struct OverrideRedirectStackState {
    dirty: bool,
    epoch: u64,
    pending: Option<PendingQuery>,
    last_applied_epoch: Option<u64>,
    metrics: OverrideRedirectStackMetrics,
}

#[derive(Debug, Clone, Copy)]
struct PendingQuery {
    sequence: SequenceNumber,
    epoch: u64,
}

impl Xwm {
    pub(crate) fn mark_override_redirect_stack_dirty(&mut self) {
        let state = &mut self.override_redirect_stack;
        state.dirty = true;
        state.epoch = state.epoch.saturating_add(1).max(1);
        if state.pending.is_some() {
            state.metrics.queries_coalesced = state.metrics.queries_coalesced.saturating_add(1);
        }
    }

    pub(crate) fn reconcile_override_redirect_stack(&mut self) -> Result<(), XwmError> {
        self.consume_override_redirect_stack_reply()?;
        if self.override_redirect_stack.dirty && self.override_redirect_stack.pending.is_none() {
            self.issue_override_redirect_stack_query()?;
        }
        Ok(())
    }

    fn issue_override_redirect_stack_query(&mut self) -> Result<(), XwmError> {
        let cookie = self
            .connection
            .query_tree(self.root)
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        self.override_redirect_stack.pending = Some(PendingQuery {
            sequence,
            epoch: self.override_redirect_stack.epoch,
        });
        self.override_redirect_stack.metrics.queries_issued = self
            .override_redirect_stack
            .metrics
            .queries_issued
            .saturating_add(1);
        self.connection.flush().map_err(XwmError::Connection)
    }

    fn consume_override_redirect_stack_reply(&mut self) -> Result<(), XwmError> {
        let Some(pending) = self.override_redirect_stack.pending.take() else {
            return Ok(());
        };
        let cookie = Cookie::<connection::X11Connection, xproto::QueryTreeReply>::new(
            &self.connection,
            pending.sequence,
        );
        let reply = match cookie.reply_unchecked() {
            Ok(Some(reply)) => reply,
            Ok(None) => {
                return Err(XwmError::InvalidCommand(
                    "malformed override-redirect QueryTree reply",
                ));
            }
            Err(x11rb::errors::ConnectionError::IoError(error))
                if error.kind() == io::ErrorKind::WouldBlock =>
            {
                self.override_redirect_stack.pending = Some(pending);
                return Ok(());
            }
            Err(error) => return Err(XwmError::Connection(error)),
        };
        self.override_redirect_stack.metrics.replies = self
            .override_redirect_stack
            .metrics
            .replies
            .saturating_add(1);

        if pending.epoch != self.override_redirect_stack.epoch {
            self.override_redirect_stack.metrics.superseded_replies = self
                .override_redirect_stack
                .metrics
                .superseded_replies
                .saturating_add(1);
            self.override_redirect_stack.dirty = true;
            return Ok(());
        }

        let (bottom_to_top, complete) = self.filter_override_redirect_children(&reply.children);
        if !complete {
            self.override_redirect_stack.metrics.incomplete_replies = self
                .override_redirect_stack
                .metrics
                .incomplete_replies
                .saturating_add(1);
            self.override_redirect_stack.dirty = true;
            return Ok(());
        }

        self.override_redirect_stack.dirty = false;
        self.override_redirect_stack.last_applied_epoch = Some(pending.epoch);
        self.override_redirect_stack.metrics.snapshots_emitted = self
            .override_redirect_stack
            .metrics
            .snapshots_emitted
            .saturating_add(1);
        self.outgoing_events
            .push_back(XwmEvent::OverrideRedirectStackSnapshot {
                generation: self.generation,
                epoch: pending.epoch,
                bottom_to_top,
            });
        Ok(())
    }

    fn filter_override_redirect_children(
        &mut self,
        children: &[u32],
    ) -> (Vec<super::X11WindowHandle>, bool) {
        let live = self
            .windows
            .iter()
            .filter_map(|(handle, record)| {
                (record.kind == DesktopWindowKind::OverrideRedirect
                    && record.mapped_notified
                    && !matches!(
                        record.lifecycle,
                        X11WindowLifecycle::Iconic
                            | X11WindowLifecycle::Withdrawn
                            | X11WindowLifecycle::Destroyed
                    ))
                .then_some(*handle)
            })
            .collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        let mut pruned = 0_u64;
        let bottom_to_top = children
            .iter()
            .filter_map(|xid| {
                let handle = self.windows.handle_by_xid(*xid)?;
                if !live.contains(&handle) || !seen.insert(handle) {
                    pruned = pruned.saturating_add(1);
                    return None;
                }
                Some(handle)
            })
            .collect::<Vec<_>>();
        let complete = live.iter().all(|handle| seen.contains(handle));
        self.override_redirect_stack.metrics.entries_pruned = self
            .override_redirect_stack
            .metrics
            .entries_pruned
            .saturating_add(pruned);
        (bottom_to_top, complete)
    }

    pub(crate) fn clear_override_redirect_stack_state(&mut self) {
        self.override_redirect_stack = OverrideRedirectStackState::default();
    }

    #[cfg(test)]
    pub(crate) fn override_redirect_stack_query_for_test(&self) -> Option<(SequenceNumber, u64)> {
        self.override_redirect_stack
            .pending
            .map(|pending| (pending.sequence, pending.epoch))
    }

    #[cfg(test)]
    pub(crate) fn override_redirect_stack_metrics_for_test(&self) -> OverrideRedirectStackMetrics {
        self.override_redirect_stack.metrics
    }
}
