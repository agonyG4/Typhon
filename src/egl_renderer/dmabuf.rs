use std::{
    ffi::{CStr, c_char, c_void},
    fs,
    os::unix::fs::MetadataExt,
    ptr,
};

use khronos_egl as egl;
use oblivion_one::render_backend::{
    buffer::{DrmFormat, DrmModifier},
    egl_gles::{EglGlesDmabufFeedback, EglGlesDmabufFormat},
};

use super::EglInstance;

type EglQueryDmaBufFormatsExt = unsafe extern "system" fn(
    egl::EGLDisplay,
    egl::Int,
    *mut egl::Int,
    *mut egl::Int,
) -> egl::Boolean;
type EglQueryDmaBufModifiersExt = unsafe extern "system" fn(
    egl::EGLDisplay,
    egl::Int,
    egl::Int,
    *mut u64,
    *mut egl::Boolean,
    *mut egl::Int,
) -> egl::Boolean;
type EglQueryDisplayAttribExt =
    unsafe extern "system" fn(egl::EGLDisplay, egl::Int, *mut egl::Attrib) -> egl::Boolean;
type EglQueryDeviceStringExt = unsafe extern "system" fn(*mut c_void, egl::Int) -> *const c_char;

const EGL_DEVICE_EXT: egl::Int = 0x322c;
const EGL_DRM_DEVICE_FILE_EXT: egl::Int = 0x3233;
const EGL_DRM_RENDER_NODE_FILE_EXT: egl::Int = 0x3377;

pub(super) fn query_egl_dmabuf_feedback(
    egl: &EglInstance,
    display: egl::Display,
) -> EglGlesDmabufFeedback {
    let Some((query_formats, query_modifiers)) = load_dmabuf_modifier_queries(egl, display) else {
        return EglGlesDmabufFeedback::default();
    };

    let mut format_count = 0;
    let ok = unsafe { query_formats(display.as_ptr(), 0, ptr::null_mut(), &mut format_count) };
    if ok != egl::TRUE || format_count <= 0 {
        return EglGlesDmabufFeedback::default();
    }

    let mut raw_formats = vec![0; format_count as usize];
    let ok = unsafe {
        query_formats(
            display.as_ptr(),
            format_count,
            raw_formats.as_mut_ptr(),
            &mut format_count,
        )
    };
    if ok != egl::TRUE {
        return EglGlesDmabufFeedback::default();
    }

    let queried_formats = raw_formats
        .into_iter()
        .take(format_count as usize)
        .map(|raw_format| {
            (
                DrmFormat::from_fourcc(raw_format as u32),
                query_egl_modifiers_for_format(query_modifiers, display, raw_format),
            )
        })
        .collect::<Vec<_>>();
    let has_nvidia_modifiers = queried_formats
        .iter()
        .flat_map(|(_, modifiers)| modifiers)
        .any(|modifier| is_nvidia_block_linear_modifier(modifier.modifier));

    let mut tranche_formats = Vec::new();
    for (drm_format, modifiers) in queried_formats {
        let mut has_tranche_modifier = false;
        for modifier in modifiers {
            if !modifier.external_only {
                tranche_formats.push(EglGlesDmabufFormat::new(drm_format, modifier.modifier));
                has_tranche_modifier = true;
            }
        }
        if has_tranche_modifier
            || (has_nvidia_modifiers
                && nvidia_implicit_dmabuf_tranche_format(drm_format.as_fourcc()))
        {
            tranche_formats.push(EglGlesDmabufFormat::new(drm_format, DrmModifier::INVALID));
        }
    }

    if tranche_formats.is_empty() {
        EglGlesDmabufFeedback::default()
    } else {
        tranche_formats.sort_by_key(preferred_dmabuf_format_key);
        let mut table_formats = tranche_formats.clone();
        if has_nvidia_modifiers {
            table_formats.extend(nvidia_unindexed_dmabuf_table_tail());
        }
        EglGlesDmabufFeedback::from_table_and_tranche_formats(table_formats, tranche_formats)
    }
}

#[derive(Debug, Clone, Copy)]
struct EglQueriedModifier {
    modifier: DrmModifier,
    external_only: bool,
}

fn preferred_dmabuf_format_key(format: &EglGlesDmabufFormat) -> (u8, u32, u64) {
    (
        preferred_dmabuf_fourcc_rank(format.format.as_fourcc()),
        format.format.as_fourcc(),
        preferred_dmabuf_modifier_rank(format.modifier),
    )
}

fn preferred_dmabuf_fourcc_rank(fourcc: u32) -> u8 {
    match fourcc {
        0x3432_4241 => 0, // AB24
        0x3432_4258 => 1, // XB24
        DrmFormat::ARGB8888_FOURCC => 2,
        DrmFormat::XRGB8888_FOURCC => 3,
        _ => 16,
    }
}

fn preferred_dmabuf_modifier_rank(modifier: DrmModifier) -> u64 {
    if modifier == DrmModifier::INVALID {
        u64::MAX
    } else {
        modifier.0
    }
}

fn is_nvidia_block_linear_modifier(modifier: DrmModifier) -> bool {
    (modifier.0 & 0xff00_0000_0000_0000) == 0x0300_0000_0000_0000
}

