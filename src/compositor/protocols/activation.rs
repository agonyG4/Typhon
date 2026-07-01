use super::super::*;

impl GlobalDispatch<xdg_activation_v1::XdgActivationV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<xdg_activation_v1::XdgActivationV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<xdg_activation_v1::XdgActivationV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        _resource: &xdg_activation_v1::XdgActivationV1,
        request: xdg_activation_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_activation_v1::Request::GetActivationToken { id } => {
                let token = data_init.init(id, ());
                state.pending_activation_tokens.insert(
                    token.id().protocol_id(),
                    PendingActivationToken {
                        client_id: client.id(),
                        serial: None,
                        surface_id: None,
                        app_id: None,
                    },
                );
                activation_debug_log(|| {
                    format!(
                        "activation_token_create resource={} client={:?}",
                        token.id().protocol_id(),
                        client.id()
                    )
                });
            }
            xdg_activation_v1::Request::Activate { token, surface } => {
                state.activate_surface_with_token(client.id(), token, surface);
            }
            xdg_activation_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
        request: xdg_activation_token_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let resource_id = resource.id().protocol_id();
        match request {
            xdg_activation_token_v1::Request::SetSerial { serial, seat: _ } => {
                if let Some(token) = state.pending_activation_tokens.get_mut(&resource_id) {
                    token.serial = Some(serial);
                }
            }
            xdg_activation_token_v1::Request::SetAppId { app_id } => {
                if let Some(token) = state.pending_activation_tokens.get_mut(&resource_id) {
                    token.app_id = Some(app_id);
                }
            }
            xdg_activation_token_v1::Request::SetSurface { surface } => {
                if let Some(token) = state.pending_activation_tokens.get_mut(&resource_id) {
                    token.surface_id = Some(compositor_surface_id(&surface));
                }
            }
            xdg_activation_token_v1::Request::Commit => {
                state.commit_activation_token(client.id(), resource);
            }
            xdg_activation_token_v1::Request::Destroy => {
                state.pending_activation_tokens.remove(&resource_id);
            }
            _ => {}
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn commit_activation_token(
        &mut self,
        client_id: ClientId,
        resource: &xdg_activation_token_v1::XdgActivationTokenV1,
    ) {
        let resource_id = resource.id().protocol_id();
        let Some(pending) = self.pending_activation_tokens.remove(&resource_id) else {
            activation_debug_log(|| {
                format!("activation_reject resource={resource_id} reason=unknown_pending")
            });
            return;
        };
        if pending.client_id != client_id {
            activation_debug_log(|| {
                format!(
                    "activation_reject resource={resource_id} reason=wrong_client client={client_id:?}"
                )
            });
            return;
        }
        if let Some(serial) = pending.serial
            && !self.client_has_recent_input_serial(&client_id, serial)
        {
            activation_debug_log(|| {
                format!(
                    "activation_reject resource={resource_id} reason=stale_serial serial={serial}"
                )
            });
            return;
        }
        if let Some(surface_id) = pending.surface_id
            && !self.surface_resources.contains_key(&surface_id)
        {
            activation_debug_log(|| {
                format!(
                    "activation_reject resource={resource_id} reason=dead_source surface={surface_id}"
                )
            });
            return;
        }

        self.next_activation_token_serial = self.next_activation_token_serial.saturating_add(1);
        let generation = self.next_activation_token_serial;
        let token = format!("typhon-activation-{generation}");
        self.activation_tokens.insert(
            token.clone(),
            ActivationTokenState {
                client_id,
                serial: pending.serial,
                surface_id: pending.surface_id,
                app_id: pending.app_id,
                generation,
                used: false,
            },
        );
        resource.done(token.clone());
        activation_debug_log(|| {
            format!(
                "activation_token_done resource={resource_id} token={token} generation={generation}"
            )
        });
    }

    pub(in crate::compositor) fn activate_surface_with_token(
        &mut self,
        client_id: ClientId,
        token: String,
        surface: wl_surface::WlSurface,
    ) -> bool {
        let surface_id = compositor_surface_id(&surface);
        let Some(mut token_state) = self.activation_tokens.remove(&token) else {
            activation_debug_log(|| {
                format!("activation_reject token={token} target={surface_id} reason=unknown")
            });
            return false;
        };
        if token_state.used || token_state.client_id != client_id {
            activation_debug_log(|| {
                format!(
                    "activation_reject token={token} target={surface_id} reason=used_or_wrong_client generation={}",
                    token_state.generation
                )
            });
            return false;
        }
        token_state.used = true;
        if self.active_exclusive_layer_surface_id().is_some()
            && !self.layer_surfaces.contains_key(&surface_id)
        {
            activation_debug_log(|| {
                format!(
                    "activation_reject token={token} target={surface_id} reason=exclusive_layer_active generation={}",
                    token_state.generation
                )
            });
            return false;
        }

        if self.toplevel_surfaces.contains_key(&surface_id) {
            self.raise_renderable_surface_tree(surface_id);
            self.focus_surface(surface);
            activation_debug_log(|| {
                format!(
                    "activation_accept token={token} target={surface_id} kind=toplevel generation={}",
                    token_state.generation
                )
            });
            return true;
        }

        if self.layer_surfaces.contains_key(&surface_id) {
            let activated = self.activate_layer_surface_from_activation(surface_id);
            if !activated {
                activation_debug_log(|| {
                    format!(
                        "activation_reject token={token} target={surface_id} reason=ineligible_layer generation={}",
                        token_state.generation
                    )
                });
                return false;
            }
            activation_debug_log(|| {
                format!(
                    "activation_accept token={token} target={surface_id} kind=ondemand-layer accepted={activated} generation={}",
                    token_state.generation
                )
            });
            return activated;
        }

        activation_debug_log(|| {
            format!(
                "activation_reject token={token} target={surface_id} reason=ineligible generation={} source={:?} serial={:?} app_id={:?}",
                token_state.generation,
                token_state.surface_id,
                token_state.serial,
                token_state.app_id
            )
        });
        false
    }
}

fn activation_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_FOCUS_DEBUG").is_some()) {
        eprintln!("oblivion-one focus: {}", message());
    }
}
