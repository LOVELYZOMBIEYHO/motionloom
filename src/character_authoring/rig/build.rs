// =========================================
// =========================================
// src/character_authoring/rig/build.rs

use super::{ReferenceJoint, RigError, RigMesh, humanoid_rig_standard, math::*};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigExtension {
    pub id: String,
    pub parent: String,
    pub position: V3,
    pub rotation: Q4,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigBuildRequest {
    pub expected_geometry_fingerprint: String,
    #[serde(default = "reference_id")]
    pub reference: String,
    #[serde(default)]
    pub landmarks: BTreeMap<String, V3>,
    #[serde(default)]
    pub rotations: BTreeMap<String, Q4>,
    #[serde(default)]
    pub extensions: Vec<RigExtension>,
}
fn reference_id() -> String {
    "character1".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigJoint {
    pub id: String,
    pub reference_name: Option<String>,
    pub parent: Option<String>,
    pub core: bool,
    pub endpoint: bool,
    pub position: V3,
    pub rotation: Q4,
    pub local_position: V3,
    pub local_rotation: Q4,
    pub inverse_bind_matrix: M4,
    pub position_source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidSkeleton {
    pub standard_id: String,
    pub reference: String,
    pub geometry_fingerprint: String,
    pub joints: Vec<RigJoint>,
    pub inferred_landmarks: Vec<String>,
    pub skeleton_dsl: String,
}

pub(crate) fn child_id(id: &str, joints: &[ReferenceJoint]) -> Option<String> {
    let preferred = match id {
        "root" => Some("hips".into()),
        "hips" => Some("spine".into()),
        "upper_chest" => Some("neck".into()),
        "hand_l" => Some("middle_1_l".into()),
        "hand_r" => Some("middle_1_r".into()),
        _ => None,
    };
    preferred.or_else(|| {
        joints
            .iter()
            .find(|j| j.parent.as_deref() == Some(id))
            .map(|j| j.id.clone())
    })
}
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// Fit the CC0 hierarchy to mesh-space landmarks; never infer from an input GLB skeleton.
pub fn build_humanoid_skeleton(
    mesh: &RigMesh,
    request: &RigBuildRequest,
) -> Result<HumanoidSkeleton, RigError> {
    super::mesh::validate_mesh(mesh)?;
    if request.expected_geometry_fingerprint != mesh.inspection.geometry_fingerprint {
        return Err(RigError::SourceChanged);
    }
    let standard = humanoid_rig_standard();
    let reference = standard
        .references
        .iter()
        .find(|r| r.id == request.reference)
        .ok_or_else(|| RigError::Invalid("Unknown rig reference".into()))?;
    let definitions = &reference.joints;
    let names = definitions
        .iter()
        .map(|j| j.id.as_str())
        .collect::<BTreeSet<_>>();
    for (id, p) in &request.landmarks {
        if !names.contains(id.as_str()) || !p.iter().all(|v| v.is_finite()) {
            return Err(RigError::Invalid(format!(
                "Unknown or non-finite landmark: {id}"
            )));
        }
    }
    for (id, q) in &request.rotations {
        if !names.contains(id.as_str())
            || !q.iter().all(|v| v.is_finite())
            || q.iter().map(|v| v * v).sum::<f32>() < 1e-10
        {
            return Err(RigError::Invalid(format!(
                "Invalid global joint rotation: {id}"
            )));
        }
    }
    let height = mesh.inspection.height;
    let mut positions = BTreeMap::new();
    // Unspecified descendants follow edited ancestors instead of remaining at stale coordinates.
    for definition in definitions {
        let mut p = scale(definition.position, height);
        if let Some(parent) = definition.parent.as_deref() {
            let parent_definition = definitions.iter().find(|j| j.id == parent).unwrap();
            let parent_position = positions[parent];
            p = add(
                p,
                sub(parent_position, scale(parent_definition.position, height)),
            );
            // Unspecified leaf positions follow the fitted terminal segment, including its roll.
            if definition.endpoint && !request.landmarks.contains_key(&definition.id) {
                let ancestor = parent_definition.parent.as_deref().unwrap();
                let ancestor_definition = definitions.iter().find(|j| j.id == ancestor).unwrap();
                let from = sub(parent_definition.position, ancestor_definition.position);
                let to = sub(parent_position, positions[ancestor]);
                if length(to) < height * 1e-5 {
                    return Err(RigError::Invalid(format!(
                        "Coincident terminal joints: {ancestor} and {parent}"
                    )));
                }
                let delta = request
                    .rotations
                    .get(parent)
                    .map(|q| qmul(qnorm(*q), conjugate(parent_definition.rotation)))
                    .unwrap_or_else(|| arc(from, to));
                let proportional_scale = length(to) / length(from);
                p = add(
                    parent_position,
                    rotate(
                        delta,
                        scale(
                            sub(definition.position, parent_definition.position),
                            proportional_scale,
                        ),
                    ),
                );
            }
        }
        positions.insert(
            definition.id.clone(),
            request.landmarks.get(&definition.id).copied().unwrap_or(p),
        );
    }
    // The CC0 reference faces +Z and its anatomical left is +X; reject swapped hints early.
    for chain in ["upper_arm", "hand", "upper_leg", "foot"] {
        if positions[&format!("{chain}_l")][0] <= positions[&format!("{chain}_r")][0] {
            return Err(RigError::Invalid(format!(
                "Anatomical left/right landmarks are reversed for {chain}; normalize mesh facing before fitting"
            )));
        }
    }
    if positions["head"][1] <= positions["hips"][1] {
        return Err(RigError::Invalid(
            "Head must be above pelvis in the Y-up reference space".into(),
        ));
    }
    let mut global_rotations = BTreeMap::new();
    for definition in definitions {
        let q = if let Some(q) = request.rotations.get(&definition.id) {
            qnorm(*q)
        } else if let Some(child) = child_id(&definition.id, definitions) {
            let child_definition = definitions.iter().find(|j| j.id == child).unwrap();
            let from = sub(child_definition.position, definition.position);
            let to = sub(positions[&child], positions[&definition.id]);
            if length(to) < height * 1e-5 {
                return Err(RigError::Invalid(format!(
                    "Coincident joints: {} and {child}",
                    definition.id
                )));
            }
            qmul(arc(from, to), definition.rotation)
        } else if let Some(parent) = definition.parent.as_deref() {
            let parent_definition = definitions.iter().find(|j| j.id == parent).unwrap();
            qmul(
                qmul(
                    global_rotations[parent],
                    conjugate(parent_definition.rotation),
                ),
                definition.rotation,
            )
        } else {
            definition.rotation
        };
        global_rotations.insert(definition.id.clone(), q);
    }
    let mut joints = Vec::new();
    let mut inferred = vec![];
    for d in definitions {
        let p = positions[&d.id];
        let q = global_rotations[&d.id];
        let (lp, lq) = if let Some(parent) = &d.parent {
            (
                rotate(
                    conjugate(global_rotations[parent]),
                    sub(p, positions[parent]),
                ),
                qmul(conjugate(global_rotations[parent]), q),
            )
        } else {
            (p, q)
        };
        let explicit = request.landmarks.contains_key(&d.id);
        if !explicit && !d.endpoint && d.id != "root" {
            inferred.push(d.id.clone());
        }
        joints.push(RigJoint {
            id: d.id.clone(),
            reference_name: Some(d.reference_name.clone()),
            parent: d.parent.clone(),
            core: true,
            endpoint: d.endpoint,
            position: p,
            rotation: q,
            local_position: lp,
            local_rotation: lq,
            inverse_bind_matrix: rigid_inverse(p, q),
            position_source: if explicit {
                "landmark"
            } else {
                "referenceProportionPrior"
            }
            .into(),
        });
    }
    for extension in &request.extensions {
        if !valid_id(&extension.id)
            || extension.id.starts_with("mesh_")
            || joints.iter().any(|j| j.id == extension.id)
            || extension
                .position
                .iter()
                .chain(extension.rotation.iter())
                .any(|v| !v.is_finite())
            || extension.rotation.iter().map(|v| v * v).sum::<f32>() < 1e-10
        {
            return Err(RigError::Invalid(
                "Invalid or duplicate extension joint".into(),
            ));
        }
        let parent = joints
            .iter()
            .find(|j| j.id == extension.parent)
            .ok_or_else(|| {
                RigError::Invalid(
                    "Extensions must follow an existing core or extension parent".into(),
                )
            })?;
        let q = qnorm(extension.rotation);
        joints.push(RigJoint {
            id: extension.id.clone(),
            reference_name: None,
            parent: Some(extension.parent.clone()),
            core: false,
            endpoint: false,
            position: extension.position,
            rotation: q,
            local_position: rotate(
                conjugate(parent.rotation),
                sub(extension.position, parent.position),
            ),
            local_rotation: qmul(conjugate(parent.rotation), q),
            inverse_bind_matrix: rigid_inverse(extension.position, q),
            position_source: "extensionHint".into(),
        });
    }
    if joints.len() > u16::MAX as usize {
        return Err(RigError::Invalid("Too many extension joints".into()));
    }
    let mut dsl = String::from(
        "<Skeleton id=\"humanoid_rig\" profile=\"motionloom_humanoid_v1\" sourceRig=\"character1_reference_v1\" space=\"3d\">\n",
    );
    for j in &joints {
        let parent = j
            .parent
            .as_ref()
            .map(|p| format!(" parent=\"{p}\""))
            .unwrap_or_default();
        let r = euler(j.local_rotation);
        let p = j.local_position;
        dsl.push_str(&format!(
            "  <Bone id=\"{}\"{parent} position={{[{},{},{}]}} rotation={{[{},{},{}]}} />\n",
            j.id, p[0], p[1], p[2], r[0], r[1], r[2]
        ));
    }
    dsl.push_str("</Skeleton>\n");
    // Parsing emitted text checks the existing DSL contract without adding parser syntax.
    crate::api::parse_graph_script(&format!("<Graph fps=\"24\" duration=\"1s\" size={{[64,64]}}>\n{dsl}<Scene id=\"empty\"><Timeline>\n</Timeline></Scene>\n<Present from=\"empty\" />\n</Graph>" )).map_err(|e|RigError::Invalid(format!("Generated Skeleton DSL: {e}")))?;
    Ok(HumanoidSkeleton {
        standard_id: standard.standard_id.clone(),
        reference: reference.id.clone(),
        geometry_fingerprint: mesh.inspection.geometry_fingerprint.clone(),
        joints,
        inferred_landmarks: inferred,
        skeleton_dsl: dsl,
    })
}
