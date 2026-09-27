use std::collections::{HashMap, VecDeque};

use oblivion_one::{
    compositor::DrmContentType,
    core::OutputId,
    native::kms::{AtomicPipelineProperties, AtomicPlaneGeometry},
    render_backend::buffer::DmabufBufferHandle,
};

use super::direct_validation::CursorAtomicValidationKey;
use super::direct_validation::plane_layout_hash;

const SCALED_PROBE_CACHE_CAPACITY: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ScaledPrimaryProbeKey {
    pub(crate) output_id: OutputId,
    pub(crate) drm_generation: u64,
    pub(crate) output_generation: u64,
    pub(crate) connector_id: u32,
    pub(crate) connector_content_type_property: Option<u32>,
    pub(crate) connector_content_type_value: Option<u64>,
    pub(crate) crtc_id: u32,
    pub(crate) primary_plane_id: u32,
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) plane_layout_hash: u64,
    pub(crate) content_type: u64,
    pub(crate) primary_in_fence_property: Option<u32>,
    pub(crate) primary_rotation_property: Option<u32>,
    pub(crate) cursor_atomic_key: Option<CursorAtomicValidationKey>,
}

impl ScaledPrimaryProbeKey {
    pub(crate) fn new(
        output_id: OutputId,
        drm_generation: u64,
        output_generation: u64,
        pipeline: &AtomicPipelineProperties,
        mode_width: u32,
        mode_height: u32,
        buffer: &DmabufBufferHandle,
        content_type: DrmContentType,
        cursor_atomic_key: Option<CursorAtomicValidationKey>,
    ) -> Option<Self> {
        let modifier = buffer.planes().first()?.descriptor().modifier.0;
        Some(Self {
            output_id,
            drm_generation,
            output_generation,
            connector_id: pipeline.connector.get(),
            connector_content_type_property: pipeline
                .connector_props
                .content_type
                .map(|property| property.0.get()),
            connector_content_type_value: pipeline
                .connector_props
                .content_type_value(content_type.as_str()),
            crtc_id: pipeline.crtc.get(),
            primary_plane_id: pipeline.plane.get(),
            mode_width,
            mode_height,
            format: buffer.format().as_fourcc(),
            modifier,
            source_width: buffer.size().width,
            source_height: buffer.size().height,
            plane_layout_hash: plane_layout_hash(buffer),
            content_type: content_type as u64,
            primary_in_fence_property: pipeline
                .plane_props
                .in_fence_fd
                .map(|property| property.0.get()),
            primary_rotation_property: pipeline
                .plane_props
                .rotation
                .map(|property| property.0.get()),
            cursor_atomic_key,
        })
    }

