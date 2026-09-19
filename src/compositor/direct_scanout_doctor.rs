use super::{
    DirectScanoutEffectAnalysis, DirectScanoutEffectDisposition, DirectScanoutEffectDoctorDetails,
    EffectAnchor,
};
use crate::effects::{EffectRegistryGeneration, builtin_background_blur_program_id};

pub(super) fn effect_details(
    analysis: &DirectScanoutEffectAnalysis,
    trusted_effect_registry: &EffectRegistryGeneration,
) -> DirectScanoutEffectDoctorDetails {
    let details = analysis
        .instances
        .iter()
        .map(|instance| {
            let (anchor, target_surface) = match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id) => (
                    format!("before_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::ReplaceSurface(surface_id) => (
                    format!("replace_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::AfterSurface(surface_id) => (
                    format!("after_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::OutputPostProcess => ("output_post_process".to_string(), None),
            };
            let region = if instance.region.rects().is_empty() {
                "full".to_string()
            } else {
                instance
                    .region
                    .rects()
                    .iter()
                    .map(|rect| format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height))
                    .collect::<Vec<_>>()
                    .join(";")
            };
            let program_name = trusted_effect_registry
                .effect_for_program(instance.program)
                .map(|effect| effect.name.as_str())
                .or_else(|| {
                    (instance.program == builtin_background_blur_program_id())
                        .then_some("background_blur")
                })
                .unwrap_or("unknown");
            let requires_composition = matches!(
                instance.disposition,
                DirectScanoutEffectDisposition::ContributingAboveSource
                    | DirectScanoutEffectDisposition::ContributingAtSource
                    | DirectScanoutEffectDisposition::OutputPostProcess
                    | DirectScanoutEffectDisposition::UnknownOrder
            );
            format!(
                "{{id:{} program:{} program_id:{} anchor:{} region:{} target_surface:{} disposition:{} requires_composition:{}}}",
                instance.id.get(),
                program_name,
                instance.program.get(),
                anchor,
                region,
                target_surface.map_or_else(|| "none".to_string(), |id| id.to_string()),
                instance.disposition.as_str(),
                requires_composition,
            )
        })
        .collect::<Vec<_>>();
    DirectScanoutEffectDoctorDetails {
        raw_instance_count: analysis.raw_instance_count,
        presentation_instance_count: analysis.presentation_instance_count,
        culled_instance_count: analysis.culled_instance_count,
        outside_output_instance_count: analysis.outside_output_instance_count,
        occluded_instance_count: analysis.occluded_instance_count,
        contributing_instance_count: analysis.contributing_instance_count,
        requires_composition: analysis.requires_composition,
        details,
        details_truncated: analysis.instances_truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::DirectScanoutEffectInstanceAnalysis;
    use crate::effects::{
        EffectInstanceId, EffectProgramId, EffectRect, EffectRegion, EffectRegistryGeneration,
    };

    fn instance(
        id: u64,
        disposition: DirectScanoutEffectDisposition,
    ) -> DirectScanoutEffectInstanceAnalysis {
        DirectScanoutEffectInstanceAnalysis {
            id: EffectInstanceId::new(id).unwrap(),
            program: EffectProgramId::new(2).unwrap(),
            anchor: EffectAnchor::BeforeSurface(7),
            region: EffectRegion::from_rect(EffectRect::new(0, 0, 10, 10).unwrap()),
            disposition,
        }
    }

    #[test]
    fn effect_details_report_the_typed_contribution_decisions() {
        let analysis = DirectScanoutEffectAnalysis {
            raw_instance_count: 3,
            presentation_instance_count: 2,
            culled_instance_count: 1,
            outside_output_instance_count: 0,
            occluded_instance_count: 1,
            contributing_instance_count: 1,
            requires_composition: true,
            instances: vec![
                instance(1, DirectScanoutEffectDisposition::PresentationCulled),
                instance(
                    2,
                    DirectScanoutEffectDisposition::OccludedByOpaqueScanoutSource,
                ),
                instance(3, DirectScanoutEffectDisposition::ContributingAboveSource),
            ],
            instances_truncated: false,
        };
        let details = effect_details(&analysis, &EffectRegistryGeneration::empty());

        assert_eq!(details.raw_instance_count, 3);
        assert_eq!(details.presentation_instance_count, 2);
        assert_eq!(details.culled_instance_count, 1);
        assert_eq!(details.occluded_instance_count, 1);
        assert_eq!(details.contributing_instance_count, 1);
        assert!(details.requires_composition);
        assert!(
            details
                .details
                .iter()
                .any(|detail| detail.contains("disposition:presentation_culled"))
        );
        assert!(
            details
                .details
                .iter()
                .any(|detail| detail.contains("disposition:occluded_by_scanout_source"))
        );
        assert!(
            details
                .details
                .iter()
                .any(|detail| detail.contains("disposition:contributing_above_source"))
        );
    }
}
