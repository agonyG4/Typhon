use std::{collections::HashMap, num::NonZeroU64};

use oblivion_one::compositor::DirectScanoutSceneCandidate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ContentEpochId(NonZeroU64);

impl ContentEpochId {
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    #[allow(dead_code)]
    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PresentationTransactionId(NonZeroU64);

impl PresentationTransactionId {
    #[allow(dead_code)]
    pub(crate) const fn new(value: NonZeroU64) -> Self {
        Self(value)
    }

    #[allow(dead_code)]
    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PresentationTransactionAllocator {
    next: NonZeroU64,
}

impl Default for PresentationTransactionAllocator {
    fn default() -> Self {
        Self {
            next: NonZeroU64::MIN,
        }
    }
}

impl PresentationTransactionAllocator {
    pub(crate) fn allocate(&mut self) -> PresentationTransactionId {
        let id = PresentationTransactionId(self.next);
        self.next =
            id.0.get()
                .checked_add(1)
                .and_then(NonZeroU64::new)
                .expect("presentation transaction identifiers exhausted");
        id
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentObservation {
    pub(crate) surface_id: u32,
    pub(crate) buffer_id: NonZeroU64,
    pub(crate) attachment_sequence: u64,
    pub(crate) epoch: ContentEpochId,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct ContentEpochTracker {
    current_by_surface: HashMap<u32, ContentObservation>,
    next: NonZeroU64,
}

impl Default for ContentEpochTracker {
    fn default() -> Self {
        Self {
            current_by_surface: HashMap::new(),
            next: NonZeroU64::MIN,
        }
    }
}

#[allow(dead_code)]
impl ContentEpochTracker {
    pub(crate) fn observe(
        &mut self,
        surface_id: u32,
        buffer_id: NonZeroU64,
        attachment_sequence: u64,
    ) -> ContentObservation {
        let epoch = ContentEpochId(self.next);
        self.next = epoch
            .0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .expect("content epoch identifiers exhausted");
        let observation = ContentObservation {
            surface_id,
            buffer_id,
            attachment_sequence,
            epoch,
        };
        self.current_by_surface.insert(surface_id, observation);
        observation
    }

    pub(crate) fn record_metadata_commit(&self, surface_id: u32) -> Option<ContentEpochId> {
        self.current_by_surface
            .get(&surface_id)
            .map(|observation| observation.epoch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct OutputContentKey {
    pub(crate) surface_id: u32,
    pub(crate) buffer_id: NonZeroU64,
    pub(crate) content_epoch: ContentEpochId,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) transform: u32,
    pub(crate) scale_milli: u32,
    pub(crate) color_epoch: u64,
}

impl OutputContentKey {
    #[allow(clippy::too_many_arguments)]
    pub(crate) const fn new(
        surface_id: u32,
        buffer_id: NonZeroU64,
        content_epoch: ContentEpochId,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        transform: u32,
        scale_milli: u32,
        color_epoch: u64,
    ) -> Self {
        Self {
            surface_id,
            buffer_id,
            content_epoch,
            width,
            height,
            format,
            modifier,
            transform,
            scale_milli,
            color_epoch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectScanoutCandidateKey {
    pub(crate) content: OutputContentKey,
    pub(crate) output_generation: u64,
    pub(crate) cursor_plan_key: Option<u64>,
    pub(crate) color_epoch: u64,
}

impl DirectScanoutCandidateKey {
    pub(crate) fn from_candidate(
        candidate: &DirectScanoutSceneCandidate,
        output_generation: u64,
        cursor_plan_key: Option<u64>,
        color_epoch: u64,
    ) -> Option<Self> {
        let buffer_id = NonZeroU64::new(candidate.buffer_identity.id().get())?;
        let modifier = candidate.buffer.planes().first()?.descriptor().modifier.0;
        Some(Self {
            content: OutputContentKey::new(
                candidate.surface_id,
                buffer_id,
                ContentEpochId::new(NonZeroU64::new(candidate.content_epoch)?),
                candidate.buffer_size.width,
                candidate.buffer_size.height,
                candidate.buffer.format().as_fourcc(),
                modifier,
                0,
                1_000,
                color_epoch,
            ),
            output_generation,
            cursor_plan_key,
            color_epoch,
        })
    }
}
