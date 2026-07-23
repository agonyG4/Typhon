use std::sync::atomic::{AtomicU64, Ordering};

use super::{NativeScanoutBackend, OwnCompositorServer};

pub(crate) fn apply_native_scanout_feedback(
    server: &mut OwnCompositorServer,
    scanout: &NativeScanoutBackend,
) {
    server.set_dmabuf_feedback(
        scanout.dmabuf_feedback(),
        scanout.dmabuf_main_device(),
        scanout.dmabuf_main_device_path(),
    );
}

pub(crate) static NEXT_NATIVE_PAGE_FLIP_TOKEN: AtomicU64 = AtomicU64::new(1);
pub(crate) static NEXT_NATIVE_DRM_FILE_GENERATION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn allocate_native_page_flip_token() -> u64 {
    loop {
        let current = NEXT_NATIVE_PAGE_FLIP_TOKEN.load(Ordering::Relaxed);
        let token = current.max(1);
        let next = next_nonzero_page_flip_token(token);
        if NEXT_NATIVE_PAGE_FLIP_TOKEN
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return token;
        }
    }
}

pub(crate) fn allocate_native_drm_file_generation() -> u64 {
    loop {
        let current = NEXT_NATIVE_DRM_FILE_GENERATION.load(Ordering::Relaxed);
        let generation = current.max(1);
        let next = generation.checked_add(1).unwrap_or(1);
        if NEXT_NATIVE_DRM_FILE_GENERATION
            .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return generation;
        }
    }
}

pub(crate) const fn next_nonzero_page_flip_token(token: u64) -> u64 {
    let next = token.wrapping_add(1);
    if next == 0 { 1 } else { next }
}
