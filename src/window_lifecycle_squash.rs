use crate::presentation_animation::PresentationRect;

pub(crate) const MINIMIZE_SQUASH_BASE_DURATION_MS: u64 = 250;

pub(crate) fn squash_deformation_amount(raw_progress: f64) -> f64 {
    raw_progress.clamp(0.0, 1.0).powi(3)
}

pub(crate) fn squash_client_rect(
    source_client_rect: PresentationRect,
    anchor_rect: PresentationRect,
    raw_progress: f64,
) -> Option<PresentationRect> {
    let amount = squash_deformation_amount(raw_progress);
    lerp_rect(source_client_rect, anchor_rect, amount)
}

pub(crate) fn squash_transform_rect(
    source_client_rect: PresentationRect,
    current_client_rect: PresentationRect,
    source_rect: PresentationRect,
) -> Option<PresentationRect> {
    let scale_x = current_client_rect.width() / source_client_rect.width();
    let scale_y = current_client_rect.height() / source_client_rect.height();
    PresentationRect::new(
        current_client_rect.x() + (source_rect.x() - source_client_rect.x()) * scale_x,
        current_client_rect.y() + (source_rect.y() - source_client_rect.y()) * scale_y,
        source_rect.width() * scale_x,
        source_rect.height() * scale_y,
    )
}

pub(crate) fn squash_visual_rect(
    source_client_rect: PresentationRect,
    anchor_rect: PresentationRect,
    source_visual_rect: PresentationRect,
    raw_progress: f64,
) -> Option<PresentationRect> {
    let current_client_rect = squash_client_rect(source_client_rect, anchor_rect, raw_progress)?;
    squash_transform_rect(source_client_rect, current_client_rect, source_visual_rect)
}

pub(crate) fn squash_visual_transition_bounds(
    source_client_rect: PresentationRect,
    anchor_rect: PresentationRect,
    source_visual_rect: PresentationRect,
) -> Option<PresentationRect> {
    let target_visual_rect =
        squash_transform_rect(source_client_rect, anchor_rect, source_visual_rect)?;
    let left = source_visual_rect.x().min(target_visual_rect.x());
    let top = source_visual_rect.y().min(target_visual_rect.y());
    let right = (source_visual_rect.x() + source_visual_rect.width())
        .max(target_visual_rect.x() + target_visual_rect.width());
    let bottom = (source_visual_rect.y() + source_visual_rect.height())
        .max(target_visual_rect.y() + target_visual_rect.height());
    PresentationRect::new(left, top, right - left, bottom - top)
}

pub(crate) fn squash_opacity(raw_progress: f64, canonical_opacity: f64) -> f64 {
    canonical_opacity.clamp(0.0, 1.0) * (1.0 - squash_deformation_amount(raw_progress))
}

pub(crate) fn squash_duration_nanos(speed: f64, remaining_progress: f64) -> u64 {
    lifecycle_duration_nanos(MINIMIZE_SQUASH_BASE_DURATION_MS, speed, remaining_progress)
}

