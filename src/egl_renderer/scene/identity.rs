use oblivion_one::compositor::{
    DecorationRenderInstance, DecorationSceneSnapshot, RenderableSurface,
};

use super::super::OutputFramebufferOrigin;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) struct EglSceneCacheKey {
    pub(in crate::egl_renderer) width: u32,
    pub(in crate::egl_renderer) height: u32,
    pub(in crate::egl_renderer) content_generation: u64,
    pub(in crate::egl_renderer) output_scale_key: u32,
    pub(in crate::egl_renderer) surface_signature_hash: u64,
    pub(in crate::egl_renderer) decoration_signature_hash: u64,
    pub(in crate::egl_renderer) popup_surface_signature_hash: u64,
    pub(in crate::egl_renderer) external_overlay_surface_signature_hash: u64,
    pub(in crate::egl_renderer) presentation_geometry_signature: u64,
    pub(in crate::egl_renderer) framebuffer_origin: OutputFramebufferOrigin,
}

impl EglSceneCacheKey {
    #[allow(dead_code)] // Retained for focused cache-key unit tests.
    pub(in crate::egl_renderer) fn new(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self::new_with_presentation(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            0,
            framebuffer_origin,
        )
    }

    pub(in crate::egl_renderer) fn new_with_presentation(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self::new_with_presentation_and_external_overlay_ids(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            presentation_geometry_signature,
            framebuffer_origin,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key construction keeps the explicit overlay partition authority alongside scene state"
    )]
    pub(in crate::egl_renderer) fn new_with_presentation_and_external_overlay_ids(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        Self {
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signature_hash: egl_scene_surface_signature_hash(surface_signatures),
            decoration_signature_hash: egl_decoration_signature_hash(&[]),
            popup_surface_signature_hash: 0,
            external_overlay_surface_signature_hash: egl_external_overlay_surface_signature_hash(
                external_overlay_surface_ids,
            ),
            presentation_geometry_signature,
            framebuffer_origin,
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key construction keeps each render-state component and overlay partition explicit"
    )]
    pub(in crate::egl_renderer) fn new_with_decorations_and_external_overlay_ids(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        presentation_geometry_signature: u64,
        external_overlay_surface_ids: &[u32],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let mut key = Self::new_with_presentation_and_external_overlay_ids(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            external_overlay_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        );
        key.decoration_signature_hash = egl_decoration_signature_hash(decoration_instances);
        key.popup_surface_signature_hash = egl_popup_surface_signature_hash(popup_surface_ids);
        key
    }

    #[cfg(test)]
    pub(in crate::egl_renderer) fn new_with_decoration_snapshots(
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_snapshots: &[DecorationSceneSnapshot],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let mut key = Self::new(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            framebuffer_origin,
        );
        key.decoration_signature_hash =
            egl_decoration_snapshot_signature_hash(decoration_snapshots);
        key
    }

    #[cfg(test)]
    pub(in crate::egl_renderer) fn is_current(
        self,
        width: u32,
        height: u32,
        content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.is_current_with_decorations(
            width,
            height,
            content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            &[],
            0,
            framebuffer_origin,
        )
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key validation keeps each render-state component explicit"
    )]
    pub(in crate::egl_renderer) fn is_current_with_decorations(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.is_current_with_decorations_and_external_overlay_ids(
            width,
            height,
            _content_generation,
            output_scale_key,
            surface_signatures,
            &[],
            decoration_instances,
            popup_surface_ids,
            presentation_geometry_signature,
            framebuffer_origin,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "cache-key validation keeps each render-state component and overlay partition explicit"
    )]
    pub(in crate::egl_renderer) fn is_current_with_decorations_and_external_overlay_ids(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        external_overlay_surface_ids: &[u32],
        decoration_instances: &[DecorationRenderInstance],
        popup_surface_ids: &[u32],
        presentation_geometry_signature: u64,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.width == width
            && self.height == height
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
            && self.decoration_signature_hash == egl_decoration_signature_hash(decoration_instances)
            && self.popup_surface_signature_hash
                == egl_popup_surface_signature_hash(popup_surface_ids)
            && self.external_overlay_surface_signature_hash
                == egl_external_overlay_surface_signature_hash(external_overlay_surface_ids)
            && self.presentation_geometry_signature == presentation_geometry_signature
            && self.framebuffer_origin == framebuffer_origin
    }

    #[cfg(test)]
    #[expect(
        clippy::too_many_arguments,
        reason = "test cache-key validation mirrors the production state comparison"
    )]
    pub(in crate::egl_renderer) fn is_current_with_decoration_snapshots(
        self,
        width: u32,
        height: u32,
        _content_generation: u64,
        output_scale_key: u32,
        surface_signatures: &[EglSceneSurfaceSignature],
        decoration_snapshots: &[DecorationSceneSnapshot],
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> bool {
        self.width == width
            && self.height == height
            && self.output_scale_key == output_scale_key
            && self.surface_signature_hash == egl_scene_surface_signature_hash(surface_signatures)
            && self.decoration_signature_hash
                == egl_decoration_snapshot_signature_hash(decoration_snapshots)
            && self.framebuffer_origin == framebuffer_origin
    }
}

