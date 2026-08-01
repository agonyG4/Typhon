use std::collections::HashMap;

use crate::native_output::OutputTransactionId;
use oblivion_one::native::kms::AtomicCursorVisualState;

use super::plane::CursorRevision;

const FIXED_POINT_ONE: u32 = 1 << 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum CursorGeometryClass {
    FullyVisible,
    EdgeClipped,
    CornerClipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CursorCapabilityKey {
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) plane_id: u32,
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    pub(crate) output_transform: u32,
    pub(crate) output_scale_milli: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) cursor_width: u32,
    pub(crate) cursor_height: u32,
    pub(crate) hotspot_property_available: bool,
    pub(crate) geometry_class: CursorGeometryClass,
    /// Exact normalized source crop used by the Atomic cursor assignment.
    /// The geometry class is only a policy category; these fields are the
    /// validation identity for the payload the kernel will receive.
    pub(crate) source_x: u32,
    pub(crate) source_y: u32,
    pub(crate) source_width: u32,
    pub(crate) source_height: u32,
    pub(crate) destination_x: i32,
    pub(crate) destination_y: i32,
    pub(crate) destination_width: u32,
    pub(crate) destination_height: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorQuarantineReason {
    UnsupportedSize,
    UnsupportedFormat,
    UnsupportedModifier,
    UnsupportedTransform,
    UnsupportedHotspot,
    TestOnlyRejected,
    PermanentSubmitRejection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorCapabilityStatus {
    Unknown,
    Proven,
    Quarantined {
        reason: CursorQuarantineReason,
        failure_count: u32,
    },
}

#[derive(Debug, Default)]
pub(crate) struct PlaneCapabilityCache {
    entries: HashMap<CursorCapabilityKey, CursorCapabilityStatus>,
}

impl PlaneCapabilityCache {
    pub(crate) fn status(&self, key: CursorCapabilityKey) -> CursorCapabilityStatus {
        self.entries
            .get(&key)
            .copied()
            .unwrap_or(CursorCapabilityStatus::Unknown)
    }

    #[cfg(test)]
    pub(crate) fn set_status(&mut self, key: CursorCapabilityKey, status: CursorCapabilityStatus) {
        if status == CursorCapabilityStatus::Unknown {
            self.entries.remove(&key);
        } else {
            self.entries.insert(key, status);
        }
    }

    pub(crate) fn mark_proven(&mut self, key: CursorCapabilityKey) {
        self.entries.insert(key, CursorCapabilityStatus::Proven);
    }

    pub(crate) fn quarantine(&mut self, key: CursorCapabilityKey, reason: CursorQuarantineReason) {
        let failure_count = match self.entries.get(&key) {
            Some(CursorCapabilityStatus::Quarantined {
                reason: existing,
                failure_count,
            }) if *existing == reason => failure_count.saturating_add(1),
            Some(CursorCapabilityStatus::Unknown)
            | Some(CursorCapabilityStatus::Proven)
            | Some(CursorCapabilityStatus::Quarantined { .. })
            | None => 1,
        };
        self.entries.insert(
            key,
            CursorCapabilityStatus::Quarantined {
                reason,
                failure_count,
            },
        );
    }

    pub(crate) fn invalidate_generation(&mut self, output_generation: u64) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|key, _| key.output_generation == output_generation);
        before.saturating_sub(self.entries.len())
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorFailureKind {
    Busy,
    AdmissionContention,
    UnsupportedSize,
    UnsupportedFormat,
    UnsupportedModifier,
    UnsupportedTransform,
    UnsupportedHotspot,
    TestOnlyInvalid,
    PermanentProperty,
    TransientIo,
    GenerationMismatch,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorFailureDisposition {
    Defer,
    Retry,
    Quarantine(CursorQuarantineReason),
    Invalidate,
}

#[cfg(test)]
pub(crate) const fn classify_cursor_failure(
    failure: CursorFailureKind,
) -> CursorFailureDisposition {
    match failure {
        CursorFailureKind::Busy | CursorFailureKind::AdmissionContention => {
            CursorFailureDisposition::Defer
        }
        CursorFailureKind::UnsupportedSize => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::UnsupportedSize)
        }
        CursorFailureKind::UnsupportedFormat => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::UnsupportedFormat)
        }
        CursorFailureKind::UnsupportedModifier => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::UnsupportedModifier)
        }
        CursorFailureKind::UnsupportedTransform => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::UnsupportedTransform)
        }
        CursorFailureKind::UnsupportedHotspot => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::UnsupportedHotspot)
        }
        CursorFailureKind::TestOnlyInvalid => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::TestOnlyRejected)
        }
        CursorFailureKind::PermanentProperty => {
            CursorFailureDisposition::Quarantine(CursorQuarantineReason::PermanentSubmitRejection)
        }
        CursorFailureKind::TransientIo => CursorFailureDisposition::Retry,
        CursorFailureKind::GenerationMismatch => CursorFailureDisposition::Invalidate,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorGeometryInput {
    pub(crate) pointer_x: i32,
    pub(crate) pointer_y: i32,
    pub(crate) hotspot_x: i32,
    pub(crate) hotspot_y: i32,
    pub(crate) cursor_width: u32,
    pub(crate) cursor_height: u32,
    pub(crate) output_width: u32,
    pub(crate) output_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorDestinationRect {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorSourceRect {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NormalizedCursorGeometry {
    pub(crate) class: CursorGeometryClass,
    pub(crate) destination: CursorDestinationRect,
    pub(crate) source: CursorSourceRect,
}

pub(crate) fn normalize_cursor_geometry(
    input: CursorGeometryInput,
) -> Option<NormalizedCursorGeometry> {
    if input.cursor_width == 0
        || input.cursor_height == 0
        || input.output_width == 0
        || input.output_height == 0
    {
        return None;
    }
    let origin_x = i64::from(input.pointer_x) - i64::from(input.hotspot_x);
    let origin_y = i64::from(input.pointer_y) - i64::from(input.hotspot_y);
    let right = origin_x.saturating_add(i64::from(input.cursor_width));
    let bottom = origin_y.saturating_add(i64::from(input.cursor_height));
    let output_right = i64::from(input.output_width);
    let output_bottom = i64::from(input.output_height);
    let clipped_left = origin_x.max(0);
    let clipped_top = origin_y.max(0);
    let clipped_right = right.min(output_right);
    let clipped_bottom = bottom.min(output_bottom);
    if clipped_left >= clipped_right || clipped_top >= clipped_bottom {
        return None;
    }

    let clipped_x = clipped_left != origin_x || clipped_right != right;
    let clipped_y = clipped_top != origin_y || clipped_bottom != bottom;
    let class = match (clipped_x, clipped_y) {
        (false, false) => CursorGeometryClass::FullyVisible,
        (true, true) => CursorGeometryClass::CornerClipped,
        (true, false) | (false, true) => CursorGeometryClass::EdgeClipped,
    };
    let source_x = u32::try_from(clipped_left.saturating_sub(origin_x)).ok()?;
    let source_y = u32::try_from(clipped_top.saturating_sub(origin_y)).ok()?;
    let width = u32::try_from(clipped_right.saturating_sub(clipped_left)).ok()?;
    let height = u32::try_from(clipped_bottom.saturating_sub(clipped_top)).ok()?;
    Some(NormalizedCursorGeometry {
        class,
        destination: CursorDestinationRect {
            x: i32::try_from(clipped_left).ok()?,
            y: i32::try_from(clipped_top).ok()?,
            width,
            height,
        },
        source: CursorSourceRect {
            x: source_x.checked_mul(FIXED_POINT_ONE)?,
            y: source_y.checked_mul(FIXED_POINT_ONE)?,
            width: width.checked_mul(FIXED_POINT_ONE)?,
            height: height.checked_mul(FIXED_POINT_ONE)?,
        },
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorPreference {
    Auto,
    Hardware,
    Software,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorDeltaClass {
    PositionOnly,
    Visual,
    Visibility,
    DeliveryModeTransition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorDeliveryMode {
    Hidden,
    Hardware,
    Software,
}

pub(crate) fn classify_cursor_delta(
    previous_delivery: CursorDeliveryMode,
    next_delivery: CursorDeliveryMode,
    previous: Option<&AtomicCursorVisualState>,
    next: Option<&AtomicCursorVisualState>,
    validation_base_unchanged: bool,
) -> CursorDeltaClass {
    let delivery_changed = previous_delivery != next_delivery;
    if delivery_changed {
        if matches!(
            (previous_delivery, next_delivery),
            (CursorDeliveryMode::Hardware, CursorDeliveryMode::Software)
                | (CursorDeliveryMode::Software, CursorDeliveryMode::Hardware)
        ) {
            return CursorDeltaClass::DeliveryModeTransition;
        }
        return if matches!(
            (previous_delivery, next_delivery),
            (CursorDeliveryMode::Hidden, CursorDeliveryMode::Hardware)
                | (CursorDeliveryMode::Hardware, CursorDeliveryMode::Hidden)
        ) {
            CursorDeltaClass::Visibility
        } else {
            CursorDeltaClass::DeliveryModeTransition
        };
    }
    if previous_delivery != CursorDeliveryMode::Hardware
        || next_delivery != CursorDeliveryMode::Hardware
    {
        return CursorDeltaClass::Visual;
    }
    let (Some(previous), Some(next)) = (previous, next) else {
        return CursorDeltaClass::Visibility;
    };
    if !validation_base_unchanged {
        return CursorDeltaClass::Visual;
    }
    if previous.visible != next.visible {
        return CursorDeltaClass::Visibility;
    }
    if previous.framebuffer_id != next.framebuffer_id
        || previous.width != next.width
        || previous.height != next.height
        || previous.hotspot_x != next.hotspot_x
        || previous.hotspot_y != next.hotspot_y
        || previous.image_generation != next.image_generation
    {
        CursorDeltaClass::Visual
    } else {
        CursorDeltaClass::PositionOnly
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorHardwareStatus {
    Unavailable,
    Unknown,
    Proven,
    Quarantined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorModePolicyInput {
    pub(crate) preference: CursorPreference,
    pub(crate) visible: bool,
    pub(crate) hardware_status: CursorHardwareStatus,
    pub(crate) geometry_valid: bool,
    pub(crate) software_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorModeSelection {
    Hidden,
    Hardware,
    Software,
    Rejected,
}

pub(crate) fn select_cursor_delivery_mode(input: CursorModePolicyInput) -> CursorModeSelection {
    if !input.visible {
        return CursorModeSelection::Hidden;
    }
    if input.preference == CursorPreference::Software {
        return if input.software_allowed {
            CursorModeSelection::Software
        } else {
            CursorModeSelection::Rejected
        };
    }
    let hardware_usable = input.geometry_valid
        && matches!(
            input.hardware_status,
            CursorHardwareStatus::Unknown | CursorHardwareStatus::Proven
        );
    if hardware_usable {
        return CursorModeSelection::Hardware;
    }
    if input.preference == CursorPreference::Hardware || !input.software_allowed {
        CursorModeSelection::Rejected
    } else {
        CursorModeSelection::Software
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanePrimaryMode {
    Composed,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CursorHardwareCapability {
    pub(crate) key: CursorCapabilityKey,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PlaneSchedulingInput<'a> {
    pub(crate) revision: CursorRevision,
    pub(crate) preference: CursorPreference,
    pub(crate) visible: bool,
    pub(crate) geometry: CursorGeometryInput,
    pub(crate) geometry_valid: bool,
    pub(crate) hardware: Option<CursorHardwareCapability>,
    pub(crate) capabilities: &'a PlaneCapabilityCache,
    pub(crate) primary_mode: PlanePrimaryMode,
    pub(crate) software_allowed: bool,
    pub(crate) predictive_triple_active: bool,
    pub(crate) cursor_kms_changed: bool,
    pub(crate) hardware_plane_visible: bool,
    pub(crate) delta_class: CursorDeltaClass,
    pub(crate) validation_base_unchanged: bool,
    pub(crate) attachable_primary: Option<OutputTransactionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorDeliveryChoice {
    Hardware {
        revision: CursorRevision,
        geometry: NormalizedCursorGeometry,
        capability_key: CursorCapabilityKey,
    },
    Software {
        revision: CursorRevision,
    },
    Hidden {
        revision: CursorRevision,
        disable_hardware_plane: bool,
    },
    Rejected {
        revision: CursorRevision,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrimaryPlaneAction {
    Preserve,
    TransitionToComposed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorPlaneAction {
    None,
    Independent,
    EmbedInPrimary,
    AwaitPrimaryTransition,
    MustBundleWith(OutputTransactionId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorPacingConstraint {
    Unchanged,
    ReactiveDouble,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KmsCursorTestPolicy {
    NotApplicable,
    Required,
    SkipProven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaneSchedulingReason {
    Hidden,
    FullyOutside,
    SoftwareRequested,
    HardwareCapabilityUnknown,
    HardwareCapabilityProven,
    HardwareCapabilityQuarantined,
    HardwareUnavailable,
    InvalidHardwareGeometry,
    SoftwareUnavailable,
    HardwareRequiredUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlaneSchedulingDecision {
    pub(crate) delivery: CursorDeliveryChoice,
    pub(crate) direct_scanout_compatible: bool,
    pub(crate) primary_action: PrimaryPlaneAction,
    pub(crate) cursor_action: CursorPlaneAction,
    pub(crate) pacing_constraint: CursorPacingConstraint,
    pub(crate) test_policy: KmsCursorTestPolicy,
    pub(crate) reason: PlaneSchedulingReason,
}

pub(crate) fn schedule_planes(input: PlaneSchedulingInput<'_>) -> PlaneSchedulingDecision {
    let _predictive_triple_active = input.predictive_triple_active;
    let hardware_status = input
        .hardware
        .map_or(CursorHardwareStatus::Unavailable, |hardware| {
            match input.capabilities.status(hardware.key) {
                CursorCapabilityStatus::Unknown => CursorHardwareStatus::Unknown,
                CursorCapabilityStatus::Proven => CursorHardwareStatus::Proven,
                CursorCapabilityStatus::Quarantined { .. } => CursorHardwareStatus::Quarantined,
            }
        });
    let mode = select_cursor_delivery_mode(CursorModePolicyInput {
        preference: input.preference,
        visible: input.visible,
        hardware_status,
        geometry_valid: input.geometry_valid,
        software_allowed: input.software_allowed,
    });
    if mode == CursorModeSelection::Hidden {
        return hidden_decision(input, PlaneSchedulingReason::Hidden);
    }
    let Some(geometry) = normalize_cursor_geometry(input.geometry) else {
        return hidden_decision(input, PlaneSchedulingReason::FullyOutside);
    };
    if mode == CursorModeSelection::Software && input.preference == CursorPreference::Software {
        return software_decision(input, PlaneSchedulingReason::SoftwareRequested);
    }
    if mode == CursorModeSelection::Rejected {
        let reason = if !input.software_allowed && input.preference != CursorPreference::Hardware {
            PlaneSchedulingReason::SoftwareUnavailable
        } else {
            PlaneSchedulingReason::HardwareRequiredUnavailable
        };
        return rejected_decision(input, reason);
    }
    let Some(hardware) = input.hardware else {
        return hardware_fallback(input, PlaneSchedulingReason::HardwareUnavailable);
    };
    let expected_destination_x = if geometry.class == CursorGeometryClass::FullyVisible {
        0
    } else {
        geometry.destination.x
    };
    let expected_destination_y = if geometry.class == CursorGeometryClass::FullyVisible {
        0
    } else {
        geometry.destination.y
    };
    let geometry_matches_key = hardware.key.geometry_class == geometry.class
        && hardware.key.source_x == geometry.source.x
        && hardware.key.source_y == geometry.source.y
        && hardware.key.source_width == geometry.source.width
        && hardware.key.source_height == geometry.source.height
        && hardware.key.destination_x == expected_destination_x
        && hardware.key.destination_y == expected_destination_y
        && hardware.key.destination_width == geometry.destination.width
        && hardware.key.destination_height == geometry.destination.height;
    if !input.geometry_valid || !geometry_matches_key {
        return hardware_fallback(input, PlaneSchedulingReason::InvalidHardwareGeometry);
    }

    match input.capabilities.status(hardware.key) {
        CursorCapabilityStatus::Unknown => hardware_decision(
            input,
            geometry,
            hardware.key,
            KmsCursorTestPolicy::Required,
            PlaneSchedulingReason::HardwareCapabilityUnknown,
        ),
        CursorCapabilityStatus::Proven => hardware_decision(
            input,
            geometry,
            hardware.key,
            if input.delta_class == CursorDeltaClass::PositionOnly
                && input.validation_base_unchanged
                && geometry.class == CursorGeometryClass::FullyVisible
            {
                KmsCursorTestPolicy::SkipProven
            } else {
                KmsCursorTestPolicy::Required
            },
            PlaneSchedulingReason::HardwareCapabilityProven,
        ),
        CursorCapabilityStatus::Quarantined { .. } => {
            hardware_fallback(input, PlaneSchedulingReason::HardwareCapabilityQuarantined)
        }
    }
}

fn hidden_decision(
    input: PlaneSchedulingInput<'_>,
    reason: PlaneSchedulingReason,
) -> PlaneSchedulingDecision {
    let disable_hardware_plane = input.hardware_plane_visible;
    PlaneSchedulingDecision {
        delivery: CursorDeliveryChoice::Hidden {
            revision: input.revision,
            disable_hardware_plane,
        },
        direct_scanout_compatible: true,
        primary_action: PrimaryPlaneAction::Preserve,
        cursor_action: if disable_hardware_plane && input.cursor_kms_changed {
            CursorPlaneAction::Independent
        } else {
            CursorPlaneAction::None
        },
        pacing_constraint: CursorPacingConstraint::Unchanged,
        test_policy: KmsCursorTestPolicy::NotApplicable,
        reason,
    }
}

fn hardware_fallback(
    input: PlaneSchedulingInput<'_>,
    reason: PlaneSchedulingReason,
) -> PlaneSchedulingDecision {
    if input.preference == CursorPreference::Hardware {
        return rejected_decision(input, PlaneSchedulingReason::HardwareRequiredUnavailable);
    }
    software_decision(input, reason)
}

fn software_decision(
    input: PlaneSchedulingInput<'_>,
    reason: PlaneSchedulingReason,
) -> PlaneSchedulingDecision {
    if !input.software_allowed {
        return rejected_decision(input, PlaneSchedulingReason::SoftwareUnavailable);
    }
    let transition = input.primary_mode == PlanePrimaryMode::Direct;
    let cursor_action = if let Some(primary) = input.attachable_primary {
        CursorPlaneAction::MustBundleWith(primary)
    } else if transition {
        CursorPlaneAction::AwaitPrimaryTransition
    } else {
        CursorPlaneAction::EmbedInPrimary
    };
    PlaneSchedulingDecision {
        delivery: CursorDeliveryChoice::Software {
            revision: input.revision,
        },
        direct_scanout_compatible: false,
        primary_action: if transition {
            PrimaryPlaneAction::TransitionToComposed
        } else {
            PrimaryPlaneAction::Preserve
        },
        cursor_action,
        pacing_constraint: CursorPacingConstraint::ReactiveDouble,
        test_policy: KmsCursorTestPolicy::NotApplicable,
        reason,
    }
}

fn hardware_decision(
    input: PlaneSchedulingInput<'_>,
    geometry: NormalizedCursorGeometry,
    capability_key: CursorCapabilityKey,
    test_policy: KmsCursorTestPolicy,
    reason: PlaneSchedulingReason,
) -> PlaneSchedulingDecision {
    PlaneSchedulingDecision {
        delivery: CursorDeliveryChoice::Hardware {
            revision: input.revision,
            geometry,
            capability_key,
        },
        direct_scanout_compatible: true,
        primary_action: PrimaryPlaneAction::Preserve,
        cursor_action: if input.cursor_kms_changed {
            CursorPlaneAction::Independent
        } else {
            CursorPlaneAction::None
        },
        pacing_constraint: CursorPacingConstraint::Unchanged,
        test_policy,
        reason,
    }
}

fn rejected_decision(
    input: PlaneSchedulingInput<'_>,
    reason: PlaneSchedulingReason,
) -> PlaneSchedulingDecision {
    PlaneSchedulingDecision {
        delivery: CursorDeliveryChoice::Rejected {
            revision: input.revision,
        },
        direct_scanout_compatible: false,
        primary_action: if input.primary_mode == PlanePrimaryMode::Direct {
            PrimaryPlaneAction::TransitionToComposed
        } else {
            PrimaryPlaneAction::Preserve
        },
        cursor_action: CursorPlaneAction::None,
        pacing_constraint: CursorPacingConstraint::Unchanged,
        test_policy: KmsCursorTestPolicy::NotApplicable,
        reason,
    }
}
