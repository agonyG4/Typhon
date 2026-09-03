use super::*;
use std::time::Instant;

#[derive(Debug)]
pub(in crate::compositor) struct ResolvedPointerConstraintRegion {
    pub(in crate::compositor) region: Option<OutputRegion>,
    pub(in crate::compositor) timing: Option<PointerConstraintRegionResolutionTiming>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SurfaceRect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl SurfaceRect {
    fn from_input_rect(rect: InputRegionRect) -> Self {
        let (x, y, width, height) = rect.coordinates();
        let left = i64::from(x);
        let top = i64::from(y);
        Self {
            left,
            top,
            right: left.saturating_add(i64::from(width)),
            bottom: top.saturating_add(i64::from(height)),
        }
    }

    fn full(width: u32, height: u32) -> Option<Self> {
        Self::new(0, 0, i64::from(width), i64::from(height))
    }

    fn new(left: i64, top: i64, right: i64, bottom: i64) -> Option<Self> {
        (right > left && bottom > top).then_some(Self {
            left,
            top,
            right,
            bottom,
        })
    }

    fn intersect(self, other: Self) -> Option<Self> {
        Self::new(
            self.left.max(other.left),
            self.top.max(other.top),
            self.right.min(other.right),
            self.bottom.min(other.bottom),
        )
    }

    fn subtract(self, excluded: Self) -> Vec<Self> {
        let Some(intersection) = self.intersect(excluded) else {
            return vec![self];
        };
        let mut pieces = Vec::with_capacity(4);
        Self::push(
            &mut pieces,
            self.left,
            self.top,
            self.right,
            intersection.top,
        );
        Self::push(
            &mut pieces,
            self.left,
            intersection.bottom,
            self.right,
            self.bottom,
        );
        Self::push(
            &mut pieces,
            self.left,
            intersection.top,
            intersection.left,
            intersection.bottom,
        );
        Self::push(
            &mut pieces,
            intersection.right,
            intersection.top,
            self.right,
            intersection.bottom,
        );
        pieces
    }

    fn push(pieces: &mut Vec<Self>, left: i64, top: i64, right: i64, bottom: i64) {
        if let Some(rect) = Self::new(left, top, right, bottom) {
            pieces.push(rect);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SurfaceRectRegion {
    rects: Vec<SurfaceRect>,
}

#[derive(Debug)]
struct SurfaceRectBand {
    top: i64,
    bottom: i64,
    intervals: Vec<(i64, i64)>,
}

impl SurfaceRectRegion {
    fn empty() -> Self {
        Self::default()
    }

    fn full(width: u32, height: u32) -> Self {
        let mut region = Self::empty();
        if let Some(rect) = SurfaceRect::full(width, height) {
            region.rects.push(rect);
        }
        region
    }

    fn from_surface_input_region(input: &SurfaceInputRegion, width: u32, height: u32) -> Self {
        match input {
            SurfaceInputRegion::Default => Self::full(width, height),
            SurfaceInputRegion::Custom(ops) => {
                let bounds = SurfaceRect::full(width, height);
                let mut region = Self::empty();
                for op in ops.iter().copied() {
                    let Some(rect) = bounds.and_then(|bounds| {
                        SurfaceRect::from_input_rect(op.rect()).intersect(bounds)
                    }) else {
                        continue;
                    };
                    match op {
                        InputRegionOp::Add(_) => region.add(rect),
                        InputRegionOp::Subtract(_) => region.subtract_rect(rect),
                    }
                }
                region
            }
        }
    }

    fn add(&mut self, rect: SurfaceRect) {
        let mut pieces = vec![rect];
        for existing in self.rects.iter().copied() {
            pieces = pieces
                .into_iter()
                .flat_map(|piece| {
                    record_region_operation();
                    piece.subtract(existing)
                })
                .collect();
            if pieces.is_empty() {
                break;
            }
        }
        self.rects.extend(pieces);
        self.sort_deterministically();
    }

    fn subtract_rect(&mut self, excluded: SurfaceRect) {
        self.rects = self
            .rects
            .iter()
            .copied()
            .flat_map(|existing| {
                record_region_operation();
                existing.subtract(excluded)
            })
            .collect();
        self.sort_deterministically();
    }

    fn intersection(&self, other: &Self) -> Self {
        let mut result = Self::empty();
        for left in self.rects.iter().copied() {
            for right in other.rects.iter().copied() {
                record_region_operation();
                if let Some(rect) = left.intersect(right) {
                    result.rects.push(rect);
                }
            }
        }
        result.sort_deterministically();
        result
    }

    fn sort_deterministically(&mut self) {
        self.rects
            .sort_unstable_by_key(|rect| (rect.top, rect.left, rect.bottom, rect.right));
    }

    fn canonicalized(&self) -> Self {
        if self.rects.len() <= 1 {
            return self.clone();
        }

        let mut y_edges = self
            .rects
            .iter()
            .flat_map(|rect| [rect.top, rect.bottom])
            .collect::<Vec<_>>();
        y_edges.sort_unstable();
        y_edges.dedup();

        let mut bands = Vec::<SurfaceRectBand>::new();
        for edge_pair in y_edges.windows(2) {
            let [top, bottom] = *edge_pair else {
                continue;
            };
            let mut intervals = self
                .rects
                .iter()
                .filter_map(|rect| {
                    record_region_operation();
                    (rect.top <= top && rect.bottom >= bottom).then_some((rect.left, rect.right))
                })
                .collect::<Vec<_>>();
            if intervals.is_empty() {
                continue;
            }

            intervals.sort_unstable();
            let mut merged_intervals: Vec<(i64, i64)> = Vec::with_capacity(intervals.len());
            for (left, right) in intervals {
                if let Some(previous) = merged_intervals.last_mut()
                    && left <= previous.1
                {
                    previous.1 = previous.1.max(right);
                } else {
                    merged_intervals.push((left, right));
                }
            }

            if let Some(previous) = bands.last_mut()
                && previous.bottom == top
                && previous.intervals == merged_intervals
            {
                previous.bottom = bottom;
            } else {
                bands.push(SurfaceRectBand {
                    top,
                    bottom,
                    intervals: merged_intervals,
                });
            }
        }

        let rects = bands
            .into_iter()
            .flat_map(|band| {
                band.intervals
                    .into_iter()
                    .map(move |(left, right)| SurfaceRect {
                        left,
                        top: band.top,
                        right,
                        bottom: band.bottom,
                    })
            })
            .collect();
        Self { rects }
    }

    fn into_output_region(self, origin: (i32, i32)) -> Option<OutputRegion> {
        let canonical = self.canonicalized();
        if canonical.rects.is_empty() {
            return None;
        }
        let rects = canonical
            .rects
            .into_iter()
            .filter_map(|rect| {
                OutputRect::new(
                    f64::from(origin.0) + rect.left as f64,
                    f64::from(origin.1) + rect.top as f64,
                    (rect.right - rect.left) as f64,
                    (rect.bottom - rect.top) as f64,
                )
            })
            .collect();
        Some(OutputRegion { rects })
    }
}

pub(in crate::compositor) fn resolve_pointer_constraint_output_region(
    constraint: &SurfaceInputRegion,
    input: &SurfaceInputRegion,
    width: u32,
    height: u32,
    origin: (i32, i32),
) -> Option<OutputRegion> {
    let constraint = SurfaceRectRegion::from_surface_input_region(constraint, width, height);
    let input = SurfaceRectRegion::from_surface_input_region(input, width, height);
    constraint.intersection(&input).into_output_region(origin)
}

pub(in crate::compositor) fn resolve_pointer_constraint_output_region_with_timing(
    constraint: &SurfaceInputRegion,
    input: &SurfaceInputRegion,
    width: u32,
    height: u32,
    origin: (i32, i32),
) -> Option<ResolvedPointerConstraintRegion> {
    let timing_enabled = crate::pointer_debug::timing_trace_enabled();
    let start = timing_enabled.then(Instant::now);
    let thread_cpu_start = timing_enabled.then(region_resolution_thread_cpu_time_ns);
    let region = resolve_pointer_constraint_output_region(constraint, input, width, height, origin);
    let timing = timing_enabled.then(|| PointerConstraintRegionResolutionTiming {
        duration_ns: start
            .and_then(|start| u64::try_from(start.elapsed().as_nanos()).ok())
            .unwrap_or(u64::MAX),
        thread_cpu_ns: thread_cpu_start.flatten().and_then(|start| {
            region_resolution_thread_cpu_time_ns().map(|end| end.saturating_sub(start))
        }),
    });
    Some(ResolvedPointerConstraintRegion { region, timing })
}

fn region_resolution_thread_cpu_time_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } < 0 {
            return None;
        }
        let seconds = u64::try_from(time.tv_sec).ok()?;
        let nanoseconds = u64::try_from(time.tv_nsec).ok()?;
        seconds
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(nanoseconds))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(test)]
#[derive(Debug)]
struct RegionResolutionTestResult {
    output: Option<OutputRegion>,
    operation_count: usize,
}

#[cfg(test)]
thread_local! {
    static REGION_OPERATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_region_operation() {
    REGION_OPERATION_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
#[inline]
fn record_region_operation() {}

#[cfg(test)]
fn resolve_pointer_constraint_region_for_test(
    constraint: &SurfaceInputRegion,
    input: &SurfaceInputRegion,
    width: u32,
    height: u32,
) -> RegionResolutionTestResult {
    REGION_OPERATION_COUNT.with(|count| count.set(0));
    let output = resolve_pointer_constraint_output_region(constraint, input, width, height, (0, 0));
    let operation_count = REGION_OPERATION_COUNT.with(std::cell::Cell::get);
    RegionResolutionTestResult {
        output,
        operation_count,
    }
}

#[cfg(test)]
fn raster_pointer_constraint_region_for_test(
    constraint: &SurfaceInputRegion,
    input: &SurfaceInputRegion,
    width: u32,
    height: u32,
) -> Option<OutputRegion> {
    let mut rows = Vec::new();
    for y in 0..height {
        let mut run_start = None;
        for x in 0..width {
            let contained = constraint.contains(x as f64, y as f64, width, height)
                && input.contains(x as f64, y as f64, width, height);
            match (run_start, contained) {
                (None, true) => run_start = Some(x),
                (Some(start), false) => {
                    if let Some(rect) =
                        OutputRect::new(f64::from(start), f64::from(y), f64::from(x - start), 1.0)
                    {
                        rows.push(rect);
                    }
                    run_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = run_start
            && let Some(rect) = OutputRect::new(
                f64::from(start),
                f64::from(y),
                f64::from(width - start),
                1.0,
            )
        {
            rows.push(rect);
        }
    }
    (!rows.is_empty()).then_some(OutputRegion {
        rects: coalesce_output_row_rects(rows),
    })
}

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

        assert_eq!(
            observations[0].0.as_ref().map(|region| region.rects.len()),
            Some(1)
        );
        assert_eq!(
            observations[1].0.as_ref().map(|region| region.rects.len()),
            Some(1)
        );
        assert_eq!(
            observations[2].0.as_ref().map(|region| region.rects.len()),
            Some(1)
        );
        assert_eq!(
            observations[0]
                .0
                .as_ref()
                .map(|region| region.rects[0].width),
            Some(40.0)
        );
        assert_eq!(
            observations[1]
                .0
                .as_ref()
                .map(|region| region.rects[0].width),
            Some(1_920.0)
        );
        assert_eq!(
            observations[2]
                .0
                .as_ref()
                .map(|region| region.rects[0].width),
            Some(7_680.0)
        );
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

        assert_region_membership_matches(result.output.as_ref(), oracle.as_ref(), 10, 10);
        assert!(result.operation_count > 0);
    }

    #[test]
    fn adjacent_additions_have_canonical_geometry_and_closest_point_behavior() {
        let single = region([InputRegionOp::Add(
            InputRegionRect::new(0, 0, 2, 5).unwrap(),
        )]);
        let adjacent = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 2, 1).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(0, 1, 2, 4).unwrap()),
        ]);
        let single = resolve_pointer_constraint_region_for_test(
            &single,
            &SurfaceInputRegion::Default,
            10,
            10,
        )
        .output
        .expect("single rectangle");
        let adjacent = resolve_pointer_constraint_region_for_test(
            &adjacent,
            &SurfaceInputRegion::Default,
            10,
            10,
        )
        .output
        .expect("adjacent rectangles");

        assert_eq!(adjacent.rects, single.rects);
        assert_eq!(
            adjacent.closest_point(OutputPosition { x: 0.0, y: 0.5 }),
            single.closest_point(OutputPosition { x: 0.0, y: 0.5 })
        );
    }

