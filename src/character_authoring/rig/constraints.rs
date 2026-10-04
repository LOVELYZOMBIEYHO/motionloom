// =========================================
// =========================================
// src/character_authoring/rig/constraints.rs

//! Persistent skin constraints and reviewed mesh-only head annotations.
use super::{math::*, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RigWeightConstraint {
    RigidJoint {
        id: String,
        vertices: Vec<usize>,
        joint: String,
    },
}
impl RigWeightConstraint {
    pub fn id(&self) -> &str {
        match self {
            Self::RigidJoint { id, .. } => id,
        }
    }
    pub fn vertices(&self) -> &[usize] {
        match self {
            Self::RigidJoint { vertices, .. } => vertices,
        }
    }
    pub fn joint(&self) -> &str {
        match self {
            Self::RigidJoint { joint, .. } => joint,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RigHeadRegionKind {
    RigidHead,
    NeckTransition,
}

/// A caller-reviewed annotation may select a subset of a shared body primitive.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigHeadRegion {
    pub id: String,
    pub vertices: Vec<usize>,
    pub kind: RigHeadRegionKind,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigHeadConstraintOptions {
    pub reviewed_regions: Vec<RigHeadRegion>,
    /// Limit geometric suggestions to explicit vertices, e.g. body surfaces excluding a backpack.
    pub search_vertices: Option<Vec<usize>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigHeadRegionEvidence {
    pub id: String,
    pub vertices: Vec<usize>,
    pub kind: RigHeadRegionKind,
    pub reviewed: bool,
    pub confidence: f32,
    pub bounds_min: V3,
    pub bounds_max: V3,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigHeadConstraintProposal {
    pub geometry_fingerprint: String,
    pub refinement: RigWeightRefinement,
    pub evidence: Vec<RigHeadRegionEvidence>,
    /// These geometric suggestions are not applied until the caller reviews their anatomy.
    pub suggested_constraints: Vec<RigWeightConstraint>,
    pub requires_review: bool,
    pub limitations: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigConstraintViolation {
    pub constraint_id: String,
    pub joint: String,
    pub evidence: RigVertexWeightEvidence,
    pub expected_weight: f32,
    pub actual_weight: f32,
    pub cause: String,
}

// Resolve canonical IDs once and reject contradictory annotations rather than choosing a winner.
pub(crate) fn constraint_assignments(
    vertex_count: usize,
    skeleton: &HumanoidSkeleton,
    constraints: &[RigWeightConstraint],
) -> Result<BTreeMap<usize, usize>, RigError> {
    let mut ids = BTreeSet::new();
    let mut assignments = BTreeMap::new();
    for constraint in constraints {
        if constraint.id().trim().is_empty()
            || !ids.insert(constraint.id())
            || constraint.vertices().is_empty()
        {
            return Err(RigError::Invalid(
                "Constraints need unique nonempty IDs and selected vertices".into(),
            ));
        }
        let joint = skeleton
            .joints
            .iter()
            .position(|j| j.id == constraint.joint())
            .ok_or_else(|| RigError::Missing(constraint.joint().into()))?;
        if skeleton.joints[joint].endpoint || skeleton.joints[joint].id == "root" {
            return Err(RigError::Invalid(
                "Rigid constraints cannot use root or endpoint joints".into(),
            ));
        }
        for &vertex in constraint.vertices() {
            if vertex >= vertex_count || assignments.insert(vertex, joint).is_some() {
                return Err(RigError::Invalid(
                    "Invalid or overlapping constrained vertex".into(),
                ));
            }
        }
    }
    Ok(assignments)
}

fn region_evidence(
    mesh: &RigMesh,
    region: &RigHeadRegion,
    reviewed: bool,
    reason: &str,
) -> RigHeadRegionEvidence {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for &v in &region.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(mesh.positions[v][k]);
            hi[k] = hi[k].max(mesh.positions[v][k]);
        }
    }
    RigHeadRegionEvidence {
        id: region.id.clone(),
        vertices: region.vertices.clone(),
        kind: region.kind,
        reviewed,
        confidence: if reviewed { 1. } else { 0.5 },
        bounds_min: lo,
        bounds_max: hi,
        reason: reason.into(),
    }
}

/// Propose annotations using geometry and fitted landmarks; source skins and node names are ignored.
pub fn propose_rig_head_constraints(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    options: &RigHeadConstraintOptions,
) -> Result<RigHeadConstraintProposal, RigError> {
    super::weights::validate_binding(mesh, binding)?;
    let head = binding
        .skeleton
        .joints
        .iter()
        .find(|j| j.id == "head")
        .ok_or_else(|| RigError::Missing("head".into()))?;
    let neck = binding
        .skeleton
        .joints
        .iter()
        .find(|j| j.id == "neck")
        .ok_or_else(|| RigError::Missing("neck".into()))?;
    let mut reviewed = BTreeSet::new();
    let mut region_ids = BTreeSet::new();
    let mut refinement = RigWeightRefinement {
        expected_binding_fingerprint: binding.fingerprint.clone(),
        regions: vec![],
        pending_vertices: vec![],
        constraints: vec![],
        surface_regions: vec![],
        vertex_weights: vec![],
    };
    let mut evidence = vec![];
    for region in &options.reviewed_regions {
        if region.id.trim().is_empty()
            || !region_ids.insert(&region.id)
            || region.vertices.is_empty()
        {
            return Err(RigError::Invalid(
                "Head regions need unique IDs and nonempty vertices".into(),
            ));
        }
        for &v in &region.vertices {
            if v >= mesh.positions.len() || !reviewed.insert(v) {
                return Err(RigError::Invalid(
                    "Invalid or overlapping reviewed head region".into(),
                ));
            }
        }
        match region.kind {
            RigHeadRegionKind::RigidHead => {
                refinement
                    .constraints
                    .push(RigWeightConstraint::RigidJoint {
                        id: region.id.clone(),
                        vertices: region.vertices.clone(),
                        joint: "head".into(),
                    })
            }
            RigHeadRegionKind::NeckTransition => refinement.regions.push(RigWeightRegion {
                vertices: region.vertices.clone(),
                influences: vec!["neck".into(), "head".into()],
            }),
        }
        evidence.push(region_evidence(mesh, region, true, "Caller-reviewed mesh anatomy; confidence records annotation provenance, not an automatic anatomical certificate."));
    }
    let search = options
        .search_vertices
        .clone()
        .unwrap_or_else(|| (0..mesh.positions.len()).collect());
    let existing = binding
        .binding_options
        .as_ref()
        .map(|o| constraint_assignments(mesh.positions.len(), &binding.skeleton, &o.constraints))
        .transpose()?
        .unwrap_or_default();
    let mut searched = BTreeSet::new();
    let h = mesh.inspection.height;
    let direction = sub(head.position, neck.position);
    if length(direction) < h * 1e-5 {
        return Err(RigError::Invalid(
            "Head and neck landmarks must be distinct".into(),
        ));
    }
    let axis = unit(direction);
    let mut suggested = vec![];
    for v in search {
        if v >= mesh.positions.len() || !searched.insert(v) {
            return Err(RigError::Invalid(
                "Invalid or duplicate head search vertex".into(),
            ));
        }
        let p = mesh.positions[v];
        // A fitted head envelope only locates candidates; touching clothes and hair still need review.
        if !reviewed.contains(&v)
            && !existing.contains_key(&v)
            && dot(sub(p, neck.position), axis) >= 0.
            && distance_segment(p, neck.position, head.position) <= h * 0.16
        {
            suggested.push(v);
        }
    }
    let mut suggestions = vec![];
    if !suggested.is_empty() {
        let region = RigHeadRegion {
            id: "head.unreviewed".into(),
            vertices: suggested.clone(),
            kind: RigHeadRegionKind::RigidHead,
        };
        evidence.push(region_evidence(mesh, &region, false, "Inside the fitted head envelope. Geometry alone cannot distinguish facial skin, hair, collars or accessories; review before constraining."));
        suggestions.push(RigWeightConstraint::RigidJoint {
            id: region.id,
            vertices: suggested.clone(),
            joint: "head".into(),
        });
        refinement.pending_vertices = suggested;
    }
    Ok(RigHeadConstraintProposal { geometry_fingerprint: mesh.inspection.geometry_fingerprint.clone(),
        requires_review: !refinement.pending_vertices.is_empty(), refinement, evidence, suggested_constraints: suggestions,
        limitations: vec!["Head constraints certify only selected vertices. Review face, skull, eyes, teeth and attachments, including shared body primitives; selection completeness is not inferred from mesh names or source rigs.".into(),
            "Rigid head constraints are for characters without facial articulation. Exclude articulated jaws, long hair dynamics and the neck transition from rigid face selections.".into()] })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigHeadCheckOptions {
    /// Legacy callers may omit head checks; explicit head constraints always activate their gate.
    pub required: bool,
    pub maximum_rigid_deviation_height_fraction: f32,
    pub maximum_edge_relative_error: f32,
}
impl Default for RigHeadCheckOptions {
    fn default() -> Self {
        Self {
            required: false,
            maximum_rigid_deviation_height_fraction: 1e-5,
            maximum_edge_relative_error: 0.005,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigHeadDeformationEvidence {
    pub constraint_id: String,
    pub vertex: usize,
    pub rest_position: V3,
    pub expected_position: V3,
    pub actual_position: V3,
    pub deviation: f32,
    pub influences: Vec<RigNamedInfluence>,
    pub cause: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigHeadShapeFailure {
    pub vertices: [usize; 2],
    pub constraint_ids: [String; 2],
    pub rest_length: f32,
    pub posed_length: f32,
    pub relative_error: f32,
    pub cause: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigHeadRigiditySample {
    pub status: BindingStatus,
    pub constrained_vertices: usize,
    pub checked_edges: usize,
    pub head_rotation_relative_to_parent_degrees: f32,
    pub maximum_rigid_deviation: f32,
    pub maximum_edge_relative_error: f32,
    pub failure_count: usize,
    pub failures: Vec<RigHeadDeformationEvidence>,
    pub shape_failures: Vec<RigHeadShapeFailure>,
}

// Compare production LBS positions to the same head transform for every facial component.
pub(crate) fn head_rigidity_sample(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    posed: &[V3],
    current: &BTreeMap<String, M4>,
    neutral: &BTreeMap<String, M4>,
    options: &RigHeadCheckOptions,
    maximum_examples: usize,
    edges: &[[usize; 2]],
) -> Result<RigHeadRigiditySample, RigError> {
    let head = binding
        .skeleton
        .joints
        .iter()
        .find(|j| j.id == "head")
        .ok_or_else(|| RigError::Missing("head".into()))?;
    let constraints = binding
        .binding_options
        .as_ref()
        .map(|o| o.constraints.as_slice())
        .unwrap_or_default();
    let mut selected = BTreeMap::new();
    for constraint in constraints.iter().filter(|c| c.joint() == "head") {
        for &v in constraint.vertices() {
            selected.insert(v, constraint.id());
        }
    }
    let mut sample = RigHeadRigiditySample {
        status: BindingStatus::Inconclusive,
        constrained_vertices: selected.len(),
        checked_edges: 0,
        head_rotation_relative_to_parent_degrees: 0.,
        maximum_rigid_deviation: 0.,
        maximum_edge_relative_error: 0.,
        failure_count: 0,
        failures: vec![],
        shape_failures: vec![],
    };
    if selected.is_empty() {
        return Ok(sample);
    }
    let parent = head.parent.as_deref().unwrap_or("neck");
    let relative = qmul(conjugate(quat(current[parent])), quat(current["head"]));
    let rest = qmul(conjugate(quat(neutral[parent])), quat(neutral["head"]));
    sample.head_rotation_relative_to_parent_degrees =
        crate::api::quaternion_angular_error_deg(relative, rest);
    let transform = mul(current["head"], head.inverse_bind_matrix);
    for (&v, &id) in &selected {
        let expected = point(transform, mesh.positions[v]);
        let deviation = length(sub(posed[v], expected));
        sample.maximum_rigid_deviation = sample.maximum_rigid_deviation.max(deviation);
        if deviation > mesh.inspection.height * options.maximum_rigid_deviation_height_fraction {
            sample.failure_count += 1;
            if sample.failures.len() < maximum_examples {
                let influences = binding.joints[v]
                    .iter()
                    .zip(binding.weights[v])
                    .filter(|(_, w)| *w > 0.)
                    .map(|(i, weight)| RigNamedInfluence {
                        bone: binding.skeleton.joints[*i as usize].id.clone(),
                        weight,
                    })
                    .collect();
                sample.failures.push(RigHeadDeformationEvidence {
                    constraint_id: id.into(),
                    vertex: v,
                    rest_position: mesh.positions[v],
                    expected_position: expected,
                    actual_position: posed[v],
                    deviation,
                    influences,
                    cause: "HEAD_RIGID_TRANSFORM_MISMATCH".into(),
                });
            }
        }
    }
    for &[a, b] in edges {
        let rest = length(sub(mesh.positions[a], mesh.positions[b]));
        if rest < mesh.inspection.height * 1e-4 {
            continue;
        }
        sample.checked_edges += 1;
        let posed_length = length(sub(posed[a], posed[b]));
        let relative_error = (posed_length / rest - 1.).abs();
        sample.maximum_edge_relative_error = sample.maximum_edge_relative_error.max(relative_error);
        if relative_error > options.maximum_edge_relative_error {
            sample.failure_count += 1;
            if sample.shape_failures.len() < maximum_examples {
                sample.shape_failures.push(RigHeadShapeFailure {
                    vertices: [a, b],
                    constraint_ids: [selected[&a].into(), selected[&b].into()],
                    rest_length: rest,
                    posed_length,
                    relative_error,
                    cause: "HEAD_SURFACE_SHAPE_CHANGED".into(),
                });
            }
        }
    }
    sample.status = if sample.failure_count > 0
        || sample.maximum_edge_relative_error > options.maximum_edge_relative_error
    {
        BindingStatus::Fail
    } else if selected.len() < 3 || sample.checked_edges < 3 {
        BindingStatus::Inconclusive
    } else {
        BindingStatus::Pass
    };
    Ok(sample)
}

// Surface connectivity is immutable across the whole action suite; compute it once.
pub(crate) fn rigid_head_edges(mesh: &RigMesh, binding: &HumanoidBinding) -> Vec<[usize; 2]> {
    let mut selected = vec![false; mesh.positions.len()];
    if let Some(options) = &binding.binding_options {
        for constraint in options.constraints.iter().filter(|c| c.joint() == "head") {
            for &v in constraint.vertices() {
                selected[v] = true;
            }
        }
    }
    let mut edges = BTreeSet::new();
    for t in &mesh.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (a, b) = (t[a] as usize, t[b] as usize);
            if selected[a] && selected[b] {
                edges.insert([a.min(b), a.max(b)]);
            }
        }
    }
    edges.into_iter().collect()
}
