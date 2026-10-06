use std::collections::{HashMap, HashSet};

use khronos_egl as egl;
use oblivion_one::compositor::{
    DecorationRenderInstance, DecorationRenderPrimitive, ServerFrameColor,
};
use oblivion_one::cursor_theme::CompositorCursorImage;

#[cfg(test)]
use super::super::EglDrawLayer;
use super::super::{EglInstance, RendererResult};
use super::image::{
    EglImageResource, create_uploaded_resource, destroy_image_resource,
    write_argb_pixels_to_resource, write_rgba_bytes_to_resource,
};
use super::upload::UploadScratch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DecorationResourceKey {
    Solid(u32),
    Asset(u64),
}

pub(super) struct DecorationResourceRequirements<'a> {
    pub(super) required: HashSet<DecorationResourceKey>,
    pub(super) required_assets: HashMap<u64, &'a oblivion_one::compositor::DecorationRasterAsset>,
}

pub(super) struct UiResourceStore {
    pub(super) cursor: Option<EglImageResource>,
    cursor_stale: bool,
    pub(super) frames: HashMap<ServerFrameColor, EglImageResource>,
    pub(super) decorations: HashMap<DecorationResourceKey, EglImageResource>,
}

impl Default for UiResourceStore {
    fn default() -> Self {
        Self {
            cursor: None,
            cursor_stale: false,
            frames: HashMap::new(),
            decorations: HashMap::new(),
        }
    }
}

impl UiResourceStore {
    pub(super) fn mark_cursor_stale(&mut self) {
        self.cursor_stale = true;
    }

    pub(super) fn ensure_cursor_resource(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        cursor_image: &CompositorCursorImage,
        upload: &mut UploadScratch,
    ) -> RendererResult<()> {
        let width = cursor_image.width;
        let height = cursor_image.height;
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|resource| resource.size == (width, height) && !self.cursor_stale)
        {
            return Ok(());
        }

