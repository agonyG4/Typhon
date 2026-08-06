use crate::astrea_toplevel_management::server::{astrea_toplevel_manager_v1, astrea_toplevel_v1};
use std::collections::BTreeMap;

use super::super::*;

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
        let manager_for_error = manager.clone();
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
                "client is not an authorized Astrea shell descendant",
            );
            return;
        }
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

        let collection = state.collect_astrea_toplevels();
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
        let manager_id = manager.id();
        if state
            .astrea_toplevel_publisher
            .bind_manager(handle, manager, client.clone(), &collection)
            .is_err()
        {
            state.post_protocol_error(
                client,
                &manager_for_error,
                astrea_toplevel_manager_v1::Error::ResourceLimit,
                "unable to create Astrea toplevel resources",
            );
            return;
        }
        state
            .astrea_toplevel_publisher
            .send_initial_done(&manager_id, &collection);
    }
}

impl Dispatch<astrea_toplevel_manager_v1::AstreaToplevelManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
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
                    .remove_manager(&resource.id());
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &astrea_toplevel_manager_v1::AstreaToplevelManagerV1,
        _data: &(),
    ) {
        state
            .astrea_toplevel_publisher
            .remove_manager(&resource.id());
    }
}

impl Dispatch<astrea_toplevel_v1::AstreaToplevelV1, AstreaToplevelResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &astrea_toplevel_v1::AstreaToplevelV1,
        request: astrea_toplevel_v1::Request,
        data: &AstreaToplevelResourceData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            astrea_toplevel_v1::Request::Destroy => {
                state.astrea_toplevel_publisher.remove_handle(
                    &data.manager_id,
                    data.window_id,
                    &resource.id(),
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &astrea_toplevel_v1::AstreaToplevelV1,
        data: &AstreaToplevelResourceData,
    ) {
        state.astrea_toplevel_publisher.remove_handle(
            &data.manager_id,
            data.window_id,
            &resource.id(),
        );
    }
}
