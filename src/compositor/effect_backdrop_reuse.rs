use super::blur_assignment::BlurAssignmentSource;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum EffectBackdropReuse {
    #[default]
    Exact,
    PrecomputedWindowCandidate,
    /// A shell layer blur (topbar, dock) that may sample the shared backdrop
    /// captured below every window. The renderer keeps it only while no draw
    /// below the layer and above that capture point covers the layer's own
    /// bounds, so windows next to the bar never re-run its blur.
    PrecomputedShellCandidate,
}

impl From<BlurAssignmentSource> for EffectBackdropReuse {
    fn from(source: BlurAssignmentSource) -> Self {
        match source {
            BlurAssignmentSource::WaylandAuto | BlurAssignmentSource::WindowRule => {
                Self::PrecomputedWindowCandidate
            }
            BlurAssignmentSource::Client | BlurAssignmentSource::LayerRule => Self::Exact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::blur_assignment::BlurAssignmentSource;
    use super::EffectBackdropReuse;

    #[test]
    fn blur_assignment_provenance_maps_only_synthesized_window_sources_to_candidate() {
        assert_eq!(
            EffectBackdropReuse::from(BlurAssignmentSource::Client),
            EffectBackdropReuse::Exact
        );
        assert_eq!(
            EffectBackdropReuse::from(BlurAssignmentSource::WaylandAuto),
            EffectBackdropReuse::PrecomputedWindowCandidate
        );
        assert_eq!(
            EffectBackdropReuse::from(BlurAssignmentSource::WindowRule),
            EffectBackdropReuse::PrecomputedWindowCandidate
        );
        assert_eq!(
            EffectBackdropReuse::from(BlurAssignmentSource::LayerRule),
            EffectBackdropReuse::Exact
        );
    }
}