pub(in crate::egl_renderer) fn egl_popup_surface_signature_hash(popup_surface_ids: &[u32]) -> u64 {
    if popup_surface_ids.is_empty() {
        return 0;
    }
    popup_surface_ids
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, id| {
            (hash ^ u64::from(*id)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

pub(in crate::egl_renderer) fn egl_external_overlay_surface_signature_hash(
    external_overlay_surface_ids: &[u32],
) -> u64 {
    if external_overlay_surface_ids.is_empty() {
        return 0;
    }
    external_overlay_surface_ids
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, id| {
            (hash ^ u64::from(*id)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) struct EglSceneSurfaceSignature {
    pub(in crate::egl_renderer) surface_id: u32,
    pub(in crate::egl_renderer) commit_sequence: u64,
    pub(in crate::egl_renderer) buffer_id: u64,
    pub(in crate::egl_renderer) buffer_width: u32,
    pub(in crate::egl_renderer) buffer_height: u32,
    pub(in crate::egl_renderer) buffer_scale: u32,
    pub(in crate::egl_renderer) buffer_transform: wayland_server::protocol::wl_output::Transform,
    pub(in crate::egl_renderer) x: i32,
    pub(in crate::egl_renderer) y: i32,
    pub(in crate::egl_renderer) width: u32,
    pub(in crate::egl_renderer) height: u32,
    pub(in crate::egl_renderer) render_x: i32,
    pub(in crate::egl_renderer) render_y: i32,
    pub(in crate::egl_renderer) clip_x: i32,
    pub(in crate::egl_renderer) clip_y: i32,
    pub(in crate::egl_renderer) clip_width: u32,
    pub(in crate::egl_renderer) clip_height: u32,
    pub(in crate::egl_renderer) generation: u64,
}

pub(in crate::egl_renderer) fn egl_scene_surface_signatures(
    surfaces: &[RenderableSurface],
) -> Vec<EglSceneSurfaceSignature> {
    surfaces
        .iter()
        .map(|surface| {
            let render_placement = surface.render_placement.unwrap_or(surface.placement);
            let clip = surface.visual_clip.as_ref();
            EglSceneSurfaceSignature {
                surface_id: surface.surface_id,
                commit_sequence: surface.commit_sequence.get(),
                buffer_id: surface.buffer_id().get(),
                buffer_width: surface.buffer_size().width,
                buffer_height: surface.buffer_size().height,
                buffer_scale: surface.buffer_scale,
                buffer_transform: surface.buffer_transform,
                x: surface.x,
                y: surface.y,
                width: surface.width,
                height: surface.height,
                render_x: render_placement.local_x,
                render_y: render_placement.local_y,
                clip_x: clip.map_or(0, |clip| clip.x()),
                clip_y: clip.map_or(0, |clip| clip.y()),
                clip_width: clip.map_or(0, |clip| clip.width()),
                clip_height: clip.map_or(0, |clip| clip.height()),
                generation: surface.generation,
            }
        })
        .collect()
}

pub(in crate::egl_renderer) fn egl_scene_surface_signature_hash(
    signatures: &[EglSceneSurfaceSignature],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for signature in signatures {
        hash = fnv1a_u64(hash, u64::from(signature.surface_id));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_width));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_height));
        hash = fnv1a_u64(hash, u64::from(signature.buffer_scale));
        hash = fnv1a_u64(hash, signature.buffer_transform as u32 as u64);
        hash = fnv1a_u64(hash, signature.x as u32 as u64);
        hash = fnv1a_u64(hash, signature.y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.width));
        hash = fnv1a_u64(hash, u64::from(signature.height));
        hash = fnv1a_u64(hash, signature.render_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.render_y as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_x as u32 as u64);
        hash = fnv1a_u64(hash, signature.clip_y as u32 as u64);
        hash = fnv1a_u64(hash, u64::from(signature.clip_width));
        hash = fnv1a_u64(hash, u64::from(signature.clip_height));
    }
    hash
}

pub(in crate::egl_renderer) fn egl_decoration_signature_hash(
    instances: &[DecorationRenderInstance],
) -> u64 {
    let snapshots = instances
        .iter()
        .map(DecorationRenderInstance::scene_snapshot)
        .collect::<Vec<_>>();
    egl_decoration_snapshot_signature_hash(&snapshots)
}

pub(in crate::egl_renderer) fn egl_decoration_snapshot_signature_hash(
    snapshots: &[DecorationSceneSnapshot],
) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for snapshot in snapshots {
        let (window_id, root_surface_id) = snapshot.identity();
        let (x, y, width, height) = snapshot.bounds();
        for value in [
            window_id.get(),
            u64::from(root_surface_id),
            x as u64,
            y as u64,
            u64::from(width),
            u64::from(height),
            snapshot.visual_signature(),
        ] {
            hash = fnv1a_u64(hash, value);
        }
    }
    hash
}

pub(in crate::egl_renderer) const fn fnv1a_u64(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(0x0000_0100_0000_01b3)
}
