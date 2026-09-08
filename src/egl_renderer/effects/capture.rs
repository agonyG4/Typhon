use oblivion_one::compositor::{EffectAnchor, VisualGroupId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureLayer {
    Surface(u32),
    Other,
}

pub(crate) fn indices_below_anchor(layers: &[CaptureLayer], anchor: EffectAnchor) -> Vec<usize> {
    let cutoff = match anchor {
        EffectAnchor::BeforeSurface(surface_id) | EffectAnchor::ReplaceSurface(surface_id) => {
            layers
                .iter()
                .position(|layer| *layer == CaptureLayer::Surface(surface_id))
                .unwrap_or(layers.len())
        }
        EffectAnchor::AfterSurface(surface_id) => layers
            .iter()
            .rposition(|layer| *layer == CaptureLayer::Surface(surface_id))
            .map_or(layers.len(), |index| index.saturating_add(1)),
        EffectAnchor::OutputPostProcess => layers.len(),
    };
    (0..cutoff).collect()
}

pub(crate) fn indices_for_capture(
    layers: &[CaptureLayer],
    visual_groups: &[Option<VisualGroupId>],
    anchor: EffectAnchor,
    target_content: bool,
    visual_group: Option<VisualGroupId>,
) -> Vec<usize> {
    if let Some(visual_group) = visual_group {
        if target_content {
            return visual_groups
                .iter()
                .enumerate()
                .filter_map(|(index, group)| (*group == Some(visual_group)).then_some(index))
                .collect();
        }
        let cutoff = match anchor {
            EffectAnchor::BeforeSurface(_) | EffectAnchor::ReplaceSurface(_) => visual_groups
                .iter()
                .position(|group| *group == Some(visual_group))
                .unwrap_or(visual_groups.len()),
            EffectAnchor::AfterSurface(_) => visual_groups
                .iter()
                .rposition(|group| *group == Some(visual_group))
                .map_or(visual_groups.len(), |index| index.saturating_add(1)),
            EffectAnchor::OutputPostProcess => visual_groups.len(),
        };
        return (0..cutoff).collect();
    }
    if !target_content {
        return indices_below_anchor(layers, anchor);
    }
    let target = match anchor {
        EffectAnchor::BeforeSurface(surface_id)
        | EffectAnchor::ReplaceSurface(surface_id)
        | EffectAnchor::AfterSurface(surface_id) => surface_id,
        EffectAnchor::OutputPostProcess => return Vec::new(),
    };
    layers
        .iter()
        .enumerate()
        .filter_map(|(index, layer)| (*layer == CaptureLayer::Surface(target)).then_some(index))
        .collect()
}

#[cfg(test)]
mod target_content_tests {
    use super::*;

    #[test]
    fn target_content_capture_contains_only_the_target_surface() {
        let layers = [
            CaptureLayer::Other,
            CaptureLayer::Surface(10),
            CaptureLayer::Surface(20),
            CaptureLayer::Surface(30),
        ];
        assert_eq!(
            indices_for_capture(
                &layers,
                &[None, None, None, None],
                EffectAnchor::BeforeSurface(20),
                true,
                None,
            ),
            vec![2]
        );
    }

    #[test]
    fn visual_group_capture_keeps_subsurfaces_and_separates_popups() {
        let group = VisualGroupId::new(1).unwrap();
        let popup_group = VisualGroupId::new(2).unwrap();
        let layers = [
            CaptureLayer::Other,
            CaptureLayer::Surface(10),
            CaptureLayer::Surface(11),
            CaptureLayer::Surface(20),
        ];
        let groups = [Some(group), Some(group), Some(group), Some(popup_group)];
        assert_eq!(
            indices_for_capture(
                &layers,
                &groups,
                EffectAnchor::BeforeSurface(10),
                true,
                Some(group),
            ),
            vec![0, 1, 2]
        );
        assert_eq!(
            indices_for_capture(
                &layers,
                &groups,
                EffectAnchor::BeforeSurface(20),
                false,
                Some(popup_group),
            ),
            vec![0, 1, 2]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdrop_capture_stops_before_target_and_higher_surfaces() {
        let layers = [
            CaptureLayer::Other,
            CaptureLayer::Surface(10),
            CaptureLayer::Surface(20),
            CaptureLayer::Surface(30),
        ];
        assert_eq!(
            indices_below_anchor(&layers, EffectAnchor::BeforeSurface(20)),
            vec![0, 1]
        );
    }

    #[test]
    fn output_capture_includes_the_full_ordered_scene() {
        let layers = [CaptureLayer::Other, CaptureLayer::Surface(10)];
        assert_eq!(
            indices_below_anchor(&layers, EffectAnchor::OutputPostProcess),
            vec![0, 1]
        );
    }
}
