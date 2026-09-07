use oblivion_one::compositor::EffectAnchor;

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
