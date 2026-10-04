// =========================================
// =========================================
// src/character_authoring/rig/binding.rs

use super::{
    HumanoidSkeleton, RigError, RigMesh,
    math::*,
    mesh::{fingerprint, pack_glb},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigWeightRegion {
    pub vertices: Vec<usize>,
    pub influences: Vec<String>,
}
/// Exact reviewed weight painting, still subject to independent anatomy and motion verification.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigVertexWeight {
    pub vertex: usize,
    pub influences: Vec<super::RigNamedInfluence>,
}
/// Reviewed mesh semantics distinguish body transitions from deforming clothes and attachments.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigSurfaceRegion {
    pub id: String,
    pub vertices: Vec<usize>,
    pub purpose: RigSurfacePurpose,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RigSurfacePurpose {
    Body { family: RigAnatomicalFamily },
    Attachment { anchors: Vec<String> },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum RigAnatomicalFamily {
    #[serde(rename = "torso")]
    Torso,
    #[serde(rename = "head")]
    Head,
    #[serde(rename = "arm_l")]
    ArmLeft,
    #[serde(rename = "arm_r")]
    ArmRight,
    #[serde(rename = "leg_l")]
    LegLeft,
    #[serde(rename = "leg_r")]
    LegRight,
}
impl RigAnatomicalFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Torso => "torso",
            Self::Head => "head",
            Self::ArmLeft => "arm_l",
            Self::ArmRight => "arm_r",
            Self::LegLeft => "leg_l",
            Self::LegRight => "leg_r",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigBindingOptions {
    #[serde(default, skip_serializing_if = "RigWeightMethod::is_local")]
    pub weight_method: RigWeightMethod,
    #[serde(default = "falloff")]
    pub falloff: f32,
    #[serde(default = "smooth_iterations")]
    pub smoothing_iterations: usize,
    #[serde(default)]
    pub regions: Vec<RigWeightRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surface_regions: Vec<RigSurfaceRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vertex_weights: Vec<RigVertexWeight>,
    /// Persistent constraints participate in the fingerprint and survive all weight regeneration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<super::RigWeightConstraint>,
    /// Unresolved vertices may be previewed, but cannot receive an acceptance certificate.
    #[serde(default)]
    pub pending_vertices: Vec<usize>,
    #[serde(default)]
    pub axis_map: Option<crate::ModelProfileBoneAxisMapNode>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RigWeightMethod {
    #[default]
    LocalDistance,
    SurfaceGeodesic,
}
impl RigWeightMethod {
    fn is_local(&self) -> bool {
        *self == Self::LocalDistance
    }
}
fn falloff() -> f32 {
    0.035
}
fn smooth_iterations() -> usize {
    3
}
impl Default for RigBindingOptions {
    fn default() -> Self {
        Self {
            weight_method: RigWeightMethod::LocalDistance,
            falloff: falloff(),
            smoothing_iterations: smooth_iterations(),
            regions: vec![],
            surface_regions: vec![],
            vertex_weights: vec![],
            constraints: vec![],
            pending_vertices: vec![],
            axis_map: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidBinding {
    pub skeleton: HumanoidSkeleton,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub profile: crate::ModelProfileNode,
    pub profile_dsl: String,
    pub fingerprint: String,
    pub assumptions: Vec<String>,
    /// Keep the caller's regions and unresolved vertices with the immutable skin.
    #[serde(default)]
    pub binding_options: Option<RigBindingOptions>,
}

// Legacy receipts omit options; new receipts also protect region provenance and pending state.
pub(crate) fn binding_fingerprint(binding: &HumanoidBinding) -> Result<String, RigError> {
    let payload = (
        &binding.skeleton,
        &binding.joints,
        &binding.weights,
        &binding.profile,
        &binding.profile_dsl,
    );
    Ok(fingerprint(
        &if let Some(options) = &binding.binding_options {
            serde_json::to_vec(&(payload, options))?
        } else {
            serde_json::to_vec(&payload)?
        },
    ))
}

pub(crate) fn append_accessor(
    doc: &mut Value,
    bin: &mut Vec<u8>,
    rows: &[Vec<f32>],
    kind: &str,
    component: u32,
) -> usize {
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let start = bin.len();
    for row in rows {
        for x in row {
            if component == 5123 {
                bin.extend((*x as u16).to_le_bytes());
            } else {
                bin.extend(x.to_le_bytes());
            }
        }
    }
    if !doc["bufferViews"].is_array() {
        doc["bufferViews"] = json!([]);
    }
    if !doc["accessors"].is_array() {
        doc["accessors"] = json!([]);
    }
    let views = doc["bufferViews"].as_array_mut().unwrap();
    let view = views.len();
    views.push(json!({"buffer":0,"byteOffset":start,"byteLength":bin.len()-start}));
    let mut a = json!({"bufferView":view,"componentType":component,"count":rows.len(),"type":kind});
    if kind == "VEC3" && !rows.is_empty() {
        a["min"] = json!(
            (0..3)
                .map(|k| rows.iter().map(|r| r[k]).fold(f32::INFINITY, f32::min))
                .collect::<Vec<_>>()
        );
        a["max"] = json!(
            (0..3)
                .map(|k| rows.iter().map(|r| r[k]).fold(f32::NEG_INFINITY, f32::max))
                .collect::<Vec<_>>()
        );
    }
    let accessors = doc["accessors"].as_array_mut().unwrap();
    let index = accessors.len();
    accessors.push(a);
    index
}

/// Export replaces all source skin data while retaining the mesh/material/image payload.
pub fn export_rig_glb(mesh: &RigMesh, binding: &HumanoidBinding) -> Result<Vec<u8>, RigError> {
    super::mesh::validate_mesh(mesh)?;
    if mesh.inspection.geometry_fingerprint != binding.skeleton.geometry_fingerprint
        || binding.joints.len() != mesh.positions.len()
        || binding.weights.len() != mesh.positions.len()
    {
        return Err(RigError::SourceChanged);
    }
    let mut doc = mesh.document.clone();
    let mut binary = mesh.binary.clone();
    let base = doc["nodes"].as_array().unwrap().len();
    let count = binding.skeleton.joints.len();
    let names = binding
        .skeleton
        .joints
        .iter()
        .map(|j| j.id.as_str())
        .collect::<BTreeSet<_>>();
    if !names.contains("root")
        || names.len() != count
        || binding
            .skeleton
            .joints
            .iter()
            .any(|j| j.parent.as_deref().is_some_and(|p| !names.contains(p)))
        || binding
            .joints
            .iter()
            .flatten()
            .any(|i| *i as usize >= count)
    {
        return Err(RigError::Invalid(
            "Invalid exported joint hierarchy or skin indices".into(),
        ));
    }
    let ibm = append_accessor(
        &mut doc,
        &mut binary,
        &binding
            .skeleton
            .joints
            .iter()
            .map(|j| j.inverse_bind_matrix.to_vec())
            .collect::<Vec<_>>(),
        "MAT4",
        5126,
    );
    for part in &mesh.inspection.parts {
        let range = part.vertex_start..part.vertex_start + part.vertex_count;
        let pos = append_accessor(
            &mut doc,
            &mut binary,
            &mesh.positions[range.clone()]
                .iter()
                .map(|v| v.to_vec())
                .collect::<Vec<_>>(),
            "VEC3",
            5126,
        );
        let normal = if mesh.normals[range.clone()].iter().all(Option::is_some) {
            Some(append_accessor(
                &mut doc,
                &mut binary,
                &mesh.normals[range.clone()]
                    .iter()
                    .map(|v| v.unwrap().to_vec())
                    .collect::<Vec<_>>(),
                "VEC3",
                5126,
            ))
        } else {
            None
        };
        let joint = append_accessor(
            &mut doc,
            &mut binary,
            &binding.joints[range.clone()]
                .iter()
                .map(|v| v.map(|x| x as f32).to_vec())
                .collect::<Vec<_>>(),
            "VEC4",
            5123,
        );
        let weight = append_accessor(
            &mut doc,
            &mut binary,
            &binding.weights[range]
                .iter()
                .map(|v| v.to_vec())
                .collect::<Vec<_>>(),
            "VEC4",
            5126,
        );
        let primitive = &mut doc["meshes"][part.mesh]["primitives"][part.primitive];
        primitive["attributes"]["POSITION"] = json!(pos);
        primitive["attributes"]["JOINTS_0"] = json!(joint);
        primitive["attributes"]["WEIGHTS_0"] = json!(weight);
        if let Some(n) = normal {
            primitive["attributes"]["NORMAL"] = json!(n);
        }
    }
    let indices = binding
        .skeleton
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| (j.id.as_str(), base + i))
        .collect::<BTreeMap<_, _>>();
    let nodes = doc["nodes"].as_array_mut().unwrap();
    for node in nodes.iter_mut() {
        node["skin"] = json!(0);
    }
    for j in &binding.skeleton.joints {
        let children = binding
            .skeleton
            .joints
            .iter()
            .filter(|c| c.parent.as_deref() == Some(&j.id))
            .map(|c| indices[c.id.as_str()])
            .collect::<Vec<_>>();
        nodes.push(json!({"name":j.id,"translation":j.local_position,"rotation":j.local_rotation,"children":children}));
    }
    doc["skins"] = json!([{"joints":(base..base+count).collect::<Vec<_>>(),"skeleton":indices["root"],"inverseBindMatrices":ibm}]);
    let roots = doc["scenes"][0]["nodes"].as_array_mut().unwrap();
    roots.push(json!(indices["root"]));
    let asset = doc["asset"]
        .as_object_mut()
        .ok_or_else(|| RigError::Invalid("Missing GLB asset metadata".into()))?;
    asset.insert(
        "generator".into(),
        json!("MotionLoom Character Authoring humanoid65_v1"),
    );
    if !doc["extras"].is_object() {
        let old = doc["extras"].take();
        doc["extras"] = json!({"sourceExtras":old});
    }
    doc["extras"]["motionloomRig"] = json!({"standardId":binding.skeleton.standard_id,"bindingFingerprint":binding.fingerprint,"sourceGeometryFingerprint":mesh.inspection.geometry_fingerprint,"inputSkeletonPolicy":mesh.inspection.skeleton_policy,"weightConstraints":binding.binding_options.as_ref().map(|o| &o.constraints)});
    pack_glb(&doc, &binary)
}

/// Region-constrained segment weights are a deterministic proposal, not a production auto-rig claim.
pub fn bind_humanoid_skin(
    mesh: &RigMesh,
    skeleton: HumanoidSkeleton,
    options: &RigBindingOptions,
) -> Result<HumanoidBinding, RigError> {
    super::mesh::validate_mesh(mesh)?;
    if skeleton.geometry_fingerprint != mesh.inspection.geometry_fingerprint {
        return Err(RigError::SourceChanged);
    }
    if !options.falloff.is_finite() || options.falloff <= 0. || options.smoothing_iterations > 12 {
        return Err(RigError::Invalid(
            "Invalid skin falloff or smoothing iterations".into(),
        ));
    }
    let bones = &skeleton.joints;
    let ids = bones
        .iter()
        .enumerate()
        .map(|(i, j)| (j.id.as_str(), i))
        .collect::<BTreeMap<_, _>>();
    let h = mesh.inspection.height;
    let mut restrictions = BTreeMap::<usize, BTreeSet<usize>>::new();
    for region in &options.regions {
        if region.vertices.is_empty() || region.influences.is_empty() {
            return Err(RigError::Invalid("Weight regions cannot be empty".into()));
        }
        let influences = region
            .influences
            .iter()
            .map(|id| {
                ids.get(id.as_str())
                    .copied()
                    .ok_or_else(|| RigError::Invalid(format!("Unknown influence: {id}")))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if influences
            .iter()
            .any(|i| bones[*i].endpoint || bones[*i].id == "root")
        {
            return Err(RigError::Invalid(
                "Endpoint/root nodes are not skin influences".into(),
            ));
        }
        for v in &region.vertices {
            if *v >= mesh.positions.len() || restrictions.insert(*v, influences.clone()).is_some() {
                return Err(RigError::Invalid(
                    "Invalid or overlapping weight region".into(),
                ));
            }
        }
    }
    // Surface intent is explicit provenance, never inferred from a source skin or node name.
    let mut surface_ids = BTreeSet::new();
    let mut surface_vertices = BTreeSet::new();
    let mut attachment_vertices = BTreeSet::new();
    let mut body_families = BTreeMap::new();
    for surface in &options.surface_regions {
        if surface.id.trim().is_empty()
            || !surface_ids.insert(&surface.id)
            || surface.vertices.is_empty()
        {
            return Err(RigError::Invalid(
                "Surface annotations need unique IDs and nonempty selections".into(),
            ));
        }
        let anchors = match &surface.purpose {
            RigSurfacePurpose::Body { .. } => None,
            RigSurfacePurpose::Attachment { anchors } => {
                if anchors.is_empty() {
                    return Err(RigError::Invalid(
                        "Attachments need reviewed anchor joints".into(),
                    ));
                }
                let indices = anchors
                    .iter()
                    .map(|id| {
                        ids.get(id.as_str())
                            .copied()
                            .ok_or_else(|| RigError::Missing(id.clone()))
                    })
                    .collect::<Result<BTreeSet<_>, _>>()?;
                if indices.len() != anchors.len()
                    || indices
                        .iter()
                        .any(|i| bones[*i].endpoint || bones[*i].id == "root")
                {
                    return Err(RigError::Invalid(
                        "Invalid or duplicate attachment anchors".into(),
                    ));
                }
                Some(indices)
            }
        };
        for &v in &surface.vertices {
            if v >= mesh.positions.len() || !surface_vertices.insert(v) {
                return Err(RigError::Invalid(
                    "Invalid or overlapping surface annotation".into(),
                ));
            }
            if let Some(anchors) = &anchors {
                attachment_vertices.insert(v);
                if restrictions.get(&v).is_some_and(|r| !anchors.is_subset(r)) {
                    return Err(RigError::Invalid(
                        "Attachment anchors contradict the weight region".into(),
                    ));
                }
                restrictions.insert(v, anchors.clone());
            }
            if let RigSurfacePurpose::Body { family } = surface.purpose {
                body_families.insert(v, family);
            }
        }
    }
    // A rigid constraint is a one-joint restriction throughout distance weighting and smoothing.
    let constrained = super::constraints::constraint_assignments(
        mesh.positions.len(),
        &skeleton,
        &options.constraints,
    )?;
    for (&v, &joint) in &constrained {
        if body_families
            .get(&v)
            .is_some_and(|family| super::weights::family(&bones[joint].id) != family.as_str())
        {
            return Err(RigError::Invalid(
                "A body annotation contradicts a rigid constraint".into(),
            ));
        }
        if restrictions
            .get(&v)
            .is_some_and(|allowed| !allowed.contains(&joint))
        {
            return Err(RigError::Invalid(
                "A weight region contradicts a rigid constraint".into(),
            ));
        }
        restrictions.insert(v, BTreeSet::from([joint]));
    }
    // An uncertain vertex must not simultaneously claim a confirmed anatomical region.
    let mut pending = BTreeSet::new();
    for v in &options.pending_vertices {
        if *v >= mesh.positions.len()
            || !pending.insert(*v)
            || restrictions.contains_key(v)
            || surface_vertices.contains(v)
        {
            return Err(RigError::Invalid(
                "Invalid, duplicate or region-assigned pending vertex".into(),
            ));
        }
    }
    let mut painted = BTreeMap::new();
    for input in &options.vertex_weights {
        if input.vertex >= mesh.positions.len()
            || painted.contains_key(&input.vertex)
            || pending.contains(&input.vertex)
            || input.influences.is_empty()
            || input.influences.len() > 4
        {
            return Err(RigError::Invalid(
                "Invalid, duplicate or pending painted vertex".into(),
            ));
        }
        let mut unique = BTreeSet::new();
        let mut js = [0u16; 4];
        let mut ws = [0f32; 4];
        for (slot, influence) in input.influences.iter().enumerate() {
            let joint = *ids
                .get(influence.bone.as_str())
                .ok_or_else(|| RigError::Missing(influence.bone.clone()))?;
            if !unique.insert(joint)
                || bones[joint].endpoint
                || bones[joint].id == "root"
                || !influence.weight.is_finite()
                || influence.weight <= 0.
                || (!bones[joint].core
                    && restrictions
                        .get(&input.vertex)
                        .is_none_or(|r| !r.contains(&joint)))
                || restrictions
                    .get(&input.vertex)
                    .is_some_and(|r| !r.contains(&joint))
            {
                return Err(RigError::Invalid(
                    "Painted influences contradict the allowed joints or rigid constraint".into(),
                ));
            }
            js[slot] = joint as u16;
            ws[slot] = influence.weight;
        }
        if (ws.iter().sum::<f32>() - 1.).abs() > 1e-5 {
            return Err(RigError::Invalid("Painted weights must sum to one".into()));
        }
        painted.insert(input.vertex, (js, ws));
    }
    // Welded adjacency stops UV seams from acquiring different weights; region restrictions persist.
    let mut weld = BTreeMap::<[i64; 3], Vec<usize>>::new();
    for (i, p) in mesh.positions.iter().enumerate() {
        weld.entry(p.map(|x| (x / h * 1e7).round() as i64))
            .or_default()
            .push(i);
    }
    let mut adjacency = vec![BTreeSet::new(); mesh.positions.len()];
    for t in &mesh.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            adjacency[t[a] as usize].insert(t[b] as usize);
            adjacency[t[b] as usize].insert(t[a] as usize);
        }
    }
    for group in weld.values() {
        let neighbors = group
            .iter()
            .flat_map(|i| adjacency[*i].iter().copied())
            .chain(group.iter().copied())
            .collect::<BTreeSet<_>>();
        for i in group {
            adjacency[*i] = neighbors.clone();
        }
    }
    let inactive = constrained
        .keys()
        .copied()
        .chain(attachment_vertices.iter().copied())
        .chain(painted.keys().copied())
        .collect();
    let geodesic = (options.weight_method == RigWeightMethod::SurfaceGeodesic)
        .then(|| super::geodesic::distances(mesh, &skeleton, &adjacency, &inactive));
    let mut dense = vec![vec![0.; bones.len()]; mesh.positions.len()];
    for (v, p) in mesh.positions.iter().enumerate() {
        if let Some((js, ws)) = painted.get(&v) {
            for (&joint, &weight) in js.iter().zip(ws) {
                dense[v][joint as usize] += weight;
            }
            continue;
        }
        for (i, bone) in bones.iter().enumerate() {
            if bone.endpoint
                || bone.id == "root"
                || (!bone.core && restrictions.get(&v).is_none_or(|r| !r.contains(&i)))
                || restrictions.get(&v).is_some_and(|r| !r.contains(&i))
            {
                continue;
            }
            let child = if bone.core {
                super::build::child_id(
                    &bone.id,
                    &super::humanoid_rig_standard().references[0].joints,
                )
                .and_then(|id| bones.iter().find(|j| j.id == id))
            } else {
                bones.iter().find(|j| j.parent.as_deref() == Some(&bone.id))
            };
            let end = child.map(|j| j.position).unwrap_or(bone.position);
            let finger = ["thumb_", "index_", "middle_", "ring_", "pinky_"]
                .iter()
                .any(|prefix| bone.id.starts_with(prefix));
            let radius = h
                * options.falloff
                * if finger {
                    // A reviewed single-digit region can use its full cross-section without attracting neighboring digits.
                    if restrictions.get(&v).is_some_and(|r| r.len() <= 5) {
                        0.7
                    } else {
                        0.18
                    }
                } else {
                    1.
                };
            let distance = if constrained.contains_key(&v) || attachment_vertices.contains(&v) {
                distance_segment(*p, bone.position, end)
            } else {
                geodesic.as_ref().map_or_else(
                    || distance_segment(*p, bone.position, end),
                    |field| field[v][i],
                )
            };
            // Opposite limbs cannot attract a vertex through the torso midline.
            if bone.id.ends_with("_l") || bone.id.ends_with("_r") {
                let sign = bone.position[0].signum();
                if p[0] * sign < -h * 0.015 {
                    continue;
                }
            }
            // Inverse-square support avoids overly sharp transitions on thick joints and shoulders.
            dense[v][i] = 1. / ((distance / radius).powi(2) + 0.0001);
        }
        let sum = dense[v].iter().sum::<f32>();
        if sum <= 0. || !sum.is_finite() {
            return Err(RigError::Invalid(format!(
                "No usable influence for vertex {v}"
            )));
        }
        for w in &mut dense[v] {
            *w /= sum;
        }
        project_four(&mut dense[v]);
    }
    // Metric diffusion keeps tiny connected edges coherent without joining nearby loose surfaces.
    let adjacency = adjacency
        .iter()
        .enumerate()
        .map(|(v, neighbors)| {
            let mut weighted = neighbors
                .iter()
                .filter_map(|n| {
                    let d = length(sub(mesh.positions[v], mesh.positions[*n]));
                    // Weld entries include the vertex itself; zero-length neighbors must not stop diffusion.
                    (d > h * 1e-7).then_some((*n, 1. / (d * d).max(h * h * 1e-12)))
                })
                .collect::<Vec<_>>();
            let sum = weighted.iter().map(|(_, w)| w).sum::<f32>();
            for (_, w) in &mut weighted {
                *w /= sum;
            }
            weighted
        })
        .collect::<Vec<_>>();
    for _ in 0..options.smoothing_iterations {
        let old = dense.clone();
        for (v, neighbors) in adjacency.iter().enumerate() {
            if neighbors.is_empty() || painted.contains_key(&v) {
                continue;
            }
            for i in 0..bones.len() {
                if restrictions.get(&v).is_none_or(|r| r.contains(&i)) {
                    dense[v][i] = old[v][i] * 0.5
                        + neighbors.iter().map(|(n, w)| old[*n][i] * w).sum::<f32>() * 0.5;
                }
            }
            // A restricted neighborhood must still contribute a normalized distribution next time.
            let sum = dense[v].iter().sum::<f32>();
            for w in &mut dense[v] {
                *w /= sum;
            }
            // Diffuse the weights that will actually be rendered, not a dense field later truncated.
            project_four(&mut dense[v]);
        }
    }
    // Blend into reviewed rigid surfaces over geodesic distance. A hard face boundary must
    // approach weight one continuously through the neck, without leaking across loose parts.
    let pinned_joints = constrained.values().copied().collect::<BTreeSet<_>>();
    let transition = h * options.falloff * 2.;
    for joint in pinned_joints {
        let mut distances = vec![f32::INFINITY; mesh.positions.len()];
        for (&v, &anchor) in &constrained {
            if anchor == joint {
                distances[v] = 0.;
            }
        }
        let mut queue = BinaryHeap::new();
        for (&v, &anchor) in &constrained {
            if anchor != joint {
                continue;
            }
            for &(n, _) in &adjacency[v] {
                if constrained.contains_key(&n)
                    || restrictions.get(&n).is_some_and(|r| !r.contains(&joint))
                {
                    continue;
                }
                let d = length(sub(mesh.positions[v], mesh.positions[n]));
                if d < transition && d < distances[n] {
                    distances[n] = d;
                    queue.push(Reverse((d.to_bits(), n)));
                }
            }
        }
        while let Some(Reverse((bits, v))) = queue.pop() {
            let distance = f32::from_bits(bits);
            if distance > distances[v] {
                continue;
            }
            for &(n, _) in &adjacency[v] {
                if constrained.contains_key(&n)
                    || restrictions.get(&n).is_some_and(|r| !r.contains(&joint))
                {
                    continue;
                }
                let d = distance + length(sub(mesh.positions[v], mesh.positions[n]));
                if d < transition && d < distances[n] {
                    distances[n] = d;
                    queue.push(Reverse((d.to_bits(), n)));
                }
            }
        }
        for (v, distance) in distances.into_iter().enumerate() {
            if constrained.contains_key(&v) || painted.contains_key(&v) || distance >= transition {
                continue;
            }
            let t = distance / transition;
            let minimum = 1. - t * t * (3. - 2. * t);
            let old = dense[v][joint];
            if minimum > old {
                let scale = (1. - minimum) / (1. - old).max(1e-8);
                for w in &mut dense[v] {
                    *w *= scale;
                }
                dense[v][joint] = minimum;
            }
        }
    }
    let mut joints = vec![];
    let mut weights = vec![];
    for (v, mut row) in dense.into_iter().enumerate() {
        if let Some(&(js, ws)) = painted.get(&v) {
            joints.push(js);
            weights.push(ws);
            continue;
        }
        project_four(&mut row);
        let mut order = row
            .into_iter()
            .enumerate()
            .filter(|(_, w)| *w > 0.)
            .collect::<Vec<_>>();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        order.truncate(4);
        let sum = order.iter().map(|(_, w)| w).sum::<f32>();
        let mut js = [0; 4];
        let mut ws = [0.; 4];
        for (k, (i, w)) in order.into_iter().enumerate() {
            js[k] = i as u16;
            ws[k] = w / sum;
        }
        joints.push(js);
        weights.push(ws);
    }
    let mut binding=HumanoidBinding{skeleton,joints,weights,profile:crate::ModelProfileNode{id:"humanoid_profile".into(),kind:"3d".into(),model:Some("rigged_model".into()),preset:"humanoid_v1".into(),retarget:None,bone_axis_map:None},profile_dsl:String::new(),fingerprint:String::new(),assumptions:vec!["Local-distance skin weights require action-based deformation review; no cloth, collision or volume-preserving solver is included.".into()],binding_options:Some(options.clone())};
    let bytes = export_rig_glb(mesh, &binding)?;
    let inspection = crate::api::inspect_glb_skeleton_bytes(&bytes, "rigged_model")
        .map_err(|e| RigError::Evaluation(e.to_string()))?;
    let graph=crate::api::parse_graph_script(&format!("<Graph fps=\"24\" duration=\"1s\" size={{[64,64]}}>\n<Assets><ModelAsset id=\"rigged_model\" src=\"candidate.glb\" /></Assets>\n{}\n<Scene id=\"empty\"><Timeline>\n</Timeline></Scene>\n<Present from=\"empty\" />\n</Graph>",inspection.profile_dsl)).map_err(|e|RigError::Evaluation(e.to_string()))?;
    binding.profile = graph.model_profiles[0].clone();
    binding.profile.id = "humanoid_profile".into();
    binding.profile.retarget = Some(crate::ModelProfileRetargetNode {
        preset: "humanoid_v1".into(),
        maps: binding
            .skeleton
            .joints
            .iter()
            .map(|j| crate::ModelProfileRetargetMapNode {
                from: j.id.clone(),
                to: j.id.clone(),
            })
            .collect(),
    });
    if let Some(axes) = &options.axis_map {
        binding.profile.bone_axis_map = Some(axes.clone());
    }
    binding.profile_dsl = profile_dsl(&binding.profile);
    let axes = binding
        .profile_dsl
        .split("  <BoneAxisMap>")
        .nth(1)
        .and_then(|s| s.split("  </BoneAxisMap>").next())
        .unwrap_or("");
    // Rebinding an existing skeleton replaces calibration instead of duplicating its DSL block.
    while let Some(start) = binding.skeleton.skeleton_dsl.find("<BoneAxisMap>") {
        let Some(end) = binding.skeleton.skeleton_dsl[start..].find("</BoneAxisMap>") else {
            break;
        };
        binding
            .skeleton
            .skeleton_dsl
            .drain(start..start + end + "</BoneAxisMap>".len());
    }
    binding.skeleton.skeleton_dsl = binding.skeleton.skeleton_dsl.replace(
        "</Skeleton>",
        &format!("  <BoneAxisMap>{axes}  </BoneAxisMap>\n</Skeleton>"),
    );
    let source = format!(
        "<Graph fps=\"24\" duration=\"1s\" size={{[64,64]}}>\n<Assets><ModelAsset id=\"rigged_model\" src=\"candidate.glb\" /></Assets>\n{}\n{}\n<Scene id=\"empty\"><Timeline></Timeline></Scene><Present from=\"empty\" /></Graph>",
        binding.skeleton.skeleton_dsl, binding.profile_dsl
    );
    let graph = crate::api::parse_graph_script(&source)
        .map_err(|e| RigError::Invalid(format!("Generated rig DSL: {e}")))?;
    if serde_json::to_value(&graph.model_profiles[0])? != serde_json::to_value(&binding.profile)? {
        return Err(RigError::Invalid(
            "Generated ModelProfile DSL does not match typed data".into(),
        ));
    }
    binding.fingerprint = binding_fingerprint(&binding)?;
    Ok(binding)
}

// Continuous sparse support prevents a full fourth influence disappearing when two bones exchange rank.
fn project_four(row: &mut [f32]) {
    let mut indices = [usize::MAX; 5];
    let mut values = [0f32; 5];
    for (i, &w) in row.iter().enumerate() {
        if w <= 0. {
            continue;
        }
        if let Some(slot) = (0..5).find(|k| w > values[*k] || (w == values[*k] && i < indices[*k]))
        {
            for k in (slot + 1..5).rev() {
                indices[k] = indices[k - 1];
                values[k] = values[k - 1];
            }
            indices[slot] = i;
            values[slot] = w;
        }
    }
    let mut cutoff = values[4];
    let mut sum = values[..4].iter().map(|w| w - cutoff).sum::<f32>();
    // Fully equal supports have no unique sparse solution; preserve deterministic legacy ordering.
    if sum <= 1e-8 {
        cutoff = 0.;
        sum = values[..4].iter().sum();
    }
    row.fill(0.);
    for k in 0..4 {
        if indices[k] != usize::MAX {
            row[indices[k]] = (values[k] - cutoff) / sum;
        }
    }
}

// Standalone verification also validates provenance; a forged options object must not bypass checks.
pub(crate) fn validate_surface_regions(
    count: usize,
    skeleton: &HumanoidSkeleton,
    options: &RigBindingOptions,
) -> Result<(), RigError> {
    let mut ids = BTreeSet::new();
    let mut vertices = BTreeSet::new();
    let constraints =
        super::constraints::constraint_assignments(count, skeleton, &options.constraints)?;
    for surface in &options.surface_regions {
        if surface.id.trim().is_empty() || !ids.insert(&surface.id) || surface.vertices.is_empty() {
            return Err(RigError::Invalid(
                "Invalid surface annotation IDs or empty selection".into(),
            ));
        }
        if let RigSurfacePurpose::Attachment { anchors } = &surface.purpose {
            let unique = anchors.iter().collect::<BTreeSet<_>>();
            if anchors.is_empty()
                || unique.len() != anchors.len()
                || anchors.iter().any(|id| {
                    skeleton
                        .joints
                        .iter()
                        .find(|j| &j.id == id)
                        .is_none_or(|j| j.endpoint || j.id == "root")
                })
            {
                return Err(RigError::Invalid(
                    "Invalid surface attachment anchors".into(),
                ));
            }
        }
        for &v in &surface.vertices {
            if v >= count || !vertices.insert(v) || options.pending_vertices.contains(&v) {
                return Err(RigError::Invalid(
                    "Invalid, overlapping or pending surface annotation".into(),
                ));
            }
            if let Some(&joint) = constraints.get(&v) {
                let name = &skeleton.joints[joint].id;
                let compatible = match &surface.purpose {
                    RigSurfacePurpose::Body { family } => {
                        super::weights::family(name) == family.as_str()
                    }
                    RigSurfacePurpose::Attachment { anchors } => anchors.contains(name),
                };
                if !compatible {
                    return Err(RigError::Invalid(
                        "Surface intent contradicts a rigid constraint".into(),
                    ));
                }
            }
        }
    }
    let pending = options
        .pending_vertices
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let bones = skeleton
        .joints
        .iter()
        .map(|j| (j.id.as_str(), j))
        .collect::<BTreeMap<_, _>>();
    let mut allowed = BTreeMap::new();
    for region in &options.regions {
        for &vertex in &region.vertices {
            if vertex >= count || allowed.insert(vertex, &region.influences).is_some() {
                return Err(RigError::Invalid(
                    "Invalid or overlapping weight region metadata".into(),
                ));
            }
        }
    }
    let attachment_anchors = options
        .surface_regions
        .iter()
        .filter_map(|surface| match &surface.purpose {
            RigSurfacePurpose::Attachment { anchors } => {
                Some(surface.vertices.iter().map(move |&v| (v, anchors)))
            }
            _ => None,
        })
        .flatten()
        .collect::<BTreeMap<_, _>>();
    let mut painted = BTreeSet::new();
    for input in &options.vertex_weights {
        if input.vertex >= count
            || !painted.insert(input.vertex)
            || pending.contains(&input.vertex)
            || input.influences.is_empty()
            || input.influences.len() > 4
        {
            return Err(RigError::Invalid(
                "Invalid, duplicate or pending painted weight metadata".into(),
            ));
        }
        let mut joints = BTreeSet::new();
        let mut total = 0.;
        for influence in &input.influences {
            let bone = bones
                .get(influence.bone.as_str())
                .ok_or_else(|| RigError::Missing(influence.bone.clone()))?;
            if bone.endpoint
                || bone.id == "root"
                || !joints.insert(&bone.id)
                || !influence.weight.is_finite()
                || influence.weight <= 0.
            {
                return Err(RigError::Invalid(
                    "Invalid painted influence metadata".into(),
                ));
            }
            if allowed
                .get(&input.vertex)
                .is_some_and(|r| !r.contains(&bone.id))
                || attachment_anchors
                    .get(&input.vertex)
                    .is_some_and(|r| !r.contains(&bone.id))
                || (!bone.core
                    && allowed
                        .get(&input.vertex)
                        .is_none_or(|r| !r.contains(&bone.id))
                    && attachment_anchors
                        .get(&input.vertex)
                        .is_none_or(|r| !r.contains(&bone.id)))
            {
                return Err(RigError::Invalid(
                    "Painted metadata contradicts allowed joint selections".into(),
                ));
            }
            if constraints
                .get(&input.vertex)
                .is_some_and(|j| skeleton.joints[*j].id != bone.id)
            {
                return Err(RigError::Invalid(
                    "Painted metadata contradicts a rigid constraint".into(),
                ));
            }
            total += influence.weight;
        }
        if (total - 1.).abs() > 1e-5 {
            return Err(RigError::Invalid("Painted metadata must sum to one".into()));
        }
    }
    Ok(())
}

pub(crate) fn profile_dsl(profile: &crate::ModelProfileNode) -> String {
    let mut s = String::from(
        "<ModelProfile id=\"humanoid_profile\" kind=\"3d\" model=\"rigged_model\" preset=\"humanoid_v1\">\n  <Retarget preset=\"humanoid_v1\">\n",
    );
    if let Some(retarget) = &profile.retarget {
        for m in &retarget.maps {
            s.push_str(&format!(
                "    <Map from=\"{}\" to=\"{}\" />\n",
                m.from, m.to
            ));
        }
    }
    s.push_str("  </Retarget>\n  <BoneAxisMap>\n");
    if let Some(map) = &profile.bone_axis_map {
        for axis in &map.axes {
            s.push_str(&format!("    <Axis bone=\"{}\"", axis.bone));
            for (name, value) in [
                ("forward", &axis.forward),
                ("side", &axis.side),
                ("twist", &axis.twist),
                ("bend", &axis.bend),
                ("turn", &axis.turn),
                ("restForward", &axis.rest_forward),
                ("restSide", &axis.rest_side),
                ("restTwist", &axis.rest_twist),
                ("restBend", &axis.rest_bend),
                ("restTurn", &axis.rest_turn),
            ] {
                if let Some(v) = value {
                    s.push_str(&format!(
                        " {name}=\"{}\"",
                        v.replace('&', "&amp;").replace('"', "&quot;")
                    ));
                }
            }
            s.push_str(" />\n");
        }
    }
    s.push_str("  </BoneAxisMap>\n</ModelProfile>\n");
    s
}
