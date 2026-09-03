use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn region(ops: impl IntoIterator<Item = InputRegionOp>) -> SurfaceInputRegion {
        SurfaceInputRegion::Custom(ops.into_iter().collect())
    }

    #[test]
    fn full_surface_resolution_work_is_independent_of_surface_area() {
        let mut observations = Vec::new();
        for (width, height) in [(40, 30), (1_920, 929), (7_680, 4_320)] {
            let result = resolve_pointer_constraint_region_for_test(
                &SurfaceInputRegion::Default,
                &SurfaceInputRegion::Default,
                width,
                height,
            );
            observations.push((result.output, result.operation_count));
        }

        assert_eq!(observations[0].0, observations[1].0);
        assert_eq!(observations[1].0, observations[2].0);
        assert_eq!(observations[0].1, observations[1].1);
        assert_eq!(observations[1].1, observations[2].1);
        assert_eq!(observations[0].1, 1);
    }

    #[test]
    fn ordered_add_subtract_and_clipping_match_half_open_membership() {
        let constraint = region([
            InputRegionOp::Add(InputRegionRect::new(-10, -10, 30, 30).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(4, 4, 4, 4).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(6, 6, 2, 2).unwrap()),
        ]);
        let input = region([
            InputRegionOp::Add(InputRegionRect::new(2, 2, 8, 8).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(7, 0, 4, 20).unwrap()),
        ]);
        let result = resolve_pointer_constraint_region_for_test(&constraint, &input, 10, 10);
        let oracle = raster_pointer_constraint_region_for_test(&constraint, &input, 10, 10);

        assert_eq!(result.output, oracle);
        assert!(result.operation_count > 0);
    }

    #[test]
    fn closest_point_matches_raster_oracle_for_fractional_and_tied_probes() {
        let constraint = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 10).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(4, 0, 2, 10).unwrap()),
        ]);
        let input = SurfaceInputRegion::Default;
        let result = resolve_pointer_constraint_region_for_test(&constraint, &input, 10, 10);
        let oracle = raster_pointer_constraint_region_for_test(&constraint, &input, 10, 10);

        for position in [
            OutputPosition { x: 5.5, y: 5.5 },
            OutputPosition { x: 5.0, y: 5.0 },
            OutputPosition { x: -3.25, y: 5.75 },
            OutputPosition { x: 20.0, y: 20.0 },
        ] {
            assert_eq!(
                result.output.as_ref().map(|region| region.closest_point(position)),
                oracle.as_ref().map(|region| region.closest_point(position)),
                "probe {position:?}"
            );
        }
    }

    #[test]
    fn extreme_protocol_edges_are_clipped_without_overflow() {
        let extreme = region([
            InputRegionOp::Add(InputRegionRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(i32::MAX - 2, i32::MAX - 2, 2, 2).unwrap()),
        ]);
        let result = resolve_pointer_constraint_region_for_test(
            &extreme,
            &SurfaceInputRegion::Default,
            7_680,
            4_320,
        );

        assert_eq!(result.output.as_ref().map(|region| region.rects.len()), Some(1));
        assert_eq!(result.output.as_ref().map(|region| region.rects[0].width), Some(7_680.0));
        assert_eq!(result.output.as_ref().map(|region| region.rects[0].height), Some(4_320.0));
    }

    #[test]
    fn committed_input_region_is_snapshotted_once_and_then_can_be_resolved_without_locking() {
        let surface = SurfaceData::new(1);
        let committed = region([InputRegionOp::Add(InputRegionRect::new(2, 3, 4, 5).unwrap())]);
        assert!(surface.apply_input_region_change(Some(committed.clone())));

        let snapshot = surface.committed_input_region_snapshot();
        assert_eq!(snapshot, committed);

        let result = resolve_pointer_constraint_region_for_test(
            &SurfaceInputRegion::Default,
            &snapshot,
            10,
            10,
        );
        assert_eq!(result.output.as_ref().map(|region| region.rects.len()), Some(1));
        assert_eq!(result.output.as_ref().map(|region| region.rects[0].x), Some(2.0));
    }
}
