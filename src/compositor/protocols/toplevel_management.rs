use crate::astrea_toplevel_management::server::{astrea_toplevel_manager_v1, astrea_toplevel_v1};
use std::collections::BTreeMap;

use super::super::*;

fn dispatch_action(
    state: &mut CompositorState,
    client: &Client,
    resource: &astrea_toplevel_v1::AstreaToplevelV1,
    data: &AstreaToplevelResourceData,
    display: &DisplayHandle,
    action: AstreaToplevelAction,
    token: AstreaActionToken,
) {
    if resource.version() < 2 {
        return;
    }
    if !state.astrea_toplevel_client_allowed(client, display) {
        state.post_protocol_error(
            client,
            resource,
            astrea_toplevel_manager_v1::Error::Unauthorized,
            "client is not an authorized Astrea shell client",
        );
        return;
    }

    let prepared = match state.astrea_toplevel_publisher.prepare_action(
        &data.client_id,
        &data.manager_id,
        &resource.id(),
        data.window_id,
        token,
        action,
    ) {
        Ok(prepared) => prepared,
        Err(AstreaActionPreparationError::Protocol) => return,
        Err(AstreaActionPreparationError::Unavailable) => {
            let _ = state.astrea_toplevel_publisher.send_action_done(
                &data.manager_id,
                token,
                action,
                astrea_toplevel_manager_v1::ActionResult::Unavailable,
            );
            return;
        }
        Err(AstreaActionPreparationError::Duplicate) => {
            let _ = state.astrea_toplevel_publisher.send_action_done(
                &data.manager_id,
                token,
                action,
                astrea_toplevel_manager_v1::ActionResult::Unavailable,
            );
            return;
        }
        Err(AstreaActionPreparationError::Limit) => {
            let _ = state.astrea_toplevel_publisher.send_action_done(
                &data.manager_id,
                token,
                action,
                astrea_toplevel_manager_v1::ActionResult::Unavailable,
            );
            return;
        }
    };

    let result = match action {
        AstreaToplevelAction::Activate => {
            match state.activate_desktop_window_action(prepared.window_id) {
                WindowActionOutcome::Changed => astrea_toplevel_manager_v1::ActionResult::Accepted,
                WindowActionOutcome::NoChange => astrea_toplevel_manager_v1::ActionResult::NoChange,
                WindowActionOutcome::Unavailable => {
                    astrea_toplevel_manager_v1::ActionResult::Unavailable
                }
            }
        }
        AstreaToplevelAction::Minimize => {
            match state.minimize_desktop_window_outcome(prepared.window_id) {
                WindowActionOutcome::Changed => astrea_toplevel_manager_v1::ActionResult::Accepted,
                WindowActionOutcome::NoChange => astrea_toplevel_manager_v1::ActionResult::NoChange,
                WindowActionOutcome::Unavailable => {
                    astrea_toplevel_manager_v1::ActionResult::Unavailable
                }
            }
        }
        AstreaToplevelAction::Restore => {
            match state.restore_minimized_desktop_window_outcome(prepared.window_id) {
                WindowActionOutcome::Changed => astrea_toplevel_manager_v1::ActionResult::Accepted,
                WindowActionOutcome::NoChange => astrea_toplevel_manager_v1::ActionResult::NoChange,
                WindowActionOutcome::Unavailable => {
                    astrea_toplevel_manager_v1::ActionResult::Unavailable
                }
            }
        }
        AstreaToplevelAction::Close => {
            match state.close_desktop_window_outcome(prepared.window_id) {
                WindowActionOutcome::Changed => astrea_toplevel_manager_v1::ActionResult::Accepted,
                WindowActionOutcome::NoChange => astrea_toplevel_manager_v1::ActionResult::NoChange,
                WindowActionOutcome::Unavailable => {
                    astrea_toplevel_manager_v1::ActionResult::Unavailable
                }
            }
        }
    };

    let _ = state
        .astrea_toplevel_publisher
        .complete_action(prepared, result);
}