pub(crate) fn lifecycle_duration_nanos(
    base_duration_ms: u64,
    speed: f64,
    remaining_progress: f64,
) -> u64 {
    let speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let remaining_progress = if remaining_progress.is_finite() {
        remaining_progress.clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((base_duration_ms as f64 * 1_000_000.0 / speed) * remaining_progress).round() as u64
}

fn lerp_rect(
    source: PresentationRect,
    target: PresentationRect,
    amount: f64,
) -> Option<PresentationRect> {
    PresentationRect::new(
        source.x() + (target.x() - source.x()) * amount,
        source.y() + (target.y() - source.y()) * amount,
        source.width() + (target.width() - source.width()) * amount,
        source.height() + (target.height() - source.height()) * amount,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation_animation::PresentationRect;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
        PresentationRect::new(x, y, width, height).expect("valid test rectangle")
    }

    #[test]
    fn squash_client_rect_uses_cubic_progress_for_both_directions() {
        let source = rect(100.0, 50.0, 800.0, 600.0);
        let anchor = rect(920.0, 700.0, 64.0, 40.0);

        assert_eq!(squash_deformation_amount(0.0), 0.0);
        assert_eq!(squash_client_rect(source, anchor, 0.0), Some(source));
        assert_eq!(
            squash_deformation_amount(0.5),
            0.125,
            "the same raw progress has the same deformation after reversal"
        );
        assert_eq!(
            squash_client_rect(source, anchor, 0.5),
            Some(rect(202.5, 131.25, 708.0, 530.0))
        );
        assert_eq!(squash_client_rect(source, anchor, 1.0), Some(anchor));
    }

    #[test]
    fn squash_opacity_preserves_canonical_translucency_and_restore_curve() {
        let source = rect(100.0, 50.0, 800.0, 600.0);
        let anchor = rect(920.0, 700.0, 64.0, 40.0);

        assert_eq!(squash_opacity(0.0, 0.42), 0.42);
        assert_eq!(squash_opacity(0.5, 0.42), 0.42 * 0.875);
        assert_eq!(squash_opacity(1.0, 0.42), 0.0);

        let restore_elapsed = 0.5;
        let raw_progress = 1.0 - restore_elapsed;
        assert_eq!(1.0 - squash_deformation_amount(raw_progress), 0.875);
        assert_eq!(
            squash_client_rect(source, anchor, raw_progress),
            squash_client_rect(source, anchor, 0.5)
        );
        assert_eq!(
            squash_opacity(raw_progress, 0.42),
            squash_opacity(0.5, 0.42)
        );
    }

    #[test]
    fn squash_affine_transform_keeps_visual_extensions_and_subsurface_offsets() {
        let source_client = rect(100.0, 50.0, 800.0, 600.0);
        let anchor = rect(920.0, 700.0, 64.0, 40.0);
        let visual_extension = rect(80.0, 10.0, 840.0, 680.0);
        let offset_subsurface = rect(240.0, 180.0, 90.0, 70.0);

        assert_eq!(
            squash_transform_rect(source_client, source_client, visual_extension),
            Some(visual_extension)
        );
        let half = squash_client_rect(source_client, anchor, 0.5).unwrap();
        let mapped_extension =
            squash_transform_rect(source_client, half, visual_extension).unwrap();
        let mapped_subsurface =
            squash_transform_rect(source_client, half, offset_subsurface).unwrap();
        assert_eq!(
            mapped_extension,
            rect(184.8, 95.91666666666667, 743.4, 600.6666666666666)
        );
        assert_eq!(
            mapped_subsurface,
            rect(326.4, 246.08333333333331, 79.65, 61.83333333333333)
        );
    }

    #[test]
    fn squash_damage_bounds_cover_the_complete_affine_path() {
        let source_client = rect(100.0, 50.0, 800.0, 600.0);
        let anchor = rect(920.0, 700.0, 64.0, 40.0);
        let source_visual = rect(80.0, 10.0, 840.0, 680.0);

        let bounds = squash_visual_transition_bounds(source_client, anchor, source_visual).unwrap();
        assert!(bounds.x() <= source_visual.x());
        assert!(bounds.y() <= source_visual.y());
        assert!(bounds.x() + bounds.width() >= anchor.x());
        assert!(bounds.y() + bounds.height() >= anchor.y() + anchor.height());
        assert!(bounds.width() < 1920.0);
        assert!(bounds.height() < 1080.0);
    }

    #[test]
    fn squash_duration_uses_250_ms_speed_scaling_and_remaining_progress() {
        assert_eq!(squash_duration_nanos(1.0, 1.0), 250_000_000);
        assert_eq!(squash_duration_nanos(2.0, 1.0), 125_000_000);
        assert_eq!(squash_duration_nanos(0.5, 1.0), 500_000_000);
        assert_eq!(squash_duration_nanos(1.0, 0.35), 87_500_000);
        assert_eq!(squash_duration_nanos(1.0, 0.0), 0);
    }
}
