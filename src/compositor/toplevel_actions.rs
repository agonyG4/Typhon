use std::collections::BTreeMap;

use crate::astrea_toplevel_management::server::astrea_toplevel_manager_v1;
use wayland_server::{
    Resource, WEnum,
    backend::{ClientId, ObjectId},
};

use super::WindowId;
use super::toplevel_publication::{AstreaToplevelPublisher, ToplevelHandleLifecycle};

pub(in crate::compositor) const MAX_ASTREA_PENDING_ACTIONS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::compositor) struct AstreaActionToken(u64);

impl AstreaActionToken {
    pub(in crate::compositor) const fn new(high: u32, low: u32) -> Self {
        Self(((high as u64) << 32) | low as u64)
    }

    pub(in crate::compositor) const fn wire(self) -> (u32, u32) {
        ((self.0 >> 32) as u32, self.0 as u32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum AstreaToplevelAction {
    Activate,
    Minimize,
    Restore,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct PendingAstreaAction {
    pub(in crate::compositor) token: AstreaActionToken,
    pub(in crate::compositor) action: AstreaToplevelAction,
    pub(in crate::compositor) window_id: WindowId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum AstreaActionBeginError {
    Duplicate,
    Limit,
}

#[derive(Debug, Clone, Default)]
pub(in crate::compositor) struct AstreaActionTracker {
    pending: BTreeMap<AstreaActionToken, PendingAstreaAction>,
}

impl AstreaActionTracker {
    pub(in crate::compositor) fn can_reserve(
        &self,
        token: AstreaActionToken,
    ) -> Result<(), AstreaActionBeginError> {
        if self.pending.contains_key(&token) {
            return Err(AstreaActionBeginError::Duplicate);
        }
        if self.pending.len() >= MAX_ASTREA_PENDING_ACTIONS {
            return Err(AstreaActionBeginError::Limit);
        }
        Ok(())
    }

    pub(in crate::compositor) fn reserve(
        &mut self,
        token: AstreaActionToken,
        action: AstreaToplevelAction,
        window_id: WindowId,
    ) -> Result<(), AstreaActionBeginError> {
        self.can_reserve(token)?;
        self.pending.insert(
            token,
            PendingAstreaAction {
                token,
                action,
                window_id,
            },
        );
        Ok(())
    }

    pub(in crate::compositor) fn release(
        &mut self,
        token: AstreaActionToken,
    ) -> Option<PendingAstreaAction> {
        self.pending.remove(&token)
    }

    pub(in crate::compositor) fn clear(&mut self) {
        self.pending.clear();
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

impl AstreaToplevelAction {
    pub(in crate::compositor) const fn wire(self) -> astrea_toplevel_manager_v1::Action {
        match self {
            Self::Activate => astrea_toplevel_manager_v1::Action::Activate,
            Self::Minimize => astrea_toplevel_manager_v1::Action::Minimize,
            Self::Restore => astrea_toplevel_manager_v1::Action::Restore,
            Self::Close => astrea_toplevel_manager_v1::Action::Close,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum AstreaActionPreparationError {
    Protocol,
    Unavailable,
    Duplicate,
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::compositor) struct AstreaPreparedAction {
    pub(in crate::compositor) manager_id: ObjectId,
    pub(in crate::compositor) token: AstreaActionToken,
    pub(in crate::compositor) action: AstreaToplevelAction,
    pub(in crate::compositor) window_id: WindowId,
}

impl AstreaToplevelPublisher {
    pub(in crate::compositor) fn prepare_action(
        &mut self,
        client_id: &ClientId,
        manager_id: &ObjectId,
        resource_id: &ObjectId,
        window_id: WindowId,
        token: AstreaActionToken,
        action: AstreaToplevelAction,
    ) -> Result<AstreaPreparedAction, AstreaActionPreparationError> {
        let binding = self
            .managers
            .get_mut(manager_id)
            .ok_or(AstreaActionPreparationError::Protocol)?;
        if binding.client_id != *client_id {
            return Err(AstreaActionPreparationError::Protocol);
        }

        match binding.actions.can_reserve(token) {
            Ok(()) => {}
            Err(AstreaActionBeginError::Duplicate) => {
                return Err(AstreaActionPreparationError::Duplicate);
            }
            Err(AstreaActionBeginError::Limit) => {
                return Err(AstreaActionPreparationError::Limit);
            }
        }

        if self.retired_handles.get(resource_id).is_some_and(|handle| {
            handle.client_id == *client_id
                && handle.manager_id == *manager_id
                && handle.window_id == window_id
        }) {
            return Err(AstreaActionPreparationError::Unavailable);
        }
        let handle = binding
            .handles
            .get(resource_id)
            .ok_or(AstreaActionPreparationError::Protocol)?;
        if handle.lifecycle != ToplevelHandleLifecycle::Live
            || handle.snapshot.id != window_id
            || binding.active_handles.get(&window_id) != Some(resource_id)
        {
            return Err(AstreaActionPreparationError::Unavailable);
        }

        binding
            .actions
            .reserve(token, action, window_id)
            .map_err(|error| match error {
                AstreaActionBeginError::Duplicate => AstreaActionPreparationError::Duplicate,
                AstreaActionBeginError::Limit => AstreaActionPreparationError::Limit,
            })?;

        Ok(AstreaPreparedAction {
            manager_id: manager_id.clone(),
            token,
            action,
            window_id,
        })
    }

    pub(in crate::compositor) fn send_action_done(
        &self,
        manager_id: &ObjectId,
        token: AstreaActionToken,
        action: AstreaToplevelAction,
        result: astrea_toplevel_manager_v1::ActionResult,
    ) -> Result<(), wayland_server::backend::InvalidId> {
        let Some(binding) = self.managers.get(manager_id) else {
            return Ok(());
        };
        let (token_hi, token_lo) = token.wire();
        binding
            .resource
            .send_event(astrea_toplevel_manager_v1::Event::ActionDone {
                token_hi,
                token_lo,
                action: WEnum::Value(action.wire()),
                result: WEnum::Value(result),
            })
    }

    pub(in crate::compositor) fn complete_action(
        &mut self,
        prepared: AstreaPreparedAction,
        result: astrea_toplevel_manager_v1::ActionResult,
    ) -> Result<(), wayland_server::backend::InvalidId> {
        let send_result = self.send_action_done(
            &prepared.manager_id,
            prepared.token,
            prepared.action,
            result,
        );
        if let Some(binding) = self.managers.get_mut(&prepared.manager_id) {
            let _ = binding.actions.release(prepared.token);
        }
        send_result
    }
}
