use super::super::toplevel_icon::*;
use super::super::*;
use std::sync::Mutex;
use wayland_protocols::xdg::toplevel_icon::v1::server::{
    xdg_toplevel_icon_manager_v1 as manager, xdg_toplevel_icon_v1 as icon,
};
use wayland_server::{GlobalDispatch, New, Resource};

fn no_memory(state: &mut CompositorState, client: &Client) {
    state.queue_client_resource_exhaustion(
        client.clone(),
        None,
        "window icon resource limit reached",
    );
}

impl GlobalDispatch<manager::XdgToplevelIconManagerV1, ()> for CompositorState {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<manager::XdgToplevelIconManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let manager = init.init(resource, ());
        manager.icon_size(96);
        manager.done();
    }
}

impl Dispatch<manager::XdgToplevelIconManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &manager::XdgToplevelIconManagerV1,
        request: manager::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            manager::Request::Destroy => {}
            manager::Request::CreateIcon { id } => {
                let resource = init.init(id, Mutex::new(IconBuilder::default()));
                if state.toplevel_icons.resources.len() >= MAX_ICON_OBJECTS
                    || state
                        .toplevel_icons
                        .resources
                        .values()
                        .filter(|(owner, _)| *owner == client.id())
                        .count()
                        >= MAX_ICON_OBJECTS_PER_CLIENT
                {
                    no_memory(state, client);
                    return;
                }
                state
                    .toplevel_icons
                    .resources
                    .insert(resource.id(), (client.id(), resource));
            }
            manager::Request::SetIcon {
                toplevel,
                icon: selected,
            } => {
                let Some(data) = toplevel.data::<XdgToplevelData>() else {
                    return;
                };
                let surface_id = compositor_surface_id(&data.surface);
                if !state.toplevel_icons.pending.contains_key(&surface_id)
                    && !state.toplevel_icons.current.contains_key(&surface_id)
                    && state.toplevel_icons.pending.len() + state.toplevel_icons.current.len()
                        >= MAX_ICON_SURFACES
                {
                    no_memory(state, client);
                    return;
                }
                let snapshot = if let Some(resource) = selected {
                    let Some(builder) = resource.data::<Mutex<IconBuilder>>() else {
                        return;
                    };
                    let Ok(mut builder) = builder.lock() else {
                        return;
                    };
                    if builder.buffers.iter().any(|b| !b.resource.is_alive()) {
                        resource.post_error(icon::Error::NoBuffer, "icon buffer is no longer live");
                        return;
                    }
                    builder.immutable = true;
                    let image = builder
                        .buffers
                        .iter()
                        .max_by_key(|b| b.image.size().width)
                        .map(|b| b.image.clone());
                    if builder.name.is_empty() && image.is_none() {
                        None
                    } else {
                        match ToplevelIcon::new(builder.name.clone(), image) {
                            Ok(snapshot) => Some(Arc::new(snapshot)),
                            Err(_) => {
                                no_memory(state, client);
                                return;
                            }
                        }
                    }
                } else {
                    None
                };
                state.toplevel_icons.pending.insert(surface_id, snapshot);
            }
            _ => {}
        }
    }
}

impl Dispatch<icon::XdgToplevelIconV1, Mutex<IconBuilder>> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &icon::XdgToplevelIconV1,
        request: icon::Request,
        data: &Mutex<IconBuilder>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if matches!(request, icon::Request::Destroy) {
            return;
        }
        let Ok(mut data) = data.lock() else {
            return;
        };
        if data.immutable {
            resource.post_error(
                icon::Error::Immutable,
                "icon already assigned to a toplevel",
            );
            return;
        }
        match request {
            icon::Request::SetName { icon_name } => {
                if icon_name.len() > 256 {
                    no_memory(state, client);
                    return;
                }
                data.name = if icon_name.contains(['/', '\\', ':']) {
                    String::new()
                } else {
                    icon_name
                };
            }
            icon::Request::AddBuffer { buffer, scale } => {
                let Some(shm) = buffer.data::<ShmBufferData>() else {
                    resource.post_error(icon::Error::InvalidBuffer, "icon buffer must be wl_shm");
                    return;
                };
                let image = match snapshot_icon_buffer(shm, scale) {
                    Ok(image) => image,
                    Err(error) => {
                        resource.post_error(icon::Error::InvalidBuffer, error.to_string());
                        return;
                    }
                };
                let source_edge = shm.width;
                let value = IconBuffer {
                    resource: buffer,
                    scale,
                    source_edge,
                    image,
                };
                let same_size = data
                    .buffers
                    .iter()
                    .position(|b| b.source_edge == source_edge && b.scale == scale);
                if let Some(index) = same_size {
                    data.buffers[index] = value;
                } else if data.buffers.len() < MAX_ICON_BUFFERS {
                    data.buffers.push(value);
                } else if let Some(smallest) = data
                    .buffers
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, b)| b.source_edge)
                    .filter(|(_, b)| b.source_edge < source_edge)
                    .map(|(index, _)| index)
                {
                    // Offering many sizes is legal. Only the largest is used,
                    // so past the bound keep the largest instead of failing.
                    data.buffers[smallest] = value;
                }
            }
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _: ClientId,
        resource: &icon::XdgToplevelIconV1,
        _: &Mutex<IconBuilder>,
    ) {
        state.toplevel_icons.resources.remove(&resource.id());
    }
}
