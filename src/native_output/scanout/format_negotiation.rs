use std::io;

use oblivion_one::{
    native::kms::DrmFormatModifierPair,
    render_backend::{
        buffer::{DrmFormat, DrmModifier},
        egl_gles::EglGlesDmabufFormat,
    },
};

pub(crate) trait GbmAllocationProbe {
    fn supports(&mut self, candidate: DrmFormatModifierPair) -> bool;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ExplicitFramebufferPlane {
    pub(crate) handle: u32,
    pub(crate) pitch: u32,
    pub(crate) offset: u32,
    pub(crate) modifier: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitFramebufferDescriptor {
    width: u32,
    height: u32,
    format: u32,
    plane_count: usize,
    handles: [u32; 4],
    pitches: [u32; 4],
    offsets: [u32; 4],
    modifiers: [u64; 4],
}

impl ExplicitFramebufferDescriptor {
    pub(crate) fn new(
        width: u32,
        height: u32,
        format: u32,
        planes: &[ExplicitFramebufferPlane],
    ) -> io::Result<Self> {
        if width == 0 || height == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "explicit framebuffer dimensions must be nonzero",
            ));
        }
        if !(1..=4).contains(&planes.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "explicit framebuffer plane count must be between one and four",
            ));
        }
        let mut handles = [0; 4];
        let mut pitches = [0; 4];
        let mut offsets = [0; 4];
        let mut modifiers = [0; 4];
        for (index, plane) in planes.iter().copied().enumerate() {
            if plane.handle == 0 || plane.pitch == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "explicit framebuffer plane handle and pitch must be nonzero",
                ));
            }
            if plane.modifier == DrmModifier::INVALID.0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "explicit framebuffer modifier must not be DRM_FORMAT_MOD_INVALID",
                ));
            }
            handles[index] = plane.handle;
            pitches[index] = plane.pitch;
            offsets[index] = plane.offset;
            modifiers[index] = plane.modifier;
        }
        Ok(Self {
            width,
            height,
            format,
            plane_count: planes.len(),
            handles,
            pitches,
            offsets,
            modifiers,
        })
    }

    pub(crate) const fn plane_count(&self) -> usize {
        self.plane_count
    }

    pub(crate) const fn width(&self) -> u32 {
        self.width
    }

    pub(crate) const fn height(&self) -> u32 {
        self.height
    }

    pub(crate) const fn format(&self) -> u32 {
        self.format
    }

    pub(crate) const fn handles(&self) -> &[u32; 4] {
        &self.handles
    }

    pub(crate) const fn pitches(&self) -> &[u32; 4] {
        &self.pitches
    }

    pub(crate) const fn offsets(&self) -> &[u32; 4] {
        &self.offsets
    }

    pub(crate) const fn modifiers(&self) -> &[u64; 4] {
        &self.modifiers
    }

    pub(crate) const fn flags(&self) -> u32 {
        drm_sys::DRM_MODE_FB_MODIFIERS
    }
}

pub(crate) trait ExplicitFramebufferRegistration {
    fn add(&mut self, descriptor: &ExplicitFramebufferDescriptor) -> io::Result<u32>;
    fn remove(&mut self, framebuffer: u32);
}

pub(crate) fn register_explicit_framebuffers(
    registration: &mut impl ExplicitFramebufferRegistration,
    descriptors: &[ExplicitFramebufferDescriptor],
) -> io::Result<Vec<u32>> {
    let mut framebuffers = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        match registration.add(descriptor) {
            Ok(framebuffer) if framebuffer != 0 => framebuffers.push(framebuffer),
            Ok(_) => {
                for framebuffer in framebuffers.drain(..).rev() {
                    registration.remove(framebuffer);
                }
                return Err(io::Error::other("AddFB2 returned framebuffer ID zero"));
            }
            Err(error) => {
                for framebuffer in framebuffers.drain(..).rev() {
                    registration.remove(framebuffer);
                }
                return Err(error);
            }
        }
    }
    Ok(framebuffers)
}

pub(crate) fn select_output_format_modifier(
    drm: &[DrmFormatModifierPair],
    egl: &[EglGlesDmabufFormat],
    gbm: &mut impl GbmAllocationProbe,
) -> io::Result<DrmFormatModifierPair> {
    let mut candidates = drm
        .iter()
        .copied()
        .filter(|candidate| candidate.modifier != DrmModifier::INVALID.0)
        .filter(|candidate| {
            egl.iter().any(|renderable| {
                renderable.format.as_fourcc() == candidate.fourcc
                    && renderable.modifier.0 == candidate.modifier
            })
        })
        .filter_map(|candidate| preference_key(candidate).map(|key| (key, candidate)))
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(key, _)| *key);
    candidates.dedup_by_key(|(_, candidate)| *candidate);

    let tested_count = candidates.len();
    for (_, candidate) in candidates {
        if gbm.supports(candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::other(format!(
        "no explicit output format/modifier intersection: drm_candidates={} egl_candidates={} tested_pairs={tested_count}",
        drm.len(),
        egl.len(),
    )))
}

fn preference_key(candidate: DrmFormatModifierPair) -> Option<(u8, u64)> {
    let linear = candidate.modifier == DrmModifier::LINEAR.0;
    match (candidate.fourcc, linear) {
        (DrmFormat::XRGB8888_FOURCC, false) => Some((0, candidate.modifier)),
        (DrmFormat::ARGB8888_FOURCC, false) => Some((1, candidate.modifier)),
        (DrmFormat::XRGB8888_FOURCC, true) => Some((2, candidate.modifier)),
        (DrmFormat::ARGB8888_FOURCC, true) => Some((3, candidate.modifier)),
        _ => None,
    }
}
