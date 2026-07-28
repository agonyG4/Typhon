use super::*;
use std::{
    collections::VecDeque,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use oblivion_one::render_backend::buffer::DmabufBufferHandle;

pub(crate) fn direct_cursor_content_key(
    cursor: Option<&AtomicCursorVisualState>,
    compatible: bool,
) -> Option<CursorContentKey> {
    compatible.then(|| {
        cursor.map_or(
            CursorContentKey {
                visible: false,
                framebuffer_id: None,
                image_generation: 0,
                hotspot_x: 0,
                hotspot_y: 0,
                width: 0,
                height: 0,
            },
            CursorContentKey::from_state,
        )
    })
}

pub(crate) fn direct_cursor_atomic_validation_key(
    cursor: Option<&AtomicCursorVisualState>,
    compatible: bool,
    cursor_plane_id: Option<u32>,
) -> Option<CursorAtomicValidationKey> {
    compatible
        .then_some((cursor?, cursor_plane_id?))
        .map(|(cursor, cursor_plane_id)| {
            CursorAtomicValidationKey::from_state(cursor, cursor_plane_id)
        })
}

/// Identity of the cursor image used for visual-content suppression.
///
/// Cursor movement is intentionally absent: moving an unchanged image does
/// not change the cursor buffer's content identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CursorContentKey {
    pub(crate) visible: bool,
    pub(crate) framebuffer_id: Option<u32>,
    pub(crate) image_generation: u64,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl CursorContentKey {
    pub(crate) fn from_state(state: &oblivion_one::native::kms::AtomicCursorVisualState) -> Self {
        Self {
            visible: state.visible,
            framebuffer_id: state.framebuffer_id,
            image_generation: state.image_generation,
            hotspot_x: state.hotspot_x,
            hotspot_y: state.hotspot_y,
            width: state.width,
            height: state.height,
        }
    }
}

/// Complete cursor assignment identity used by an atomic validation request.
///
/// This is deliberately distinct from [`CursorContentKey`]. Every field here
/// is part of the cursor plane state sent to KMS, including position and the
/// selected cursor plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CursorAtomicValidationKey {
    pub(crate) content: CursorContentKey,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) cursor_plane_id: u32,
}

