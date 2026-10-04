// =========================================
// =========================================
// src/scene/head_swap.rs

//! Platform-neutral GLB composition for a skinned UAL mannequin and a static head export.

use crate::dsl::HeadSwapAssetNode;
use serde_json::{Value, json};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HeadSwapError {
    #[error("invalid head-swap GLB: {0}")]
    Invalid(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

type SwapResult<T> = Result<T, HeadSwapError>;

struct Glb {
    doc: Value,
    bin: Vec<u8>,
}

impl Glb {
    fn parse(bytes: &[u8]) -> SwapResult<Self> {
        if bytes.len() < 28
            || &bytes[..4] != b"glTF"
            || u32_at(bytes, 4)? != 2
            || u32_at(bytes, 8)? as usize != bytes.len()
        {
            return Err(HeadSwapError::Invalid("not a GLB 2.0 file".into()));
        }
        let json_len = u32_at(bytes, 12)? as usize;
        if bytes.get(16..20) != Some(b"JSON".as_slice()) || 20 + json_len + 8 > bytes.len() {
            return Err(HeadSwapError::Invalid("missing JSON chunk".into()));
        }
        let bin_at = 20 + json_len;
        let bin_len = u32_at(bytes, bin_at)? as usize;
        if bytes.get(bin_at + 4..bin_at + 8) != Some(b"BIN\0".as_slice())
            || bin_at + 8 + bin_len > bytes.len()
        {
            return Err(HeadSwapError::Invalid("missing BIN chunk".into()));
        }
        Ok(Self {
            doc: serde_json::from_slice(&bytes[20..bin_at])?,
            bin: bytes[bin_at + 8..bin_at + 8 + bin_len].to_vec(),
        })
    }

    fn accessor(&self, index: usize) -> SwapResult<Vec<Vec<f32>>> {
        let accessor = &self.doc["accessors"][index];
        let view = &self.doc["bufferViews"][accessor["bufferView"]
            .as_u64()
            .ok_or_else(|| invalid("accessor bufferView"))?
            as usize];
        let count = usize_field(accessor, "count")?;
        let width = match accessor["type"].as_str().unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            _ => return Err(invalid("accessor type")),
        };
        let component = usize_field(accessor, "componentType")?;
        let size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            _ => return Err(invalid("component type")),
        };
        let base = view["byteOffset"].as_u64().unwrap_or(0) as usize
            + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = view["byteStride"].as_u64().unwrap_or((size * width) as u64) as usize;
        let normalized = accessor["normalized"].as_bool().unwrap_or(false);
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            let mut row = Vec::with_capacity(width);
            for j in 0..width {
                let at = base + i * stride + j * size;
                let bytes = self
                    .bin
                    .get(at..at + size)
                    .ok_or_else(|| invalid("accessor outside BIN chunk"))?;
                let value = match component {
                    5120 => {
                        let v = bytes[0] as i8 as f32;
                        if normalized { (v / 127.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = bytes[0] as f32;
                        if normalized { v / 255.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([bytes[0], bytes[1]]) as f32;
                        if normalized {
                            (v / 32767.0).max(-1.0)
                        } else {
                            v
                        }
                    }
                    5123 => {
                        let v = u16::from_le_bytes([bytes[0], bytes[1]]) as f32;
                        if normalized { v / 65535.0 } else { v }
                    }
                    5125 => {
                        u32::from_le_bytes(bytes.try_into().map_err(|_| invalid("u32 accessor"))?)
                            as f32
                    }
                    5126 => {
                        f32::from_le_bytes(bytes.try_into().map_err(|_| invalid("f32 accessor"))?)
                    }
                    _ => unreachable!(),
                };
                row.push(value);
            }
            result.push(row);
        }
        Ok(result)
    }

    fn append(
        &mut self,
        bytes: &[u8],
        kind: &str,
        component: u32,
        count: usize,
        target: Option<u32>,
        bounds: Option<(&[f32], &[f32])>,
    ) -> usize {
        self.bin.resize((self.bin.len() + 3) & !3, 0);
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let views = self.doc["bufferViews"]
            .as_array_mut()
            .expect("GLB bufferViews");
        let mut view = json!({"buffer":0,"byteOffset":offset,"byteLength":bytes.len()});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        views.push(view);
        let mut accessor =
            json!({"bufferView":views.len()-1,"componentType":component,"count":count,"type":kind});
        if let Some((min, max)) = bounds {
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        let accessors = self.doc["accessors"].as_array_mut().expect("GLB accessors");
        accessors.push(accessor);
        accessors.len() - 1
    }

    fn append_floats(&mut self, rows: &[Vec<f32>], kind: &str, bounds: bool) -> usize {
        let bytes: Vec<u8> = rows
            .iter()
            .flat_map(|row| row.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let limits = if bounds && !rows.is_empty() {
            let width = rows[0].len();
            let min: Vec<f32> = (0..width)
                .map(|i| rows.iter().map(|row| row[i]).fold(f32::INFINITY, f32::min))
                .collect();
            let max: Vec<f32> = (0..width)
                .map(|i| {
                    rows.iter()
                        .map(|row| row[i])
                        .fold(f32::NEG_INFINITY, f32::max)
                })
                .collect();
            Some((min, max))
        } else {
            None
        };
        self.append(
            &bytes,
            kind,
            5126,
            rows.len(),
            Some(34962),
            limits.as_ref().map(|(a, b)| (a.as_slice(), b.as_slice())),
        )
    }

    fn append_u8(&mut self, rows: &[[u8; 4]], normalized: bool) -> usize {
        let bytes: Vec<u8> = rows.iter().flat_map(|row| row.iter().copied()).collect();
        let index = self.append(&bytes, "VEC4", 5121, rows.len(), Some(34962), None);
        if normalized {
            self.doc["accessors"][index]["normalized"] = json!(true);
        }
        index
    }

    fn append_indices(&mut self, indices: &[u32]) -> usize {
        let bytes: Vec<u8> = indices
            .iter()
            .flat_map(|index| index.to_le_bytes())
            .collect();
        self.append(&bytes, "SCALAR", 5125, indices.len(), Some(34963), None)
    }

    fn finish(mut self) -> SwapResult<Vec<u8>> {
        self.doc["buffers"] = json!([{"byteLength":self.bin.len()}]);
        let mut json_bytes = serde_json::to_vec(&self.doc)?;
        json_bytes.resize((json_bytes.len() + 3) & !3, b' ');
        self.bin.resize((self.bin.len() + 3) & !3, 0);
        let len = 12 + 8 + json_bytes.len() + 8 + self.bin.len();
        if len > u32::MAX as usize {
            return Err(invalid("combined GLB too large"));
        }
        let mut out = Vec::with_capacity(len);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(len as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&self.bin);
        Ok(out)
    }
}

fn invalid(message: &str) -> HeadSwapError {
    HeadSwapError::Invalid(message.into())
}
fn u32_at(bytes: &[u8], at: usize) -> SwapResult<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(at..at + 4)
            .ok_or_else(|| invalid("truncated GLB"))?
            .try_into()
            .map_err(|_| invalid("GLB length"))?,
    ))
}
fn usize_field(value: &Value, key: &str) -> SwapResult<usize> {
    value[key]
        .as_u64()
        .map(|v| v as usize)
        .ok_or_else(|| invalid(key))
}
fn smooth(lo: f32, hi: f32, value: f32) -> f32 {
    let t = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn copy_materials(body: &mut Glb, head: &Glb) -> SwapResult<usize> {
    let image_offset = body.doc["images"].as_array().map_or(0, Vec::len);
    let sampler_offset = body.doc["samplers"].as_array().map_or(0, Vec::len);
    let texture_offset = body.doc["textures"].as_array().map_or(0, Vec::len);
    let material_offset = body.doc["materials"].as_array().map_or(0, Vec::len);
    for image in head.doc["images"].as_array().map_or(&[][..], Vec::as_slice) {
        let mut image = image.clone();
        if let Some(index) = image["bufferView"].as_u64() {
            let view = &head.doc["bufferViews"][index as usize];
            let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
            let len = usize_field(view, "byteLength")?;
            let bytes = head
                .bin
                .get(start..start + len)
                .ok_or_else(|| invalid("image bufferView"))?;
            body.bin.resize((body.bin.len() + 3) & !3, 0);
            let offset = body.bin.len();
            body.bin.extend_from_slice(bytes);
            let views = body.doc["bufferViews"]
                .as_array_mut()
                .ok_or_else(|| invalid("body bufferViews"))?;
            views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":len}));
            image["bufferView"] = json!(views.len() - 1);
        }
        if !body.doc["images"].is_array() {
            body.doc["images"] = json!([]);
        }
        body.doc["images"].as_array_mut().unwrap().push(image);
    }
    for name in ["samplers", "textures", "materials"] {
        if !body.doc[name].is_array() {
            body.doc[name] = json!([]);
        }
        for item in head.doc[name].as_array().map_or(&[][..], Vec::as_slice) {
            let mut item = item.clone();
            if name == "textures" {
                if let Some(i) = item["source"].as_u64() {
                    item["source"] = json!(i as usize + image_offset);
                }
                if let Some(i) = item["sampler"].as_u64() {
                    item["sampler"] = json!(i as usize + sampler_offset);
                }
            } else if name == "materials" {
                for key in [
                    "baseColorTexture",
                    "metallicRoughnessTexture",
                    "normalTexture",
                    "occlusionTexture",
                    "emissiveTexture",
                ] {
                    if let Some(i) = item[key]["index"].as_u64() {
                        item[key]["index"] = json!(i as usize + texture_offset);
                    }
                    if let Some(i) = item["pbrMetallicRoughness"][key]["index"].as_u64() {
                        item["pbrMetallicRoughness"][key]["index"] =
                            json!(i as usize + texture_offset);
                    }
                }
            }
            body.doc[name].as_array_mut().unwrap().push(item);
        }
    }
    Ok(material_offset)
}

/// Compose a replacement head without modifying either source GLB.
pub fn compose(
    body_bytes: &[u8],
    head_bytes: &[u8],
    spec: &HeadSwapAssetNode,
) -> SwapResult<Vec<u8>> {
    let mut body = Glb::parse(body_bytes)?;
    let head = Glb::parse(head_bytes)?;
    let meshes = body.doc["meshes"]
        .as_array()
        .ok_or_else(|| invalid("body meshes"))?;
    if meshes.len() != 1 {
        return Err(invalid("body must have one skinned mesh"));
    }
    let joints = body.doc["skins"][0]["joints"]
        .as_array()
        .ok_or_else(|| invalid("body skin joints"))?;
    let slot = |name: &str| -> SwapResult<usize> {
        let node = body.doc["nodes"]
            .as_array()
            .ok_or_else(|| invalid("body nodes"))?
            .iter()
            .position(|node| node["name"].as_str() == Some(name))
            .ok_or_else(|| HeadSwapError::Invalid(format!("missing body bone {name}")))?;
        joints
            .iter()
            .position(|joint| joint.as_u64() == Some(node as u64))
            .ok_or_else(|| invalid("bone not in skin"))
    };
    let head_slot = slot("Head")?;
    let neck_slot = slot("neck_01")?;
    let chest_slot = slot("spine_03")?;
    if [head_slot, neck_slot, chest_slot].iter().any(|&i| i > 255) {
        return Err(invalid("joint index exceeds u8"));
    }
    // Anchor a new head to the source rig's bind pose unless legacy centerY is explicit.
    let center_y = if let Some(center) = spec.center_y {
        center
    } else {
        let matrices = body.accessor(usize_field(&body.doc["skins"][0], "inverseBindMatrices")?)?;
        let row = matrices
            .get(head_slot)
            .ok_or_else(|| invalid("Head inverse bind matrix"))?;
        let matrix: [f32; 16] = row
            .as_slice()
            .try_into()
            .map_err(|_| invalid("Head inverse bind matrix"))?;
        let bind = crate::world::render::mat4_inverse_affine(matrix)
            .ok_or_else(|| invalid("noninvertible Head bind matrix"))?;
        bind[13] + spec.head_offset_y
    };
    if spec.palette == "cohesive" {
        if body.doc["materials"].as_array().map_or(0, Vec::len) < 2 {
            return Err(invalid("cohesive palette requires two body materials"));
        }
        body.doc["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"] =
            json!([0.35, 0.30, 0.47, 1.0]);
        body.doc["materials"][1]["pbrMetallicRoughness"]["baseColorFactor"] =
            json!([0.65, 0.48, 0.27, 1.0]);
    }
    let old_parts = body.doc["meshes"][0]["primitives"]
        .as_array()
        .ok_or_else(|| invalid("body primitives"))?
        .clone();
    let mut parts = Vec::new();
    for mut part in old_parts {
        let attributes = &part["attributes"];
        let pos = body.accessor(usize_field(attributes, "POSITION")?)?;
        let joints = body.accessor(usize_field(attributes, "JOINTS_0")?)?;
        let weights = body.accessor(usize_field(attributes, "WEIGHTS_0")?)?;
        let indices = body.accessor(usize_field(&part, "indices")?)?;
        if indices.len() % 3 != 0 {
            return Err(invalid("body triangle indices"));
        }
        let mut keep = Vec::new();
        for tri in indices.chunks_exact(3) {
            let ids: Vec<usize> = tri.iter().map(|v| v[0] as usize).collect();
            if ids.iter().any(|&i| i >= pos.len()) {
                return Err(invalid("body index out of range"));
            }
            let y = ids.iter().map(|&i| pos[i][1]).sum::<f32>() / 3.0;
            let influence = ids
                .iter()
                .map(|&i| {
                    (0..4)
                        .filter(|&j| {
                            joints[i][j] as usize == head_slot || joints[i][j] as usize == neck_slot
                        })
                        .map(|j| weights[i][j])
                        .sum::<f32>()
                })
                .sum::<f32>()
                / 3.0;
            if y < spec.cut_y && (y < spec.weighted_cut_y || influence < 0.15) {
                keep.extend(ids.iter().map(|&i| i as u32));
            }
        }
        part["indices"] = json!(body.append_indices(&keep));
        parts.push(part);
    }
    let material_offset = copy_materials(&mut body, &head)?;
    let head_parts = head.doc["meshes"]
        .as_array()
        .ok_or_else(|| invalid("head meshes"))?;
    if spec.neck_part >= head_parts.len() {
        return Err(invalid("neckPart exceeds head mesh count"));
    }
    for (part_id, mesh) in head_parts.iter().enumerate() {
        for part in mesh["primitives"]
            .as_array()
            .ok_or_else(|| invalid("head primitives"))?
        {
            let attr = &part["attributes"];
            let pos = head.accessor(usize_field(attr, "POSITION")?)?;
            let normal = head.accessor(usize_field(attr, "NORMAL")?)?;
            let uv = head.accessor(usize_field(attr, "TEXCOORD_0")?)?;
            let color = head.accessor(usize_field(attr, "COLOR_0")?)?;
            let old_indices = head.accessor(usize_field(part, "indices")?)?;
            let mut unique = HashMap::<Vec<u32>, u32>::new();
            let (mut p, mut n, mut t, mut c) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut new_indices = Vec::with_capacity(old_indices.len());
            for old in old_indices {
                let i = old[0] as usize;
                if i >= pos.len() || i >= normal.len() || i >= uv.len() || i >= color.len() {
                    return Err(invalid("head index out of range"));
                }
                let key: Vec<u32> = pos[i]
                    .iter()
                    .chain(&normal[i])
                    .chain(&uv[i])
                    .chain(&color[i])
                    .map(|v| v.to_bits())
                    .collect();
                let index = *unique.entry(key).or_insert_with(|| {
                    let index = p.len() as u32;
                    p.push(pos[i].clone());
                    n.push(normal[i].clone());
                    t.push(uv[i].clone());
                    c.push(color[i].clone());
                    index
                });
                new_indices.push(index);
            }
            let original_y: Vec<f32> = p.iter().map(|v| v[1]).collect();
            for value in &mut p {
                for xyz in value.iter_mut() {
                    *xyz *= spec.scale;
                }
                value[1] += center_y;
                value[2] += spec.center_z;
            }
            let mut joint = vec![[0u8; 4]; p.len()];
            let mut weight = vec![[0u8; 4]; p.len()];
            for i in 0..p.len() {
                if part_id == spec.neck_part {
                    let h = smooth(-0.85, -0.45, original_y[i]);
                    let chest = 0.35 * (1.0 - smooth(-1.31, -1.0, original_y[i]));
                    let cw = (chest * 255.0).round() as u8;
                    let hw = (h * 255.0).round() as u8;
                    joint[i] = [chest_slot as u8, neck_slot as u8, head_slot as u8, 0];
                    weight[i] = [cw, 255u8.saturating_sub(cw).saturating_sub(hw), hw, 0];
                } else {
                    joint[i][0] = head_slot as u8;
                    weight[i][0] = 255;
                }
            }
            let attributes = json!({
                "POSITION":body.append_floats(&p,"VEC3",true),
                "NORMAL":body.append_floats(&n,"VEC3",false),
                "TEXCOORD_0":body.append_floats(&t,"VEC2",false),
                "COLOR_0":body.append_floats(&c,"VEC4",false),
                "JOINTS_0":body.append_u8(&joint,false),
                "WEIGHTS_0":body.append_u8(&weight,true)
            });
            let material = usize_field(part, "material")? + material_offset;
            parts.push(json!({"attributes":attributes,"indices":body.append_indices(&new_indices),"material":material,"mode":4,
                "extras":{"source":"HeadSwapAsset","sourcePart":part_id}}));
        }
    }
    body.doc["meshes"][0]["primitives"] = json!(parts);
    body.doc["asset"]["extras"]["headAttachment"] =
        json!("Native HeadSwapAsset; original body skin retained");
    body.finish()
}

// Generated GLBs stay renderer-owned and share the normal asset loading path.
pub(crate) struct GeneratedAssetResolver {
    pub generated: std::sync::Arc<crate::asset::MemoryAssetResolver>,
    pub parent: std::sync::Arc<dyn crate::asset::AssetResolver>,
}
impl crate::asset::AssetResolver for GeneratedAssetResolver {
    fn revision(&self) -> Option<u64> {
        self.parent.revision()
    }
    fn nesting_depth(&self) -> usize {
        self.parent.nesting_depth()
    }
    fn resolve(&self, src: &str) -> Result<crate::asset::AssetSource, String> {
        self.generated
            .resolve(src)
            .or_else(|_| self.parent.resolve(src))
    }
}

// Child DSL paths are relative to that document, never the outer body's directory.
pub(crate) struct ScopedAssetResolver {
    pub base: String,
    pub parent: std::sync::Arc<dyn crate::asset::AssetResolver>,
    pub dependencies: std::sync::Mutex<std::collections::BTreeSet<String>>,
}
impl crate::asset::AssetResolver for ScopedAssetResolver {
    fn nesting_depth(&self) -> usize {
        self.parent.nesting_depth() + 1
    }
    fn resolve(&self, src: &str) -> Result<crate::asset::AssetSource, String> {
        let scoped = if src.starts_with("data:") || src.starts_with("motionloom:") {
            src.to_string()
        } else if let Ok(base) = url::Url::parse(&self.base) {
            base.join(src)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| src.into())
        } else if std::path::Path::new(src).is_absolute() {
            src.to_string()
        } else {
            std::path::Path::new(&self.base)
                .parent()
                .unwrap_or_else(|| std::path::Path::new(""))
                .join(src)
                .to_string_lossy()
                .into_owned()
        };
        self.dependencies
            .lock()
            .expect("dependency lock")
            .insert(scoped.clone());
        self.parent.resolve(&scoped)
    }
}
