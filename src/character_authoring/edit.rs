// =========================================
// =========================================
// src/character_authoring/edit.rs

use super::{CharacterDocument, CharacterError, CharacterSnapshot, cages};
use crate::api::mesh_reference::{
    MESH_REFERENCE_SCHEMA_VERSION, MeshAssetProposal, MeshProposalValidationOptions,
    MeshVertexChange, apply_mesh_asset_proposal, mesh_source_fingerprint, mesh_topology_signature,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    pub expected_revision: u64,
    pub operations: Vec<EditOperation>,
    #[serde(default)]
    pub locks: Vec<String>,
    #[serde(default)]
    pub paired: bool,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EditOperation {
    Scale {
        target: String,
        factors: [f32; 3],
    },
    Translate {
        target: String,
        delta: [f32; 3],
    },
    Depth {
        target: String,
        factor: f32,
        center_z: f32,
        falloff: f32,
    },
}
impl EditOperation {
    fn target(&self) -> &str {
        match self {
            Self::Scale { target, .. }
            | Self::Translate { target, .. }
            | Self::Depth { target, .. } => target,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditCandidate {
    pub id: String,
    pub base_revision: u64,
    pub snapshot: CharacterSnapshot,
    pub changed_assets: Vec<String>,
    pub locked_assets: Vec<String>,
    pub reason: String,
    pub validation_policy: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VertexBinding {
    pub triangle: [usize; 3],
    pub weights: [f32; 3],
    /// Offset in the surface tangent/bitangent/normal frame.
    pub rest_offset: [f32; 3],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub parent_asset: String,
    pub child_asset: String,
    pub parent_topology: String,
    pub child_topology: String,
    pub bindings: Vec<VertexBinding>,
    pub minimum_clearance: Option<f32>,
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn mul(a: [f32; 3], v: f32) -> [f32; 3] {
    a.map(|x| x * v)
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f32; 3]) -> Result<[f32; 3], CharacterError> {
    let l = dot(a, a).sqrt();
    if l < 1e-8 {
        Err(CharacterError::Geometry(
            "Degenerate attachment frame".into(),
        ))
    } else {
        Ok(mul(a, 1.0 / l))
    }
}
fn frame(points: &[[f32; 3]], ids: [usize; 3]) -> Result<[[f32; 3]; 3], CharacterError> {
    let a = points[ids[0]];
    let t = unit(sub(points[ids[1]], a))?;
    let n = unit(cross(t, sub(points[ids[2]], a)))?;
    Ok([t, cross(n, t), n])
}
fn anchor(points: &[[f32; 3]], binding: &VertexBinding) -> [f32; 3] {
    (0..3).fold([0.; 3], |p, i| {
        add(p, mul(points[binding.triangle[i]], binding.weights[i]))
    })
}

/// Closest-point barycentrics include all edge and corner regions of a triangle.
fn closest_weights(point: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(point, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0. && d2 <= 0. {
        return [1., 0., 0.];
    }
    let bp = sub(point, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0. && d4 <= d3 {
        return [0., 1., 0.];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0. && d1 >= 0. && d3 <= 0. {
        let v = d1 / (d1 - d3);
        return [1. - v, v, 0.];
    }
    let cp = sub(point, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0. && d5 <= d6 {
        return [0., 0., 1.];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0. && d2 >= 0. && d6 <= 0. {
        let v = d2 / (d2 - d6);
        return [1. - v, 0., v];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0. && d4 - d3 >= 0. && d5 - d6 >= 0. {
        let v = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0., 1. - v, v];
    }
    let inverse = 1. / (va + vb + vc);
    [1. - vb * inverse - vc * inverse, vb * inverse, vc * inverse]
}

/// Capture explicit control-surface coordinates; bindings never select new faces silently.
pub fn bind_attachment(
    snapshot: &mut CharacterSnapshot,
    parent: &str,
    child: &str,
    minimum_clearance: Option<f32>,
) -> Result<(), CharacterError> {
    if minimum_clearance.is_some_and(|v| !v.is_finite() || v < 0.) {
        return Err(CharacterError::Invalid(
            "Minimum clearance must be finite and nonnegative".into(),
        ));
    }
    if parent == child || snapshot.attachments.iter().any(|a| a.child_asset == child) {
        return Err(CharacterError::Invalid(
            "Attachment already bound or self-referential".into(),
        ));
    }
    let all = cages(&snapshot.source)?;
    let p = all
        .get(parent)
        .ok_or_else(|| CharacterError::Missing(parent.into()))?;
    let c = all
        .get(child)
        .ok_or_else(|| CharacterError::Missing(child.into()))?;
    let mut ancestor = parent;
    while let Some(a) = snapshot
        .attachments
        .iter()
        .find(|a| a.child_asset == ancestor)
    {
        if a.parent_asset == child {
            return Err(CharacterError::Invalid("Attachment cycle".into()));
        }
        ancestor = &a.parent_asset;
    }
    let triangles: Vec<[usize; 3]> = p
        .faces
        .iter()
        .flat_map(|f| (1..f.len() - 1).map(|i| [f[0] as usize, f[i] as usize, f[i + 1] as usize]))
        .collect();
    let mut bindings = vec![];
    for &point in &c.positions {
        let mut best: Option<(f32, VertexBinding)> = None;
        for &triangle in &triangles {
            let a = p.positions[triangle[0]];
            let u = sub(p.positions[triangle[1]], a);
            let v = sub(p.positions[triangle[2]], a);
            let det = dot(u, u) * dot(v, v) - dot(u, v).powi(2);
            if det.abs() < 1e-12 {
                continue;
            }
            let mut weights =
                closest_weights(point, a, p.positions[triangle[1]], p.positions[triangle[2]])
                    .map(|w| w.max(0.));
            let sum = weights.iter().sum::<f32>();
            weights = weights.map(|w| w / sum);
            let mut binding = VertexBinding {
                triangle,
                weights,
                rest_offset: [0.; 3],
            };
            let delta = sub(point, anchor(&p.positions, &binding));
            let distance = dot(delta, delta);
            if best.as_ref().is_none_or(|x| distance < x.0) {
                let basis = frame(&p.positions, triangle)?;
                binding.rest_offset = basis.map(|axis| dot(delta, axis));
                best = Some((distance, binding));
            }
        }
        let binding = best
            .ok_or_else(|| CharacterError::Geometry("No attachment surface".into()))?
            .1;
        if minimum_clearance.is_some_and(|min| binding.rest_offset[2] < min - 1e-5) {
            return Err(CharacterError::Geometry(format!(
                "{child} violates clearance from {parent}"
            )));
        }
        bindings.push(binding);
    }
    snapshot.attachments.push(Attachment {
        parent_asset: parent.into(),
        child_asset: child.into(),
        parent_topology: mesh_topology_signature(p),
        child_topology: mesh_topology_signature(c),
        bindings,
        minimum_clearance,
    });
    Ok(())
}

fn extent(points: &[[f32; 3]]) -> f32 {
    (0..3)
        .map(|axis| {
            points
                .iter()
                .map(|p| p[axis])
                .fold(f32::NEG_INFINITY, f32::max)
                - points.iter().map(|p| p[axis]).fold(f32::INFINITY, f32::min)
        })
        .fold(0., f32::max)
        .max(1e-6)
}

fn apply_positions(
    source: &str,
    id: &str,
    after: &[[f32; 3]],
    movement_budget: f32,
) -> Result<String, CharacterError> {
    let all = cages(source)?;
    let cage = &all[id];
    let changes: Vec<_> = cage
        .positions
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(vertex, (&before, &after))| MeshVertexChange {
            vertex,
            before,
            after,
            confidence: 1.,
            evidence_views: vec![],
        })
        .collect();
    if changes.is_empty() {
        return Ok(source.into());
    }
    let proposal = MeshAssetProposal {
        schema_version: MESH_REFERENCE_SCHEMA_VERSION.into(),
        source_fingerprint: mesh_source_fingerprint(source),
        topology_signature: mesh_topology_signature(cage),
        target_asset_id: id.into(),
        camera_fingerprint: None,
        reference_set_fingerprint: None,
        metric_profile_fingerprint: None,
        evaluation_fingerprint: None,
        reason: "Explicit character design edit".into(),
        changes,
        validation: MeshProposalValidationOptions {
            allow_boundary: true,
            allow_multiple_components: true,
            // A small attached prop may travel farther than its own size with its larger parent.
            max_move_relative_to_bounds: movement_budget / extent(&cage.positions),
            max_changed_vertices: 30000,
            max_edge_length_ratio: 3.,
            max_laplacian_delta_relative_to_bounds: 0.25,
        },
    };
    Ok(apply_mesh_asset_proposal(source, &proposal)
        .map_err(|error| CharacterError::AssetEdit {
            asset_id: id.into(),
            error,
        })?
        .source)
}

impl CharacterDocument {
    /// Regenerate only documents whose parameters still match their authoritative source.
    pub fn propose_parameters(
        &mut self,
        expected: u64,
        parameters: super::CharacterParameters,
        locks: Vec<String>,
    ) -> Result<EditCandidate, CharacterError> {
        self.check_revision(expected)?;
        if self.current.parameters.is_none() {
            return Err(CharacterError::Invalid(
                "Imported or freely edited meshes have no reversible template parameters".into(),
            ));
        }
        if !self.current.attachments.is_empty() {
            return Err(CharacterError::Invalid("Use local cage editing for attached templates; parameter regeneration requires rebinding".into()));
        }
        let next = super::build_template(self.id.clone(), parameters)?;
        let before = cages(&self.current.source)?;
        let after = cages(&next.current.source)?;
        let mut locked = BTreeSet::new();
        for target in locks {
            for selection in self
                .current
                .selections
                .get(&target)
                .ok_or_else(|| CharacterError::Missing(target.clone()))?
            {
                locked.insert(selection.asset_id.clone());
            }
        }
        for id in &locked {
            if serde_json::to_value(&before[id])? != serde_json::to_value(&after[id])? {
                return Err(CharacterError::Locked(id.clone()));
            }
        }
        let changed_assets = before
            .iter()
            .filter(|(id, c)| {
                serde_json::to_value(c).ok() != serde_json::to_value(&after[*id]).ok()
            })
            .map(|(id, _)| id.clone())
            .collect();
        let candidate = EditCandidate {
            id: format!("candidate-{}", self.next_candidate),
            base_revision: self.revision,
            snapshot: next.current,
            changed_assets,
            locked_assets: locked.into_iter().collect(),
            reason: "Update template parameters".into(),
            validation_policy: "explicitDesignEdit: regenerated template, geometry and locks"
                .into(),
        };
        self.next_candidate += 1;
        self.candidates
            .insert(candidate.id.clone(), candidate.clone());
        Ok(candidate)
    }

    /// Build one candidate across all assets, preserving the accepted snapshot on failure.
    pub fn propose(&mut self, request: EditRequest) -> Result<EditCandidate, CharacterError> {
        self.check_revision(request.expected_revision)?;
        if request.operations.is_empty() || request.reason.trim().is_empty() {
            return Err(CharacterError::Invalid(
                "Operations and reason are required".into(),
            ));
        }
        let original = cages(&self.current.source)?;
        let mut positions: BTreeMap<_, _> = original
            .iter()
            .map(|(id, c)| (id.clone(), c.positions.clone()))
            .collect();
        let mut locked = BTreeSet::new();
        for name in &request.locks {
            for item in self
                .current
                .selections
                .get(name)
                .ok_or_else(|| CharacterError::Missing(name.clone()))?
            {
                locked.insert(item.asset_id.clone());
            }
        }
        for op in &request.operations {
            let mut selection = self
                .current
                .selections
                .get(op.target())
                .cloned()
                .ok_or_else(|| CharacterError::Missing(op.target().into()))?;
            let original_assets: BTreeSet<_> =
                selection.iter().map(|s| s.asset_id.clone()).collect();
            if request.paired {
                for item in selection.clone() {
                    if let Some(pair) = item.pair {
                        selection.extend(
                            self.current
                                .selections
                                .get(&pair)
                                .cloned()
                                .ok_or_else(|| CharacterError::Missing(pair))?,
                        );
                    }
                }
            }
            let mut visited = BTreeSet::new();
            for item in selection {
                let mirrored = !original_assets.contains(&item.asset_id);
                if locked.contains(&item.asset_id) {
                    return Err(CharacterError::Locked(item.asset_id));
                }
                let cage = &original[&item.asset_id];
                if item.topology_signature != mesh_topology_signature(cage) {
                    return Err(CharacterError::Rebind(op.target().into()));
                }
                if item.vertices.is_empty() {
                    return Err(CharacterError::Invalid("Selection has no vertices".into()));
                }
                if item.vertices.iter().any(|i| *i >= cage.positions.len()) {
                    return Err(CharacterError::Rebind(op.target().into()));
                }
                let center: [f32; 3] = std::array::from_fn(|axis| {
                    item.vertices
                        .iter()
                        .map(|i| cage.positions[*i][axis])
                        .sum::<f32>()
                        / item.vertices.len() as f32
                });
                let lo = item
                    .vertices
                    .iter()
                    .map(|i| cage.positions[*i][1])
                    .fold(f32::INFINITY, f32::min);
                let hi = item
                    .vertices
                    .iter()
                    .map(|i| cage.positions[*i][1])
                    .fold(f32::NEG_INFINITY, f32::max);
                for i in item.vertices {
                    if !visited.insert((item.asset_id.clone(), i)) {
                        continue;
                    }
                    let p = &mut positions.get_mut(&item.asset_id).unwrap()[i];
                    match op {
                        EditOperation::Scale { factors, .. } => {
                            if factors
                                .iter()
                                .any(|v| !v.is_finite() || *v <= 0. || *v > 3.)
                            {
                                return Err(CharacterError::Invalid(
                                    "Scale factors must be finite and in (0,3]".into(),
                                ));
                            }
                            *p = std::array::from_fn(|a| {
                                center[a] + (p[a] - center[a]) * factors[a]
                            });
                        }
                        EditOperation::Translate { delta, .. } => {
                            if delta.iter().any(|x| !x.is_finite()) {
                                return Err(CharacterError::Invalid(
                                    "Translation must be finite".into(),
                                ));
                            }
                            let mut delta = *delta;
                            if mirrored {
                                delta[0] = -delta[0];
                            }
                            *p = add(*p, delta);
                        }
                        EditOperation::Depth {
                            factor,
                            center_z,
                            falloff,
                            ..
                        } => {
                            if !factor.is_finite()
                                || *factor <= 0.
                                || *factor > 3.
                                || !center_z.is_finite()
                                || !falloff.is_finite()
                                || *falloff < 0.
                            {
                                return Err(CharacterError::Invalid(
                                    "Invalid depth controls".into(),
                                ));
                            }
                            let weight = if *falloff == 0. {
                                1.
                            } else {
                                ((p[1] - lo).min(hi - p[1]) / falloff).clamp(0., 1.)
                            };
                            let weight = weight * weight * (3. - 2. * weight);
                            p[2] = center_z + (p[2] - center_z) * (1. + (factor - 1.) * weight);
                        }
                    }
                }
            }
        }
        let mut changed: BTreeSet<_> = positions
            .iter()
            .filter(|(id, p)| original[*id].positions != **p)
            .map(|(id, _)| id.clone())
            .collect();
        let mut movement_budgets: BTreeMap<_, _> = original
            .iter()
            .map(|(id, c)| (id.clone(), extent(&c.positions) * 0.25))
            .collect();
        // Direct edits retain their own movement budget before attachment propagation adds displacement.
        for id in &changed {
            if positions[id]
                .iter()
                .zip(&original[id].positions)
                .any(|(a, b)| dot(sub(*a, *b), sub(*a, *b)).sqrt() > movement_budgets[id])
            {
                return Err(CharacterError::Geometry(format!(
                    "Direct edit exceeds movement budget for {id}"
                )));
            }
        }
        let mut pending = self.current.attachments.clone();
        // Resolve parents before children and reject both cycles and stale surface bindings.
        while !pending.is_empty() {
            let index = pending
                .iter()
                .position(|a| !pending.iter().any(|p| p.child_asset == a.parent_asset))
                .ok_or_else(|| CharacterError::Invalid("Attachment cycle".into()))?;
            let a = pending.remove(index);
            let parent = original
                .get(&a.parent_asset)
                .ok_or_else(|| CharacterError::Rebind(a.child_asset.clone()))?;
            let child = original
                .get(&a.child_asset)
                .ok_or_else(|| CharacterError::Rebind(a.child_asset.clone()))?;
            if mesh_topology_signature(parent) != a.parent_topology
                || mesh_topology_signature(child) != a.child_topology
                || a.bindings.len() != child.positions.len()
            {
                return Err(CharacterError::Rebind(a.child_asset));
            }
            if changed.contains(&a.parent_asset) {
                if locked.contains(&a.child_asset) {
                    return Err(CharacterError::Locked(a.child_asset));
                }
                let p = &positions[&a.parent_asset];
                let mut result = vec![];
                for (vertex, binding) in a.bindings.iter().enumerate() {
                    if binding.triangle.iter().any(|i| *i >= p.len()) {
                        return Err(CharacterError::Rebind(a.child_asset.clone()));
                    }
                    let basis = frame(p, binding.triangle)?;
                    let old_basis = frame(&parent.positions, binding.triangle)?;
                    // Apply displacement, preserving unchanged vertices exactly and retaining child edits.
                    let displacement = (0..3).fold(
                        sub(anchor(p, binding), anchor(&parent.positions, binding)),
                        |v, i| add(v, mul(sub(basis[i], old_basis[i]), binding.rest_offset[i])),
                    );
                    let point = add(positions[&a.child_asset][vertex], displacement);
                    if a.minimum_clearance
                        .is_some_and(|min| binding.rest_offset[2] < min - 1e-5)
                    {
                        return Err(CharacterError::Geometry(
                            "Attachment clearance failed".into(),
                        ));
                    }
                    result.push(point);
                }
                if result != child.positions {
                    changed.insert(a.child_asset.clone());
                }
                let budget = movement_budgets[&a.parent_asset];
                movement_budgets.insert(
                    a.child_asset.clone(),
                    budget + movement_budgets[&a.child_asset],
                );
                positions.insert(a.child_asset.clone(), result);
            }
        }
        if changed.is_empty() {
            return Err(CharacterError::Invalid("Edit has no effect".into()));
        }
        let mut snapshot = self.current.clone();
        for id in &changed {
            snapshot.source =
                apply_positions(&snapshot.source, id, &positions[id], movement_budgets[id])?;
        }
        // A second geometric clearance check catches a closer neighboring parent surface.
        for attachment in &snapshot.attachments {
            if attachment.minimum_clearance.is_some() && changed.contains(&attachment.parent_asset)
            {
                let mut probe = snapshot.clone();
                probe.attachments.clear();
                bind_attachment(
                    &mut probe,
                    &attachment.parent_asset,
                    &attachment.child_asset,
                    attachment.minimum_clearance,
                )?;
            }
        }
        let final_cages = cages(&snapshot.source)?;
        for id in &locked {
            if serde_json::to_value(&original[id])? != serde_json::to_value(&final_cages[id])? {
                return Err(CharacterError::Locked(id.clone()));
            }
        }
        snapshot.source_fingerprint = mesh_source_fingerprint(&snapshot.source);
        // Free-form cage edits invalidate reversible template parameters instead of fabricating them.
        snapshot.parameters = None;
        let candidate = EditCandidate {
            id: format!("candidate-{}", self.next_candidate),
            base_revision: self.revision,
            snapshot,
            changed_assets: changed.into_iter().collect(),
            locked_assets: locked.into_iter().collect(),
            reason: request.reason,
            validation_policy:
                "explicitDesignEdit: geometry, bindings, locks; reference metrics are not claimed"
                    .into(),
        };
        self.next_candidate += 1;
        self.candidates
            .insert(candidate.id.clone(), candidate.clone());
        Ok(candidate)
    }
}
