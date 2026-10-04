// =========================================
// =========================================
// src/character_authoring/rig/verify.rs

use super::{math::*, mesh::fingerprint, *};
use crate::experimental::{
    WorldAction, WorldCamera, WorldGraph, WorldNode, WorldTime, diagnose_world_actor_pose,
    load_glb_mesh_data_from_bytes,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigActionTest {
    pub library_source: String,
    pub action_id: String,
    #[serde(default = "phases")]
    pub phases: Vec<f32>,
}
fn phases() -> Vec<f32> {
    vec![0.2, 0.5, 0.8]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RigVerificationOptions {
    #[serde(default)]
    pub weight_checks: RigWeightCheckOptions,
    #[serde(default)]
    pub head_checks: RigHeadCheckOptions,
    #[serde(default)]
    pub actions: Vec<RigActionTest>,
    #[serde(default = "rotation_limit")]
    pub maximum_rotation_error_degrees: f32,
    #[serde(default = "stretch_limit")]
    pub maximum_edge_stretch: f32,
    #[serde(default = "fraction_limit")]
    pub maximum_severe_edge_fraction: f32,
}
fn rotation_limit() -> f32 {
    10.
}
fn stretch_limit() -> f32 {
    4.
}
fn fraction_limit() -> f32 {
    0.01
}
impl Default for RigVerificationOptions {
    fn default() -> Self {
        Self {
            weight_checks: RigWeightCheckOptions::default(),
            head_checks: RigHeadCheckOptions::default(),
            actions: vec![],
            maximum_rotation_error_degrees: rotation_limit(),
            maximum_edge_stretch: stretch_limit(),
            maximum_severe_edge_fraction: fraction_limit(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BindingStatus {
    Pass,
    Fail,
    Inconclusive,
}
impl Default for BindingStatus {
    fn default() -> Self {
        Self::Inconclusive
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingIssue {
    pub code: String,
    pub bone: Option<String>,
    pub action: Option<String>,
    pub phase: Option<f32>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigDeformationEdge {
    pub vertices: [u32; 2],
    pub rest_length: f32,
    pub posed_length: f32,
    pub length_ratio: f32,
    #[serde(default)]
    pub vertex_evidence: Vec<RigVertexWeightEvidence>,
    #[serde(default)]
    pub likely_causes: Vec<String>,
    #[serde(default)]
    pub next_step: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigActionSample {
    pub action: String,
    pub phase: f32,
    pub evaluation_frame: u32,
    pub builtin_probe: bool,
    pub evaluated_nodes: usize,
    pub maximum_rotation_error_degrees: f32,
    pub per_bone_rotation_error_degrees: BTreeMap<String, f32>,
    pub reference_errors: BTreeMap<String, f32>,
    pub maximum_edge_stretch: f32,
    pub severe_edge_fraction: f32,
    pub worst_deformation_edges: Vec<RigDeformationEdge>,
    pub maximum_vertex_motion: f32,
    #[serde(default)]
    pub head_rigidity: Option<RigHeadRigiditySample>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidBindingVerification {
    pub schema_version: u32,
    pub standard_id: String,
    pub binding_fingerprint: String,
    pub test_suite_fingerprint: String,
    pub core_nodes: usize,
    pub extension_nodes: usize,
    pub structure_status: BindingStatus,
    pub bind_pose_status: BindingStatus,
    pub direction_status: BindingStatus,
    pub deformation_status: BindingStatus,
    #[serde(default)]
    pub anatomy_status: BindingStatus,
    #[serde(default)]
    pub weight_continuity_status: BindingStatus,
    #[serde(default)]
    pub weight_diagnostics: Option<RigWeightDiagnostics>,
    #[serde(default)]
    pub constraint_status: BindingStatus,
    #[serde(default)]
    pub head_rigidity_status: BindingStatus,
    #[serde(default)]
    pub distinct_head_moving_samples: usize,
    pub status: BindingStatus,
    pub accepted: bool,
    pub raw_bind_pose_maximum_error: f32,
    pub unconfirmed_landmarks: Vec<String>,
    pub distinct_moving_action_samples: usize,
    pub samples: Vec<RigActionSample>,
    pub issues: Vec<BindingIssue>,
    pub limitations: Vec<String>,
}

fn issue(
    code: &str,
    bone: Option<&str>,
    action: Option<&str>,
    phase: Option<f32>,
    message: impl Into<String>,
) -> BindingIssue {
    BindingIssue {
        code: code.into(),
        bone: bone.map(str::to_string),
        action: action.map(str::to_string),
        phase,
        message: message.into(),
    }
}

// Convert parsed Action nodes, preserving channels and interpolation, into the existing CPU evaluator.
fn action_value(action: &crate::ActionNode) -> Result<WorldAction, RigError> {
    let mut value = serde_json::to_value(action)?;
    fn snake(value: &mut Value) {
        match value {
            Value::Object(object) => {
                let old = std::mem::take(object);
                for (key, mut v) in old {
                    snake(&mut v);
                    let mut k = String::new();
                    for c in key.chars() {
                        if c.is_ascii_uppercase() {
                            k.push('_');
                            k.push(c.to_ascii_lowercase());
                        } else {
                            k.push(c);
                        }
                    }
                    object.insert(k, v);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(snake),
            _ => {}
        }
    }
    snake(&mut value);
    value["intent"] = Value::Null;
    if value["skeleton"].is_null() {
        value["skeleton"] = json!("humanoid_v1");
    }
    Ok(serde_json::from_value(value)?)
}
fn world_graph(
    binding: &HumanoidBinding,
    action: Option<WorldAction>,
) -> Result<WorldGraph, RigError> {
    let maps = binding
        .skeleton
        .joints
        .iter()
        .map(|j| json!({"from":j.id,"to":j.id}))
        .collect::<Vec<_>>();
    let mut profile = serde_json::to_value(&binding.profile)?;
    // Typed World IR uses snake_case; no legacy World DSL is emitted or parsed.
    let axes = profile["boneAxisMap"]["axes"]
        .as_array_mut()
        .map(|a| {
            a.iter_mut()
                .map(|axis| {
                    let mut a = axis.clone();
                    for (camel, snake) in [
                        ("restForward", "rest_forward"),
                        ("restSide", "rest_side"),
                        ("restTwist", "rest_twist"),
                        ("restBend", "rest_bend"),
                        ("restTurn", "rest_turn"),
                    ] {
                        a[snake] = a[camel].clone();
                    }
                    a
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let model_profile = serde_json::from_value(
        json!({"id":"humanoid_profile","model":"rigged_model","preset":"humanoid_v1","retarget":{"preset":"humanoid_v1","maps":maps},"bone_axis_map":{"axes":axes}}),
    )?;
    let actor = serde_json::from_value(
        json!({"id":"actor","model":"rigged_model","path_style":"relative","hide_meshes":[],"hide_materials":[],"profile":"humanoid_profile","rig":null,"retarget":null,"x":"0","y":"0","z":"0","yaw":"0","pitch":"0","roll":"0","scale":"1","opacity":"1","material":null,"play":null,"plays":[]}),
    )?;
    let duration = action.as_ref().map_or(1000, |a| a.duration_ms);
    let applies=action.as_ref().map(|a|serde_json::from_value(json!({"target":"actor","action":a.id,"at_ms":0,"loop":false,"weight":"1","duration_ms":a.duration_ms,"root_motion":"in_place"}))).transpose()?.into_iter().collect();
    Ok(WorldGraph {
        id: None,
        version: None,
        fps: 120.,
        duration_ms: duration,
        duration_explicit: true,
        size: (64, 64),
        render_size: None,
        model_profiles: vec![model_profile],
        worlds: vec![WorldNode::new(
            "verification",
            WorldCamera::default(),
            vec![actor],
        )],
        retargets: vec![],
        actions: action.into_iter().collect(),
        apply_actions: applies,
        animation_assets: vec![],
        constraints: vec![],
        attachments: vec![],
        lighting: Default::default(),
        present: crate::experimental::WorldPresent {
            from: "verification".into(),
        },
    })
}
fn matrices(
    graph: &WorldGraph,
    mesh: &crate::experimental::GlbMeshData,
    phase: f32,
) -> Result<BTreeMap<String, M4>, RigError> {
    let frame = evaluation_frame(graph, phase);
    let pose = diagnose_world_actor_pose(
        graph,
        mesh,
        "actor",
        WorldTime {
            frame,
            fps: graph.fps,
            duration_ms: graph.duration_ms,
        },
    )
    .map_err(|e| RigError::Evaluation(e.to_string()))?;
    Ok(pose
        .joints
        .into_iter()
        .map(|j| (j.node_name, j.model_global_matrix))
        .collect())
}
fn evaluation_frame(graph: &WorldGraph, phase: f32) -> u32 {
    (phase * graph.duration_ms as f32 / 1000. * graph.fps).round() as u32
}
fn deformed_vertices(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    matrices: &BTreeMap<String, M4>,
) -> Result<Vec<V3>, RigError> {
    // A joint's skin matrix is identical for all vertices in this frame.
    let skin_matrices = binding
        .skeleton
        .joints
        .iter()
        .map(|j| {
            let matrix = matrices
                .get(&j.id)
                .ok_or_else(|| RigError::Evaluation(format!("Missing evaluated node {}", j.id)))?;
            Ok(mul(*matrix, j.inverse_bind_matrix))
        })
        .collect::<Result<Vec<_>, RigError>>()?;
    mesh.positions
        .iter()
        .enumerate()
        .map(|(v, p)| {
            let mut target = [0.; 3];
            for (i, w) in binding.joints[v].iter().zip(binding.weights[v]) {
                target = add(target, scale(point(skin_matrices[*i as usize], *p), w));
            }
            if target.iter().any(|v| !v.is_finite()) {
                return Err(RigError::Evaluation("Non-finite deformed mesh".into()));
            }
            Ok(target)
        })
        .collect()
}
fn probes(profile: &crate::ModelProfileNode) -> Vec<RigActionTest> {
    let mut tests = vec![];
    for joint in &humanoid_rig_standard().references[0].joints {
        if joint.endpoint || joint.id == "root" {
            continue;
        }
        let axis = profile
            .bone_axis_map
            .as_ref()
            .and_then(|m| m.axes.iter().find(|a| a.bone == joint.id));
        let channels = axis
            .map(|a| {
                [
                    ("forward", &a.forward),
                    ("side", &a.side),
                    ("twist", &a.twist),
                    ("bend", &a.bend),
                    ("turn", &a.turn),
                ]
                .into_iter()
                .filter(|(_, v)| v.is_some())
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for channel in channels {
            let id = format!("probe_{}_{}", joint.id, channel);
            tests.push(RigActionTest{action_id:id.clone(),library_source:format!("<ActionLibrary>\n<Action id=\"{id}\" skeleton=\"humanoid_v1\" duration=\"1s\">\n<Pose t=\"0s\"><Bone id=\"{}\" {channel}=\"0\" /></Pose>\n<Pose t=\"1s\"><Bone id=\"{}\" {channel}=\"20\" /></Pose>\n</Action>\n</ActionLibrary>",joint.id,joint.id),phases:vec![1.]});
        }
    }
    tests
}

/// Certify structure, raw bind reconstruction, semantic direction and actual weighted geometry.
/// Missing evidence remains inconclusive; this does not certify contacts, cloth or collisions.
pub fn verify_humanoid_binding(
    mesh: &RigMesh,
    binding: &HumanoidBinding,
    options: &RigVerificationOptions,
) -> Result<HumanoidBindingVerification, RigError> {
    super::mesh::validate_mesh(mesh)?;
    if !options.maximum_rotation_error_degrees.is_finite()
        || options.maximum_rotation_error_degrees <= 0.
        || options.maximum_rotation_error_degrees > 45.
        || !options.maximum_edge_stretch.is_finite()
        || options.maximum_edge_stretch <= 1.
        || !options.maximum_severe_edge_fraction.is_finite()
        || !(0. ..=0.1).contains(&options.maximum_severe_edge_fraction)
    {
        return Err(RigError::Invalid("Invalid verification tolerances".into()));
    }
    if !options
        .head_checks
        .maximum_rigid_deviation_height_fraction
        .is_finite()
        || !(1e-7..=0.001).contains(&options.head_checks.maximum_rigid_deviation_height_fraction)
        || !options.head_checks.maximum_edge_relative_error.is_finite()
        || !(1e-5..=0.01).contains(&options.head_checks.maximum_edge_relative_error)
    {
        return Err(RigError::Invalid("Invalid head rigidity tolerances".into()));
    }
    let head_gated = options.head_checks.required
        || binding
            .binding_options
            .as_ref()
            .is_some_and(|o| o.constraints.iter().any(|c| c.joint() == "head"));
    let standard = humanoid_rig_standard();
    let definitions = &standard.references[0].joints;
    let mut report = HumanoidBindingVerification {
        schema_version: 1,
        standard_id: standard.standard_id.clone(),
        binding_fingerprint: binding.fingerprint.clone(),
        test_suite_fingerprint: fingerprint(&serde_json::to_vec(&(standard, options))?),
        core_nodes: binding.skeleton.joints.iter().filter(|j| j.core).count(),
        extension_nodes: binding.skeleton.joints.iter().filter(|j| !j.core).count(),
        structure_status: BindingStatus::Pass,
        bind_pose_status: BindingStatus::Pass,
        direction_status: BindingStatus::Pass,
        deformation_status: BindingStatus::Pass,
        anatomy_status: BindingStatus::Inconclusive,
        weight_continuity_status: BindingStatus::Inconclusive,
        weight_diagnostics: None,
        constraint_status: BindingStatus::Inconclusive,
        head_rigidity_status: BindingStatus::Inconclusive,
        distinct_head_moving_samples: 0,
        status: BindingStatus::Inconclusive,
        accepted: false,
        raw_bind_pose_maximum_error: 0.,
        unconfirmed_landmarks: binding.skeleton.inferred_landmarks.clone(),
        distinct_moving_action_samples: 0,
        samples: vec![],
        issues: vec![],
        limitations: vec![
            "Model-space checks use MotionLoom's production CPU pose evaluator. Scene contacts, ground IK, cloth, collisions and anatomical volume are not certified.".into(),
            "Action phases are sampled at 120 fps; requested phases and actual evaluation frames are reported.".into(),
            "Extension nodes follow their parents; extension-specific animation is outside the humanoid core certificate.".into(),
        ],
    };
    let actual_fingerprint = super::binding::binding_fingerprint(binding)?;
    let mut ids = BTreeSet::new();
    let joints = &binding.skeleton.joints;
    if actual_fingerprint != binding.fingerprint
        || binding.skeleton.geometry_fingerprint != mesh.inspection.geometry_fingerprint
        || binding.skeleton.standard_id != standard.standard_id
        || binding
            .skeleton
            .joints
            .iter()
            .any(|j| !ids.insert(j.id.clone()))
        || definitions.iter().any(|d| {
            joints
                .iter()
                .find(|j| j.id == d.id)
                .is_none_or(|j| !j.core || j.parent != d.parent || j.endpoint != d.endpoint)
        })
        || report.core_nodes != 65
    {
        report.structure_status = BindingStatus::Fail;
        report.status = BindingStatus::Fail;
        report.issues.push(issue(
            "CORE_STRUCTURE_OR_FINGERPRINT_MISMATCH",
            None,
            None,
            None,
            "The 65-node hierarchy or immutable binding fingerprint does not match the standard.",
        ));
        return Ok(report);
    }
    if binding.weights.len() != mesh.positions.len() || binding.joints.len() != mesh.positions.len()
    {
        report.bind_pose_status = BindingStatus::Fail;
        report.status = BindingStatus::Fail;
        report.issues.push(issue(
            "SKIN_VERTEX_COUNT_MISMATCH",
            None,
            None,
            None,
            "Skin arrays must match mesh vertices.",
        ));
        return Ok(report);
    }
    for (v, p) in mesh.positions.iter().enumerate() {
        let mut reconstructed = [0.; 3];
        let weights = binding.weights[v];
        if weights.iter().any(|w| !w.is_finite() || *w < 0.)
            || (weights.iter().sum::<f32>() - 1.).abs() > 1e-4
            || binding.joints[v]
                .iter()
                .any(|i| *i as usize >= joints.len())
        {
            report.bind_pose_status = BindingStatus::Fail;
            break;
        }
        for (i, w) in binding.joints[v].iter().zip(weights) {
            let j = &joints[*i as usize];
            let q = j.rotation;
            if q.iter()
                .chain(j.position.iter())
                .chain(j.inverse_bind_matrix.iter())
                .any(|x| !x.is_finite())
                || (q.iter().map(|x| x * x).sum::<f32>() - 1.).abs() > 1e-4
            {
                report.bind_pose_status = BindingStatus::Fail;
                break;
            }
            reconstructed = add(
                reconstructed,
                scale(
                    point(mul(trs(j.position, q, [1.; 3]), j.inverse_bind_matrix), *p),
                    w,
                ),
            );
        }
        report.raw_bind_pose_maximum_error = report
            .raw_bind_pose_maximum_error
            .max(length(sub(*p, reconstructed)));
    }
    if report.raw_bind_pose_maximum_error > mesh.inspection.height * 1e-5 {
        report.bind_pose_status = BindingStatus::Fail;
    }
    if report.bind_pose_status == BindingStatus::Fail {
        report.status = BindingStatus::Fail;
        report.issues.push(issue(
            "INVALID_WEIGHTS_OR_BIND_POSE",
            None,
            None,
            None,
            "Weights, joint matrices or raw bind reconstruction are invalid.",
        ));
        return Ok(report);
    }
    // Validate every serialized local transform, including unweighted root and endpoint nodes.
    let mut globals = BTreeMap::new();
    for joint in joints {
        let local = trs(joint.local_position, joint.local_rotation, [1.; 3]);
        let global = if let Some(parent) = &joint.parent {
            let Some(parent) = globals.get(parent) else {
                report.structure_status = BindingStatus::Fail;
                break;
            };
            mul(*parent, local)
        } else {
            local
        };
        if joint
            .position
            .iter()
            .chain(joint.rotation.iter())
            .chain(joint.local_position.iter())
            .chain(joint.local_rotation.iter())
            .chain(joint.inverse_bind_matrix.iter())
            .any(|v| !v.is_finite())
            || (joint.local_rotation.iter().map(|v| v * v).sum::<f32>() - 1.).abs() > 1e-4
            || length(sub(position(global), joint.position)) > mesh.inspection.height * 1e-5
            || crate::api::quaternion_angular_error_deg(quat(global), joint.rotation) > 0.1
            || mul(global, joint.inverse_bind_matrix)
                .iter()
                .zip(ID)
                .any(|(a, b)| (*a - b).abs() > 1e-4)
        {
            report.structure_status = BindingStatus::Fail;
            break;
        }
        globals.insert(joint.id.clone(), global);
    }
    let mappings = binding.profile.retarget.as_ref().map(|r| &r.maps);
    if report.structure_status == BindingStatus::Fail
        || mappings.is_none_or(|maps| {
            maps.len() != joints.len()
                || joints.iter().any(|j| {
                    maps.iter()
                        .filter(|m| m.from == j.id && m.to == j.id)
                        .count()
                        != 1
                })
        })
    {
        report.structure_status = BindingStatus::Fail;
        report.status = BindingStatus::Fail;
        report.issues.push(issue("LOCAL_TRANSFORM_OR_MAPPING_MISMATCH",None,None,None,"All 65 core nodes and extensions require consistent local/global transforms and one-to-one mappings."));
        return Ok(report);
    }
    // Static region and adjacency evidence is independent of the action renderer.
    let diagnostics = inspect_rig_weights(mesh, binding, &options.weight_checks)?;
    report.anatomy_status = diagnostics.anatomy_status;
    report.weight_continuity_status = diagnostics.continuity_status;
    for (code, count) in [
        (
            "ANATOMICAL_REGION_MISMATCH",
            diagnostics.anatomical_conflict_count,
        ),
        (
            "ADJACENT_INFLUENCE_DISCONTINUITY",
            diagnostics.discontinuity_count,
        ),
        (
            "UNRESOLVED_WEIGHT_VERTICES",
            diagnostics.pending_vertices.len(),
        ),
    ] {
        if count > 0 {
            report.issues.push(issue(
                code,
                None,
                None,
                None,
                format!(
                    "{count} affected items; inspect weightDiagnostics before refining regions."
                ),
            ));
        }
    }
    report.constraint_status = diagnostics.constraint_status;
    if diagnostics.constraint_violation_count > 0 {
        report.issues.push(issue("RIGID_JOINT_WEIGHT_MISMATCH", None, None, None, "Inspect weightDiagnostics.constraintViolations for selected vertices and named weights."));
    }
    report.weight_diagnostics = Some(diagnostics);
    let weight_context = super::weights::WeightContext::new(mesh, binding, &options.weight_checks);
    let bytes = export_rig_glb(mesh, binding)?;
    let candidate_mesh = load_glb_mesh_data_from_bytes(Path::new("candidate.glb"), &bytes)
        .map_err(|e| RigError::Evaluation(e.to_string()))?;
    let neutral = matrices(&world_graph(binding, None)?, &candidate_mesh, 0.)?;
    let neutral_vertices = deformed_vertices(mesh, binding, &neutral)?;
    let mut references = vec![];
    for reference in &standard.references {
        let request = RigBuildRequest {
            expected_geometry_fingerprint: mesh.inspection.geometry_fingerprint.clone(),
            reference: reference.id.clone(),
            landmarks: reference
                .joints
                .iter()
                .map(|j| (j.id.clone(), scale(j.position, mesh.inspection.height)))
                .collect(),
            rotations: reference
                .joints
                .iter()
                .map(|j| (j.id.clone(), j.rotation))
                .collect(),
            extensions: vec![],
        };
        let ref_binding = bind_humanoid_skin(
            mesh,
            build_humanoid_skeleton(mesh, &request)?,
            &RigBindingOptions {
                smoothing_iterations: 0,
                ..Default::default()
            },
        )?;
        let ref_mesh = load_glb_mesh_data_from_bytes(
            Path::new("reference.glb"),
            &export_rig_glb(mesh, &ref_binding)?,
        )
        .map_err(|e| RigError::Evaluation(e.to_string()))?;
        let rest = matrices(&world_graph(&ref_binding, None)?, &ref_mesh, 0.)?;
        references.push((reference.id.clone(), ref_binding, ref_mesh, rest));
    }
    let builtin = probes(&references[0].1.profile);
    let builtin_count = builtin.len();
    let tests = builtin
        .into_iter()
        .chain(options.actions.clone())
        .collect::<Vec<_>>();
    // Preserve first-seen edge order while caching immutable connectivity and rest lengths once.
    let mut seen_edges = BTreeSet::new();
    let mut surface_edges = vec![];
    for triangle in &mesh.triangles {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (a, b) = (triangle[a] as usize, triangle[b] as usize);
            let pair = [a.min(b) as u32, a.max(b) as u32];
            if seen_edges.insert(pair) {
                let rest = length(sub(mesh.positions[a], mesh.positions[b]));
                if rest >= mesh.inspection.height * 1e-6 {
                    surface_edges.push((a, b, pair, rest));
                }
            }
        }
    }
    let mut moving_samples = BTreeSet::new();
    let mut head_moving_samples = BTreeSet::new();
    let head_edges = super::constraints::rigid_head_edges(mesh, binding);
    if head_gated && !head_edges.is_empty() {
        report.head_rigidity_status = BindingStatus::Pass;
    }
    for (test_index, test) in tests.iter().enumerate() {
        if test.phases.is_empty()
            || test
                .phases
                .iter()
                .any(|p| !p.is_finite() || !(0. ..=1.).contains(p))
        {
            return Err(RigError::Invalid(
                "Each action requires finite phases in 0..=1".into(),
            ));
        }
        let actions = crate::api::parse_action_library_document(&test.library_source)
            .map_err(|e| RigError::Invalid(e.to_string()))?;
        let action = actions
            .iter()
            .find(|a| a.id == test.action_id)
            .ok_or_else(|| RigError::Missing(test.action_id.clone()))?;
        if action.poses.is_empty() || action.duration_ms == 0 || !action.iks.is_empty() {
            return Err(RigError::Unsupported("Verification accepts sampled Pose actions; authored IK certification requires Scene evaluation".into()));
        }
        let action = action_value(action)?;
        let action_fingerprint = fingerprint(&serde_json::to_vec(&action)?);
        let graph = world_graph(binding, Some(action.clone()))?;
        for phase in &test.phases {
            let current = matrices(&graph, &candidate_mesh, *phase)?;
            for joint in joints {
                if current
                    .get(&joint.id)
                    .is_none_or(|m| m.iter().any(|v| !v.is_finite()))
                {
                    return Err(RigError::Evaluation(format!(
                        "Missing or non-finite evaluated node {}",
                        joint.id
                    )));
                }
            }
            let mut errors = BTreeMap::new();
            let mut ref_errors = BTreeMap::new();
            for (name, ref_binding, ref_mesh, ref_rest) in &references {
                let posed = matrices(
                    &world_graph(ref_binding, Some(action.clone()))?,
                    ref_mesh,
                    *phase,
                )?;
                let mut max = 0_f32;
                for d in definitions {
                    let c = current.get(&d.id).ok_or_else(|| {
                        RigError::Evaluation(format!("Missing candidate node {}", d.id))
                    })?;
                    let r = posed.get(&d.id).ok_or_else(|| {
                        RigError::Evaluation(format!("Missing reference node {}", d.id))
                    })?;
                    let cd = qmul(quat(*c), conjugate(quat(neutral[&d.id])));
                    let rd = qmul(quat(*r), conjugate(quat(ref_rest[&d.id])));
                    let error = crate::api::quaternion_angular_error_deg(cd, rd);
                    max = max.max(error);
                    errors
                        .entry(d.id.clone())
                        .and_modify(|v: &mut f32| *v = v.max(error))
                        .or_insert(error);
                }
                ref_errors.insert(name.clone(), max);
            }
            if test_index < builtin_count {
                let parts = test.action_id.trim_start_matches("probe_");
                let (id, _channel) = parts.rsplit_once('_').unwrap();
                let movement =
                    crate::api::quaternion_angular_error_deg(quat(current[id]), quat(neutral[id]));
                if movement < 5. {
                    report.direction_status = BindingStatus::Fail;
                    report.issues.push(issue("SEMANTIC_CHANNEL_INACTIVE",Some(id),Some(&test.action_id),Some(*phase),"A +20 degree calibration probe did not produce the required joint rotation."));
                }
            }
            let max_error = errors.values().copied().fold(0., f32::max);
            if max_error > options.maximum_rotation_error_degrees {
                report.direction_status = BindingStatus::Fail;
                for (id, error) in &errors {
                    if *error > options.maximum_rotation_error_degrees {
                        report.issues.push(issue("REFERENCE_MOTION_DIRECTION_MISMATCH",Some(id),Some(&test.action_id),Some(*phase),format!("Model-space rotation delta differs from a CC0 reference by {error:.2} degrees.")));
                    }
                }
            }
            let posed = deformed_vertices(mesh, binding, &current)?;
            let head_rigidity = if head_gated {
                let sample = super::constraints::head_rigidity_sample(
                    mesh,
                    binding,
                    &posed,
                    &current,
                    &neutral,
                    &options.head_checks,
                    options.weight_checks.maximum_examples,
                    &head_edges,
                )?;
                if sample.status == BindingStatus::Fail {
                    report.head_rigidity_status = BindingStatus::Fail;
                    report.issues.push(issue("HEAD_RIGIDITY_FAILED", Some("head"), Some(&test.action_id), Some(*phase),
                        format!("Head deviation {:.8}; edge relative error {:.6}. Inspect samples.headRigidity.failures.", sample.maximum_rigid_deviation, sample.maximum_edge_relative_error)));
                } else if sample.status == BindingStatus::Inconclusive
                    && report.head_rigidity_status != BindingStatus::Fail
                {
                    report.head_rigidity_status = BindingStatus::Inconclusive;
                }
                if sample.head_rotation_relative_to_parent_degrees >= 5. {
                    head_moving_samples
                        .insert((action_fingerprint.clone(), evaluation_frame(&graph, *phase)));
                }
                Some(sample)
            } else {
                None
            };
            let max_motion = posed
                .iter()
                .zip(&neutral_vertices)
                .map(|(p, n)| length(sub(*p, *n)))
                .fold(0., f32::max);
            let mut edges = 0;
            let mut severe = 0;
            let mut max_stretch = 1_f32;
            let mut worst_edges = vec![];
            for &(a, b, pair, rest) in &surface_edges {
                let posed_length = length(sub(posed[a], posed[b]));
                let ratio = posed_length / rest;
                max_stretch = max_stretch.max(ratio);
                severe += usize::from(!(0.5..=2.).contains(&ratio));
                if !(0.5..=2.).contains(&ratio) {
                    worst_edges.push(RigDeformationEdge {
                        vertices: pair,
                        rest_length: rest,
                        posed_length,
                        length_ratio: ratio,
                        vertex_evidence: vec![],
                        likely_causes: vec![],
                        next_step: String::new(),
                    });
                }
                edges += 1;
            }
            let severity =
                |e: &RigDeformationEdge| e.length_ratio.max(1. / e.length_ratio.max(1e-8));
            // Compression examples must not hide the edge that caused the maximum-stretch gate.
            let maximum_stretch_edge = worst_edges
                .iter()
                .max_by(|a, b| a.length_ratio.total_cmp(&b.length_ratio))
                .cloned();
            worst_edges.sort_by(|a, b| severity(b).total_cmp(&severity(a)));
            worst_edges.truncate(8);
            if let Some(edge) = maximum_stretch_edge
                && edge.length_ratio > 2.
                && !worst_edges.iter().any(|e| e.vertices == edge.vertices)
            {
                if worst_edges.len() == 8 {
                    worst_edges.pop();
                }
                worst_edges.push(edge);
            }
            // Enrich only retained examples, keeping large mesh/action batches bounded.
            for edge in &mut worst_edges {
                let (evidence, causes) =
                    super::weights::explain_deformation_edge(&weight_context, edge.vertices);
                edge.vertex_evidence = evidence.into();
                edge.likely_causes = causes;
                edge.next_step = "Review vertex/region evidence, refine the candidate's regions or landmarks, regenerate weights and verify the new fingerprint.".into();
            }
            let fraction = severe as f32 / edges.max(1) as f32;
            if max_stretch > options.maximum_edge_stretch
                || fraction > options.maximum_severe_edge_fraction
            {
                report.deformation_status = BindingStatus::Fail;
                report.issues.push(issue(
                    "EXCESSIVE_SKIN_STRETCH",
                    None,
                    Some(&test.action_id),
                    Some(*phase),
                    format!(
                        "Maximum edge stretch {max_stretch:.2}; severe edge fraction {fraction:.4}."
                    ),
                ));
            }
            if test_index >= builtin_count && max_motion >= mesh.inspection.height * 0.01 {
                moving_samples
                    .insert((action_fingerprint.clone(), evaluation_frame(&graph, *phase)));
            }
            report.samples.push(RigActionSample {
                action: test.action_id.clone(),
                phase: *phase,
                evaluation_frame: evaluation_frame(&graph, *phase),
                builtin_probe: test_index < builtin_count,
                evaluated_nodes: joints.len(),
                maximum_rotation_error_degrees: max_error,
                per_bone_rotation_error_degrees: errors,
                reference_errors: ref_errors,
                maximum_edge_stretch: max_stretch,
                severe_edge_fraction: fraction,
                worst_deformation_edges: worst_edges,
                maximum_vertex_motion: max_motion,
                head_rigidity,
            });
        }
    }
    report.distinct_head_moving_samples = head_moving_samples.len();
    if head_gated && head_moving_samples.len() < 2 {
        if report.head_rigidity_status != BindingStatus::Fail {
            report.head_rigidity_status = BindingStatus::Inconclusive;
        }
        report.issues.push(issue("HEAD_MOTION_EVIDENCE_INCOMPLETE", Some("head"), None, None, "Provide a reviewed head surface and at least two distinct frames rotating head relative to its parent by five degrees."));
    }
    report.distinct_moving_action_samples = moving_samples.len();
    if moving_samples.len() < 3 {
        report.issues.push(issue("ACTION_EVIDENCE_INCOMPLETE",None,None,None,"Provide at least three distinct Action Library frames, each moving vertices at least 1% of mesh height relative to the calibrated neutral pose, in addition to built-in calibration probes."));
    }
    if !report.unconfirmed_landmarks.is_empty() {
        report.issues.push(issue("ANATOMICAL_HINTS_UNCONFIRMED",None,None,None,"Reference proportion priors must be replaced with explicit mesh-space landmarks before accepting a binding."));
    }
    report.status = if report.direction_status == BindingStatus::Fail
        || report.deformation_status == BindingStatus::Fail
        || report.anatomy_status == BindingStatus::Fail
        || report.weight_continuity_status == BindingStatus::Fail
        || report.constraint_status == BindingStatus::Fail
        || (head_gated && report.head_rigidity_status == BindingStatus::Fail)
    {
        BindingStatus::Fail
    } else if moving_samples.len() < 3
        || !report.unconfirmed_landmarks.is_empty()
        || report.anatomy_status == BindingStatus::Inconclusive
        || (head_gated && report.head_rigidity_status == BindingStatus::Inconclusive)
    {
        BindingStatus::Inconclusive
    } else {
        BindingStatus::Pass
    };
    report.accepted = report.status == BindingStatus::Pass;
    Ok(report)
}
