// =========================================
// =========================================
// src/character_authoring/rig/weights.rs

//! Explain anatomical conflicts and surface discontinuities before and after motion tests.
use super::{math::*, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigWeightCheckOptions {
    pub anatomical_margin_height_fraction: f32,
    pub maximum_anatomical_distance_height_fraction: f32,
    pub minimum_conflicting_weight: f32,
    pub maximum_adjacent_weight_difference: f32,
    pub maximum_adjacent_edge_height_fraction: f32,
    pub maximum_examples: usize,
}
impl Default for RigWeightCheckOptions {
    fn default() -> Self {
        Self {
            anatomical_margin_height_fraction: 0.015,
            maximum_anatomical_distance_height_fraction: 0.12,
            minimum_conflicting_weight: 0.5,
            maximum_adjacent_weight_difference: 0.65,
            maximum_adjacent_edge_height_fraction: 0.02,
            maximum_examples: 64,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigNamedInfluence {
    pub bone: String,
    pub weight: f32,
}

/// Region IDs refer to the exact candidate's bindingOptions.regions array.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigVertexWeightEvidence {
    pub vertex: usize,
    pub position: [f32; 3],
    pub region_index: Option<usize>,
    pub allowed_influences: Vec<String>,
    pub influences: Vec<RigNamedInfluence>,
    pub pending: bool,
    pub expected_family: Option<String>,
    /// Keep proximity evidence separate from the caller's reviewed rigid attachment intent.
    #[serde(default)]
    pub geometric_family: Option<String>,
    #[serde(default)]
    pub anatomical_basis: String,
    #[serde(default)]
    pub surface_region_id: Option<String>,
    /// Same-family bones and the immediate joint transition around the closest segment.
    #[serde(default)]
    pub anatomically_compatible_influences: Vec<String>,
    pub anatomical_distance: f32,
    pub anatomical_margin: f32,
    pub anatomical_confident: bool,
    #[serde(default)]
    pub constraint_id: Option<String>,
    pub suggested_influences: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigAnatomicalConflict {
    pub evidence: RigVertexWeightEvidence,
    pub conflicting_weight: f32,
    pub cause: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigWeightDiscontinuity {
    pub vertices: [usize; 2],
    pub rest_length: f32,
    /// Total variation of normalized bone weights, in 0..=1.
    pub weight_difference: f32,
    pub dominant_bone_hierarchy_distance: usize,
    pub evidence: [RigVertexWeightEvidence; 2],
    pub cause: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigWeightDiagnostics {
    pub binding_fingerprint: String,
    pub anatomy_status: BindingStatus,
    pub continuity_status: BindingStatus,
    pub pending_vertices: Vec<usize>,
    pub anatomical_conflict_count: usize,
    pub discontinuity_count: usize,
    /// Complete indices for refinement, even when diagnostic examples are capped.
    pub suspect_vertices: Vec<usize>,
    pub anatomical_conflicts: Vec<RigAnatomicalConflict>,
    pub discontinuities: Vec<RigWeightDiscontinuity>,
    #[serde(default)]
    pub constraint_status: BindingStatus,
    #[serde(default)]
    pub constraint_violation_count: usize,
    #[serde(default)]
    pub constraint_violations: Vec<RigConstraintViolation>,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigWeightRefinement {
    pub expected_binding_fingerprint: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<RigWeightConstraint>,
    pub regions: Vec<RigWeightRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface_regions: Vec<RigSurfaceRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertex_weights: Vec<RigVertexWeight>,
    /// Ambiguous corrections remain explicit; preview weights are never evidence of certainty.
    pub pending_vertices: Vec<usize>,
}

pub(crate) fn family(id: &str) -> String {
    if [
        "shoulder_",
        "upper_arm_",
        "forearm_",
        "hand_",
        "thumb_",
        "index_",
        "middle_",
        "ring_",
        "pinky_",
    ]
    .iter()
    .any(|p| id.starts_with(p))
    {
        format!("arm_{}", if id.ends_with("_l") { "l" } else { "r" })
    } else if ["upper_leg_", "lower_leg_", "foot_", "toe_"]
        .iter()
        .any(|p| id.starts_with(p))
    {
        format!("leg_{}", if id.ends_with("_l") { "l" } else { "r" })
    } else if matches!(id, "head" | "neck") {
        "head".into()
    } else {
        "torso".into()
    }
}

// Geometry proposes a body family, not ground truth or individual finger segmentation.
pub(crate) struct WeightContext<'a> {
    mesh: &'a RigMesh,
    binding: &'a HumanoidBinding,
    options: &'a RigWeightCheckOptions,
    regions: Vec<Option<usize>>,
    pending: BTreeSet<usize>,
    constraints: Vec<Option<usize>>,
    surfaces: Vec<Option<usize>>,
    hierarchy_distances: Vec<Vec<usize>>,
    segments: Vec<(usize, [f32; 3], String)>,
}
impl<'a> WeightContext<'a> {
    pub(crate) fn new(
        mesh: &'a RigMesh,
        binding: &'a HumanoidBinding,
        options: &'a RigWeightCheckOptions,
    ) -> Self {
        let mut regions = vec![None; mesh.positions.len()];
        let mut pending = BTreeSet::new();
        let mut constraints = vec![None; mesh.positions.len()];
        let mut surfaces = vec![None; mesh.positions.len()];
        if let Some(input) = &binding.binding_options {
            for (i, surface) in input.surface_regions.iter().enumerate() {
                for &v in &surface.vertices {
                    if v < surfaces.len() {
                        surfaces[v] = Some(i);
                    }
                }
            }
            for (i, region) in input.regions.iter().enumerate() {
                for v in &region.vertices {
                    if *v < regions.len() {
                        regions[*v] = Some(i);
                    }
                }
            }
            pending.extend(input.pending_vertices.iter().copied());
            for (i, constraint) in input.constraints.iter().enumerate() {
                for &v in constraint.vertices() {
                    if v < constraints.len() {
                        constraints[v] = Some(i);
                    }
                }
            }
        }
        let standard = humanoid_rig_standard();
        let definitions = &standard.references[0].joints;
        let segments = binding
            .skeleton
            .joints
            .iter()
            .enumerate()
            .filter(|(_, j)| j.core && !j.endpoint && j.id != "root")
            .map(|(i, j)| {
                let end = super::build::child_id(&j.id, definitions)
                    .and_then(|id| binding.skeleton.joints.iter().find(|c| c.id == id))
                    .map_or(j.position, |c| c.position);
                (i, end, family(&j.id))
            })
            .collect();
        Self {
            mesh,
            binding,
            options,
            regions,
            pending,
            constraints,
            surfaces,
            hierarchy_distances: (0..binding.skeleton.joints.len())
                .map(|a| {
                    (0..binding.skeleton.joints.len())
                        .map(|b| bone_distance(binding, a, b))
                        .collect()
                })
                .collect(),
            segments,
        }
    }
    pub(crate) fn evidence(&self, v: usize) -> RigVertexWeightEvidence {
        let p = self.mesh.positions[v];
        let h = self.mesh.inspection.height;
        let mut distances = BTreeMap::<String, f32>::new();
        for (i, end, name) in &self.segments {
            let bone = &self.binding.skeleton.joints[*i];
            if (name.ends_with("_l") && p[0] < -h * 0.015)
                || (name.ends_with("_r") && p[0] > h * 0.015)
            {
                continue;
            }
            let distance = distance_segment(p, bone.position, *end);
            distances
                .entry(name.clone())
                .and_modify(|d| *d = d.min(distance))
                .or_insert(distance);
        }
        let mut ranked = distances.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let best = &ranked[0];
        let margin = ranked.get(1).map_or(h, |n| n.1 - best.1);
        let confident = margin > h * self.options.anatomical_margin_height_fraction
            && best.1 < h * self.options.maximum_anatomical_distance_height_fraction;
        let influences = self.binding.joints[v]
            .iter()
            .zip(self.binding.weights[v])
            .filter(|(_, w)| *w > 0.)
            .map(|(i, weight)| RigNamedInfluence {
                bone: self.binding.skeleton.joints[*i as usize].id.clone(),
                weight,
            })
            .collect();
        let allowed = self.regions[v]
            .and_then(|i| {
                self.binding
                    .binding_options
                    .as_ref()
                    .map(|o| o.regions[i].influences.clone())
            })
            .unwrap_or_default();
        let constraint = self.constraints[v].and_then(|i| {
            self.binding
                .binding_options
                .as_ref()
                .map(|o| &o.constraints[i])
        });
        let allowed = constraint.map_or(allowed, |c| vec![c.joint().into()]);
        // A reviewed rigid hair/garment annotation is an attachment, not a nearby body limb.
        // Constraint integrity and independent motion strain checks still apply to every vertex.
        let surface = self.surfaces[v].and_then(|i| {
            self.binding
                .binding_options
                .as_ref()
                .map(|o| &o.surface_regions[i])
        });
        let body = surface.and_then(|s| match s.purpose {
            RigSurfacePurpose::Body { family } => Some(family.as_str()),
            _ => None,
        });
        let anchors = surface.and_then(|s| match &s.purpose {
            RigSurfacePurpose::Attachment { anchors } => Some(anchors),
            _ => None,
        });
        let expected = constraint.map_or_else(
            || body.unwrap_or(&best.0).to_string(),
            |c| family(c.joint()),
        );
        let closest = self
            .segments
            .iter()
            .filter(|(_, _, f)| f == &expected)
            .min_by(|(a, end_a, _), (b, end_b, _)| {
                distance_segment(p, self.binding.skeleton.joints[*a].position, *end_a).total_cmp(
                    &distance_segment(p, self.binding.skeleton.joints[*b].position, *end_b),
                )
            })
            .map(|(i, _, _)| *i);
        // Hips/upper-leg and chest/shoulder blends are legitimate articulation boundaries.
        // This cannot authorize remote torso weights on hands or distal limbs.
        let compatible = self
            .binding
            .skeleton
            .joints
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                !j.endpoint
                    && j.id != "root"
                    && (j.core
                        || constraint.is_some_and(|c| c.joint() == j.id)
                        || anchors.is_some_and(|a| a.contains(&j.id)))
            })
            .filter(|(i, j)| {
                if let Some(c) = constraint {
                    return j.id == c.joint();
                }
                if let Some(anchors) = anchors {
                    return anchors.contains(&j.id);
                }
                family(&j.id) == expected
                    || closest.is_some_and(|near| {
                        length(sub(p, j.position)) < h * 0.14
                            && self.hierarchy_distances[near][*i] <= 3
                    })
            })
            .map(|(_, j)| j.id.clone())
            .collect::<Vec<_>>();
        RigVertexWeightEvidence {
            vertex: v,
            position: p,
            region_index: self.regions[v],
            allowed_influences: allowed,
            influences,
            pending: self.pending.contains(&v),
            expected_family: Some(expected),
            geometric_family: Some(best.0.clone()),
            anatomical_basis: if constraint.is_some() {
                "reviewedRigidConstraint"
            } else if anchors.is_some() {
                "reviewedAttachment"
            } else if body.is_some() {
                "reviewedBodySurface"
            } else {
                "geometricProximity"
            }
            .into(),
            surface_region_id: surface.map(|s| s.id.clone()),
            anatomically_compatible_influences: compatible.clone(),
            anatomical_distance: best.1,
            anatomical_margin: margin,
            anatomical_confident: confident || constraint.is_some() || surface.is_some(),
            constraint_id: constraint.map(|c| c.id().into()),
            suggested_influences: compatible,
        }
    }
}

fn validate_options(options: &RigWeightCheckOptions) -> Result<(), RigError> {
    for value in [
        options.anatomical_margin_height_fraction,
        options.maximum_anatomical_distance_height_fraction,
        options.minimum_conflicting_weight,
        options.maximum_adjacent_weight_difference,
        options.maximum_adjacent_edge_height_fraction,
    ] {
        if !value.is_finite() || value <= 0. || value > 1. {
            return Err(RigError::Invalid(
                "Weight diagnostic thresholds must be finite and in (0, 1]".into(),
            ));
        }
    }
    if options.maximum_examples == 0 || options.maximum_examples > 1024 {
        return Err(RigError::Invalid(
            "Weight diagnostics require 1..=1024 examples".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_binding(mesh: &RigMesh, binding: &HumanoidBinding) -> Result<(), RigError> {
    super::mesh::validate_mesh(mesh)?;
    if binding.skeleton.geometry_fingerprint != mesh.inspection.geometry_fingerprint {
        return Err(RigError::SourceChanged);
    }
    if super::binding::binding_fingerprint(binding)? != binding.fingerprint {
        return Err(RigError::Invalid(
            "Weight diagnostic binding fingerprint changed".into(),
        ));
    }
    if let Some(options) = &binding.binding_options {
        super::binding::validate_surface_regions(mesh.positions.len(), &binding.skeleton, options)?;
    }
    if binding.joints.len() != mesh.positions.len()
        || binding.weights.len() != mesh.positions.len()
        || binding
            .skeleton
            .joints
            .iter()
            .filter(|j| j.core && !j.endpoint && j.id != "root")
            .count()
            != 52
        || binding
            .joints
            .iter()
            .flatten()
            .any(|j| *j as usize >= binding.skeleton.joints.len())
        || binding.weights.iter().any(|row| {
            row.iter().any(|w| !w.is_finite() || *w < 0.)
                || (row.iter().sum::<f32>() - 1.).abs() > 1e-4
        })
    {
        return Err(RigError::Invalid(
            "Weight diagnostics require a valid humanoid skin".into(),
        ));
    }
    Ok(())
}

fn bone_distance(binding: &HumanoidBinding, a: usize, b: usize) -> usize {
    let bones = &binding.skeleton.joints;
    let ancestors = |mut i: usize| {
        let mut chain = vec![bones[i].id.as_str()];
        while let Some(parent) = &bones[i].parent {
            let Some(next) = bones.iter().position(|j| &j.id == parent) else {
                break;
            };
            if chain.contains(&bones[next].id.as_str()) {
                break;
            }
            i = next;
            chain.push(bones[i].id.as_str());
        }
        chain
    };
    let left = ancestors(a);
    let right = ancestors(b);
    left.iter()
        .enumerate()
        .filter_map(|(i, id)| right.iter().position(|r| r == id).map(|j| i + j))
        .min()
        .unwrap_or(bones.len())
}

fn dominant(binding: &HumanoidBinding, v: usize) -> usize {
    let slot = (0..4)
        .max_by(|a, b| binding.weights[v][*a].total_cmp(&binding.weights[v][*b]))
        .unwrap();
    binding.joints[v][slot] as usize
}
fn difference(binding: &HumanoidBinding, a: usize, b: usize) -> f32 {
    let mut weights = BTreeMap::<u16, f32>::new();
    for (id, w) in binding.joints[a].iter().zip(binding.weights[a]) {
        *weights.entry(*id).or_default() += w;
    }
    for (id, w) in binding.joints[b].iter().zip(binding.weights[b]) {
        *weights.entry(*id).or_default() -= w;
    }
    weights.values().map(|w| w.abs()).sum::<f32>() * 0.5
}

/// Standalone checks expose likely wrong regions without requiring a renderer or action library.
pub fn inspect_rig_weights(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    options: &RigWeightCheckOptions,
) -> Result<RigWeightDiagnostics, RigError> {
    validate_options(options)?;
    validate_binding(mesh, binding)?;
    let context = WeightContext::new(mesh, binding, options);
    let mut report = RigWeightDiagnostics {
        binding_fingerprint: binding.fingerprint.clone(), anatomy_status: BindingStatus::Pass,
        continuity_status: BindingStatus::Pass, pending_vertices: context.pending.iter().copied().collect(),
        anatomical_conflict_count: 0, discontinuity_count: 0, suspect_vertices: vec![],
        anatomical_conflicts: vec![], discontinuities: vec![],
        constraint_status: BindingStatus::Pass, constraint_violation_count: 0, constraint_violations: vec![],
        limitations: vec!["Nearest fitted bone families are geometry heuristics, not anatomical ground truth; inspect touching limbs, clothing and uncertain landmarks.".into()],
    };
    let mut suspects = BTreeSet::new();
    for v in 0..mesh.positions.len() {
        let evidence = context.evidence(v);
        let foreign = evidence
            .influences
            .iter()
            .filter(|w| {
                let j = binding
                    .skeleton
                    .joints
                    .iter()
                    .find(|j| j.id == w.bone)
                    .unwrap();
                j.core
                    && !evidence
                        .anatomically_compatible_influences
                        .contains(&w.bone)
            })
            .map(|w| w.weight)
            .sum::<f32>();
        if evidence.anatomical_confident
            && !evidence.pending
            && foreign > options.minimum_conflicting_weight
        {
            report.anatomical_conflict_count += 1;
            suspects.insert(v);
            if report.anatomical_conflicts.len() < options.maximum_examples {
                report.anatomical_conflicts.push(RigAnatomicalConflict {
                    evidence,
                    conflicting_weight: foreign,
                    cause: "ANATOMICAL_REGION_MISMATCH".into(),
                });
            }
        }
    }
    // Check real surface edges and welded UV seams; spatially close separate surfaces are not neighbors.
    let mut edges = BTreeSet::new();
    for t in &mesh.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let a = t[a] as usize;
            let b = t[b] as usize;
            edges.insert([a.min(b), a.max(b)]);
        }
    }
    let h = mesh.inspection.height;
    let mut weld = BTreeMap::<[i64; 3], usize>::new();
    for (i, p) in mesh.positions.iter().enumerate() {
        if let Some(previous) = weld.insert(p.map(|x| (x / h * 1e7).round() as i64), i) {
            edges.insert([previous, i]);
        }
    }
    for [a, b] in edges {
        let rest = length(sub(mesh.positions[a], mesh.positions[b]));
        if rest > h * options.maximum_adjacent_edge_height_fraction {
            continue;
        }
        let delta = difference(binding, a, b);
        if delta <= options.maximum_adjacent_weight_difference {
            continue;
        }
        let distance = bone_distance(binding, dominant(binding, a), dominant(binding, b));
        if distance < 3 {
            continue;
        }
        report.discontinuity_count += 1;
        suspects.extend([a, b]);
        if report.discontinuities.len() < options.maximum_examples {
            report.discontinuities.push(RigWeightDiscontinuity {
                vertices: [a, b],
                rest_length: rest,
                weight_difference: delta,
                dominant_bone_hierarchy_distance: distance,
                evidence: [context.evidence(a), context.evidence(b)],
                cause: "ADJACENT_INFLUENCE_DISCONTINUITY".into(),
            });
        }
    }
    // Check declared invariants independently from approximate anatomical families.
    let constraints = binding
        .binding_options
        .as_ref()
        .map(|o| o.constraints.as_slice())
        .unwrap_or_default();
    super::constraints::constraint_assignments(
        mesh.positions.len(),
        &binding.skeleton,
        constraints,
    )?;
    if let Some(input) = &binding.binding_options {
        let names = binding
            .skeleton
            .joints
            .iter()
            .enumerate()
            .map(|(i, j)| (j.id.as_str(), i as u16))
            .collect::<BTreeMap<_, _>>();
        for painted in &input.vertex_weights {
            let mut expected = BTreeMap::new();
            for influence in &painted.influences {
                let joint = *names
                    .get(influence.bone.as_str())
                    .ok_or_else(|| RigError::Missing(influence.bone.clone()))?;
                expected.insert(joint, influence.weight);
            }
            let mut actual = BTreeMap::new();
            for (&joint, &weight) in binding.joints[painted.vertex]
                .iter()
                .zip(&binding.weights[painted.vertex])
            {
                if weight > 0. {
                    *actual.entry(joint).or_insert(0f32) += weight;
                }
            }
            let joints = expected
                .keys()
                .chain(actual.keys())
                .copied()
                .collect::<BTreeSet<_>>();
            if let Some(joint) = joints.into_iter().max_by(|a, b| {
                (expected.get(a).copied().unwrap_or(0.) - actual.get(a).copied().unwrap_or(0.))
                    .abs()
                    .total_cmp(
                        &(expected.get(b).copied().unwrap_or(0.)
                            - actual.get(b).copied().unwrap_or(0.))
                        .abs(),
                    )
            }) {
                let target = expected.get(&joint).copied().unwrap_or(0.);
                let weight = actual.get(&joint).copied().unwrap_or(0.);
                if (target - weight).abs() > 1e-5 {
                    report.constraint_violation_count += 1;
                    suspects.insert(painted.vertex);
                    if report.constraint_violations.len() < options.maximum_examples {
                        report.constraint_violations.push(RigConstraintViolation {
                            constraint_id: format!("vertexWeights.{}", painted.vertex),
                            joint: binding.skeleton.joints[joint as usize].id.clone(),
                            evidence: context.evidence(painted.vertex),
                            expected_weight: target,
                            actual_weight: weight,
                            cause: "PAINTED_WEIGHT_MISMATCH".into(),
                        });
                    }
                }
            }
        }
    }
    for constraint in constraints {
        for &v in constraint.vertices() {
            let evidence = context.evidence(v);
            let actual = evidence
                .influences
                .iter()
                .filter(|w| w.bone == constraint.joint())
                .map(|w| w.weight)
                .sum::<f32>();
            let foreign = evidence
                .influences
                .iter()
                .filter(|w| w.bone != constraint.joint())
                .map(|w| w.weight)
                .sum::<f32>();
            if (actual - 1.).abs() > 1e-6 || foreign > 1e-6 || evidence.pending {
                report.constraint_violation_count += 1;
                suspects.insert(v);
                if report.constraint_violations.len() < options.maximum_examples {
                    report.constraint_violations.push(RigConstraintViolation {
                        constraint_id: constraint.id().into(),
                        joint: constraint.joint().into(),
                        evidence,
                        expected_weight: 1.,
                        actual_weight: actual,
                        cause: "RIGID_JOINT_WEIGHT_MISMATCH".into(),
                    });
                }
            }
        }
    }
    if report.constraint_violation_count > 0 {
        report.constraint_status = BindingStatus::Fail;
    }
    report.suspect_vertices = suspects.into_iter().collect();
    report.anatomy_status = if report.anatomical_conflict_count > 0 {
        BindingStatus::Fail
    } else if !report.pending_vertices.is_empty() {
        BindingStatus::Inconclusive
    } else {
        BindingStatus::Pass
    };
    if report.discontinuity_count > 0 {
        report.continuity_status = BindingStatus::Fail;
    }
    Ok(report)
}

/// Suggest confirmed body-family regions; ambiguous or distant vertices remain pending.
pub fn propose_rig_weight_regions(
    mesh: &RigMesh,
    skeleton: &HumanoidSkeleton,
    options: &RigWeightCheckOptions,
) -> Result<RigBindingOptions, RigError> {
    validate_options(options)?;
    let binding = bind_humanoid_skin(
        mesh,
        skeleton.clone(),
        &RigBindingOptions {
            smoothing_iterations: 0,
            ..Default::default()
        },
    )?;
    let context = WeightContext::new(mesh, &binding, options);
    let mut groups = BTreeMap::<Vec<String>, Vec<usize>>::new();
    let mut pending = vec![];
    for v in 0..mesh.positions.len() {
        let evidence = context.evidence(v);
        if evidence.anatomical_confident {
            groups
                .entry(evidence.suggested_influences)
                .or_default()
                .push(v);
        } else {
            pending.push(v);
        }
    }
    Ok(RigBindingOptions {
        regions: groups
            .into_iter()
            .map(|(influences, vertices)| RigWeightRegion {
                vertices,
                influences,
            })
            .collect(),
        pending_vertices: pending,
        ..Default::default()
    })
}

/// Diagnostics drive a new proposal; they never mutate or certify the failed binding.
pub fn suggest_rig_weight_refinement(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    report: &HumanoidBindingVerification,
    options: &RigWeightCheckOptions,
) -> Result<RigWeightRefinement, RigError> {
    validate_options(options)?;
    validate_binding(mesh, binding)?;
    if report.binding_fingerprint != binding.fingerprint {
        return Err(RigError::Invalid(
            "Weight refinement report belongs to another binding".into(),
        ));
    }
    let diagnostics = inspect_rig_weights(mesh, binding, options)?;
    let mut affected = diagnostics
        .suspect_vertices
        .into_iter()
        .collect::<BTreeSet<_>>();
    affected.extend(diagnostics.pending_vertices);
    for sample in &report.samples {
        for edge in &sample.worst_deformation_edges {
            affected.extend(edge.vertices.map(|v| v as usize));
        }
    }
    let context = WeightContext::new(mesh, binding, options);
    // A geometric family suggestion cannot revoke a reviewed rigid annotation.
    let constraints = binding
        .binding_options
        .as_ref()
        .map(|o| o.constraints.as_slice())
        .unwrap_or_default();
    let protected = super::constraints::constraint_assignments(
        mesh.positions.len(),
        &binding.skeleton,
        constraints,
    )?;
    let mut groups = BTreeMap::<Vec<String>, Vec<usize>>::new();
    let mut pending = vec![];
    for v in affected {
        if protected.contains_key(&v) {
            continue;
        }
        if v >= mesh.positions.len() {
            return Err(RigError::Invalid(
                "Refinement report contains an invalid vertex".into(),
            ));
        }
        let evidence = context.evidence(v);
        if evidence.anatomical_confident {
            groups
                .entry(evidence.suggested_influences)
                .or_default()
                .push(v);
        } else {
            pending.push(v);
        }
    }
    Ok(RigWeightRefinement {
        expected_binding_fingerprint: binding.fingerprint.clone(),
        constraints: vec![],
        surface_regions: vec![],
        vertex_weights: vec![],
        regions: groups
            .into_iter()
            .map(|(influences, vertices)| RigWeightRegion {
                vertices,
                influences,
            })
            .collect(),
        pending_vertices: pending,
    })
}

pub(crate) fn refined_binding_options(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    refinement: &RigWeightRefinement,
) -> Result<RigBindingOptions, RigError> {
    if refinement.expected_binding_fingerprint != binding.fingerprint {
        return Err(RigError::Invalid(
            "Stale weight refinement fingerprint".into(),
        ));
    }
    let mut replaced = BTreeSet::new();
    for v in refinement
        .regions
        .iter()
        .flat_map(|r| &r.vertices)
        .chain(&refinement.pending_vertices)
        .chain(refinement.constraints.iter().flat_map(|c| c.vertices()))
    {
        if *v >= mesh.positions.len() || !replaced.insert(*v) {
            return Err(RigError::Invalid(
                "Overlapping or invalid refinement vertex".into(),
            ));
        }
    }
    let mut options = binding.binding_options.clone().unwrap_or_default();
    let mut annotated = BTreeSet::new();
    for surface in &refinement.surface_regions {
        for &v in &surface.vertices {
            if v >= mesh.positions.len()
                || !annotated.insert(v)
                || refinement.pending_vertices.contains(&v)
            {
                return Err(RigError::Invalid(
                    "Invalid, overlapping or pending surface refinement".into(),
                ));
            }
        }
    }
    replaced.extend(annotated);
    let region_replaced = replaced.clone();
    let mut painted = BTreeSet::new();
    for input in &refinement.vertex_weights {
        if input.vertex >= mesh.positions.len()
            || !painted.insert(input.vertex)
            || refinement.pending_vertices.contains(&input.vertex)
        {
            return Err(RigError::Invalid(
                "Invalid, duplicate or pending weight painting".into(),
            ));
        }
    }
    replaced.extend(painted);
    options
        .vertex_weights
        .retain(|w| !replaced.contains(&w.vertex));
    options
        .vertex_weights
        .extend(refinement.vertex_weights.clone());
    // Explicit uncertainty revokes the old annotation; an ordinary weight edit keeps reviewed intent.
    for surface in &mut options.surface_regions {
        surface
            .vertices
            .retain(|v| !refinement.pending_vertices.contains(v));
    }
    options.surface_regions.retain(|s| !s.vertices.is_empty());
    for surface in &refinement.surface_regions {
        options.surface_regions.retain(|old| old.id != surface.id);
    }
    options
        .surface_regions
        .extend(refinement.surface_regions.clone());
    // Updating an ID is explicit; unrelated rigid annotations cannot be erased by region refinement.
    for constraint in &refinement.constraints {
        options
            .constraints
            .retain(|old| old.id() != constraint.id());
    }
    options.constraints.extend(refinement.constraints.clone());
    super::constraints::constraint_assignments(
        mesh.positions.len(),
        &binding.skeleton,
        &options.constraints,
    )?;
    for region in &mut options.regions {
        region.vertices.retain(|v| !region_replaced.contains(v));
    }
    options.regions.retain(|r| !r.vertices.is_empty());
    options.regions.extend(refinement.regions.clone());
    options.pending_vertices.retain(|v| !replaced.contains(v));
    options
        .pending_vertices
        .extend(&refinement.pending_vertices);
    Ok(options)
}

pub(crate) fn explain_deformation_edge(
    context: &WeightContext<'_>,
    vertices: [u32; 2],
) -> ([RigVertexWeightEvidence; 2], Vec<String>) {
    let evidence = vertices.map(|v| context.evidence(v as usize));
    let binding = context.binding;
    let [a, b] = vertices.map(|v| v as usize);
    let mut causes = vec![];
    for e in &evidence {
        if e.pending {
            causes.push("UNRESOLVED_WEIGHT_VERTEX".into());
        }
        if e.anatomical_confident
            && e.influences
                .iter()
                .filter(|w| !e.anatomically_compatible_influences.contains(&w.bone))
                .map(|w| w.weight)
                .sum::<f32>()
                > context.options.minimum_conflicting_weight
        {
            causes.push("ANATOMICAL_REGION_MISMATCH".into());
        }
    }
    if difference(binding, a, b) > context.options.maximum_adjacent_weight_difference
        && bone_distance(binding, dominant(binding, a), dominant(binding, b)) >= 3
    {
        causes.push("ADJACENT_INFLUENCE_DISCONTINUITY".into());
    }
    if causes.is_empty() {
        causes.push("REVIEW_PIVOTS_AXES_AND_LOCAL_WEIGHTS".into());
    }
    causes.sort();
    causes.dedup();
    (evidence, causes)
}
