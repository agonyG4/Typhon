use std::collections::{BTreeSet, HashMap};

use super::footprint::node_footprint;
use super::{
    EffectFootprint, EffectNodeId, EffectNodeKind, EffectProgram, EffectValidationError,
    MAX_EFFECT_PROGRAM_NODES,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedEffectProgram {
    pub program: EffectProgram,
    pub topological_order: Vec<EffectNodeId>,
    pub aggregate_footprint: EffectFootprint,
    pub requires_offscreen: bool,
    pub requires_composition: bool,
    pub estimated_passes: u16,
}

pub fn validate_effect_program(
    program: EffectProgram,
) -> Result<ValidatedEffectProgram, EffectValidationError> {
    if program.nodes.len() > MAX_EFFECT_PROGRAM_NODES {
        return Err(EffectValidationError::TooManyNodes);
    }

    let mut nodes = HashMap::with_capacity(program.nodes.len());
    for node in &program.nodes {
        if nodes.insert(node.id, node).is_some() {
            return Err(EffectValidationError::DuplicateNodeId(node.id));
        }
    }
    if !nodes.contains_key(&program.output) {
        return Err(EffectValidationError::MissingOutputNode);
    }

    for node in &program.nodes {
        let expected_arity = match &node.kind {
            EffectNodeKind::Source(_) => 0,
            EffectNodeKind::DualKawaseBlur(_)
            | EffectNodeKind::ColorMatrix(_)
            | EffectNodeKind::Tint(_)
            | EffectNodeKind::Noise(_)
            | EffectNodeKind::Mask(_) => 1,
            EffectNodeKind::CustomFragment(spec) => 1 + spec.auxiliary_inputs.len(),
            EffectNodeKind::Blend(_) => 2,
        };
        if node.inputs.len() != expected_arity {
            return Err(EffectValidationError::InvalidArity { node: node.id });
        }
        for input in &node.inputs {
            if !nodes.contains_key(input) {
                return Err(EffectValidationError::MissingInputNode(*input));
            }
        }
        node_footprint(&node.kind)?;
    }

    let mut indegree: HashMap<EffectNodeId, usize> = program
        .nodes
        .iter()
        .map(|node| (node.id, node.inputs.len()))
        .collect();
    let mut dependents: HashMap<EffectNodeId, Vec<EffectNodeId>> = HashMap::new();
    for node in &program.nodes {
        for input in &node.inputs {
            dependents.entry(*input).or_default().push(node.id);
        }
    }
    for values in dependents.values_mut() {
        values.sort_unstable();
    }

    let mut ready = program
        .nodes
        .iter()
        .filter_map(|node| (indegree[&node.id] == 0).then_some(node.id))
        .collect::<BTreeSet<_>>();
    let mut topological_order = Vec::with_capacity(program.nodes.len());
    while let Some(id) = ready.pop_first() {
        topological_order.push(id);
        if let Some(children) = dependents.get(&id) {
            for child in children {
                let count = indegree
                    .get_mut(child)
                    .expect("validated dependent node must have indegree");
                *count -= 1;
                if *count == 0 {
                    ready.insert(*child);
                }
            }
        }
    }
    if topological_order.len() != program.nodes.len() {
        return Err(EffectValidationError::Cycle);
    }

    let mut aggregate = HashMap::with_capacity(program.nodes.len());
    let mut estimated_passes = 0u16;
    for id in &topological_order {
        let node = nodes[id];
        let stage = node_footprint(&node.kind)?;
        let input_footprint = node
            .inputs
            .iter()
            .filter_map(|input| aggregate.get(input).copied())
            .reduce(EffectFootprint::union)
            .unwrap_or(EffectFootprint::ZERO);
        let footprint = if node.inputs.is_empty() {
            stage
        } else {
            input_footprint.compose(stage)?
        };
        aggregate.insert(*id, footprint);
        estimated_passes = estimated_passes.saturating_add(match node.kind {
            EffectNodeKind::Source(_) => 0,
            EffectNodeKind::DualKawaseBlur(spec) => u16::from(spec.passes) * 2,
            _ => 1,
        });
    }

    let aggregate_footprint = aggregate[&program.output].compose(EffectFootprint {
        sample_radius_x: 0,
        sample_radius_y: 0,
        output_outsets: program.outsets,
    })?;
    let requires_composition = program
        .nodes
        .iter()
        .any(|node| !matches!(&node.kind, EffectNodeKind::Source(_)));

    Ok(ValidatedEffectProgram {
        program,
        topological_order,
        aggregate_footprint,
        requires_offscreen: requires_composition,
        requires_composition,
        estimated_passes,
    })
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn test_backdrop_blur_program() -> EffectProgram {
        let source = EffectNodeId::new(1).unwrap();
        let blur = EffectNodeId::new(2).unwrap();

        EffectProgram {
            id: EffectProgramId::new(1).unwrap(),
            nodes: vec![
                EffectNode::source(source, EffectSource::Backdrop),
                EffectNode::dual_kawase(
                    blur,
                    source,
                    DualKawaseBlurSpec::new(4.0, 2, 1.0).unwrap(),
                ),
            ],
            output: blur,
            working_space: EffectWorkingSpace::LinearSrgb,
            alpha_mode: EffectAlphaMode::Opaque,
            outsets: EffectOutsets::ZERO,
            frame_demand: EffectFrameDemand::OnDamage,
            failure_policy: EffectFailurePolicy::Passthrough,
        }
    }

    #[test]
    fn rejects_missing_output_node() {
        let mut program = test_backdrop_blur_program();
        program.output = EffectNodeId::new(31).unwrap();
        assert_eq!(
            validate_effect_program(program),
            Err(EffectValidationError::MissingOutputNode)
        );
    }

    #[test]
    fn rejects_cycle() {
        let source = EffectNodeId::new(1).unwrap();
        let a = EffectNodeId::new(2).unwrap();
        let b = EffectNodeId::new(3).unwrap();
        let mut program = test_backdrop_blur_program();
        program.nodes = vec![
            EffectNode::source(source, EffectSource::Backdrop),
            EffectNode::tint(a, b, TintSpec::WHITE),
            EffectNode::tint(b, a, TintSpec::WHITE),
        ];
        program.output = a;
        assert_eq!(
            validate_effect_program(program),
            Err(EffectValidationError::Cycle)
        );
    }

    #[test]
    fn rejects_duplicate_node_id() {
        let mut program = test_backdrop_blur_program();
        program.nodes[1].id = program.nodes[0].id;
        let duplicate = program.nodes[0].id;
        assert_eq!(
            validate_effect_program(program),
            Err(EffectValidationError::DuplicateNodeId(duplicate))
        );
    }

    #[test]
    fn rejects_more_than_max_nodes() {
        let mut program = test_backdrop_blur_program();
        program.nodes = (0..=MAX_EFFECT_PROGRAM_NODES)
            .map(|index| {
                EffectNode::source(
                    EffectNodeId::new((index + 1) as u16).unwrap(),
                    EffectSource::Backdrop,
                )
            })
            .collect();
        program.output = EffectNodeId::new(1).unwrap();
        assert_eq!(
            validate_effect_program(program),
            Err(EffectValidationError::TooManyNodes)
        );
    }

    #[test]
    fn rejects_non_finite_blur_radius() {
        assert!(matches!(
            DualKawaseBlurSpec::new(f32::NAN, 2, 1.0),
            Err(EffectValidationError::NonFiniteValue)
        ));
    }

    #[test]
    fn rejects_zero_or_excessive_blur_passes() {
        assert!(DualKawaseBlurSpec::new(4.0, 0, 1.0).is_err());
        assert!(DualKawaseBlurSpec::new(4.0, MAX_EFFECT_BLUR_PASSES + 1, 1.0).is_err());
    }

    #[test]
    fn backdrop_blur_requires_offscreen_and_composition() {
        let validated = validate_effect_program(test_backdrop_blur_program()).unwrap();
        assert!(validated.requires_offscreen);
        assert!(validated.requires_composition);
    }

    #[test]
    fn validation_order_is_deterministic_when_declarations_are_reordered() {
        let mut first = test_backdrop_blur_program();
        let mut second = first.clone();
        second.nodes.reverse();
        first.nodes.swap(0, 1);
        assert_eq!(
            validate_effect_program(first).unwrap().topological_order,
            validate_effect_program(second).unwrap().topological_order
        );
    }
}