        let mut resource = create_uploaded_resource(gl, width, height)?;
        write_argb_pixels_to_resource(
            gl,
            &resource,
            oblivion_one::compositor::SurfaceDamageRect::full(width, height),
            &cursor_image.pixels_argb8888,
            upload,
        );
        if let Some(old) = self.cursor.take() {
            destroy_image_resource(gl, egl, egl_display, old);
        }
        resource.generation = 1;
        self.cursor = Some(resource);
        self.cursor_stale = false;
        Ok(())
    }

    pub(super) fn ensure_frame_resources(
        &mut self,
        gl: &glow::Context,
        upload: &mut UploadScratch,
    ) -> RendererResult<()> {
        for color in ServerFrameColor::ALL {
            if self.frames.contains_key(&color) {
                continue;
            }

            let mut resource = create_uploaded_resource(gl, 1, 1)?;
            write_argb_pixels_to_resource(
                gl,
                &resource,
                oblivion_one::compositor::SurfaceDamageRect::full(1, 1),
                &[color.pixel()],
                upload,
            );
            resource.generation = 1;
            self.frames.insert(color, resource);
        }
        Ok(())
    }

    pub(super) fn ensure_decoration_resources<'a, I>(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
        instances: I,
        upload: &mut UploadScratch,
    ) -> RendererResult<()>
    where
        I: IntoIterator<Item = &'a DecorationRenderInstance>,
    {
        let DecorationResourceRequirements {
            required,
            required_assets,
        } = decoration_resource_requirements(
            instances
                .into_iter()
                .map(DecorationRenderInstance::primitives),
        );

        let stale = self
            .decorations
            .keys()
            .copied()
            .filter(|key| !required.contains(key))
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(resource) = self.decorations.remove(&key) {
                destroy_image_resource(gl, egl, egl_display, resource);
            }
        }

        for key in required {
            if self.decorations.contains_key(&key) {
                continue;
            }
            let mut resource = match key {
                DecorationResourceKey::Solid(color) => {
                    let resource = create_uploaded_resource(gl, 1, 1)?;
                    write_argb_pixels_to_resource(
                        gl,
                        &resource,
                        oblivion_one::compositor::SurfaceDamageRect::full(1, 1),
                        &[color],
                        upload,
                    );
                    resource
                }
                DecorationResourceKey::Asset(asset_id) => {
                    let asset = required_assets
                        .get(&asset_id)
                        .expect("required decoration asset was collected");
                    let resource = create_uploaded_resource(gl, asset.width(), asset.height())?;
                    write_rgba_bytes_to_resource(
                        gl,
                        &resource,
                        oblivion_one::compositor::SurfaceDamageRect::full(
                            asset.width(),
                            asset.height(),
                        ),
                        asset.rgba_premultiplied(),
                    );
                    resource
                }
            };
            resource.generation = 1;
            self.decorations.insert(key, resource);
        }
        Ok(())
    }

    pub(super) fn destroy_cursor(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        if let Some(resource) = self.cursor.take() {
            destroy_image_resource(gl, egl, egl_display, resource);
        }
    }

    pub(super) fn destroy_frames(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        for (_, resource) in self.frames.drain() {
            destroy_image_resource(gl, egl, egl_display, resource);
        }
    }

    pub(super) fn destroy_decorations(
        &mut self,
        gl: &glow::Context,
        egl: &EglInstance,
        egl_display: egl::Display,
    ) {
        for (_, resource) in self.decorations.drain() {
            destroy_image_resource(gl, egl, egl_display, resource);
        }
    }

    #[cfg(test)]
    pub(super) fn test_install_frame_texture(
        &mut self,
        color: ServerFrameColor,
        texture: glow::Texture,
    ) {
        self.frames.insert(color, test_image_resource(texture));
    }

    #[cfg(test)]
    pub(super) fn test_install_decoration_texture(
        &mut self,
        layer: EglDrawLayer,
        texture: glow::Texture,
    ) {
        let key = match layer {
            EglDrawLayer::SolidRgba(color) => DecorationResourceKey::Solid(color),
            EglDrawLayer::DecorationAsset(asset_id) => DecorationResourceKey::Asset(asset_id),
            _ => panic!("test decoration texture requires a decoration draw layer"),
        };
        self.decorations.insert(key, test_image_resource(texture));
    }

    #[cfg(test)]
    pub(super) fn test_install_cursor_texture(&mut self, texture: glow::Texture) {
        self.cursor = Some(test_image_resource(texture));
        self.cursor_stale = false;
    }
}

fn decoration_resource_requirements<'a, I>(primitive_sets: I) -> DecorationResourceRequirements<'a>
where
    I: IntoIterator<Item = &'a [DecorationRenderPrimitive]>,
{
    let mut required = HashSet::new();
    let mut required_assets = HashMap::new();
    for primitives in primitive_sets {
        for primitive in primitives {
            match primitive {
                DecorationRenderPrimitive::SolidRect { color, .. } => {
                    required.insert(DecorationResourceKey::Solid(argb_color_key(*color)));
                }
                DecorationRenderPrimitive::Image { asset, .. } => {
                    required.insert(DecorationResourceKey::Asset(asset.asset_id()));
                    required_assets.insert(asset.asset_id(), asset);
                }
                DecorationRenderPrimitive::Text { .. } => {}
            }
        }
    }
    DecorationResourceRequirements {
        required,
        required_assets,
    }
}

fn argb_color_key(color: [u8; 4]) -> u32 {
    (u32::from(color[3]) << 24)
        | (u32::from(color[0]) << 16)
        | (u32::from(color[1]) << 8)
        | u32::from(color[2])
}

#[cfg(test)]
pub(super) fn test_decoration_resource_requirements<'a, I>(
    primitive_sets: I,
) -> DecorationResourceRequirements<'a>
where
    I: IntoIterator<Item = &'a [DecorationRenderPrimitive]>,
{
    decoration_resource_requirements(primitive_sets)
}

#[cfg(test)]
fn test_image_resource(texture: glow::Texture) -> EglImageResource {
    EglImageResource {
        texture,
        size: (1, 1),
        generation: 1,
        egl_image: None,
    }
}