fn nvidia_implicit_dmabuf_tranche_format(fourcc: u32) -> bool {
    !matches!(
        fourcc,
        0x3834_4241 | // AB48
        0x5659_5559 | // YUYV
        0x3234_564e | // NV42
        0x3136_564e // NV61
    )
}

fn nvidia_unindexed_dmabuf_table_tail() -> Vec<EglGlesDmabufFormat> {
    const NVIDIA_UNINDEXED_FOURCCS: [u32; 12] = [
        0x3834_4241, // AB48
        0x5659_5559, // YUYV
        0x5956_5955, // UYVY
        0x3234_564e, // NV42
        0x3432_564e, // NV24
        0x3136_564e, // NV61
        0x3631_564e, // NV16
        0x3132_564e, // NV21
        0x3231_564e, // NV12
        0x3031_3250, // P210
        0x3031_3050, // P010
        0x3231_3050, // P012
    ];
    NVIDIA_UNINDEXED_FOURCCS
        .into_iter()
        .flat_map(|fourcc| {
            (0..=5).rev().map(move |height| {
                EglGlesDmabufFormat::new(
                    DrmFormat::from_fourcc(fourcc),
                    DrmModifier(0x0300_0000_0060_6010 | height),
                )
            })
        })
        .collect()
}

fn query_egl_modifiers_for_format(
    query_modifiers: EglQueryDmaBufModifiersExt,
    display: egl::Display,
    raw_format: egl::Int,
) -> Vec<EglQueriedModifier> {
    let mut modifier_count = 0;
    let ok = unsafe {
        query_modifiers(
            display.as_ptr(),
            raw_format,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut modifier_count,
        )
    };
    if ok != egl::TRUE || modifier_count <= 0 {
        return Vec::new();
    }

    let mut modifiers = vec![0; modifier_count as usize];
    let mut external_only = vec![egl::FALSE; modifier_count as usize];
    let ok = unsafe {
        query_modifiers(
            display.as_ptr(),
            raw_format,
            modifier_count,
            modifiers.as_mut_ptr(),
            external_only.as_mut_ptr(),
            &mut modifier_count,
        )
    };
    if ok != egl::TRUE {
        return Vec::new();
    }

    modifiers
        .into_iter()
        .zip(external_only)
        .take(modifier_count as usize)
        .filter_map(|(modifier, external_only)| {
            (modifier != DrmModifier::LINEAR.0).then_some(EglQueriedModifier {
                modifier: DrmModifier(modifier),
                external_only: external_only != egl::FALSE,
            })
        })
        .collect()
}

fn load_dmabuf_modifier_queries(
    egl: &EglInstance,
    display: egl::Display,
) -> Option<(EglQueryDmaBufFormatsExt, EglQueryDmaBufModifiersExt)> {
    let extensions = egl
        .query_string(Some(display), egl::EXTENSIONS)
        .ok()?
        .to_string_lossy();
    if !extensions.contains("EGL_EXT_image_dma_buf_import_modifiers") {
        return None;
    }

    let query_formats = egl.get_proc_address("eglQueryDmaBufFormatsEXT")?;
    let query_modifiers = egl.get_proc_address("eglQueryDmaBufModifiersEXT")?;
    Some(unsafe {
        (
            std::mem::transmute::<extern "system" fn(), EglQueryDmaBufFormatsExt>(query_formats),
            std::mem::transmute::<extern "system" fn(), EglQueryDmaBufModifiersExt>(
                query_modifiers,
            ),
        )
    })
}

pub(super) fn query_egl_main_device(
    egl: &EglInstance,
    display: egl::Display,
) -> Option<(String, u64)> {
    let query_display_attrib = egl.get_proc_address("eglQueryDisplayAttribEXT")?;
    let query_device_string = egl.get_proc_address("eglQueryDeviceStringEXT")?;
    let query_display_attrib = unsafe {
        std::mem::transmute::<extern "system" fn(), EglQueryDisplayAttribExt>(query_display_attrib)
    };
    let query_device_string = unsafe {
        std::mem::transmute::<extern "system" fn(), EglQueryDeviceStringExt>(query_device_string)
    };

    let mut device = 0;
    let ok = unsafe { query_display_attrib(display.as_ptr(), EGL_DEVICE_EXT, &mut device) };
    if ok != egl::TRUE || device == 0 {
        return None;
    }
    let device = device as *mut c_void;
    let path = query_egl_device_path(query_device_string, device, EGL_DRM_RENDER_NODE_FILE_EXT)
        .or_else(|| query_egl_device_path(query_device_string, device, EGL_DRM_DEVICE_FILE_EXT))?;
    let metadata = fs::metadata(&path).ok()?;
    let main_device = metadata.rdev();
    eprintln!("oblivion-one compositor: EGL dmabuf main device: {path} ({main_device})");
    Some((path, main_device))
}

fn query_egl_device_path(
    query_device_string: EglQueryDeviceStringExt,
    device: *mut c_void,
    name: egl::Int,
) -> Option<String> {
    let raw_path = unsafe { query_device_string(device, name) };
    if raw_path.is_null() {
        return None;
    }
    let path = unsafe { CStr::from_ptr(raw_path) }
        .to_string_lossy()
        .into_owned();
    (!path.is_empty()).then_some(path)
}