impl CursorAtomicValidationKey {
    pub(crate) fn from_state(
        state: &oblivion_one::native::kms::AtomicCursorVisualState,
        cursor_plane_id: u32,
    ) -> Self {
        Self {
            content: CursorContentKey::from_state(state),
            x: state.x,
            y: state.y,
            cursor_plane_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectPlaneValidationKey {
    /// The DRM generation identifies the currently bound KMS target.
    pub(crate) output_generation: u64,
    /// The CRTC is part of the atomic object assignment.
    pub(crate) crtc_id: u32,
    /// The selected primary plane determines the accepted plane properties.
    pub(crate) primary_plane_id: u32,
    /// The active mode dimensions are part of the identity fullscreen geometry.
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    /// The FourCC selects the primary-plane framebuffer format.
    pub(crate) format: u32,
    /// The modifier selects the framebuffer memory layout accepted by KMS.
    pub(crate) modifier: u64,
    /// Buffer dimensions are carried by the direct candidate and affect geometry.
    pub(crate) buffer_width: u32,
    pub(crate) buffer_height: u32,
    /// Stable plane offsets, strides, count, and modifiers used by AddFB2.
    pub(crate) plane_layout_hash: u64,
    /// The complete cursor assignment used by atomic TEST_ONLY, if present.
    pub(crate) cursor_atomic_key: Option<CursorAtomicValidationKey>,
    /// Stable input-fence, explicit-sync, and release-fence contract bits.
    pub(crate) synchronization_key: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectValidationReleaseMode {
    Pageflip,
    OutFence,
}

pub(crate) const fn synchronization_contract_key(
    input_fence_required: bool,
    explicit_sync_active: bool,
    release_mode: DirectValidationReleaseMode,
) -> u64 {
    (if input_fence_required { 1 } else { 0 })
        | ((if explicit_sync_active { 1 } else { 0 }) << 1)
        | ((if matches!(release_mode, DirectValidationReleaseMode::OutFence) {
            1
        } else {
            0
        }) << 2)
}

pub(crate) fn plane_layout_hash(buffer: &DmabufBufferHandle) -> u64 {
    let mut hasher = DefaultHasher::new();
    buffer.planes().len().hash(&mut hasher);
    for plane in buffer.planes() {
        let descriptor = plane.descriptor();
        descriptor.plane_index.hash(&mut hasher);
        descriptor.offset.hash(&mut hasher);
        descriptor.stride.hash(&mut hasher);
        descriptor.modifier.0.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
pub(crate) fn test_validation_key(seed: u64) -> DirectPlaneValidationKey {
    DirectPlaneValidationKey {
        output_generation: seed,
        crtc_id: 7,
        primary_plane_id: 11,
        mode_width: 1920,
        mode_height: 1080,
        format: 0x3432_5241,
        modifier: 0,
        buffer_width: 1920,
        buffer_height: 1080,
        plane_layout_hash: 0x1000,
        cursor_atomic_key: None,
        synchronization_key: 0x2000,
    }
}

#[derive(Debug, Default)]
pub(crate) struct DirectPlaneValidationCache {
    entries: VecDeque<DirectPlaneValidationKey>,
}

impl DirectPlaneValidationCache {
    pub(crate) const CAPACITY: usize = 8;

    pub(crate) fn contains(&self, key: DirectPlaneValidationKey) -> bool {
        self.entries.contains(&key)
    }

    pub(crate) fn record_success(&mut self, key: DirectPlaneValidationKey) {
        if let Some(index) = self.entries.iter().position(|entry| *entry == key) {
            self.entries.remove(index);
        }
        self.entries.push_back(key);
        while self.entries.len() > Self::CAPACITY {
            self.entries.pop_front();
        }
    }

    pub(crate) fn invalidate(&mut self, key: DirectPlaneValidationKey) {
        self.entries.retain(|entry| *entry != key);
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::render_backend::buffer::{
        BufferSize, DmabufBufferHandle, DmabufPlane, DmabufPlaneDescriptor, DrmFormat, DrmModifier,
    };
    use std::fs::File;
    use std::os::fd::OwnedFd;

    fn key(seed: u64) -> DirectPlaneValidationKey {
        DirectPlaneValidationKey {
            output_generation: seed,
            crtc_id: 7,
            primary_plane_id: 11,
            mode_width: 1920,
            mode_height: 1080,
            format: DrmFormat::XRGB8888_FOURCC,
            modifier: 0,
            buffer_width: 1920,
            buffer_height: 1080,
            plane_layout_hash: 0x1000,
            cursor_atomic_key: None,
            synchronization_key: 0x2000,
        }
    }

    fn buffer_with_fd() -> DmabufBufferHandle {
        DmabufBufferHandle::new(
            BufferSize::new(4, 4).expect("test buffer size"),
            DrmFormat::Xrgb8888,
            vec![DmabufPlane::new(
                OwnedFd::from(File::open("/dev/null").expect("test dma-buf fd")),
                DmabufPlaneDescriptor {
                    plane_index: 0,
                    offset: 0,
                    stride: 16,
                    modifier: DrmModifier::LINEAR,
                },
            )],
        )
        .expect("test dma-buf")
    }

    #[test]
    fn validation_key_changes_with_output_generation() {
        assert_ne!(key(1), key(2));
    }

    #[test]
    fn validation_key_changes_with_cursor_assignment() {
        let mut changed = key(1);
        changed.cursor_atomic_key = Some(CursorAtomicValidationKey {
            content: CursorContentKey {
                visible: true,
                framebuffer_id: Some(1),
                image_generation: 1,
                hotspot_x: 0,
                hotspot_y: 0,
                width: 1,
                height: 1,
            },
            x: 0,
            y: 0,
            cursor_plane_id: 1,
        });
        assert_ne!(key(1), changed);
    }

    #[test]
    fn validation_key_changes_with_modifier_and_layout() {
        let mut modifier_changed = key(1);
        modifier_changed.modifier = 7;
        assert_ne!(key(1), modifier_changed);

        let mut layout_changed = key(1);
        layout_changed.plane_layout_hash = 8;
        assert_ne!(key(1), layout_changed);
    }

    #[test]
    fn validation_key_changes_with_mode() {
        let mut width_changed = key(1);
        width_changed.mode_width = 2560;
        assert_ne!(key(1), width_changed);

        let mut height_changed = key(1);
        height_changed.mode_height = 1440;
        assert_ne!(key(1), height_changed);
    }

    #[test]
    fn validation_key_changes_with_plane_identity() {
        let mut crtc_changed = key(1);
        crtc_changed.crtc_id = 8;
        assert_ne!(key(1), crtc_changed);

        let mut plane_changed = key(1);
        plane_changed.primary_plane_id = 12;
        assert_ne!(key(1), plane_changed);
    }

    #[test]
    fn validation_key_changes_with_synchronization_contract() {
        let mut changed = key(1);
        changed.synchronization_key = 0x3000;
        assert_ne!(key(1), changed);
    }

    #[test]
    fn equivalent_layouts_with_different_fd_numbers_have_the_same_hash() {
        let first = buffer_with_fd();
        let second = buffer_with_fd();
        assert_eq!(plane_layout_hash(&first), plane_layout_hash(&second));
    }

    #[test]
    fn positive_cache_is_bounded_to_eight_entries() {
        let mut cache = DirectPlaneValidationCache::default();
        for seed in 1..=9 {
            cache.record_success(key(seed));
        }

        assert_eq!(
            (1..=9).filter(|seed| cache.contains(key(*seed))).count(),
            DirectPlaneValidationCache::CAPACITY
        );
        assert!(!cache.contains(key(1)));
        for seed in 2..=9 {
            assert!(cache.contains(key(seed)));
        }
    }

    #[test]
    fn recording_existing_key_moves_it_to_newest_position() {
        let mut cache = DirectPlaneValidationCache::default();
        for seed in 1..=8 {
            cache.record_success(key(seed));
        }
        cache.record_success(key(1));
        cache.record_success(key(9));

        assert!(!cache.contains(key(2)));
        assert!(cache.contains(key(1)));
        for seed in 3..=9 {
            assert!(cache.contains(key(seed)));
        }
    }

    #[test]
    fn real_submit_rejection_invalidates_matching_entry() {
        let mut cache = DirectPlaneValidationCache::default();
        cache.record_success(key(4));
        cache.invalidate(key(4));
        assert!(!cache.contains(key(4)));
    }

    #[test]
    fn invalidation_does_not_remove_unrelated_entries() {
        let mut cache = DirectPlaneValidationCache::default();
        cache.record_success(key(4));
        cache.record_success(key(5));
        cache.invalidate(key(4));
        assert!(!cache.contains(key(4)));
        assert!(cache.contains(key(5)));
    }

    #[test]
    fn output_rebuild_invalidates_all_entries() {
        let mut cache = DirectPlaneValidationCache::default();
        for seed in 1..=3 {
            cache.record_success(key(seed));
        }
        cache.invalidate_all();
        for seed in 1..=3 {
            assert!(!cache.contains(key(seed)));
        }
    }

    #[test]
    fn test_only_rejection_is_not_cached() {
        let cache = DirectPlaneValidationCache::default();
        assert!(!cache.contains(key(1)));
    }

    fn cursor_state() -> oblivion_one::native::kms::AtomicCursorVisualState {
        oblivion_one::native::kms::AtomicCursorVisualState {
            visible: true,
            x: 10,
            y: 20,
            hotspot_x: 3,
            hotspot_y: 4,
            width: 32,
            height: 24,
            framebuffer_id: Some(41),
            image_generation: 7,
        }
    }

    fn atomic_cursor_key() -> CursorAtomicValidationKey {
        CursorAtomicValidationKey::from_state(&cursor_state(), 99)
    }

    #[test]
    fn cursor_content_key_can_ignore_position() {
        let first = cursor_state();
        let mut second = first.clone();
        second.x = 100;
        second.y = 200;

        assert_eq!(
            CursorContentKey::from_state(&first),
            CursorContentKey::from_state(&second)
        );
    }

    #[test]
    fn cursor_atomic_validation_key_never_ignores_position() {
        let first = atomic_cursor_key();
        let mut moved = cursor_state();
        moved.x += 1;
        assert_ne!(first, CursorAtomicValidationKey::from_state(&moved, 99));

        let mut moved_y = cursor_state();
        moved_y.y += 1;
        assert_ne!(first, CursorAtomicValidationKey::from_state(&moved_y, 99));
    }

    macro_rules! cursor_atomic_key_changes_with {
        ($name:ident, $field:ident, $value:expr) => {
            #[test]
            fn $name() {
                let first = atomic_cursor_key();
                let mut changed = cursor_state();
                changed.$field = $value;
                assert_ne!(first, CursorAtomicValidationKey::from_state(&changed, 99));
            }
        };
    }

    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_x, x, 11);
    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_y, y, 21);
    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_hotspot_x, hotspot_x, 5);
    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_hotspot_y, hotspot_y, 6);
    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_width, width, 33);
    cursor_atomic_key_changes_with!(cursor_validation_key_changes_with_height, height, 25);
    cursor_atomic_key_changes_with!(
        cursor_validation_key_changes_with_framebuffer,
        framebuffer_id,
        Some(42)
    );
    cursor_atomic_key_changes_with!(
        cursor_validation_key_changes_with_visibility,
        visible,
        false
    );
}