    pub(crate) fn geometry(
        self,
    ) -> Result<AtomicPlaneGeometry, oblivion_one::native::kms::AtomicKmsError> {
        AtomicPlaneGeometry::full_source_to_output(
            self.source_width,
            self.source_height,
            self.mode_width,
            self.mode_height,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScaledPrimaryProbeResult {
    Supported,
    Rejected(String),
    ImportFailed(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScaledPrimaryProbeCounters {
    pub(crate) candidate_observations: u64,
    pub(crate) test_only_attempts: u64,
    pub(crate) accepted: u64,
    pub(crate) rejected: u64,
    pub(crate) cache_hits: u64,
    pub(crate) busy_skips: u64,
    pub(crate) unavailable_skips: u64,
    pub(crate) import_failures: u64,
    pub(crate) buffer_size_mismatch_rejections: u64,
}

#[derive(Debug, Default)]
pub(crate) struct ScaledPrimaryProbeCache {
    entries: HashMap<ScaledPrimaryProbeKey, ScaledPrimaryProbeResult>,
    insertion_order: VecDeque<ScaledPrimaryProbeKey>,
    counters: ScaledPrimaryProbeCounters,
}

impl ScaledPrimaryProbeCache {
    pub(crate) fn observe_candidate(&mut self) {
        self.counters.candidate_observations =
            self.counters.candidate_observations.saturating_add(1);
    }

    pub(crate) fn record_buffer_size_mismatch_rejection(&mut self) {
        self.counters.buffer_size_mismatch_rejections = self
            .counters
            .buffer_size_mismatch_rejections
            .saturating_add(1);
    }

    pub(crate) fn lookup(
        &mut self,
        key: ScaledPrimaryProbeKey,
    ) -> Option<ScaledPrimaryProbeResult> {
        let result = self.entries.get(&key).cloned();
        if result.is_some() {
            self.counters.cache_hits = self.counters.cache_hits.saturating_add(1);
        }
        result
    }

    pub(crate) fn record_test_result(
        &mut self,
        key: ScaledPrimaryProbeKey,
        result: ScaledPrimaryProbeResult,
    ) {
        self.counters.test_only_attempts = self.counters.test_only_attempts.saturating_add(1);
        match &result {
            ScaledPrimaryProbeResult::Supported => {
                self.counters.accepted = self.counters.accepted.saturating_add(1);
            }
            ScaledPrimaryProbeResult::Rejected(_) => {
                self.counters.rejected = self.counters.rejected.saturating_add(1);
            }
            ScaledPrimaryProbeResult::ImportFailed(_) => {
                unreachable!("framebuffer import failures are not TEST_ONLY results")
            }
        }
        self.insert(key, result);
    }

    pub(crate) fn record_import_failure(
        &mut self,
        key: ScaledPrimaryProbeKey,
        detail: String,
    ) -> ScaledPrimaryProbeResult {
        self.counters.import_failures = self.counters.import_failures.saturating_add(1);
        let result = ScaledPrimaryProbeResult::ImportFailed(detail);
        self.insert(key, result.clone());
        result
    }

    pub(crate) fn record_busy_skip(&mut self) {
        self.counters.busy_skips = self.counters.busy_skips.saturating_add(1);
    }

    pub(crate) fn record_unavailable_skip(&mut self) {
        self.counters.unavailable_skips = self.counters.unavailable_skips.saturating_add(1);
    }

    pub(crate) const fn counters(&self) -> ScaledPrimaryProbeCounters {
        self.counters
    }

    fn insert(&mut self, key: ScaledPrimaryProbeKey, result: ScaledPrimaryProbeResult) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key, result);
            return;
        }
        while self.entries.len() >= SCALED_PROBE_CACHE_CAPACITY {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.insertion_order.push_back(key);
        self.entries.insert(key, result);
    }
}

pub(crate) fn scaled_primary_probe_enabled() -> bool {
    let value = std::env::var("TYPHON_SCALED_DIRECT_PROBE").ok();
    scaled_probe_enabled_value(value.as_deref())
}

fn scaled_probe_enabled_value(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(crate) fn log_scaled_probe_result(
    result: &ScaledPrimaryProbeResult,
    key: ScaledPrimaryProbeKey,
) {
    if std::env::var("TYPHON_DIRECT_SCANOUT_DEBUG").ok().as_deref() != Some("1") {
        return;
    }
    match result {
        ScaledPrimaryProbeResult::Supported => eprintln!(
            "direct scanout: scaled-primary probe result=supported source={}x{} output={}x{} format={:#010x} modifier={:#x} crtc={} plane={}",
            key.source_width,
            key.source_height,
            key.mode_width,
            key.mode_height,
            key.format,
            key.modifier,
            key.crtc_id,
            key.primary_plane_id,
        ),
        ScaledPrimaryProbeResult::Rejected(detail) => eprintln!(
            "direct scanout: scaled-primary probe result=rejected source={}x{} output={}x{} format={:#010x} modifier={:#x} crtc={} plane={} error={}",
            key.source_width,
            key.source_height,
            key.mode_width,
            key.mode_height,
            key.format,
            key.modifier,
            key.crtc_id,
            key.primary_plane_id,
            detail,
        ),
        ScaledPrimaryProbeResult::ImportFailed(detail) => eprintln!(
            "direct scanout: scaled-primary probe result=unavailable source={}x{} output={}x{} format={:#010x} modifier={:#x} crtc={} plane={} error={}",
            key.source_width,
            key.source_height,
            key.mode_width,
            key.mode_height,
            key.format,
            key.modifier,
            key.crtc_id,
            key.primary_plane_id,
            detail,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::core::OutputId;

    fn key() -> ScaledPrimaryProbeKey {
        ScaledPrimaryProbeKey {
            output_id: OutputId::from_raw(1).unwrap(),
            drm_generation: 5,
            output_generation: 6,
            connector_id: 10,
            connector_content_type_property: Some(100),
            connector_content_type_value: Some(0),
            crtc_id: 11,
            primary_plane_id: 12,
            mode_width: 1920,
            mode_height: 1080,
            format: 0x3432_5258,
            modifier: 7,
            source_width: 1600,
            source_height: 900,
            plane_layout_hash: 88,
            content_type: 0,
            primary_in_fence_property: Some(101),
            primary_rotation_property: None,
            cursor_atomic_key: None,
        }
    }

    #[test]
    fn supported_probe_is_cached_after_one_attempt() {
        let mut cache = ScaledPrimaryProbeCache::default();
        let key = key();
        cache.observe_candidate();
        assert_eq!(cache.lookup(key), None);
        cache.record_test_result(key, ScaledPrimaryProbeResult::Supported);
        assert_eq!(cache.lookup(key), Some(ScaledPrimaryProbeResult::Supported));
        assert_eq!(cache.counters().test_only_attempts, 1);
        assert_eq!(cache.counters().accepted, 1);
        assert_eq!(cache.counters().cache_hits, 1);
    }

    #[test]
    fn rejected_probe_is_cached_without_repeating_test_only() {
        let mut cache = ScaledPrimaryProbeCache::default();
        let key = key();
        cache.record_test_result(
            key,
            ScaledPrimaryProbeResult::Rejected("unsupported".into()),
        );
        assert_eq!(
            cache.lookup(key),
            Some(ScaledPrimaryProbeResult::Rejected("unsupported".into()))
        );
        assert_eq!(cache.counters().test_only_attempts, 1);
        assert_eq!(cache.counters().rejected, 1);
    }

    #[test]
    fn output_generation_mode_format_and_layout_invalidate_probe_identity() {
        let key = key();
        let changed = [
            ScaledPrimaryProbeKey {
                output_id: OutputId::from_raw(2).unwrap(),
                ..key
            },
            ScaledPrimaryProbeKey {
                drm_generation: 6,
                ..key
            },
            ScaledPrimaryProbeKey {
                output_generation: 7,
                ..key
            },
            ScaledPrimaryProbeKey {
                connector_id: 19,
                ..key
            },
            ScaledPrimaryProbeKey {
                connector_content_type_property: Some(200),
                ..key
            },
            ScaledPrimaryProbeKey {
                connector_content_type_value: Some(2),
                ..key
            },
            ScaledPrimaryProbeKey { crtc_id: 13, ..key },
            ScaledPrimaryProbeKey {
                primary_plane_id: 14,
                ..key
            },
            ScaledPrimaryProbeKey {
                mode_width: 2560,
                ..key
            },
            ScaledPrimaryProbeKey {
                mode_height: 1440,
                ..key
            },
            ScaledPrimaryProbeKey {
                format: 0x3432_5241,
                ..key
            },
            ScaledPrimaryProbeKey { modifier: 8, ..key },
            ScaledPrimaryProbeKey {
                source_width: 1280,
                ..key
            },
            ScaledPrimaryProbeKey {
                source_height: 720,
                ..key
            },
            ScaledPrimaryProbeKey {
                plane_layout_hash: 89,
                ..key
            },
            ScaledPrimaryProbeKey {
                content_type: 1,
                ..key
            },
            ScaledPrimaryProbeKey {
                primary_in_fence_property: None,
                ..key
            },
            ScaledPrimaryProbeKey {
                primary_rotation_property: Some(102),
                ..key
            },
        ];
        let mut cache = ScaledPrimaryProbeCache::default();
        cache.record_test_result(key, ScaledPrimaryProbeResult::Supported);
        for changed in changed {
            assert_eq!(cache.lookup(changed), None);
        }
    }

    #[test]
    fn opt_in_requires_the_exact_environment_value() {
        assert!(scaled_probe_enabled_value(Some("1")));
        assert!(!scaled_probe_enabled_value(None));
        assert!(!scaled_probe_enabled_value(Some("true")));
        assert!(!scaled_probe_enabled_value(Some("01")));
        let cache = ScaledPrimaryProbeCache::default();
        assert_eq!(cache.counters(), ScaledPrimaryProbeCounters::default());
    }

    #[test]
    fn busy_skip_does_not_cache_a_probe_result() {
        let mut cache = ScaledPrimaryProbeCache::default();
        let key = key();
        cache.record_busy_skip();

        assert_eq!(cache.lookup(key), None);
        assert_eq!(cache.counters().busy_skips, 1);
        assert_eq!(cache.counters().test_only_attempts, 0);
    }
}
