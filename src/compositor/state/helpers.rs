use super::*;

pub(in crate::compositor) fn same_surface_resource(
    left: &wl_surface::WlSurface,
    right: &wl_surface::WlSurface,
) -> bool {
    same_wayland_resource(left, right)
}

pub(in crate::compositor) fn same_buffer_resource(
    left: &wl_buffer::WlBuffer,
    right: &wl_buffer::WlBuffer,
) -> bool {
    same_wayland_resource(left, right)
}

pub(in crate::compositor) fn resource_owned_by_client<R>(resource: &R, client_id: &ClientId) -> bool
where
    R: Resource,
{
    resource
        .client()
        .is_some_and(|client| client.id() == *client_id)
}

pub(in crate::compositor) fn normalize_selection_mime_types(
    mime_types: Vec<String>,
) -> Vec<String> {
    const MAX_SOURCE_MIME_TYPES: usize = 128;
    const MAX_MIME_TYPE_LEN: usize = 4096;
    let mut normalized = Vec::new();
    for mime_type in mime_types {
        if mime_type.is_empty()
            || mime_type.len() > MAX_MIME_TYPE_LEN
            || normalized.iter().any(|existing| existing == &mime_type)
        {
            continue;
        }
        normalized.push(mime_type);
        if normalized.len() >= MAX_SOURCE_MIME_TYPES {
            break;
        }
    }
    normalized
}

#[allow(clippy::too_many_arguments)]
pub(in crate::compositor) fn update_renderable_surface_buffer(
    surface: &mut RenderableSurface,
    pending: &PendingSurfaceBuffer,
    buffer_size: BufferSize,
    width: u32,
    height: u32,
    placement: SurfacePlacement,
    generation: u64,
    resize_commit: Option<ResizeCommitSnapshot>,
    damage: RenderableSurfaceDamage,
) -> io::Result<()> {
    if pending.data.is_shm()
        && surface.buffer_size() == buffer_size
        && surface.buffer_id() == pending.data.buffer_id()
        && let Some(pixels) = surface.shm_pixels_mut()
    {
        pending.data.read_pixels_into_with_damage(pixels, &damage)?;
    } else {
        surface.buffer = pending.data.to_committed_buffer_for_size(buffer_size)?;
    }
    surface.x = pending.x;
    surface.y = pending.y;
    surface.width = width;
    surface.height = height;
    surface.placement = placement;
    surface.generation = generation;
    surface.commit_sequence = pending.commit_sequence;
    surface.viewport_source = pending.viewport_source;
    surface.damage = surface
        .damage
        .clone()
        .union(damage, buffer_size.width, buffer_size.height);
    let _ = resize_commit;
    Ok(())
}

pub(in crate::compositor) fn root_surface_id_for_surface_in_placements(
    placements: &HashMap<u32, SurfacePlacement>,
    surface_id: u32,
) -> u32 {
    let mut current = surface_id;
    for _ in 0..placements.len().saturating_add(1) {
        let Some(parent) = placements
            .get(&current)
            .copied()
            .unwrap_or_default()
            .parent_surface_id
            .filter(|parent_id| *parent_id != current)
        else {
            return current;
        };
        current = parent;
    }

    surface_id
}