impl GlobalDispatch<astrea_toplevel_manager_v1::AstreaToplevelManagerV1, ()> for CompositorState {
    fn bind(
        state: &mut Self,
        handle: &DisplayHandle,
        client: &Client,
        resource: New<astrea_toplevel_manager_v1::AstreaToplevelManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let manager = data_init.init(resource, ());
        if !state.astrea_toplevel_client_allowed(client, handle) {
            state
                .astrea_toplevel_publisher
                .metrics
                .unauthorized_manager_binds = state
                .astrea_toplevel_publisher
                .metrics
                .unauthorized_manager_binds
                .saturating_add(1);
            state.post_protocol_error(
                client,
                &manager,
                astrea_toplevel_manager_v1::Error::Unauthorized,
                "client is not an authorized Astrea shell client",
            );
            return;
        }
        state.astrea_toplevel_publisher.prune_dead_resources();
        let client_id = client.id();
        let manager_limit = state.astrea_toplevel_publisher.manager_count()
            >= MAX_ASTREA_TOPLEVEL_MANAGERS
            || state
                .astrea_toplevel_publisher
                .manager_count_for_client(&client_id)
                >= MAX_ASTREA_TOPLEVEL_MANAGERS_PER_CLIENT;
        if manager_limit {
            state
                .astrea_toplevel_publisher
                .metrics
                .manager_limit_rejections = state
                .astrea_toplevel_publisher
                .metrics
                .manager_limit_rejections
                .saturating_add(1);
            state.post_protocol_error(
                client,
                &manager,
                astrea_toplevel_manager_v1::Error::ManagerLimit,
                "Astrea toplevel manager limit exceeded",
            );
            return;
        }

        let collection = match state.collect_astrea_toplevels() {
            Ok(collection) => collection,
            Err(()) => {
                state
                    .astrea_toplevel_publisher
                    .metrics
                    .handle_limit_rejections = state
                    .astrea_toplevel_publisher
                    .metrics
                    .handle_limit_rejections
                    .saturating_add(1);
                state.post_protocol_error(
                    client,
                    &manager,
                    astrea_toplevel_manager_v1::Error::ResourceLimit,
                    "Astrea toplevel eligibility limit exceeded",
                );
                return;
            }
        };
        if !state
            .astrea_toplevel_publisher
            .can_allocate_manager(&client_id, collection.snapshots.len())
        {
            state
                .astrea_toplevel_publisher
                .metrics
                .handle_limit_rejections = state
                .astrea_toplevel_publisher
                .metrics
                .handle_limit_rejections
                .saturating_add(1);
            state.post_protocol_error(
                client,
                &manager,
                astrea_toplevel_manager_v1::Error::ResourceLimit,
                "Astrea toplevel handle limit exceeded",
            );
            return;
        }
        state.astrea_toplevel_publisher.reconcile(
            handle,
            Some(collection.clone()),
            BTreeMap::new(),
        );
        let admission_collection = state.astrea_toplevel_publisher.admission_collection();
        if !state
            .astrea_toplevel_publisher
            .can_allocate_manager(&client_id, admission_collection.snapshots.len())
        {
            state
                .astrea_toplevel_publisher
                .metrics
                .handle_limit_rejections = state
                .astrea_toplevel_publisher
                .metrics
                .handle_limit_rejections
                .saturating_add(1);
            state.post_protocol_error(
                client,
                &manager,
                astrea_toplevel_manager_v1::Error::ResourceLimit,
                "Astrea toplevel handle limit exceeded",
            );
            return;
        }
        if state
            .astrea_toplevel_publisher
            .bind_manager(handle, manager, client.clone(), &admission_collection)
            .is_err()
        {
            // Admission failures are reported before binding. Once the
            // manager resource has been admitted, publication failures are
            // terminally isolated to that manager through `failed`.
        }
    }
}

impl Dispatch<astrea_toplevel_manager_v1::AstreaToplevelManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        request: astrea_toplevel_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_toplevel_manager_v1::Request::Destroy => {
                state
                    .astrea_toplevel_publisher
                    .remove_manager(&client.id(), &resource.id());
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        client_id: ClientId,
        resource: &astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        _data: &(),
    ) {
        state
            .astrea_toplevel_publisher
            .remove_manager(&client_id, &resource.id());
    }
}

impl Dispatch<astrea_toplevel_v1::AstreaToplevelV1, AstreaToplevelResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &astrea_toplevel_v1::AstreaToplevelV1,
        request: astrea_toplevel_v1::Request,
        data: &AstreaToplevelResourceData,
        handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_toplevel_v1::Request::Destroy => {
                state.astrea_toplevel_publisher.remove_handle(
                    &data.client_id,
                    &data.manager_id,
                    data.window_id,
                    &resource.id(),
                );
            }
            astrea_toplevel_v1::Request::Activate { token_hi, token_lo } => {
                dispatch_action(
                    state,
                    client,
                    resource,
                    data,
                    handle,
                    AstreaToplevelAction::Activate,
                    AstreaActionToken::new(token_hi, token_lo),
                );
            }
            astrea_toplevel_v1::Request::Minimize { token_hi, token_lo } => {
                dispatch_action(
                    state,
                    client,
                    resource,
                    data,
                    handle,
                    AstreaToplevelAction::Minimize,
                    AstreaActionToken::new(token_hi, token_lo),
                );
            }
            astrea_toplevel_v1::Request::Restore { token_hi, token_lo } => {
                dispatch_action(
                    state,
                    client,
                    resource,
                    data,
                    handle,
                    AstreaToplevelAction::Restore,
                    AstreaActionToken::new(token_hi, token_lo),
                );
            }
            astrea_toplevel_v1::Request::Close { token_hi, token_lo } => {
                dispatch_action(
                    state,
                    client,
                    resource,
                    data,
                    handle,
                    AstreaToplevelAction::Close,
                    AstreaActionToken::new(token_hi, token_lo),
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client_id: ClientId,
        resource: &astrea_toplevel_v1::AstreaToplevelV1,
        data: &AstreaToplevelResourceData,
    ) {
        state.astrea_toplevel_publisher.remove_handle(
            &data.client_id,
            &data.manager_id,
            data.window_id,
            &resource.id(),
        );
    }
}