    #[test]
    fn canonicalization_work_is_independent_of_surface_area() {
        let constraint = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 4).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(0, 4, 10, 6).unwrap()),
        ]);
        let small = resolve_pointer_constraint_region_for_test(
            &constraint,
            &SurfaceInputRegion::Default,
            40,
            30,
        );
        let large = resolve_pointer_constraint_region_for_test(
            &constraint,
            &SurfaceInputRegion::Default,
            7_680,
            4_320,
        );

        assert_eq!(small.operation_count, large.operation_count);
        assert_eq!(small.output, large.output);
    }

    #[test]
    fn equivalent_operation_histories_have_equal_canonical_regions_and_probes() {
        let single = region([InputRegionOp::Add(
            InputRegionRect::new(0, 0, 10, 10).unwrap(),
        )]);
        let adjacent = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 4).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(0, 4, 10, 6).unwrap()),
        ]);
        let hole = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 10).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(4, 4, 2, 2).unwrap()),
        ]);
        let hole_pieces = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 4).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(0, 6, 10, 4).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(0, 4, 4, 2).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(6, 4, 4, 2).unwrap()),
        ]);

        let resolve = |constraint: &SurfaceInputRegion| {
            resolve_pointer_constraint_region_for_test(
                constraint,
                &SurfaceInputRegion::Default,
                10,
                10,
            )
            .output
            .expect("non-empty region")
        };
        let single = resolve(&single);
        let adjacent = resolve(&adjacent);
        let hole = resolve(&hole);
        let hole_pieces = resolve(&hole_pieces);

        assert_eq!(single.rects, adjacent.rects);
        assert_eq!(hole.rects, hole_pieces.rects);
        for position in [
            OutputPosition { x: 5.5, y: 5.5 },
            OutputPosition { x: -2.0, y: 5.0 },
            OutputPosition { x: 5.0, y: 5.0 },
            OutputPosition { x: 20.0, y: 20.0 },
        ] {
            assert_eq!(
                single.closest_point(position),
                adjacent.closest_point(position),
                "adjacent probe {position:?}"
            );
        }
        for position in [
            OutputPosition { x: 5.5, y: 5.0 },
            OutputPosition { x: 5.0, y: 5.0 },
            OutputPosition { x: -2.0, y: 5.0 },
            OutputPosition { x: 20.0, y: 20.0 },
        ] {
            assert_eq!(
                hole.closest_point(position),
                hole_pieces.closest_point(position),
                "hole probe {position:?}"
            );
        }

        let islands = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 2, 2).unwrap()),
            InputRegionOp::Add(InputRegionRect::new(4, 0, 2, 2).unwrap()),
        ]);
        let islands = resolve(&islands);
        assert_eq!(islands.rects.len(), 2);
        assert!(
            !islands
                .rects
                .iter()
                .any(|rect| { rect.x < 4.0 && rect.x + rect.width > 2.0 })
        );
        assert!(!hole.rects.iter().any(|rect| {
            rect.x < 5.0 && rect.x + rect.width > 5.0 && rect.y < 5.0 && rect.y + rect.height > 5.0
        }));
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
            OutputPosition { x: 5.5, y: 5.0 },
            OutputPosition { x: 5.0, y: 5.0 },
            OutputPosition { x: -3.25, y: 5.0 },
            OutputPosition { x: 20.0, y: 20.0 },
        ] {
            assert_eq!(
                result
                    .output
                    .as_ref()
                    .map(|region| region.closest_point(position)),
                oracle.as_ref().map(|region| region.closest_point(position)),
                "probe {position:?}"
            );
        }

        let full_result = resolve_pointer_constraint_region_for_test(
            &SurfaceInputRegion::Default,
            &SurfaceInputRegion::Default,
            10,
            10,
        );
        let full_oracle = raster_pointer_constraint_region_for_test(
            &SurfaceInputRegion::Default,
            &SurfaceInputRegion::Default,
            10,
            10,
        );
        let fractional = OutputPosition { x: 5.5, y: 5.5 };
        assert_eq!(
            full_result
                .output
                .as_ref()
                .map(|region| region.closest_point(fractional)),
            full_oracle
                .as_ref()
                .map(|region| region.closest_point(fractional)),
        );
    }

    #[test]
    fn extreme_protocol_edges_are_clipped_without_overflow() {
        let extreme = region([
            InputRegionOp::Add(InputRegionRect::new(-100, -100, i32::MAX, i32::MAX).unwrap()),
            InputRegionOp::Subtract(
                InputRegionRect::new(i32::MAX - 2, i32::MAX - 2, 2, 2).unwrap(),
            ),
        ]);
        let result = resolve_pointer_constraint_region_for_test(
            &extreme,
            &SurfaceInputRegion::Default,
            7_680,
            4_320,
        );

        assert_eq!(
            result.output.as_ref().map(|region| region.rects.len()),
            Some(1)
        );
        assert_eq!(
            result.output.as_ref().map(|region| region.rects[0].width),
            Some(7_680.0)
        );
        assert_eq!(
            result.output.as_ref().map(|region| region.rects[0].height),
            Some(4_320.0)
        );
    }

    #[test]
    fn defaults_custom_regions_and_empty_intersections_match_the_region_contract() {
        let custom_constraint = region([InputRegionOp::Add(
            InputRegionRect::new(2, 3, 4, 5).unwrap(),
        )]);
        let custom_input = region([InputRegionOp::Add(
            InputRegionRect::new(4, 5, 4, 5).unwrap(),
        )]);
        let disjoint_constraint = region([InputRegionOp::Add(
            InputRegionRect::new(0, 0, 2, 2).unwrap(),
        )]);
        let disjoint_input = region([InputRegionOp::Add(
            InputRegionRect::new(4, 4, 2, 2).unwrap(),
        )]);
        let empty = SurfaceInputRegion::Custom(Vec::new());

        let cases = [
            (&custom_constraint, &SurfaceInputRegion::Default, 20usize),
            (&SurfaceInputRegion::Default, &custom_input, 20usize),
            (&custom_constraint, &custom_input, 6usize),
            (&disjoint_constraint, &disjoint_input, 0usize),
            (&empty, &SurfaceInputRegion::Default, 0usize),
        ];
        for (constraint, input, expected_pixels) in cases {
            let result = resolve_pointer_constraint_region_for_test(constraint, input, 10, 10);
            let oracle = raster_pointer_constraint_region_for_test(constraint, input, 10, 10);
            assert_region_membership_matches(result.output.as_ref(), oracle.as_ref(), 10, 10);
            let actual_pixels = result.output.as_ref().map_or(0, |output| {
                output
                    .rects
                    .iter()
                    .map(|rect| (rect.width * rect.height) as usize)
                    .sum()
            });
            assert_eq!(actual_pixels, expected_pixels);
        }
    }

    #[test]
    fn rectangle_order_is_deterministic_for_subtracted_holes() {
        let constraint = region([
            InputRegionOp::Add(InputRegionRect::new(0, 0, 10, 10).unwrap()),
            InputRegionOp::Subtract(InputRegionRect::new(4, 4, 2, 2).unwrap()),
        ]);
        let result = resolve_pointer_constraint_region_for_test(
            &constraint,
            &SurfaceInputRegion::Default,
            10,
            10,
        );
        let rects = result.output.expect("non-empty hole region").rects;

        assert_eq!(
            rects,
            vec![
                OutputRect::new(0.0, 0.0, 10.0, 4.0).unwrap(),
                OutputRect::new(0.0, 4.0, 4.0, 2.0).unwrap(),
                OutputRect::new(6.0, 4.0, 4.0, 2.0).unwrap(),
                OutputRect::new(0.0, 6.0, 10.0, 4.0).unwrap(),
            ]
        );
    }

    fn assert_region_membership_matches(
        left: Option<&OutputRegion>,
        right: Option<&OutputRegion>,
        width: u32,
        height: u32,
    ) {
        for y in 0..height {
            for x in 0..width {
                let left_contains = left.is_some_and(|region| {
                    region.rects.iter().any(|rect| {
                        f64::from(x) >= rect.x
                            && f64::from(x) < rect.x + rect.width
                            && f64::from(y) >= rect.y
                            && f64::from(y) < rect.y + rect.height
                    })
                });
                let right_contains = right.is_some_and(|region| {
                    region.rects.iter().any(|rect| {
                        f64::from(x) >= rect.x
                            && f64::from(x) < rect.x + rect.width
                            && f64::from(y) >= rect.y
                            && f64::from(y) < rect.y + rect.height
                    })
                });
                assert_eq!(left_contains, right_contains, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn committed_input_region_is_snapshotted_once_and_then_can_be_resolved_without_locking() {
        let surface = SurfaceData::new(1);
        let committed = region([InputRegionOp::Add(
            InputRegionRect::new(2, 3, 4, 5).unwrap(),
        )]);
        assert!(surface.apply_input_region_change(Some(committed.clone())));

        SurfaceData::reset_input_region_snapshot_lock_count();
        let snapshot = surface.committed_input_region_snapshot();
        assert_eq!(snapshot, committed);
        assert_eq!(SurfaceData::input_region_snapshot_lock_count(), 1);

        let result = resolve_pointer_constraint_region_for_test(
            &SurfaceInputRegion::Default,
            &snapshot,
            10,
            10,
        );
        assert_eq!(
            result.output.as_ref().map(|region| region.rects.len()),
            Some(1)
        );
        assert_eq!(
            result.output.as_ref().map(|region| region.rects[0].x),
            Some(2.0)
        );
    }
}
