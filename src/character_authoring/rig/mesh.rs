// =========================================
// =========================================
// src/character_authoring/rig/mesh.rs

use super::{RigError, math::*};
use crate::experimental::load_glb_mesh_data_from_bytes;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshInspectionOptions {
    #[serde(default)]
    pub mesh_nodes: Vec<usize>,
    #[serde(default)]
    pub target_height: Option<f32>,
    #[serde(default = "identity_rotation")]
    pub facing_rotation: Q4,
}
fn identity_rotation() -> Q4 {
    QID
}
impl Default for MeshInspectionOptions {
    fn default() -> Self {
        Self {
            mesh_nodes: vec![],
            target_height: None,
            facing_rotation: QID,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigMeshPart {
    pub source_node: usize,
    pub name: String,
    pub mesh: usize,
    pub primitive: usize,
    pub vertex_start: usize,
    pub vertex_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigMeshInspection {
    pub source_fingerprint: String,
    pub geometry_fingerprint: String,
    pub skeleton_policy: String,
    pub vertex_count: usize,
    pub triangle_count: usize,
    pub bounds_min: V3,
    pub bounds_max: V3,
    pub height: f32,
    pub parts: Vec<RigMeshPart>,
    pub assumptions: Vec<String>,
}

/// Only baked static geometry is authoritative; input skins and animation are discarded.
#[derive(Clone, Debug)]
pub struct RigMesh {
    pub inspection: RigMeshInspection,
    pub positions: Vec<V3>,
    pub normals: Vec<Option<V3>>,
    pub triangles: Vec<[u32; 3]>,
    pub(crate) document: Value,
    pub(crate) binary: Vec<u8>,
}

pub(crate) fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn validate_mesh(mesh: &RigMesh) -> Result<(), RigError> {
    if mesh.positions.len() != mesh.inspection.vertex_count
        || mesh.normals.len() != mesh.positions.len()
        || mesh.triangles.len() != mesh.inspection.triangle_count
        || mesh.positions.iter().flatten().any(|v| !v.is_finite())
        || mesh
            .normals
            .iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite())
        || mesh
            .triangles
            .iter()
            .flatten()
            .any(|i| *i as usize >= mesh.positions.len())
        || fingerprint(&serde_json::to_vec(&(&mesh.positions, &mesh.triangles))?)
            != mesh.inspection.geometry_fingerprint
    {
        return Err(RigError::SourceChanged);
    }
    let mut offset = 0;
    for part in &mesh.inspection.parts {
        if part.vertex_start != offset
            || part.vertex_count > mesh.positions.len().saturating_sub(offset)
            || mesh.document["meshes"]
                .get(part.mesh)
                .and_then(|m| m["primitives"].get(part.primitive))
                .is_none()
        {
            return Err(RigError::Invalid("Invalid mesh part ranges".into()));
        }
        offset += part.vertex_count;
    }
    if offset != mesh.positions.len() {
        return Err(RigError::Invalid(
            "Mesh parts do not cover all vertices".into(),
        ));
    }
    Ok(())
}

// Strip skeletal fields BEFORE invoking MotionLoom's loader, including malformed skin accessors.
pub(crate) fn read_glb(bytes: &[u8]) -> Result<(Value, Vec<u8>), RigError> {
    if bytes.len() < 20 || &bytes[..4] != b"glTF" {
        return Err(RigError::Unsupported("Expected a GLB 2.0 file".into()));
    }
    let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
    if u32_at(4) != 2 || u32_at(8) != bytes.len() {
        return Err(RigError::Invalid("Invalid GLB header".into()));
    }
    let mut offset = 12;
    let mut document = None;
    let mut binary = vec![];
    while offset + 8 <= bytes.len() {
        let n = u32_at(offset);
        let tag = u32_at(offset + 4);
        offset += 8;
        let data = bytes
            .get(
                offset
                    ..offset
                        .checked_add(n)
                        .ok_or_else(|| RigError::Invalid("GLB chunk overflow".into()))?,
            )
            .ok_or_else(|| RigError::Invalid("Truncated GLB chunk".into()))?;
        match tag {
            0x4e4f534a => document = Some(serde_json::from_slice(data)?),
            0x004e4942 => binary = data.to_vec(),
            _ => {}
        }
        offset += n;
    }
    if offset != bytes.len() {
        return Err(RigError::Invalid("Trailing GLB chunk bytes".into()));
    }
    let document: Value = document.ok_or_else(|| RigError::Invalid("Missing GLB JSON".into()))?;
    if !document.is_object() {
        return Err(RigError::Invalid("GLB JSON must be an object".into()));
    }
    Ok((document, binary))
}
pub(crate) fn pack_glb(document: &Value, binary: &[u8]) -> Result<Vec<u8>, RigError> {
    let mut doc = document.clone();
    doc["buffers"] = json!([{"byteLength":binary.len()}]);
    let mut text = serde_json::to_vec(&doc)?;
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut bin = binary.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let mut bytes = vec![];
    for n in [
        0x46546c67,
        2,
        (28 + text.len() + bin.len()) as u32,
        text.len() as u32,
        0x4e4f534a,
    ] {
        bytes.extend(n.to_le_bytes());
    }
    bytes.extend(text);
    bytes.extend((bin.len() as u32).to_le_bytes());
    bytes.extend(0x004e4942u32.to_le_bytes());
    bytes.extend(bin);
    Ok(bytes)
}
fn vec<const N: usize>(v: Option<&Value>, fallback: [f32; N]) -> Result<[f32; N], RigError> {
    let Some(v) = v else { return Ok(fallback) };
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| RigError::Invalid("Invalid node transform vector".into()))?;
    let mut out = fallback;
    for i in 0..N {
        out[i] = a[i]
            .as_f64()
            .ok_or_else(|| RigError::Invalid("Non-numeric transform".into()))?
            as f32;
    }
    if !out.iter().all(|x| x.is_finite()) {
        return Err(RigError::Invalid("Non-finite node transform".into()));
    }
    Ok(out)
}
fn node_matrix(
    nodes: &[Value],
    index: usize,
    parents: &[Option<usize>],
    ambiguous: &BTreeSet<usize>,
    skeletal_nodes: &BTreeSet<usize>,
    seen: &mut BTreeSet<usize>,
) -> Result<M4, RigError> {
    if !seen.insert(index) {
        return Err(RigError::Invalid("Cycle in mesh object ancestry".into()));
    }
    if ambiguous.contains(&index) {
        return Err(RigError::Invalid("Ambiguous mesh object ancestry".into()));
    }
    let n = &nodes[index];
    let local = if skeletal_nodes.contains(&index) {
        ID
    } else if n.get("matrix").is_some() {
        vec(n.get("matrix"), ID)?
    } else {
        trs(
            vec(n.get("translation"), [0.; 3])?,
            vec(n.get("rotation"), QID)?,
            vec(n.get("scale"), [1.; 3])?,
        )
    };
    let global = if let Some(p) = parents[index] {
        mul(
            node_matrix(nodes, p, parents, ambiguous, skeletal_nodes, seen)?,
            local,
        )
    } else {
        local
    };
    seen.remove(&index);
    if dot(
        cross(
            [global[0], global[1], global[2]],
            [global[4], global[5], global[6]],
        ),
        [global[8], global[9], global[10]],
    ) <= 1e-10
    {
        return Err(RigError::Unsupported(
            "Reflected or singular object transforms must be baked before inspection".into(),
        ));
    }
    Ok(global)
}

pub fn inspect_rig_mesh(
    bytes: &[u8],
    options: &MeshInspectionOptions,
) -> Result<RigMesh, RigError> {
    if options
        .target_height
        .is_some_and(|h| !h.is_finite() || h <= 0.)
        || options.facing_rotation.iter().any(|x| !x.is_finite())
        || options.facing_rotation.iter().map(|x| x * x).sum::<f32>() < 1e-10
    {
        return Err(RigError::Invalid(
            "Invalid height or facing quaternion".into(),
        ));
    }
    let (mut doc, binary) = read_glb(bytes)?;
    if doc["buffers"]
        .as_array()
        .is_none_or(|b| b.len() != 1 || b[0].get("uri").is_some())
    {
        return Err(RigError::Unsupported(
            "Only one embedded GLB buffer is supported".into(),
        ));
    }
    if doc["extensionsRequired"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        return Err(RigError::Unsupported(
            "Required compressed/extended geometry must be decoded before rig inspection".into(),
        ));
    }
    if doc["images"].as_array().into_iter().flatten().any(|i| {
        i["uri"]
            .as_str()
            .is_some_and(|uri| !uri.starts_with("data:"))
    }) {
        return Err(RigError::Unsupported(
            "Images must be embedded in the GLB".into(),
        ));
    }
    let nodes = doc["nodes"]
        .as_array()
        .cloned()
        .ok_or_else(|| RigError::Invalid("Missing mesh nodes".into()))?;
    let meshes = doc["meshes"]
        .as_array()
        .cloned()
        .ok_or_else(|| RigError::Invalid("Missing meshes".into()))?;
    let mut parents = vec![None; nodes.len()];
    let mut ambiguous = BTreeSet::new();
    let skeletal_nodes = doc["skins"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["joints"].as_array().into_iter().flatten())
        .filter_map(|i| i.as_u64().map(|i| i as usize))
        .collect::<BTreeSet<_>>();
    for (i, n) in nodes.iter().enumerate() {
        for child in n["children"].as_array().into_iter().flatten() {
            if let Some(c) = child
                .as_u64()
                .map(|c| c as usize)
                .filter(|c| *c < nodes.len())
                && parents[c].replace(i).is_some()
            {
                ambiguous.insert(c);
            }
        }
    }
    let selected = if options.mesh_nodes.is_empty() {
        nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.get("mesh").is_some())
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    } else {
        options.mesh_nodes.clone()
    };
    if selected.is_empty() || selected.iter().collect::<BTreeSet<_>>().len() != selected.len() {
        return Err(RigError::Invalid(
            "Choose at least one unique mesh node".into(),
        ));
    }
    let mut clean_nodes = vec![];
    let mut clean_meshes = vec![];
    let mut parts = vec![];
    let mut offset = 0;
    let mut matrices = vec![];
    for i in selected {
        let node = nodes
            .get(i)
            .ok_or_else(|| RigError::Invalid("Unknown mesh node".into()))?;
        let mi = node["mesh"]
            .as_u64()
            .ok_or_else(|| RigError::Invalid("Selected node has no mesh".into()))?
            as usize;
        let mut mesh = meshes
            .get(mi)
            .cloned()
            .ok_or_else(|| RigError::Invalid("Mesh index out of range".into()))?;
        let transform = node_matrix(
            &nodes,
            i,
            &parents,
            &ambiguous,
            &skeletal_nodes,
            &mut BTreeSet::new(),
        )?;
        let primitives = mesh["primitives"]
            .as_array_mut()
            .ok_or_else(|| RigError::Invalid("Missing primitives".into()))?;
        let mesh_index = clean_meshes.len();
        for (pi, p) in primitives.iter_mut().enumerate() {
            if p["mode"].as_u64().unwrap_or(4) != 4 {
                return Err(RigError::Unsupported(
                    "Rig input must use triangle primitives".into(),
                ));
            }
            p.as_object_mut()
                .ok_or_else(|| RigError::Invalid("Primitive must be an object".into()))?
                .remove("targets");
            let attrs = p["attributes"]
                .as_object_mut()
                .ok_or_else(|| RigError::Invalid("Missing vertex attributes".into()))?;
            attrs.retain(|k, _| {
                !k.starts_with("JOINTS_") && !k.starts_with("WEIGHTS_") && k != "TANGENT"
            });
            let ai = attrs
                .get("POSITION")
                .and_then(Value::as_u64)
                .ok_or_else(|| RigError::Invalid("Missing POSITION".into()))?
                as usize;
            let count = doc["accessors"][ai]["count"]
                .as_u64()
                .ok_or_else(|| RigError::Invalid("Invalid POSITION count".into()))?
                as usize;
            parts.push(RigMeshPart {
                source_node: i,
                name: node["name"].as_str().unwrap_or("mesh").into(),
                mesh: mesh_index,
                primitive: pi,
                vertex_start: offset,
                vertex_count: count,
            });
            offset += count;
        }
        clean_nodes.push(json!({"name":format!("mesh_{mesh_index}"),"mesh":mesh_index}));
        clean_meshes.push(mesh);
        matrices.push(transform);
    }
    // Keep material/image payloads, but no node, extension or animation can reintroduce a rig.
    doc.as_object_mut().unwrap().remove("skins");
    doc.as_object_mut().unwrap().remove("animations");
    doc.as_object_mut().unwrap().remove("extensions");
    doc.as_object_mut().unwrap().remove("extensionsUsed");
    doc["nodes"] = json!(clean_nodes);
    doc["meshes"] = json!(clean_meshes);
    doc["scenes"] = json!([{"nodes":(0..matrices.len()).collect::<Vec<_>>()}]);
    doc["scene"] = json!(0);
    let clean = pack_glb(&doc, &binary)?;
    let mesh = load_glb_mesh_data_from_bytes(Path::new("mesh-only.glb"), &clean)
        .map_err(|e| RigError::Invalid(e.to_string()))?;
    if mesh.positions.len() != offset {
        return Err(RigError::Unsupported(
            "Decoded vertex layout differs from the source".into(),
        ));
    }
    let mut positions = mesh.positions;
    let mut normals = mesh.normals;
    let facing = qnorm(options.facing_rotation);
    for part in &parts {
        let m = matrices[part.mesh];
        let x = [m[0], m[1], m[2]];
        let y = [m[4], m[5], m[6]];
        let z = [m[8], m[9], m[10]];
        let det = dot(x, cross(y, z));
        for v in part.vertex_start..part.vertex_start + part.vertex_count {
            positions[v] = rotate(facing, point(m, positions[v]));
            if let Some(n) = normals[v] {
                normals[v] = Some(unit(rotate(
                    facing,
                    scale(
                        add(
                            add(scale(cross(y, z), n[0]), scale(cross(z, x), n[1])),
                            scale(cross(x, y), n[2]),
                        ),
                        1. / det,
                    ),
                )));
            }
        }
    }
    if positions.iter().flatten().any(|v| !v.is_finite()) {
        return Err(RigError::Invalid("Non-finite mesh position".into()));
    }
    let lo: V3 =
        std::array::from_fn(|k| positions.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min));
    let hi: V3 = std::array::from_fn(|k| {
        positions
            .iter()
            .map(|v| v[k])
            .fold(f32::NEG_INFINITY, f32::max)
    });
    let h = hi[1] - lo[1];
    if h <= 1e-6 {
        return Err(RigError::Invalid("Mesh has no usable height".into()));
    }
    let factor = options.target_height.unwrap_or(h) / h;
    let origin = [(lo[0] + hi[0]) / 2., lo[1], (lo[2] + hi[2]) / 2.];
    for p in &mut positions {
        *p = scale(sub(*p, origin), factor);
    }
    let triangles = mesh.triangles.iter().map(|t| t.indices).collect::<Vec<_>>();
    if triangles
        .iter()
        .flatten()
        .any(|i| *i as usize >= positions.len())
    {
        return Err(RigError::Invalid(
            "Triangle index exceeds vertex count".into(),
        ));
    }
    let geometry_fingerprint = fingerprint(&serde_json::to_vec(&(&positions, &triangles))?);
    Ok(RigMesh{inspection:RigMeshInspection{source_fingerprint:fingerprint(bytes),geometry_fingerprint,skeleton_policy:"ignoreInputSkinsWeightsAnimationsAndJointNames".into(),vertex_count:positions.len(),triangle_count:triangles.len(),bounds_min:scale(sub(lo,origin),factor),bounds_max:scale(sub(hi,origin),factor),height:h*factor,parts,assumptions:vec!["Static mesh object transforms are baked. Declared joint transforms are skipped; skin bind matrices, joint names, weights, animation and morph targets never guide inspection or fitting.".into(),"Facing is caller-supplied; geometry alone cannot always identify front or anatomical left.".into(),"Tangents are omitted after object transforms; positions, normals, UVs, materials and embedded images are retained.".into()]},positions,normals,triangles,document:doc,binary})
}

/// Query actual mesh vertices to refine anatomical hints without consulting a source rig.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigVertexSample {
    pub vertex: usize,
    pub position: V3,
    pub distance: f32,
}

pub fn nearest_rig_vertices(
    mesh: &RigMesh,
    position: V3,
    count: usize,
) -> Result<Vec<RigVertexSample>, RigError> {
    validate_mesh(mesh)?;
    if !position.iter().all(|v| v.is_finite()) || count == 0 || count > 256 {
        return Err(RigError::Invalid(
            "Vertex query requires a finite point and count in 1..=256".into(),
        ));
    }
    let mut vertices = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| RigVertexSample {
            vertex: i,
            position: *p,
            distance: length(sub(*p, position)),
        })
        .collect::<Vec<_>>();
    vertices.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then(a.vertex.cmp(&b.vertex))
    });
    vertices.truncate(count);
    Ok(vertices)
}
