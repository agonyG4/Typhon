use std::{collections::VecDeque, fmt::Write as _};

use oblivion_one::compositor::{CursorRevealAuthority, PointerConstraintBackendId};
use oblivion_one::native::kms::{
    AtomicCursorPlaneAssignment, AtomicCursorVisualState, AtomicPipelineProperties, PageFlipToken,
};

use super::plane::{
    CursorPlanePoint, CursorRevision, CursorSource, PlanePageflipIdentity, PresentedCursorDelivery,
    PresentedCursorState,
};

#[cfg(test)]
use super::plane::CursorCoupling;

pub(crate) const CURSOR_REVEAL_TRACE_CAPACITY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CursorRevealPhysicalIdentity {
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) token: PageFlipToken,
}

impl CursorRevealPhysicalIdentity {
    pub(crate) const fn from_pageflip(identity: PlanePageflipIdentity) -> Self {
        Self {
            output_generation: identity.output_generation,
            crtc_id: identity.crtc_id,
            token: identity.token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CursorRevealTraceSnapshot {
    pub(crate) authority: CursorRevealAuthority,
    pub(crate) expected_epoch: Option<u64>,
    pub(crate) expected_revision: Option<CursorRevision>,
    pub(crate) expected_delivery: Option<PresentedCursorDelivery>,
    pub(crate) expected_position: Option<CursorPlanePoint>,
    pub(crate) expected_hotspot: Option<CursorPlanePoint>,
    pub(crate) expected_framebuffer_id: Option<Option<u32>>,
    pub(crate) expected_image_generation: Option<u64>,
    pub(crate) expected_source: Option<CursorSource>,
}

impl CursorRevealTraceSnapshot {
    pub(crate) fn from_atomic(
        authority: CursorRevealAuthority,
        expected_epoch: Option<u64>,
        expected_revision: Option<CursorRevision>,
        expected_delivery: PresentedCursorDelivery,
        state: &AtomicCursorVisualState,
        source: Option<CursorSource>,
    ) -> Self {
        Self {
            authority,
            expected_epoch,
            expected_revision,
            expected_delivery: Some(expected_delivery),
            expected_position: Some(CursorPlanePoint {
                x: state.x,
                y: state.y,
            }),
            expected_hotspot: Some(CursorPlanePoint {
                x: state.hotspot_x,
                y: state.hotspot_y,
            }),
            expected_framebuffer_id: Some(state.framebuffer_id),
            expected_image_generation: Some(state.image_generation),
            expected_source: source,
        }
    }

    pub(crate) fn from_presented(
        authority: CursorRevealAuthority,
        expected_epoch: Option<u64>,
        state: PresentedCursorState,
        source: Option<CursorSource>,
    ) -> Self {
        Self {
            authority,
            expected_epoch,
            expected_revision: Some(state.revision),
            expected_delivery: Some(state.delivery),
            expected_position: Some(state.output_position),
            expected_hotspot: Some(state.hotspot),
            expected_framebuffer_id: Some(state.framebuffer_id),
            expected_image_generation: state.image_generation,
            expected_source: source.or(state.source),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorTraceMatch {
    True,
    False,
    Unknown,
}

impl CursorTraceMatch {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::Unknown => "unknown",
        }
    }
}

fn compare<T: PartialEq>(expected: Option<T>, actual: Option<T>) -> CursorTraceMatch {
    match (expected, actual) {
        (Some(expected), Some(actual)) if expected == actual => CursorTraceMatch::True,
        (Some(_), Some(_)) => CursorTraceMatch::False,
        _ => CursorTraceMatch::Unknown,
    }
}

fn all(matches: impl IntoIterator<Item = CursorTraceMatch>) -> CursorTraceMatch {
    let mut unknown = false;
    for value in matches {
        match value {
            CursorTraceMatch::False => return CursorTraceMatch::False,
            CursorTraceMatch::Unknown => unknown = true,
            CursorTraceMatch::True => {}
        }
    }
    if unknown {
        CursorTraceMatch::Unknown
    } else {
        CursorTraceMatch::True
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorRevealTraceComparison {
    pub(crate) position_match: CursorTraceMatch,
    pub(crate) revision_match: CursorTraceMatch,
    pub(crate) delivery_match: CursorTraceMatch,
    pub(crate) hotspot_match: CursorTraceMatch,
    pub(crate) framebuffer_match: CursorTraceMatch,
    pub(crate) image_generation_match: CursorTraceMatch,
    pub(crate) source_match: CursorTraceMatch,
    pub(crate) visual_match: CursorTraceMatch,
    pub(crate) overall_match: CursorTraceMatch,
}

impl CursorRevealTraceSnapshot {
    pub(crate) fn compare(self, presented: PresentedCursorState) -> CursorRevealTraceComparison {
        let position_match = compare(self.expected_position, Some(presented.output_position));
        let revision_match = compare(self.expected_revision, Some(presented.revision));
        let delivery_match = compare(self.expected_delivery, Some(presented.delivery));
        let hotspot_match = compare(self.expected_hotspot, Some(presented.hotspot));
        let framebuffer_match =
            compare(self.expected_framebuffer_id, Some(presented.framebuffer_id));
        let image_generation_match =
            compare(self.expected_image_generation, presented.image_generation);
        let source_match = compare(self.expected_source, presented.source);
        let visual_match = all([
            position_match,
            hotspot_match,
            framebuffer_match,
            image_generation_match,
            source_match,
        ]);
        let overall_match = all([visual_match, revision_match, delivery_match]);
        CursorRevealTraceComparison {
            position_match,
            revision_match,
            delivery_match,
            hotspot_match,
            framebuffer_match,
            image_generation_match,
            source_match,
            visual_match,
            overall_match,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CursorRevealTraceEntry {
    pub(crate) identity: CursorRevealPhysicalIdentity,
    pub(crate) reveal: PointerConstraintBackendId,
    pub(crate) snapshot: CursorRevealTraceSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct RevealState {
    id: PointerConstraintBackendId,
    first_visible_reported: bool,
    no_visible_terminal: bool,
}

#[derive(Debug)]
pub(crate) struct CursorRevealTraceLedger {
    entries: VecDeque<CursorRevealTraceEntry>,
    reveals: VecDeque<RevealState>,
    capacity: usize,
}

impl CursorRevealTraceLedger {
    pub(crate) fn new() -> Self {
        Self::with_capacity(CURSOR_REVEAL_TRACE_CAPACITY)
    }

    #[cfg(test)]
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            reveals: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    #[cfg(not(test))]
    fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            reveals: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn bind(
        &mut self,
        identity: CursorRevealPhysicalIdentity,
        snapshot: CursorRevealTraceSnapshot,
    ) {
        self.ensure_reveal(snapshot.authority);
        if self.entries.len() >= self.capacity {
            self.entries.pop_front();
            self.trace_overflow("physical_entry");
        }
        self.entries.push_back(CursorRevealTraceEntry {
            identity,
            reveal: snapshot.authority.constraint,
            snapshot,
        });
    }

    fn ensure_reveal(&mut self, authority: CursorRevealAuthority) {
        if self
            .reveals
            .iter()
            .any(|reveal| reveal.id == authority.constraint)
        {
            return;
        }
        if self.reveals.len() >= self.capacity
            && let Some(retired) = self.reveals.pop_front()
        {
            self.entries.retain(|entry| entry.reveal != retired.id);
            self.trace_overflow("reveal_state");
        }
        self.reveals.push_back(RevealState {
            id: authority.constraint,
            first_visible_reported: false,
            no_visible_terminal: !authority.visibility_requested,
        });
    }

    pub(crate) fn take(
        &mut self,
        identity: CursorRevealPhysicalIdentity,
    ) -> Option<CursorRevealTraceEntry> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.identity == identity)?;
        self.entries.remove(index)
    }

    pub(crate) fn mark_first_visible(&mut self, reveal: PointerConstraintBackendId) -> bool {
        let Some(state) = self.reveals.iter_mut().find(|state| state.id == reveal) else {
            return false;
        };
        if state.no_visible_terminal || state.first_visible_reported {
            return false;
        }
        state.first_visible_reported = true;
        true
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    fn trace_overflow(&self, retired: &'static str) {
        crate::pointer_debug::cursor_presentation_log_lazy(|| {
            format!(
                "event=cursor_reveal_trace_overflow capacity={} retired={}",
                self.capacity, retired
            )
        });
    }
}

pub(crate) fn trace_comparison_fields(
    output: &mut String,
    comparison: CursorRevealTraceComparison,
) {
    let _ = write!(
        output,
        "position_match={} revision_match={} delivery_match={} hotspot_match={} framebuffer_match={} image_generation_match={} source_match={} visual_match={} overall_match={}",
        comparison.position_match.as_str(),
        comparison.revision_match.as_str(),
        comparison.delivery_match.as_str(),
        comparison.hotspot_match.as_str(),
        comparison.framebuffer_match.as_str(),
        comparison.image_generation_match.as_str(),
        comparison.source_match.as_str(),
        comparison.visual_match.as_str(),
        comparison.overall_match.as_str()
    );
}

pub(crate) enum CursorKmsAssignment<'a> {
    Set(&'a AtomicCursorVisualState),
    Disable,
    Unchanged,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CursorKmsSubmitContext {
    pub(crate) output_generation: u64,
    pub(crate) transaction_id: Option<crate::native_output::OutputTransactionId>,
    pub(crate) token: PageFlipToken,
    pub(crate) crtc_id: u32,
    pub(crate) cursor_epoch: Option<u64>,
    pub(crate) cursor_revision: Option<CursorRevision>,
    pub(crate) submission_kind: &'static str,
    pub(crate) transport: &'static str,
    pub(crate) delivery: PresentedCursorDelivery,
}

pub(crate) fn trace_cursor_kms_submit(
    pipeline: &AtomicPipelineProperties,
    assignment: CursorKmsAssignment<'_>,
    context: CursorKmsSubmitContext,
) {
    if !crate::pointer_debug::cursor_presentation_trace_enabled() {
        return;
    }
    let line = format_cursor_kms_submit(pipeline, assignment, context);
    crate::pointer_debug::cursor_presentation_log_lazy(|| line);
}

fn format_cursor_kms_submit(
    pipeline: &AtomicPipelineProperties,
    assignment: CursorKmsAssignment<'_>,
    context: CursorKmsSubmitContext,
) -> String {
    let unchanged = matches!(&assignment, CursorKmsAssignment::Unchanged);
    let assignment = match assignment {
        CursorKmsAssignment::Set(state) => {
            oblivion_one::native::kms::cursor_plane_assignment(pipeline, Some(state))
                .unwrap_or(AtomicCursorPlaneAssignment::Unavailable)
        }
        CursorKmsAssignment::Disable => {
            oblivion_one::native::kms::cursor_plane_assignment(pipeline, None)
                .unwrap_or(AtomicCursorPlaneAssignment::Unavailable)
        }
        CursorKmsAssignment::Unchanged => AtomicCursorPlaneAssignment::Unavailable,
    };
    format_cursor_kms_submit_assignment(assignment, context, unchanged)
}

fn format_cursor_kms_submit_assignment(
    assignment: AtomicCursorPlaneAssignment,
    context: CursorKmsSubmitContext,
    unchanged: bool,
) -> String {
    let transaction_id = context
        .transaction_id
        .map_or_else(|| "unknown".to_string(), |id| id.get().to_string());
    let epoch = context
        .cursor_epoch
        .map_or_else(|| "unknown".to_string(), |epoch| epoch.to_string());
    let revision = context
        .cursor_revision
        .map_or_else(|| "unknown".to_string(), |revision| format!("{revision:?}"));
    let mut line = format!(
        "event=cursor_kms_submit output_generation={} transaction_id={} pageflip_token={} crtc_id={} cursor_epoch={} cursor_revision={} submission_kind={} transport={} delivery={:?} assignment=",
        context.output_generation,
        transaction_id,
        context.token.get(),
        context.crtc_id,
        epoch,
        revision,
        context.submission_kind,
        context.transport,
        context.delivery,
    );
    match assignment {
        AtomicCursorPlaneAssignment::Unavailable => {
            if unchanged {
                line.push_str("unchanged plane_id=unknown FB_ID=unknown CRTC_ID=unknown SRC_X=unknown SRC_Y=unknown SRC_W=unknown SRC_H=unknown CRTC_X=unknown CRTC_Y=unknown CRTC_W=unknown CRTC_H=unknown position=unknown hotspot=unknown framebuffer_id=unknown image_generation=unknown");
            } else {
                line.push_str("unavailable plane_id=unknown FB_ID=unknown CRTC_ID=unknown SRC_X=unknown SRC_Y=unknown SRC_W=unknown SRC_H=unknown CRTC_X=unknown CRTC_Y=unknown CRTC_W=unknown CRTC_H=unknown position=unknown hotspot=unknown framebuffer_id=unknown image_generation=unknown");
            }
        }
        AtomicCursorPlaneAssignment::Disabled { plane_id } => {
            let _ = write!(
                line,
                "disabled plane_id={} FB_ID=0 CRTC_ID=0 SRC_X=unknown SRC_Y=unknown SRC_W=unknown SRC_H=unknown CRTC_X=unknown CRTC_Y=unknown CRTC_W=unknown CRTC_H=unknown position=unknown hotspot=unknown framebuffer_id=none image_generation=unknown",
                plane_id
            );
        }
        AtomicCursorPlaneAssignment::Enabled {
            plane_id,
            framebuffer_id,
            crtc_id,
            src_x,
            src_y,
            src_w,
            src_h,
            crtc_x,
            crtc_y,
            crtc_w,
            crtc_h,
            hotspot_x,
            hotspot_y,
            image_generation,
            ..
        } => {
            let _ = write!(
                line,
                "enabled plane_id={} FB_ID={} CRTC_ID={} SRC_X={} SRC_Y={} SRC_W={} SRC_H={} CRTC_X={} CRTC_Y={} CRTC_W={} CRTC_H={} position=({}, {}) hotspot=({}, {}) framebuffer_id={} image_generation={}",
                plane_id,
                framebuffer_id,
                crtc_id,
                src_x,
                src_y,
                src_w,
                src_h,
                crtc_x,
                crtc_y,
                crtc_w,
                crtc_h,
                crtc_x,
                crtc_y,
                hotspot_x,
                hotspot_y,
                framebuffer_id,
                image_generation
            );
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    fn authority(id: u64, visible: bool) -> CursorRevealAuthority {
        CursorRevealAuthority {
            constraint: PointerConstraintBackendId {
                constraint_id: id,
                generation: id + 100,
            },
            final_position: oblivion_one::compositor::OutputPosition {
                x: id as f64,
                y: (id + 1) as f64,
            },
            visibility_requested: visible,
        }
    }

    fn state(id: u64) -> PresentedCursorState {
        PresentedCursorState {
            revision: CursorRevision::initial(),
            coupling: CursorCoupling::IndependentPlane,
            delivery: PresentedCursorDelivery::Hardware,
            framebuffer_id: Some(id as u32),
            image_generation: Some(id),
            source: Some(CursorSource::Client),
            visible: true,
            output_position: CursorPlanePoint {
                x: id as i32,
                y: (id + 1) as i32,
            },
            hotspot: CursorPlanePoint { x: 2, y: 3 },
        }
    }

    fn snapshot(id: u64, visible: bool) -> CursorRevealTraceSnapshot {
        let authority = authority(id, visible);
        CursorRevealTraceSnapshot::from_presented(
            authority,
            Some(id),
            state(id),
        )
    }

    fn identity(token: u64) -> CursorRevealPhysicalIdentity {
        CursorRevealPhysicalIdentity {
            output_generation: 7,
            crtc_id: 11,
            token: PageFlipToken::new(token).unwrap(),
        }
    }

    #[test]
    fn overlapping_reveals_keep_submission_identity() {
        let mut ledger = CursorRevealTraceLedger::new();
        ledger.bind(identity(10), snapshot(1, true));
        ledger.bind(identity(20), snapshot(2, true));

        assert_eq!(ledger.take(identity(10)).unwrap().reveal.constraint_id, 1);
        assert_eq!(ledger.take(identity(20)).unwrap().reveal.constraint_id, 2);
    }

    #[test]
    fn first_visible_slots_are_independent_and_no_visible_cannot_claim_one() {
        let mut ledger = CursorRevealTraceLedger::new();
        ledger.bind(identity(10), snapshot(1, true));
        ledger.bind(identity(20), snapshot(2, true));
        ledger.bind(identity(30), snapshot(3, false));

        assert!(ledger.mark_first_visible(authority(1, true).constraint));
        assert!(!ledger.mark_first_visible(authority(1, true).constraint));
        assert!(ledger.mark_first_visible(authority(2, true).constraint));
        assert!(!ledger.mark_first_visible(authority(3, false).constraint));
    }

    #[test]
    fn stale_same_position_is_not_a_visual_match() {
        let mut expected = state(1);
        expected.revision = CursorRevision::initial().advance_image();
        let authority = authority(1, true);
        let snapshot = CursorRevealTraceSnapshot::from_presented(
            authority,
            Some(1),
            expected,
        );
        let mut presented = state(1);
        presented.revision = CursorRevision::initial();
        presented.image_generation = Some(0);
        presented.hotspot = CursorPlanePoint { x: 9, y: 9 };
        presented.framebuffer_id = Some(99);
        let comparison = snapshot.compare(presented);

        assert_eq!(comparison.position_match, CursorTraceMatch::True);
        assert_eq!(comparison.revision_match, CursorTraceMatch::False);
        assert_eq!(comparison.visual_match, CursorTraceMatch::False);
        assert_eq!(comparison.overall_match, CursorTraceMatch::False);
    }

    #[test]
    fn exact_known_visual_state_matches() {
        let snapshot = snapshot(1, true);
        let comparison = snapshot.compare(state(1));
        assert_eq!(comparison.position_match, CursorTraceMatch::True);
        assert_eq!(comparison.revision_match, CursorTraceMatch::True);
        assert_eq!(comparison.delivery_match, CursorTraceMatch::True);
        assert_eq!(comparison.hotspot_match, CursorTraceMatch::True);
        assert_eq!(comparison.framebuffer_match, CursorTraceMatch::True);
        assert_eq!(comparison.image_generation_match, CursorTraceMatch::True);
        assert_eq!(comparison.source_match, CursorTraceMatch::True);
        assert_eq!(comparison.visual_match, CursorTraceMatch::True);
        assert_eq!(comparison.overall_match, CursorTraceMatch::True);
    }

    #[test]
    fn disabled_runtime_has_no_ledger_state() {
        let mut ledger: Option<CursorRevealTraceLedger> = None;
        if let Some(ledger) = ledger.as_mut() {
            ledger.bind(identity(10), snapshot(1, true));
        }
        assert!(ledger.is_none());
    }

    #[test]
    fn overflow_is_bounded() {
        let mut ledger = CursorRevealTraceLedger::with_capacity(2);
        ledger.bind(identity(10), snapshot(1, true));
        ledger.bind(identity(20), snapshot(2, true));
        ledger.bind(identity(30), snapshot(3, true));
        assert_eq!(ledger.len(), 2);
        assert!(ledger.take(identity(10)).is_none());
        assert!(ledger.take(identity(20)).is_some());
        assert!(ledger.take(identity(30)).is_some());
    }

    #[test]
    fn synchronous_kms_payload_uses_canonical_exact_fields() {
        let line = format_cursor_kms_submit_assignment(
            AtomicCursorPlaneAssignment::Enabled {
                plane_id: 9,
                framebuffer_id: 99,
                crtc_id: 11,
                src_x: 0,
                src_y: 0,
                src_w: 64 << 16,
                src_h: 64 << 16,
                crtc_x: 20,
                crtc_y: 33,
                pointer_x: 25,
                pointer_y: 40,
                plane_origin_x: 20,
                plane_origin_y: 33,
                crtc_w: 64,
                crtc_h: 64,
                hotspot_x: 5,
                hotspot_y: 7,
                width: 64,
                height: 64,
                image_generation: 3,
                rotation: None,
                alpha: None,
                pixel_blend_mode: None,
            },
            CursorKmsSubmitContext {
                output_generation: 4,
                transaction_id: Some(crate::native_output::OutputTransactionId::new(
                    NonZeroU64::new(12).unwrap(),
                )),
                token: PageFlipToken::new(55).unwrap(),
                crtc_id: 11,
                cursor_epoch: Some(8),
                cursor_revision: Some(CursorRevision::initial()),
                submission_kind: "primary_plus_cursor",
                transport: "synchronous",
                delivery: PresentedCursorDelivery::Hardware,
            },
            false,
        );
        assert!(line.contains("transport=synchronous"));
        assert!(line.contains("submission_kind=primary_plus_cursor"));
        assert!(line.contains("FB_ID=99 CRTC_ID=11"));
        assert!(line.contains("SRC_W=4194304 SRC_H=4194304"));
        assert!(line.contains("CRTC_X_RAW=20 CRTC_Y_RAW=33"));
        assert!(line.contains("pointer_position=(25, 40) hotspot=(5, 7)"));
        assert!(line.contains("plane_origin_signed=(20, 33)"));
        assert!(line.contains("image_generation=3"));
    }

    #[test]
    fn disabled_and_unchanged_kms_payloads_do_not_invent_geometry() {
        let context = CursorKmsSubmitContext {
            output_generation: 4,
            transaction_id: None,
            token: PageFlipToken::new(55).unwrap(),
            crtc_id: 11,
            cursor_epoch: None,
            cursor_revision: None,
            submission_kind: "cursor_only",
            transport: "synchronous",
            delivery: PresentedCursorDelivery::Hidden,
        };
        let disabled = format_cursor_kms_submit_assignment(
            AtomicCursorPlaneAssignment::Disabled { plane_id: 9 },
            context,
            false,
        );
        let unchanged = format_cursor_kms_submit_assignment(
            AtomicCursorPlaneAssignment::Unavailable,
            context,
            true,
        );
        assert!(disabled.contains("assignment=disabled plane_id=9 FB_ID=0 CRTC_ID=0"));
        assert!(disabled.contains("SRC_X=unknown"));
        assert!(unchanged.contains("assignment=unchanged"));
    }
}
