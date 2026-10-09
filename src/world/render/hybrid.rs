// src/world/render/hybrid.rs
//! Camera-independent software BVH shared by native and WebGPU specular transport.
//!
//! The single storage buffer includes geometry, materials and bounded texture tiles.
//! Rigid/pose-only changes refit the retained BVH; identical evaluated scenes reuse
//! the packed bytes. Secondary-hit texture resolution is an internal preview budget.

use super::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};

const NODE_STRIDE: usize = 3;
const TRIANGLE_STRIDE: usize = 11;
const MATERIAL_STRIDE: usize = 10;
const TEXTURE_EDGE: u32 = 256;
const LEAF_SIZE: usize = 4;
const MAX_LEAF_SIZE: usize = 8;
const SAH_BINS: usize = 16;
const MAX_BVH_DEPTH: usize = 30;
const HEADER_VECTORS: usize = 4;
// Query masks let shadow/ownership rays skip unrelated subtrees before
// intersection or texture sampling. Kept in sync with hybrid_transport.wgsl.
const QUERY_CAST_TRANSMISSION: u32 = 1;
const QUERY_SOLID_TRANSMISSION: u32 = 2;
const QUERY_CAST_SHADOW: u32 = 4;

#[derive(Debug, thiserror::Error)]
pub(super) enum HybridGeometryError {
    #[error("hybrid geometry triangle budget exceeded: {actual} triangles, limit {limit}")]
    TriangleBudget { actual: usize, limit: usize },
    #[error(
        "solid refraction on actor {actor_id:?} needs a closed oriented manifold: {boundary_edges} boundary, {nonmanifold_edges} nonmanifold, {orientation_edges} inconsistent edges, volume {volume}"
    )]
    InvalidSolid {
        actor_id: String,
        boundary_edges: usize,
        nonmanifold_edges: usize,
        orientation_edges: usize,
        volume: f64,
    },
    #[error("hybrid geometry on actor {actor_id:?} contains non-finite evaluated vertices")]
    NonFinite { actor_id: String },
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct ObjectKey {
    actor: String,
    path: PathBuf,
    mesh: Option<usize>,
    node: Option<usize>,
}
fn object_key(draw: &GpuWorldDraw) -> ObjectKey {
    ObjectKey {
        actor: draw.instance_key.actor_id.clone(),
        path: draw.resource_key.model_path.clone(),
        mesh: draw.resource_key.draw_key.mesh,
        node: draw.resource_key.draw_key.mesh_node,
    }
}

#[derive(Clone, Debug)]
struct Triangle {
    p: [[f32; 3]; 3],
    n: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    color: [[f32; 4]; 3],
    material: u32,
    object: u32,
    query_mask: u32,
}
impl Triangle {
    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        (
            std::array::from_fn(|a| self.p.iter().map(|p| p[a]).fold(f32::INFINITY, f32::min)),
            std::array::from_fn(|a| {
                self.p
                    .iter()
                    .map(|p| p[a])
                    .fold(f32::NEG_INFINITY, f32::max)
            }),
        )
    }
    #[cfg(test)]
    fn center(&self, axis: usize) -> f32 {
        (self.p[0][axis] + self.p[1][axis] + self.p[2][axis]) / 3.0
    }
}
#[derive(Clone, Debug)]
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    first: usize,
    count: usize,
    escape: usize,
    query_mask: u32,
    parent: Option<usize>,
    right: usize,
}

impl Node {
    fn metadata(&self) -> [f32; 4] {
        [
            self.escape as f32,
            self.query_mask as f32,
            self.parent.map_or(0, |index| index + 1) as f32,
            self.right as f32,
        ]
    }
}

#[derive(Clone, Debug)]
pub(super) struct HybridScene {
    pub(super) vectors: Vec<[f32; 4]>,
    pub(super) signature: u64,
    pub(super) node_count: usize,
    pub(super) triangle_count: usize,
    pub(super) object_count: usize,
    pub(super) refit: bool,
    pub(super) cache_hit: bool,
    pub(super) object_ids: HashMap<GpuWorldInstanceKey, u32>,
    pub(super) opaque_caster_bounds: Option<([f32; 3], [f32; 3])>,
}
impl HybridScene {
    pub(super) fn bytes(&self) -> Vec<u8> {
        self.vectors
            .iter()
            .flatten()
            .flat_map(|v| v.to_ne_bytes())
            .collect()
    }
    pub(super) fn object_id(&self, draw: &GpuWorldDraw) -> u32 {
        self.object_ids
            .get(&draw.instance_key)
            .copied()
            .unwrap_or(u32::MAX)
    }

    /// Prove that a perspective shadow view's clipped near region contains no
    /// opaque caster. This runs on retained evaluated geometry, once per view,
    /// rather than repeating the proof for every secondary shaded fragment.
    pub(super) fn opaque_caster_overlaps(&self, minimum: [f32; 3], maximum: [f32; 3]) -> bool {
        if minimum.into_iter().chain(maximum).any(|v| !v.is_finite()) {
            return true;
        }
        let base = self.vectors[0][0] as usize;
        let count = self.vectors[0][1] as usize;
        let material_base = self.vectors[1][0] as usize;
        let mut node = 0;
        while node < count {
            let a = self.vectors[base + node * NODE_STRIDE];
            let b = self.vectors[base + node * NODE_STRIDE + 1];
            let metadata = self.vectors[base + node * NODE_STRIDE + 2];
            let escape = metadata[0] as usize;
            if metadata[1] as u32 & QUERY_CAST_SHADOW == 0
                || (0..3).any(|axis| a[axis] > maximum[axis] || b[axis] < minimum[axis])
            {
                node = escape;
                continue;
            }
            if b[3] > 0.5 {
                for triangle in a[3] as usize..a[3] as usize + b[3] as usize {
                    let t = self.vectors[0][2] as usize + triangle * TRIANGLE_STRIDE;
                    let material = material_base + self.vectors[t][3] as usize * MATERIAL_STRIDE;
                    if self.vectors[t + 10][2] < 0.5 || self.vectors[material + 1][2] > 0.001 {
                        continue;
                    }
                    if (0..3).all(|axis| {
                        let lo = (0..3).map(|v| self.vectors[t + v][axis]).fold(f32::INFINITY, f32::min);
                        let hi = (0..3).map(|v| self.vectors[t + v][axis]).fold(f32::NEG_INFINITY, f32::max);
                        lo <= maximum[axis] && hi >= minimum[axis]
                    }) {
                        return true;
                    }
                }
                node = escape;
            } else {
                node += 1;
            }
        }
        false
    }
}

#[derive(Default)]
pub(super) struct HybridSceneCache {
    scene: Option<HybridScene>,
    evaluated_signature: u64,
    topology_signature: u64,
    nodes: Vec<Node>,
    order: Vec<usize>,
    textures: HashMap<u64, (u32, u32, Vec<u32>)>,
    evaluated_vertices: HashMap<GpuWorldInstanceKey, (u64, Vec<([f32; 3], [f32; 3])>)>,
    validated_solids: HashSet<u64>,
    validated_slabs: HashMap<u64, bool>,
}
impl HybridSceneCache {
    pub(super) fn scene(&self) -> Option<&HybridScene> {
        self.scene.as_ref()
    }
    #[cfg(test)]
    pub(super) fn update(
        &mut self,
        draws: &[GpuWorldDraw],
        triangle_limit: usize,
    ) -> Result<&HybridScene, HybridGeometryError> {
        self.update_with_shadow_flags(draws, triangle_limit, &HashMap::new(), false)
    }
    pub(super) fn update_with_shadow_flags(
        &mut self,
        draws: &[GpuWorldDraw],
        triangle_limit: usize,
        flags: &HashMap<String, [bool; 2]>,
        per_light: bool,
    ) -> Result<&HybridScene, HybridGeometryError> {
        let triangle_count: usize = draws.iter().map(|d| d.indices.len() / 3).sum();
        if triangle_count > triangle_limit {
            return Err(HybridGeometryError::TriangleBudget {
                actual: triangle_count,
                limit: triangle_limit,
            });
        }
        let mut evaluated = DefaultHasher::new();
        let mut topology = DefaultHasher::new();
        for draw in draws {
            draw.instance_key.hash(&mut topology);
            draw.vertex_signature.hash(&mut topology);
            draw.indices.len().hash(&mut topology);
            draw.instance_key.hash(&mut evaluated);
            (draw.phase as u8).hash(&mut evaluated);
            shadow_flags(draw, flags, per_light).hash(&mut evaluated);
            draw.vertex_signature.hash(&mut evaluated);
            for v in draw
                .params
                .model
                .into_iter()
                .chain(draw.params.actor)
                .chain(draw.params.actor_rotation)
                .chain(geometry_vegetation(draw.params.vegetation))
                .chain(draw.params.style)
                .chain(draw.params.material0)
                .chain(draw.params.material1)
                .chain(draw.params.material2)
                .chain(draw.params.material3)
                .chain(draw.params.material4)
                .chain(draw.params.material5)
                .chain(draw.params.material6)
                .chain(draw.params.material7)
                .chain(draw.params.material8)
                .chain(draw.params.material9)
                .chain(draw.params.material10)
            {
                v.to_bits().hash(&mut evaluated);
            }
            // material11.y is frame-assigned object identity and must not dirty the cache.
            draw.params.material11[0].to_bits().hash(&mut evaluated);
            for matrix in &draw.bone_matrices {
                for v in matrix {
                    v.to_bits().hash(&mut evaluated);
                }
            }
            for texture in [
                &draw.texture,
                &draw.metallic_roughness_texture,
                &draw.emissive_texture,
            ] {
                texture.signature.hash(&mut evaluated);
            }
        }
        let signature = evaluated.finish();
        if self.scene.is_some() && signature == self.evaluated_signature {
            let scene = self.scene.as_mut().unwrap();
            scene.cache_hit = true;
            scene.refit = false;
            return Ok(scene);
        }
        let topology_signature = topology.finish();
        let refit = self.scene.is_some()
            && self.topology_signature == topology_signature
            && self.order.len() == triangle_count;
        let mut objects = BTreeMap::<ObjectKey, u32>::new();
        for draw in draws {
            objects.entry(object_key(draw)).or_default();
        }
        for (i, id) in objects.values_mut().enumerate() {
            *id = i as u32;
        }
        let mut object_ids = HashMap::new();
        let mut triangles = Vec::with_capacity(triangle_count);
        let mut materials = Vec::with_capacity(draws.len());
        let mut solid = vec![false; objects.len()];
        let mut transmissive = vec![false; objects.len()];
        let mut closed = vec![false; objects.len()];
        let mut texture_vectors = Vec::<[f32; 4]>::new();
        let mut texture_offsets = HashMap::<u64, [f32; 4]>::new();
        for (material_index, draw) in draws.iter().enumerate() {
            let object_id = objects[&object_key(draw)];
            object_ids.insert(draw.instance_key.clone(), object_id);
            solid[object_id as usize] |= draw.params.material11[0] > 0.5;
            transmissive[object_id as usize] |= draw.params.material6[0] > 0.001;
            let cast_shadow = shadow_flags(draw, flags, per_light)[0];
            let transmission = draw.params.material6[0] > 0.001;
            let query_mask = (u32::from(cast_shadow && transmission) * QUERY_CAST_TRANSMISSION)
                | (u32::from(transmission && draw.params.material11[0] > 0.5)
                    * QUERY_SOLID_TRANSMISSION)
                | (u32::from(cast_shadow) * QUERY_CAST_SHADOW);
            let mut draw_hasher = DefaultHasher::new();
            draw.vertex_signature.hash(&mut draw_hasher);
            for v in draw
                .params
                .model
                .into_iter()
                .chain(draw.params.actor)
                .chain(draw.params.actor_rotation)
                .chain(geometry_vegetation(draw.params.vegetation))
            {
                v.to_bits().hash(&mut draw_hasher);
            }
            for matrix in &draw.bone_matrices {
                for v in matrix {
                    v.to_bits().hash(&mut draw_hasher);
                }
            }
            let draw_signature = draw_hasher.finish();
            let entry = self
                .evaluated_vertices
                .entry(draw.instance_key.clone())
                .or_insert_with(|| (u64::MAX, Vec::new()));
            if entry.0 != draw_signature {
                entry.1 = draw
                    .vertices
                    .iter()
                    .map(|v| evaluated_vertex(v, draw))
                    .collect();
                entry.0 = draw_signature;
            }
            let verts = &entry.1;
            if verts
                .iter()
                .any(|(p, n)| p.iter().chain(n).any(|v| !v.is_finite()))
            {
                return Err(HybridGeometryError::NonFinite {
                    actor_id: draw.instance_key.actor_id.clone(),
                });
            }
            for tri in draw.indices.chunks_exact(3) {
                if tri.iter().any(|i| *i as usize >= verts.len()) {
                    continue;
                }
                let indices = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                let p = indices.map(|i| verts[i].0);
                if length3(cross3(subtract3(p[1], p[0]), subtract3(p[2], p[0]))) < 1e-12 {
                    continue;
                }
                triangles.push(Triangle {
                    p,
                    n: indices.map(|i| verts[i].1),
                    uv: indices.map(|i| transformed_uv(draw.vertices[i].uv, draw.params)),
                    color: indices.map(|i| draw.vertices[i].color),
                    material: material_index as u32,
                    object: object_id,
                    query_mask,
                });
            }
            let mut m = [[0.0; 4]; MATERIAL_STRIDE];
            m[0] = draw.params.material4;
            m[0][3] *= draw.params.style[0];
            m[1] = [
                draw.params.material0[0],
                draw.params.material0[1],
                draw.params.material6[0],
                draw.params.material6[1],
            ];
            m[2] = [
                draw.params.material1[0],
                draw.params.material1[1],
                draw.params.material1[2],
                draw.params.material1[3],
            ];
            m[3] = [
                draw.params.material7[0],
                draw.params.material7[1],
                draw.params.material7[2],
                draw.params.material7[3],
            ];
            m[4] = [
                draw.params.material6[3],
                draw.params.material11[0],
                draw.params.material10[0],
                draw.params.material10[1],
            ];
            m[5] = draw.params.material2;
            // Tile offsets are relative to the texture section, patched in WGSL by header.
            for (slot, texture) in [
                (6, &draw.texture),
                (7, &draw.metallic_roughness_texture),
                (8, &draw.emissive_texture),
            ] {
                let color_role = slot != 7;
                let key = texture.signature ^ if color_role { 0x9173f123ca991d81 } else { 0 };
                let info = *texture_offsets.entry(key).or_insert_with(|| {
                    let (w, h, pixels) = self
                        .textures
                        .entry(key)
                        .or_insert_with(|| downsample_texture(texture, color_role));
                    // Base/emissive tiles can certify opaque alpha. The MR role
                    // overwrites this word with its existing channel swizzle.
                    let opaque = pixels.iter().all(|pixel| pixel >> 24 == 255);
                    let info = [texture_vectors.len() as f32, *w as f32, *h as f32, opaque as u8 as f32];
                    for chunk in pixels.chunks(4) {
                        let mut packed = [0.0; 4];
                        for (i, p) in chunk.iter().enumerate() {
                            packed[i] = f32::from_bits(*p);
                        }
                        texture_vectors.push(packed);
                    }
                    info
                });
                m[slot] = info;
            }
            // Specular-channel swizzle is retained in the unused tile header word.
            m[7][3] = draw.params.material8[2];
            m[9] = [
                draw.params.material0[3],
                draw.params.material8[1],
                draw.params.material6[2],
                draw.params.style[2],
            ];
            materials.push(m);
        }
        // Quantized weld validation spans material chunks of the same mesh object.
        for (key, id) in &objects {
            if solid[*id as usize] || transmissive[*id as usize] {
                let mut h = DefaultHasher::new();
                topology_signature.hash(&mut h);
                key.hash(&mut h);
                // Reflected/zero uniform transforms change the closed-solid
                // orientation contract even though mesh topology is unchanged.
                for d in draws.iter().filter(|d| object_key(d) == *key) {
                    (d.params.model[3] <= 0.0).hash(&mut h);
                }
                let solid_signature = h.finish();
                if solid[*id as usize] {
                    if !self.validated_solids.contains(&solid_signature) {
                        validate_closed(&triangles, *id, &key.actor)?;
                        self.validated_solids.insert(solid_signature);
                    }
                    closed[*id as usize] = true;
                } else {
                    // Closed slab geometry still uses authored thickness. Its
                    // paired optical interface must not be charged at both
                    // geometric front/back crossings; open planes stay valid.
                    closed[*id as usize] = *self
                        .validated_slabs
                        .entry(solid_signature)
                        .or_insert_with(|| validate_closed(&triangles, *id, &key.actor).is_ok());
                }
            }
        }
        // Degenerate triangles are omitted consistently; topology refit checks count.
        let refit = refit && self.order.len() == triangles.len();
        if refit {
            refit_nodes(&mut self.nodes, &triangles, &self.order);
        } else {
            self.order = (0..triangles.len()).collect();
            self.nodes.clear();
            if !triangles.is_empty() {
                build_nodes(
                    &mut self.nodes,
                    &mut self.order,
                    &triangles,
                    0,
                    triangles.len(),
                );
            }
        }
        // Primary shadow maps already contain opaque geometry. Build a compact
        // independent tree for casting glass, preserving its triangle identities
        // through a remap into the unrestricted reflection/solid geometry.
        let mut shadow_order: Vec<usize> = triangles
            .iter()
            .enumerate()
            .filter_map(|(index, triangle)| {
                (triangle.query_mask & QUERY_CAST_TRANSMISSION != 0).then_some(index)
            })
            .collect();
        let mut shadow_nodes = Vec::new();
        let shadow_triangle_count = shadow_order.len();
        if shadow_triangle_count != 0 {
            build_nodes(
                &mut shadow_nodes,
                &mut shadow_order,
                &triangles,
                0,
                shadow_triangle_count,
            );
        }
        let mut packed_triangle_indices = vec![0; triangles.len()];
        for (packed_index, &triangle_index) in self.order.iter().enumerate() {
            packed_triangle_indices[triangle_index] = packed_index;
        }
        let mut vectors = Vec::with_capacity(
            HEADER_VECTORS + self.nodes.len() * NODE_STRIDE
                + triangles.len() * TRIANGLE_STRIDE
                + materials.len() * MATERIAL_STRIDE
                + texture_vectors.len()
                + shadow_nodes.len() * NODE_STRIDE
                + shadow_triangle_count.div_ceil(4),
        );
        vectors.extend([[0.0; 4]; HEADER_VECTORS]);
        let node_base = vectors.len();
        for node in &self.nodes {
            vectors.push([node.min[0], node.min[1], node.min[2], node.first as f32]);
            vectors.push([node.max[0], node.max[1], node.max[2], node.count as f32]);
            vectors.push(node.metadata());
        }
        let triangle_base = vectors.len();
        for &index in &self.order {
            let t = &triangles[index];
            let flags = shadow_flags(&draws[t.material as usize], flags, per_light);
            vectors.extend([
                [t.p[0][0], t.p[0][1], t.p[0][2], t.material as f32],
                [t.p[1][0], t.p[1][1], t.p[1][2], t.object as f32],
                [t.p[2][0], t.p[2][1], t.p[2][2], closed[t.object as usize] as u8 as f32],
                [t.n[0][0], t.n[0][1], t.n[0][2], 0.0],
                [t.n[1][0], t.n[1][1], t.n[1][2], 0.0],
                [t.n[2][0], t.n[2][1], t.n[2][2], 0.0],
                t.color[0],
                t.color[1],
                t.color[2],
                [t.uv[0][0], t.uv[0][1], t.uv[1][0], t.uv[1][1]],
                [t.uv[2][0], t.uv[2][1], flags[0] as u8 as f32, flags[1] as u8 as f32],
            ]);
        }
        let material_base = vectors.len();
        vectors.extend(materials.iter().flatten().copied());
        let texture_base = vectors.len();
        vectors.extend(texture_vectors);
        let shadow_node_base = vectors.len();
        for node in &shadow_nodes {
            vectors.push([node.min[0], node.min[1], node.min[2], node.first as f32]);
            vectors.push([node.max[0], node.max[1], node.max[2], node.count as f32]);
            vectors.push(node.metadata());
        }
        let shadow_index_base = vectors.len();
        for chunk in shadow_order.chunks(4) {
            let mut remap = [0.0; 4];
            for (slot, &triangle_index) in chunk.iter().enumerate() {
                remap[slot] = packed_triangle_indices[triangle_index] as f32;
            }
            vectors.push(remap);
        }
        vectors[0] = [
            node_base as f32,
            self.nodes.len() as f32,
            triangle_base as f32,
            triangles.len() as f32,
        ];
        vectors[1] = [
            material_base as f32,
            materials.len() as f32,
            texture_base as f32,
            objects.len() as f32,
        ];
        vectors[2] = [
            (shadow_triangle_count != 0) as u8 as f32,
            solid.iter().filter(|s| **s).count() as f32,
            TEXTURE_EDGE as f32,
            draws.iter().all(|draw| {
                !shadow_flags(draw, flags, per_light)[0]
                    || draw.params.material6[0] > 0.001
                    || draw.phase == GpuWorldDrawPhase::Opaque
            }) as u8 as f32,
        ];
        vectors[3] = [
            shadow_node_base as f32,
            shadow_nodes.len() as f32,
            shadow_index_base as f32,
            shadow_triangle_count as f32,
        ];
        let mut opaque_caster_bounds: Option<([f32; 3], [f32; 3])> = None;
        for triangle in &triangles {
            if triangle.query_mask & QUERY_CAST_SHADOW == 0
                || draws[triangle.material as usize].params.material6[0] > 0.001 {
                continue;
            }
            let (lo, hi) = triangle.bounds();
            let (min, max) = opaque_caster_bounds.get_or_insert((lo, hi));
            for axis in 0..3 {
                min[axis] = min[axis].min(lo[axis]);
                max[axis] = max[axis].max(hi[axis]);
            }
        }
        self.scene = Some(HybridScene {
            vectors,
            signature,
            node_count: self.nodes.len(),
            triangle_count: triangles.len(),
            object_count: objects.len(),
            refit,
            cache_hit: false,
            object_ids,
            opaque_caster_bounds,
        });
        self.evaluated_signature = signature;
        self.topology_signature = topology_signature;
        // Bound CPU tile retention independently of animated material churn.
        if self.textures.len() > 256 {
            self.textures.clear();
        }
        if self.evaluated_vertices.len() > draws.len().saturating_mul(2).max(64) {
            self.evaluated_vertices
                .retain(|key, _| self.scene.as_ref().unwrap().object_ids.contains_key(key));
        }
        if self.validated_solids.len() > 128 {
            self.validated_solids.clear();
        }
        if self.validated_slabs.len() > 128 {
            self.validated_slabs.clear();
        }
        Ok(self.scene.as_ref().unwrap())
    }
}

pub(super) fn shadow_flags(
    draw: &GpuWorldDraw,
    flags: &HashMap<String, [bool; 2]>,
    per_light: bool,
) -> [bool; 2] {
    if !per_light {
        return [true, true];
    }
    let actor = draw
        .instance_key
        .actor_id
        .split("::")
        .next()
        .unwrap_or(&draw.instance_key.actor_id);
    flags.get(actor).copied().unwrap_or([true, true])
}
// Global time also serves non-geometric shading effects. It changes the BVH
// only when the vertex shader actually enables vegetation deformation.
fn geometry_vegetation(vegetation: [f32; 4]) -> [f32; 4] {
    if vegetation[0] > 0.5 {
        vegetation
    } else {
        [0.0; 4]
    }
}
fn transformed_uv(uv: [f32; 2], p: GpuWorldParams) -> [f32; 2] {
    let u = uv[0] * p.material5[0];
    let v = uv[1] * p.material5[1];
    [
        u * p.material3[2] - v * p.material3[3] + p.material5[2] + p.material3[0],
        u * p.material3[3] + v * p.material3[2] + p.material5[3] + p.material3[1],
    ]
}
fn evaluated_vertex(v: &GpuWorldVertex, draw: &GpuWorldDraw) -> ([f32; 3], [f32; 3]) {
    let weight_sum: f32 = v.weights.iter().sum();
    let mut position = v.position;
    let mut normal = v.normal;
    if weight_sum > 1e-6 {
        position = [0.0; 3];
        normal = [0.0; 3];
        for i in 0..4 {
            if v.weights[i] <= 0.0 {
                continue;
            }
            let joint = v.joints[i].max(0.0).round() as usize;
            if let Some(m) = draw.bone_matrices.get(joint) {
                let w = v.weights[i] / weight_sum;
                for a in 0..3 {
                    position[a] += (m[a] * v.position[0]
                        + m[4 + a] * v.position[1]
                        + m[8 + a] * v.position[2]
                        + m[12 + a])
                        * w;
                    normal[a] +=
                        (m[a] * v.normal[0] + m[4 + a] * v.normal[1] + m[8 + a] * v.normal[2]) * w;
                }
            }
        }
    } else if draw.params.vegetation[0] > 0.5 {
        let height = draw.params.vegetation[1].max(0.001);
        let weight = smoothstep(0.04, 1.0, (position[1] / height).clamp(0.0, 1.0));
        let phase =
            draw.params.vegetation[2] + draw.params.vegetation[3] * 1.35 + position[1] * 0.73;
        position[0] += phase.sin() * weight * weight * height * 0.026;
        position[2] += (phase * 0.83).cos() * weight * weight * height * 0.026;
    }
    let local =
        std::array::from_fn(|a| (position[a] - draw.params.model[a]) * draw.params.model[3]);
    let rotated = rotate_quat(local, draw.params.actor_rotation);
    (
        std::array::from_fn(|a| rotated[a] + draw.params.actor[a]),
        normalize3(rotate_quat(normal, draw.params.actor_rotation)),
    )
}
fn subtract3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn length3(v: [f32; 3]) -> f32 {
    dot3(v, v).sqrt()
}
fn rotate_quat(v: [f32; 3], mut q: [f32; 4]) -> [f32; 3] {
    let length = q.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    for x in &mut q {
        *x /= length;
    }
    let xyz = [q[0], q[1], q[2]];
    let t = cross3(xyz, v).map(|x| x * 2.0);
    let c = cross3(xyz, t);
    std::array::from_fn(|a| v[a] + q[3] * t[a] + c[a])
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn downsample_texture(texture: &GpuWorldTexture, color_role: bool) -> (u32, u32, Vec<u32>) {
    let ratio = (TEXTURE_EDGE as f32 / texture.width.max(texture.height).max(1) as f32).min(1.0);
    let w = (texture.width as f32 * ratio).round().max(1.0) as u32;
    let h = (texture.height as f32 * ratio).round().max(1.0) as u32;
    let mut pixels = Vec::with_capacity((w * h) as usize);
    // Color area averaging uses linear light; packed bytes retain encoding. WGSL
    // decodes color texels before bilinear filtering, matching the main pass.
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0.0f64; 4];
            let mut count = 0u64;
            let y0 = y * texture.height / h;
            let y1 = ((y + 1) * texture.height / h).max(y0 + 1);
            let x0 = x * texture.width / w;
            let x1 = ((x + 1) * texture.width / w).max(x0 + 1);
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = ((sy * texture.width + sx) * 4) as usize;
                    if let Some(p) = texture.rgba.get(i..i + 4) {
                        for c in 0..4 {
                            let value = p[c] as f32 / 255.0;
                            sum[c] += if color_role && c < 3 {
                                materials::srgb_decode(value) as f64
                            } else {
                                value as f64
                            };
                        }
                        count += 1;
                    }
                }
            }
            let bytes: [u8; 4] = std::array::from_fn(|c| {
                let value = (sum[c] / count.max(1) as f64) as f32;
                let encoded = if color_role && c < 3 {
                    if value <= 0.0031308 {
                        value * 12.92
                    } else {
                        1.055 * value.powf(1.0 / 2.4) - 0.055
                    }
                } else {
                    value
                };
                (encoded * 255.0).round().clamp(0.0, 255.0) as u8
            });
            pixels.push(u32::from_le_bytes(bytes));
        }
    }
    (w, h, pixels)
}
fn build_nodes(
    nodes: &mut Vec<Node>,
    order: &mut [usize],
    triangles: &[Triangle],
    first: usize,
    count: usize,
) -> usize {
    // Cache primitive bounds once. Repeated vertex reductions and median
    // partitioning made large room meshes both slow to build and expensive to
    // traverse when a wall dominated the enclosing node's longest axis.
    let mut primitives: Vec<_> = order[first..first + count]
        .iter()
        .map(|&index| {
            let triangle = &triangles[index];
            let (min, max) = triangle.bounds();
            BvhPrimitive {
                index,
                min,
                max,
                center: std::array::from_fn(|axis| triangle.p.iter().map(|p| p[axis] / 3.0).sum()),
                query_mask: triangle.query_mask,
            }
        })
        .collect();
    let index = build_sah_nodes(nodes, &mut primitives, first, 0, None);
    for (position, primitive) in primitives.into_iter().enumerate() {
        order[first + position] = primitive.index;
    }
    index
}

#[derive(Clone, Copy)]
struct BvhPrimitive {
    index: usize,
    min: [f32; 3],
    max: [f32; 3],
    center: [f32; 3],
    query_mask: u32,
}

#[derive(Clone, Copy)]
struct BvhBin {
    min: [f32; 3],
    max: [f32; 3],
    count: usize,
}
impl BvhBin {
    const EMPTY: Self = Self {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
        count: 0,
    };
    fn include(&mut self, min: [f32; 3], max: [f32; 3], count: usize) {
        if count == 0 {
            return;
        }
        self.count += count;
        for axis in 0..3 {
            self.min[axis] = self.min[axis].min(min[axis]);
            self.max[axis] = self.max[axis].max(max[axis]);
        }
    }
    fn area(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let extent: [f64; 3] =
            std::array::from_fn(|axis| f64::from(self.max[axis]) - f64::from(self.min[axis]));
        2.0 * (extent[0] * extent[1] + extent[0] * extent[2] + extent[1] * extent[2])
    }
}

fn sah_bin(center: f32, minimum: f32, inverse_extent: f64) -> usize {
    (((f64::from(center) - f64::from(minimum)) * inverse_extent) as usize).min(SAH_BINS - 1)
}

fn build_sah_nodes(
    nodes: &mut Vec<Node>,
    primitives: &mut [BvhPrimitive],
    first: usize,
    depth: usize,
    parent: Option<usize>,
) -> usize {
    let index = nodes.len();
    let count = primitives.len();
    let mut bounds = BvhBin::EMPTY;
    let mut center_min = [f32::INFINITY; 3];
    let mut center_max = [f32::NEG_INFINITY; 3];
    let mut query_mask = 0;
    for primitive in primitives.iter() {
        bounds.include(primitive.min, primitive.max, 1);
        for axis in 0..3 {
            center_min[axis] = center_min[axis].min(primitive.center[axis]);
            center_max[axis] = center_max[axis].max(primitive.center[axis]);
        }
        query_mask |= primitive.query_mask;
    }
    nodes.push(Node {
        min: bounds.min,
        max: bounds.max,
        first,
        count,
        escape: index + 1,
        query_mask,
        parent,
        right: 0,
    });
    debug_assert!(depth <= MAX_BVH_DEPTH);
    debug_assert!(depth != MAX_BVH_DEPTH || count <= MAX_LEAF_SIZE);
    if count > LEAF_SIZE && depth < MAX_BVH_DEPTH {
        let parent_area = bounds.area();
        let mut best = None;
        let mut best_cost = f64::INFINITY;
        if parent_area > 0.0 {
            let inverse_extents: [f64; 3] = std::array::from_fn(|axis| {
                let extent = f64::from(center_max[axis]) - f64::from(center_min[axis]);
                if extent > 0.0 {
                    SAH_BINS as f64 / extent
                } else {
                    0.0
                }
            });
            let mut bins = [[BvhBin::EMPTY; SAH_BINS]; 3];
            for primitive in primitives.iter() {
                for axis in 0..3 {
                    if inverse_extents[axis] > 0.0 {
                        let bin = sah_bin(
                            primitive.center[axis],
                            center_min[axis],
                            inverse_extents[axis],
                        );
                        bins[axis][bin].include(primitive.min, primitive.max, 1);
                    }
                }
            }
            for axis in 0..3 {
                if inverse_extents[axis] == 0.0 {
                    continue;
                }
                let mut suffix = [BvhBin::EMPTY; SAH_BINS];
                let mut right = BvhBin::EMPTY;
                for bin in (0..SAH_BINS).rev() {
                    right.include(
                        bins[axis][bin].min,
                        bins[axis][bin].max,
                        bins[axis][bin].count,
                    );
                    suffix[bin] = right;
                }
                let mut left = BvhBin::EMPTY;
                for bin in 0..SAH_BINS - 1 {
                    left.include(
                        bins[axis][bin].min,
                        bins[axis][bin].max,
                        bins[axis][bin].count,
                    );
                    let right = suffix[bin + 1];
                    if left.count == 0 || right.count == 0 {
                        continue;
                    }
                    let cost = parent_area
                        + left.area() * left.count as f64
                        + right.area() * right.count as f64;
                    // Axis and bin iteration order break equal-cost ties, and
                    // partitions retain deterministic primitive identities.
                    if cost < best_cost {
                        best_cost = cost;
                        best = Some((axis, bin, inverse_extents[axis]));
                    }
                }
            }
        }
        let should_split = count > MAX_LEAF_SIZE || best_cost < parent_area * count as f64;
        if should_split {
            let mut middle = 0;
            if let Some((axis, bin, inverse_extent)) = best {
                let mut end = count;
                while middle < end {
                    if sah_bin(
                        primitives[middle].center[axis],
                        center_min[axis],
                        inverse_extent,
                    ) <= bin
                    {
                        middle += 1;
                    } else {
                        end -= 1;
                        primitives.swap(middle, end);
                    }
                }
            }
            let child_capacity = (MAX_LEAF_SIZE as u64) << (MAX_BVH_DEPTH - depth - 1);
            if middle == 0
                || middle == count
                || middle.max(count - middle) as u64 > child_capacity
            {
                // Each child must fit its remaining depth with bounded leaves.
                // This also balances identical centroids. Triangle identity
                // resolves ties without changing stackless preorder layout.
                let axis = (0..3)
                    .max_by(|&a, &b| {
                        (f64::from(center_max[a]) - f64::from(center_min[a]))
                            .total_cmp(&(f64::from(center_max[b]) - f64::from(center_min[b])))
                    })
                    .unwrap();
                middle = count / 2;
                primitives.select_nth_unstable_by(middle, |a, b| {
                    a.center[axis]
                        .total_cmp(&b.center[axis])
                        .then_with(|| a.index.cmp(&b.index))
                });
            }
            let (left, right) = primitives.split_at_mut(middle);
            build_sah_nodes(nodes, left, first, depth + 1, Some(index));
            let right = build_sah_nodes(nodes, right, first + middle, depth + 1, Some(index));
            nodes[index].count = 0;
            nodes[index].right = right;
        }
    }
    nodes[index].escape = nodes.len();
    index
}
fn range_bounds(order: &[usize], triangles: &[Triangle]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for &i in order {
        let (a, b) = triangles[i].bounds();
        for axis in 0..3 {
            min[axis] = min[axis].min(a[axis]);
            max[axis] = max[axis].max(b[axis]);
        }
    }
    (min, max)
}
fn refit_nodes(nodes: &mut [Node], triangles: &[Triangle], order: &[usize]) {
    for i in (0..nodes.len()).rev() {
        if nodes[i].count > 0 {
            (nodes[i].min, nodes[i].max) = range_bounds(
                &order[nodes[i].first..nodes[i].first + nodes[i].count],
                triangles,
            );
            nodes[i].query_mask = order[nodes[i].first..nodes[i].first + nodes[i].count]
                .iter()
                .fold(0, |mask, &j| mask | triangles[j].query_mask);
        } else {
            let left = i + 1;
            let right = nodes[left].escape;
            nodes[i].min = std::array::from_fn(|a| nodes[left].min[a].min(nodes[right].min[a]));
            nodes[i].max = std::array::from_fn(|a| nodes[left].max[a].max(nodes[right].max[a]));
            nodes[i].query_mask = nodes[left].query_mask | nodes[right].query_mask;
        }
    }
}
fn validate_closed(
    triangles: &[Triangle],
    object: u32,
    actor: &str,
) -> Result<(), HybridGeometryError> {
    let selected: Vec<_> = triangles.iter().filter(|t| t.object == object).collect();
    if selected.is_empty() {
        return Err(HybridGeometryError::InvalidSolid {
            actor_id: actor.to_owned(),
            boundary_edges: 0,
            nonmanifold_edges: 0,
            orientation_edges: 0,
            volume: 0.0,
        });
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for t in &selected {
        let (a, b) = t.bounds();
        for i in 0..3 {
            lo[i] = lo[i].min(a[i]);
            hi[i] = hi[i].max(b[i]);
        }
    }
    let extent = length3(subtract3(hi, lo)).max(1e-6);
    let weld = extent as f64 * 1e-6;
    // Check neighboring bins so tiny seam/pole differences remain welded after
    // world translation; rounding a single key can split coincident vertices.
    let mut bins = HashMap::<[i64; 3], Vec<usize>>::new();
    let mut positions = Vec::<[f64; 3]>::new();
    let mut weld_position = |p: [f32; 3]| -> usize {
        let p = p.map(f64::from);
        let bin: [i64; 3] =
            std::array::from_fn(|a| ((p[a] - f64::from(lo[a])) / weld).floor() as i64);
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let neighbor = [bin[0] + x, bin[1] + y, bin[2] + z];
                    if let Some(ids) = bins.get(&neighbor) {
                        for &id in ids {
                            let distance_squared = (0..3)
                                .map(|a| (p[a] - positions[id][a]).powi(2))
                                .sum::<f64>();
                            if distance_squared <= weld * weld {
                                return id;
                            }
                        }
                    }
                }
            }
        }
        let id = positions.len();
        positions.push(p);
        bins.entry(bin).or_default().push(id);
        id
    };
    let mut edges = HashMap::<(usize, usize), (usize, i32)>::new();
    let center: [f32; 3] = std::array::from_fn(|a| (lo[a] + hi[a]) * 0.5);
    let mut volume = 0.0_f64;
    for t in selected {
        let keys = t.p.map(&mut weld_position);
        let area = cross3(subtract3(t.p[1], t.p[0]), subtract3(t.p[2], t.p[0]));
        // UV-sphere pole quads contain triangles with no optical boundary.
        // Do not count their collapsed edges as nonmanifold intersections.
        if keys[0] == keys[1] || keys[1] == keys[2] || keys[2] == keys[0] || dot3(area, area) == 0.0
        {
            continue;
        }
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (edge, sign) = if keys[a] < keys[b] {
                ((keys[a], keys[b]), 1)
            } else {
                ((keys[b], keys[a]), -1)
            };
            let v = edges.entry(edge).or_default();
            v.0 += 1;
            v.1 += sign;
        }
        let a = subtract3(t.p[0], center);
        let b = subtract3(t.p[1], center);
        let c = subtract3(t.p[2], center);
        volume += dot3(a, cross3(b, c)) as f64 / 6.0;
    }
    let boundary = edges.values().filter(|(n, _)| *n == 1).count();
    let nonmanifold = edges.values().filter(|(n, _)| *n > 2).count();
    let orientation = edges.values().filter(|(n, s)| *n == 2 && *s != 0).count();
    if boundary > 0
        || nonmanifold > 0
        || orientation > 0
        || volume <= (extent as f64).powi(3) * 1e-10
    {
        return Err(HybridGeometryError::InvalidSolid {
            actor_id: actor.to_owned(),
            boundary_edges: boundary,
            nonmanifold_edges: nonmanifold,
            orientation_edges: orientation,
            volume,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn triangle(p: [[f32; 3]; 3]) -> Triangle {
        Triangle {
            p,
            n: [[0., 0., 1.]; 3],
            uv: [[0., 0.]; 3],
            color: [[1.; 4]; 3],
            material: 0,
            object: 0,
            query_mask: QUERY_CAST_SHADOW,
        }
    }
    fn cube() -> Vec<Triangle> {
        let p = [
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., 1., -1.],
            [-1., 1., -1.],
            [-1., -1., 1.],
            [1., -1., 1.],
            [1., 1., 1.],
            [-1., 1., 1.],
        ];
        [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ]
        .map(|i| triangle(i.map(|j| p[j])))
        .to_vec()
    }
    #[test]
    fn manifold_gate_accepts_seam_welded_closed_geometry_and_rejects_open_and_reversed_faces() {
        let t = cube();
        validate_closed(&t, 0, "cube").unwrap();
        assert!(matches!(
            validate_closed(&t[..11], 0, "open"),
            Err(HybridGeometryError::InvalidSolid {
                boundary_edges: 3,
                ..
            })
        ));
        let mut t = t;
        t[0].p.swap(0, 1);
        assert!(matches!(
            validate_closed(&t, 0, "orientation"),
            Err(HybridGeometryError::InvalidSolid {
                orientation_edges: 3,
                ..
            })
        ));
    }
    #[test]
    fn manifold_gate_accepts_generated_sphere_poles_and_translated_seams() {
        let asset = crate::dsl::PrimitiveAssetNode {
            id: "solid-sphere".into(),
            geometry: crate::dsl::PrimitiveGeometry::Sphere {
                radius: 0.85,
                segments: 48,
                rings: 32,
            },
            color: [1.; 4],
            material: None,
            material_definition: None,
            bevel_radius: 0.,
            bevel_segments: 0,
            material_seed: None,
            collision: Default::default(),
            modifiers: Vec::new(),
            mesh_build: Default::default(),
            lod: Default::default(),
        };
        // Use the actual engine builder: UV seams and pole quads duplicate
        // positions, including tiny trigonometric differences at the seam.
        let mesh = crate::world::primitive::generate_primitive_mesh(&asset);
        let sphere: Vec<_> = mesh
            .indices
            .chunks_exact(3)
            .map(|i| triangle([i[0], i[1], i[2]].map(|j| mesh.positions[j as usize])))
            .collect();
        for offset in [
            [0., 0., 0.],
            [-0.25, 0., -0.3],
            [0.25, 0., 0.3],
            [1., 0., 1.5],
            [17.125, -3.75, 0.123456],
        ] {
            let mut moved = sphere.clone();
            for t in &mut moved {
                for p in &mut t.p {
                    for a in 0..3 {
                        p[a] += offset[a];
                    }
                }
            }
            validate_closed(&moved, 0, "generated-sphere")
                .unwrap_or_else(|error| panic!("translation {offset:?}: {error}"));
        }
        let mut open = sphere.clone();
        open.remove(open.len() / 2);
        assert!(matches!(
            validate_closed(&open, 0, "open-sphere"),
            Err(HybridGeometryError::InvalidSolid {
                boundary_edges: 3,
                ..
            })
        ));
        let mut reversed = sphere;
        for t in &mut reversed {
            t.p.swap(0, 1);
        }
        assert!(matches!(
            validate_closed(&reversed, 0, "inward-sphere"),
            Err(HybridGeometryError::InvalidSolid { volume, .. }) if volume < 0.
        ));
        let collapsed = [
            triangle([[0.; 3]; 3]),
            triangle([[0., 0., 0.], [1., 0., 0.], [2., 0., 0.]]),
        ];
        assert!(matches!(
            validate_closed(&collapsed, 0, "zero-volume"),
            Err(HybridGeometryError::InvalidSolid { volume, .. }) if volume == 0.
        ));
    }
    #[test]
    fn stackless_bvh_refit_preserves_escape_links_and_all_triangle_coverage() {
        let mut triangles = cube();
        let mut order: Vec<_> = (0..triangles.len()).collect();
        let mut nodes = vec![];
        build_nodes(&mut nodes, &mut order, &triangles, 0, triangles.len());
        let escape: Vec<_> = nodes.iter().map(|n| n.escape).collect();
        for t in &mut triangles {
            for p in &mut t.p {
                p[0] += 3.;
            }
        }
        refit_nodes(&mut nodes, &triangles, &order);
        assert_eq!(nodes[0].min, [2., -1., -1.]);
        assert_eq!(nodes[0].max, [4., 1., 1.]);
        assert_eq!(escape, nodes.iter().map(|n| n.escape).collect::<Vec<_>>());
        assert_eq!(
            nodes.iter().map(|n| n.count).sum::<usize>(),
            triangles.len()
        );
        assert_eq!(nodes[0].escape, nodes.len());
    }

    fn bvh_room_fixture(grid: usize) -> Vec<Triangle> {
        let mut triangles = Vec::new();
        // Coarse surrounding walls and dense furniture surfaces reproduce the
        // disparity of primitive size in a furnished interior.
        for mut wall in cube() {
            for p in &mut wall.p {
                p[0] *= 13.0;
                p[1] = p[1] * 3.0 + 3.0;
                p[2] *= 8.0;
            }
            triangles.push(wall);
        }
        for object in 0..18 {
            let origin = [
                (object % 6) as f32 * 3.4 - 8.5,
                0.6 + (object % 3) as f32 * 0.25,
                (object / 6) as f32 * 4.0 - 4.0,
            ];
            let scale = [
                0.3 + (object % 4) as f32 * 0.15,
                0.3 + (object % 5) as f32 * 0.18,
                0.25 + (object % 3) as f32 * 0.25,
            ];
            for axis in 0..3 {
                let u = (axis + 1) % 3;
                let v = (axis + 2) % 3;
                for side in [-1.0, 1.0] {
                    for y in 0..grid {
                        for x in 0..grid {
                            let corner = |dx: usize, dy: usize| {
                                let mut p = origin;
                                p[axis] += side * scale[axis];
                                p[u] += ((x + dx) as f32 / grid as f32 * 2.0 - 1.0) * scale[u];
                                p[v] += ((y + dy) as f32 / grid as f32 * 2.0 - 1.0) * scale[v];
                                p
                            };
                            let a = corner(0, 0);
                            let b = corner(1, 0);
                            let c = corner(1, 1);
                            let d = corner(0, 1);
                            triangles.push(triangle([a, b, c]));
                            triangles.push(triangle([a, c, d]));
                        }
                    }
                }
            }
        }
        triangles
    }

    fn bvh_reference_median(
        nodes: &mut Vec<Node>,
        order: &mut [usize],
        triangles: &[Triangle],
        first: usize,
        count: usize,
    ) {
        let index = nodes.len();
        let (min, max) = range_bounds(&order[first..first + count], triangles);
        nodes.push(Node {
            min,
            max,
            first,
            count,
            escape: index + 1,
            query_mask: QUERY_CAST_SHADOW,
            parent: None,
            right: 0,
        });
        if count > LEAF_SIZE {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let middle = first + count / 2;
            order[first..first + count].select_nth_unstable_by(count / 2, |a, b| {
                triangles[*a]
                    .center(axis)
                    .total_cmp(&triangles[*b].center(axis))
            });
            bvh_reference_median(nodes, order, triangles, first, middle - first);
            let right = nodes.len();
            bvh_reference_median(nodes, order, triangles, middle, first + count - middle);
            nodes[index].count = 0;
            nodes[index].right = right;
            nodes[index + 1].parent = Some(index);
            nodes[right].parent = Some(index);
        }
        nodes[index].escape = nodes.len();
    }

    fn bvh_reference_ray_box(
        origin: [f32; 3],
        direction: [f32; 3],
        min: [f32; 3],
        max: [f32; 3],
        limit: f32,
    ) -> bool {
        let mut near = 0.0f32;
        let mut far = limit;
        for axis in 0..3 {
            if direction[axis].abs() < 1e-8 {
                if origin[axis] < min[axis] || origin[axis] > max[axis] {
                    return false;
                }
            } else {
                let a = (min[axis] - origin[axis]) / direction[axis];
                let b = (max[axis] - origin[axis]) / direction[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
                if near > far {
                    return false;
                }
            }
        }
        far >= near.max(0.0)
    }

    fn bvh_reference_ray_entry(
        origin: [f32; 3],
        direction: [f32; 3],
        node: &Node,
        limit: f32,
        query_mask: u32,
    ) -> Option<f32> {
        if query_mask != 0 && node.query_mask & query_mask == 0 {
            return None;
        }
        let mut near = 0.0f32;
        let mut far = limit;
        for axis in 0..3 {
            if direction[axis].abs() < 1e-8 {
                if origin[axis] < node.min[axis] || origin[axis] > node.max[axis] {
                    return None;
                }
            } else {
                let a = (node.min[axis] - origin[axis]) / direction[axis];
                let b = (node.max[axis] - origin[axis]) / direction[axis];
                near = near.max(a.min(b));
                far = far.min(a.max(b));
                if near > far {
                    return None;
                }
            }
        }
        (far >= near.max(0.0)).then_some(near)
    }

    // Uses the same two scalar bit stacks as the GPU design, reading packed
    // parent/right metadata rather than retaining a per-depth node array.
    fn bvh_reference_near_query(
        nodes: &[Node],
        order: &[usize],
        triangles: &[Triangle],
        origin: [f32; 3],
        direction: [f32; 3],
        query_mask: u32,
    ) -> (f32, usize, usize, usize) {
        let mut node = 0;
        let mut depth = 0;
        let mut pending = 0u32;
        let mut right_first = 0u32;
        let mut prevalidated = false;
        let mut visits = 0;
        let mut tests = 0;
        let mut boxes = 0;
        let mut distance = 100.0;
        loop {
            visits += 1;
            let current = &nodes[node];
            let covered = if prevalidated {
                true
            } else {
                boxes += 1;
                bvh_reference_ray_entry(origin, direction, current, distance, query_mask).is_some()
            };
            prevalidated = false;
            if covered {
                if current.count != 0 {
                    for &index in &order[current.first..current.first + current.count] {
                        if query_mask != 0 && triangles[index].query_mask & query_mask == 0 {
                            continue;
                        }
                        tests += 1;
                        distance = bvh_reference_triangle_hit(
                            origin,
                            direction,
                            &triangles[index],
                            distance,
                        );
                    }
                } else {
                    assert!(depth < MAX_BVH_DEPTH);
                    let bit = 1u32 << depth;
                    let left = node + 1;
                    let right = current.metadata()[3] as usize;
                    boxes += 2;
                    let left_entry = bvh_reference_ray_entry(
                        origin, direction, &nodes[left], distance, query_mask,
                    );
                    let right_entry = bvh_reference_ray_entry(
                        origin, direction, &nodes[right], distance, query_mask,
                    );
                    let next = match (left_entry, right_entry) {
                        (Some(a), Some(b)) => {
                            pending |= bit;
                            if b < a {
                                right_first |= bit;
                                Some(right)
                            } else {
                                right_first &= !bit;
                                Some(left)
                            }
                        }
                        (Some(_), None) => {
                            pending &= !bit;
                            Some(left)
                        }
                        (None, Some(_)) => {
                            pending &= !bit;
                            Some(right)
                        }
                        (None, None) => None,
                    };
                    if let Some(next) = next {
                        node = next;
                        depth += 1;
                        // No triangle was tested after selecting this child,
                        // so its existing finite-entry evidence is still valid.
                        prevalidated = true;
                        continue;
                    }
                }
            }
            // A pending sibling is revisited against the updated closest hit.
            // Completed ancestors are climbed through their parent pointers.
            loop {
                let parent_plus_one = nodes[node].metadata()[2] as usize;
                if parent_plus_one == 0 {
                    return (distance, visits, tests, boxes);
                }
                let parent = parent_plus_one - 1;
                depth -= 1;
                let bit = 1u32 << depth;
                if pending & bit != 0 {
                    pending &= !bit;
                    node = if right_first & bit != 0 {
                        parent + 1
                    } else {
                        nodes[parent].metadata()[3] as usize
                    };
                    depth += 1;
                    break;
                }
                node = parent;
            }
        }
    }

    fn bvh_max_depth(nodes: &[Node]) -> usize {
        nodes
            .iter()
            .enumerate()
            .map(|(mut index, _)| {
                let mut depth = 0;
                while let Some(parent) = nodes[index].parent {
                    assert!(parent < index);
                    index = parent;
                    depth += 1;
                }
                depth
            })
            .max()
            .unwrap_or(0)
    }

    fn bvh_reference_triangle_hit(
        origin: [f32; 3],
        direction: [f32; 3],
        triangle: &Triangle,
        limit: f32,
    ) -> f32 {
        let e1 = subtract3(triangle.p[1], triangle.p[0]);
        let e2 = subtract3(triangle.p[2], triangle.p[0]);
        let c = cross3(direction, e2);
        let determinant = dot3(e1, c);
        if determinant.abs() < 1e-8 {
            return limit;
        }
        let delta = subtract3(origin, triangle.p[0]);
        let u = dot3(delta, c) / determinant;
        let q = cross3(delta, e1);
        let v = dot3(direction, q) / determinant;
        let distance = dot3(e2, q) / determinant;
        if u >= -1e-6 && v >= -1e-6 && u + v <= 1.000001 && distance > 1e-4 && distance < limit {
            distance
        } else {
            limit
        }
    }

    fn bvh_reference_query(
        nodes: &[Node],
        order: &[usize],
        triangles: &[Triangle],
        origin: [f32; 3],
        direction: [f32; 3],
        query_mask: u32,
    ) -> (f32, usize, usize) {
        let mut node = 0;
        let mut visits = 0;
        let mut tests = 0;
        let mut distance = 100.0;
        while node < nodes.len() {
            let current = &nodes[node];
            visits += 1;
            if (query_mask != 0 && current.query_mask & query_mask == 0)
                || !bvh_reference_ray_box(origin, direction, current.min, current.max, distance)
            {
                node = current.escape;
                continue;
            }
            if current.count != 0 {
                for &index in &order[current.first..current.first + current.count] {
                    if query_mask != 0 && triangles[index].query_mask & query_mask == 0 {
                        continue;
                    }
                    tests += 1;
                    distance =
                        bvh_reference_triangle_hit(origin, direction, &triangles[index], distance);
                }
                node = current.escape;
            } else {
                node += 1;
            }
        }
        (distance, visits, tests)
    }

    fn bvh_fixture_rays() -> Vec<([f32; 3], [f32; 3])> {
        (0..512)
            .map(|i| {
                let origin = [
                    (i % 32) as f32 * 0.65 - 10.0,
                    0.2 + (i % 13) as f32 * 0.24,
                    (i / 32) as f32 * 0.85 - 6.0,
                ];
                let target = [
                    ((i * 17) % 29) as f32 * 0.7 - 9.0,
                    0.3 + ((i * 11) % 23) as f32 * 0.12,
                    ((i * 7) % 19) as f32 * 0.6 - 5.0,
                ];
                let delta = subtract3(target, origin);
                (origin, delta.map(|value| value / length3(delta)))
            })
            .collect()
    }

    #[test]
    fn sah_bvh_exact_hits_reduce_room_ray_work_against_median_partitioning() {
        let triangles = bvh_room_fixture(6);
        let mut order: Vec<_> = (0..triangles.len()).collect();
        let mut nodes = Vec::new();
        build_nodes(&mut nodes, &mut order, &triangles, 0, triangles.len());
        let mut median_order: Vec<_> = (0..triangles.len()).collect();
        let mut median_nodes = Vec::new();
        bvh_reference_median(
            &mut median_nodes,
            &mut median_order,
            &triangles,
            0,
            triangles.len(),
        );
        let mut sah_work = [0usize; 2];
        let mut median_work = [0usize; 2];
        for (origin, direction) in bvh_fixture_rays() {
            let (hit, visits, tests) =
                bvh_reference_query(&nodes, &order, &triangles, origin, direction, 0);
            let (median_hit, median_visits, median_tests) = bvh_reference_query(
                &median_nodes,
                &median_order,
                &triangles,
                origin,
                direction,
                0,
            );
            let brute_hit = triangles.iter().fold(100.0, |limit, triangle| {
                bvh_reference_triangle_hit(origin, direction, triangle, limit)
            });
            assert!((hit - brute_hit).abs() < 1e-4);
            assert!((hit - median_hit).abs() < 1e-4);
            sah_work[0] += visits;
            sah_work[1] += tests;
            median_work[0] += median_visits;
            median_work[1] += median_tests;
        }
        // Fewer nodes alone can hide additional leaf intersection work. Require
        // a substantial reduction in both costs on the representative rays.
        assert!(
            sah_work[0] * 2 < median_work[0],
            "{sah_work:?} {median_work:?}"
        );
        assert!(
            sah_work[1] * 2 < median_work[1],
            "{sah_work:?} {median_work:?}"
        );
    }

    #[test]
    fn near_ordered_bvh_queries_match_preorder_and_brute_force_with_bounded_depth() {
        let mut triangles = bvh_room_fixture(4);
        for (index, triangle) in triangles.iter_mut().enumerate() {
            triangle.query_mask = 1 << (index % 3);
        }
        let mut order: Vec<_> = (0..triangles.len()).collect();
        let mut nodes = Vec::new();
        let first = 7;
        let count = triangles.len() - first * 2;
        build_nodes(&mut nodes, &mut order, &triangles, first, count);
        assert!(bvh_max_depth(&nodes) <= MAX_BVH_DEPTH);
        let mut old_work = 0;
        let mut near_work = 0;
        for (origin, direction) in bvh_fixture_rays() {
            for mask in [0, 1, 2, 4] {
                let (old, visits, _) = bvh_reference_query(&nodes, &order, &triangles, origin, direction, mask);
                let (near, near_visits, _, _) = bvh_reference_near_query(&nodes, &order, &triangles, origin, direction, mask);
                assert!((old - near).abs() < 1e-4);
                old_work += visits;
                near_work += near_visits;
            }
        }
        assert!(near_work < old_work, "near {near_work}, preorder {old_work}");
        let compact: Vec<_> = triangles.iter().enumerate().filter_map(|(index, triangle)|
            (triangle.query_mask & QUERY_CAST_TRANSMISSION != 0).then_some(index)).collect();
        let mut compact_order = compact.clone();
        let mut compact_nodes = Vec::new();
        build_nodes(&mut compact_nodes, &mut compact_order, &triangles, 0, compact.len());
        assert!(bvh_max_depth(&compact_nodes) <= MAX_BVH_DEPTH);
        for (origin, direction) in bvh_fixture_rays().into_iter().take(64) {
            let (hit, _, _, _) = bvh_reference_near_query(&compact_nodes, &compact_order, &triangles, origin, direction, 1);
            let brute = compact.iter().fold(100.0, |distance, &index|
                bvh_reference_triangle_hit(origin, direction, &triangles[index], distance));
            assert!((hit - brute).abs() < 1e-4);
        }
    }

    #[test]
    fn sah_bvh_is_deterministic_bounded_and_refits_subset_masks() {
        let mut triangles = bvh_room_fixture(2);
        for (index, triangle) in triangles.iter_mut().enumerate() {
            triangle.query_mask = 1 << (index % 3);
        }
        let first = 7;
        let count = triangles.len() - 14;
        let mut order: Vec<_> = (0..triangles.len()).collect();
        order[first..first + count].reverse();
        let before = order.clone();
        let mut nodes = Vec::new();
        build_nodes(&mut nodes, &mut order, &triangles, first, count);
        let mut repeat_order = before.clone();
        let mut repeat_nodes = Vec::new();
        build_nodes(
            &mut repeat_nodes,
            &mut repeat_order,
            &triangles,
            first,
            count,
        );
        assert_eq!(order, repeat_order);
        assert_eq!(nodes.len(), repeat_nodes.len());
        for (node, repeat) in nodes.iter().zip(&repeat_nodes) {
            assert_eq!(node.min, repeat.min);
            assert_eq!(node.max, repeat.max);
            assert_eq!(node.first, repeat.first);
            assert_eq!(node.count, repeat.count);
            assert_eq!(node.escape, repeat.escape);
            assert_eq!(node.query_mask, repeat.query_mask);
            assert!(node.count <= MAX_LEAF_SIZE);
        }
        assert_eq!(&order[..first], &before[..first]);
        assert_eq!(&order[first + count..], &before[first + count..]);
        let mut covered = order[first..first + count].to_vec();
        covered.sort_unstable();
        assert_eq!(covered, (first..first + count).collect::<Vec<_>>());
        assert_eq!(nodes.iter().map(|node| node.count).sum::<usize>(), count);
        let escape: Vec<_> = nodes.iter().map(|node| node.escape).collect();
        for (index, triangle) in triangles.iter_mut().enumerate() {
            triangle.query_mask = if index % 5 == 0 {
                0
            } else {
                1 << ((index + 1) % 3)
            };
            for point in &mut triangle.p {
                point[0] += 1.5;
                point[1] -= 0.25;
            }
        }
        refit_nodes(&mut nodes, &triangles, &order);
        assert_eq!(
            escape,
            nodes.iter().map(|node| node.escape).collect::<Vec<_>>()
        );
        for (index, node) in nodes.iter().enumerate() {
            let (min, max, mask) = if node.count != 0 {
                let leaf = &order[node.first..node.first + node.count];
                let (min, max) = range_bounds(leaf, &triangles);
                let mask = leaf
                    .iter()
                    .fold(0, |mask, &i| mask | triangles[i].query_mask);
                assert_eq!(node.escape, index + 1);
                (min, max, mask)
            } else {
                let left = &nodes[index + 1];
                let right = &nodes[left.escape];
                assert_eq!(right.escape, node.escape);
                (
                    std::array::from_fn(|axis| left.min[axis].min(right.min[axis])),
                    std::array::from_fn(|axis| left.max[axis].max(right.max[axis])),
                    left.query_mask | right.query_mask,
                )
            };
            assert_eq!(node.min, min);
            assert_eq!(node.max, max);
            assert_eq!(node.query_mask, mask);
        }
        for (origin, direction) in bvh_fixture_rays().into_iter().take(32) {
            for query_mask in [0, 1, 2, 4] {
                let (hit, _, _) =
                    bvh_reference_query(&nodes, &order, &triangles, origin, direction, query_mask);
                let brute = triangles[first..first + count]
                    .iter()
                    .filter(|triangle| query_mask == 0 || triangle.query_mask & query_mask != 0)
                    .fold(100.0, |limit, triangle| {
                        bvh_reference_triangle_hit(origin, direction, triangle, limit)
                    });
                assert!((hit - brute).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn sah_bvh_identical_centroids_keep_bounded_leaves_and_complete_coverage() {
        let repeated = triangle([[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]]);
        for count in [5, 8, 9, 257] {
            let triangles = vec![repeated.clone(); count];
            let mut order: Vec<_> = (0..count).rev().collect();
            let mut nodes = Vec::new();
            build_nodes(&mut nodes, &mut order, &triangles, 0, count);
            assert!(nodes.iter().all(|node| node.count <= MAX_LEAF_SIZE));
            assert_eq!(nodes.iter().map(|node| node.count).sum::<usize>(), count);
            assert_eq!(nodes[0].escape, nodes.len());
            order.sort_unstable();
            assert_eq!(order, (0..count).collect::<Vec<_>>());
        }
    }
    #[test]
    fn quaternion_and_bone_eval_match_world_vertex_contract() {
        let v = rotate_quat(
            [1., 0., 0.],
            [
                0.,
                0.,
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
        );
        assert!(v[0].abs() < 1e-6 && (v[1] - 1.).abs() < 1e-6);
    }
    fn cube_draw() -> GpuWorldDraw {
        let vertices: Vec<_> = cube()
            .into_iter()
            .flat_map(|t| t.p)
            .map(|p| GpuWorldVertex {
                position: p,
                normal: normalize3(p),
                outline_normal: normalize3(p),
                tangent: [1., 0., 0.],
                bitangent: [0., 1., 0.],
                joints: [0.; 4],
                weights: [0.; 4],
                uv: [0.; 2],
                color: [1.; 4],
            })
            .collect();
        let indices = (0..vertices.len() as u32).collect::<Vec<_>>();
        let key = GpuWorldResourceKey {
            model_path: PathBuf::from("cube"),
            binding_actor: None,
            draw_key: GpuWorldDrawKey {
                material: None,
                texture: None,
                mesh: None,
                mesh_node: None,
            },
        };
        let texture = Arc::new(GpuWorldTexture::new(1, 1, vec![255, 255, 255, 255]));
        let params = GpuWorldParams {
            model: [0., 0., 0., 1.],
            actor_rotation: [0., 0., 0., 1.],
            style: [1., 1., 1., 0.],
            material0: [0., 1., 1., 1.],
            material2: [1., 1., 1., 0.],
            material3: [0., 0., 1., 0.],
            material4: [1.; 4],
            material5: [1., 1., 0., 0.],
            material6: [0., 1.5, 0., 1000000.],
            material7: [1., 1., 1., 0.],
            material8: [1., 0., 18., 0.],
            material11: [1., 0., 0., 0.],
            ..Default::default()
        };
        GpuWorldDraw {
            instance_key: GpuWorldInstanceKey {
                actor_id: "cube".into(),
                resource_key: key.clone(),
            },
            resource_key: key,
            vertex_signature: gpu_world_geometry_signature(&vertices, &indices),
            vertices: Arc::new(vertices),
            indices: Arc::new(indices),
            texture: texture.clone(),
            normal_texture: texture.clone(),
            metallic_roughness_texture: texture.clone(),
            emissive_texture: texture.clone(),
            occlusion_texture: texture.clone(),
            cel_texture: texture,
            bone_matrices: Vec::new(),
            params,
            phase: GpuWorldDrawPhase::Opaque,
            depth_write: true,
            sort_priority: 0,
            camera_depth: 0.,
        }
    }
    #[test]
    fn scene_cache_ignores_camera_but_refits_motion_and_rejects_invalid_solid_budget() {
        let mut draw = cube_draw();
        let mut cache = HybridSceneCache::default();
        let first = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        let signature = first.signature;
        assert_eq!(first.triangle_count, 12);
        assert!(!first.cache_hit);
        draw.params.camera0 = [12., 3., 9., 500.];
        draw.params.canvas = [1920., 1080., 960., 540.];
        let same = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        assert!(same.cache_hit);
        assert_eq!(same.signature, signature);
        draw.params.vegetation[3] = 12.0;
        let idle_time = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        assert!(idle_time.cache_hit);
        assert_eq!(idle_time.signature, signature);
        draw.params.actor[0] = 4.;
        let moved = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        assert!(moved.refit);
        assert!(!moved.cache_hit);
        assert_eq!(moved.vectors[moved.vectors[0][0] as usize][0], 3.);
        let mut flags = HashMap::new();
        flags.insert("cube".to_owned(), [false, true]);
        let flagged = cache
            .update_with_shadow_flags(std::slice::from_ref(&draw), 100, &flags, true)
            .unwrap();
        assert!(!flagged.cache_hit);
        let triangle_base = flagged.vectors[0][2] as usize;
        assert_eq!(flagged.vectors[triangle_base + 10][2], 0.0);
        assert_eq!(flagged.vectors[triangle_base + 10][3], 1.0);
        assert!(matches!(
            cache.update(std::slice::from_ref(&draw), 4),
            Err(HybridGeometryError::TriangleBudget {
                actual: 12,
                limit: 4
            })
        ));
        draw.params.model[3] = -1.;
        assert!(
            matches!(cache.update(std::slice::from_ref(&draw),100),Err(HybridGeometryError::InvalidSolid{volume,..}) if volume < 0.0)
        );
        draw.params.model[3] = 1.;
        Arc::make_mut(&mut draw.indices).truncate(33);
        draw.vertex_signature += 1;
        assert!(matches!(
            cache.update(std::slice::from_ref(&draw), 100),
            Err(HybridGeometryError::InvalidSolid {
                boundary_edges: 3,
                ..
            })
        ));
    }

    #[test]
    fn secondary_shadow_certificate_rejects_blended_casters_and_tracks_phase_changes() {
        let mut draw = cube_draw();
        let mut cache = HybridSceneCache::default();
        assert_eq!(cache.update(&[draw.clone()], 100).unwrap().vectors[2][3], 1.0);
        draw.phase = GpuWorldDrawPhase::AlphaBlend;
        let changed = cache.update(&[draw.clone()], 100).unwrap();
        assert!(!changed.cache_hit);
        assert_eq!(changed.vectors[2][3], 0.0);
        let flags = HashMap::from([("cube".into(), [false, true])]);
        let excluded = cache.update_with_shadow_flags(&[draw.clone()], 100, &flags, true).unwrap();
        assert_eq!(excluded.vectors[2][3], 1.0);
        draw.params.material6[0] = 1.0;
        assert_eq!(cache.update(&[draw], 100).unwrap().vectors[2][3], 1.0);
    }

    #[test]
    fn near_clip_certificate_uses_evaluated_casting_opaque_triangle_bounds() {
        let mut draw = cube_draw();
        let mut cache = HybridSceneCache::default();
        let scene = cache.update(&[draw.clone()], 100).unwrap();
        assert!(!scene.opaque_caster_overlaps([-0.01; 3], [0.01; 3]));
        assert!(scene.opaque_caster_overlaps([0.99, -0.01, -0.01], [1.01, 0.01, 0.01]));
        draw.params.actor[0] = 5.0;
        let scene = cache.update(&[draw.clone()], 100).unwrap();
        assert!(!scene.opaque_caster_overlaps([0.99, -0.01, -0.01], [1.01, 0.01, 0.01]));
        assert!(scene.opaque_caster_overlaps([5.99, -0.01, -0.01], [6.01, 0.01, 0.01]));
        draw.params.material6[0] = 1.0;
        let scene = cache.update(&[draw], 100).unwrap();
        assert!(scene.opaque_caster_bounds.is_none());
        assert!(!scene.opaque_caster_overlaps([5.99, -0.01, -0.01], [6.01, 0.01, 0.01]));
    }

    #[test]
    fn packed_color_tiles_certify_alpha_without_overwriting_material_channel_swizzle() {
        let mut draw = cube_draw();
        let mut cache = HybridSceneCache::default();
        let scene = cache.update(&[draw.clone()], 100).unwrap();
        let material = scene.vectors[1][0] as usize;
        assert_eq!(scene.vectors[material + 6][3], 1.0);
        assert_eq!(scene.vectors[material + 7][3], draw.params.material8[2]);
        draw.texture = Arc::new(GpuWorldTexture::new(2, 1, vec![255, 255, 255, 255, 255, 255, 255, 127]));
        let scene = cache.update(&[draw], 100).unwrap();
        let material = scene.vectors[1][0] as usize;
        assert_eq!(scene.vectors[material + 6][3], 0.0);
    }

    #[test]
    fn active_wind_time_refits_secondary_geometry() {
        let mut draw = cube_draw();
        // Wind can distort a closed volume, so this fixture tests ordinary
        // opaque geometry rather than the independent solid-closure contract.
        draw.params.material11[0] = 0.0;
        draw.params.vegetation = [1.0, 2.0, 0.0, 0.0];
        let mut cache = HybridSceneCache::default();
        let first = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        let signature = first.signature;
        let geometry = first.vectors.clone();
        draw.params.vegetation[3] = 0.7;
        let moved = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        assert!(!moved.cache_hit);
        assert!(moved.refit);
        assert_ne!(moved.signature, signature);
        assert_ne!(moved.vectors, geometry);
    }
    #[test]
    fn slab_crossing_metadata_distinguishes_closed_meshes_from_open_surfaces() {
        let mut draw = cube_draw();
        draw.params.material11[0] = 0.0;
        draw.params.material6[0] = 1.0;
        let mut cache = HybridSceneCache::default();
        let closed = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        let base = closed.vectors[0][2] as usize;
        assert_eq!(closed.vectors[base + 2][3], 1.0);
        Arc::make_mut(&mut draw.indices).truncate(33);
        draw.vertex_signature += 1;
        let open = cache.update(std::slice::from_ref(&draw), 100).unwrap();
        let base = open.vectors[0][2] as usize;
        assert_eq!(open.vectors[base + 2][3], 0.0);
    }
    #[test]
    fn shadow_query_masks_refit_material_and_cast_changes_without_losing_solid_ownership() {
        let mut draw = cube_draw();
        draw.params.material6[0] = 1.0;
        let mut flags = HashMap::from([("cube".to_owned(), [false, true])]);
        let mut cache = HybridSceneCache::default();
        let noncasting = cache
            .update_with_shadow_flags(std::slice::from_ref(&draw), 100, &flags, true)
            .unwrap();
        let node_base = noncasting.vectors[0][0] as usize;
        // Solid pixel ownership is independent of shadow casting.
        assert_eq!(
            noncasting.vectors[node_base + 2][1] as u32,
            QUERY_SOLID_TRANSMISSION
        );
        flags.insert("cube".to_owned(), [true, true]);
        let casting = cache
            .update_with_shadow_flags(std::slice::from_ref(&draw), 100, &flags, true)
            .unwrap();
        assert!(casting.refit);
        assert_eq!(casting.vectors[node_base + 2][1] as u32, 7);
        draw.params.material6[0] = 0.0;
        let opaque = cache
            .update_with_shadow_flags(std::slice::from_ref(&draw), 100, &flags, true)
            .unwrap();
        assert!(opaque.refit);
        assert_eq!(opaque.vectors[node_base + 2][1] as u32, QUERY_CAST_SHADOW);
        flags.insert("cube".to_owned(), [false, true]);
        let excluded = cache
            .update_with_shadow_flags(std::slice::from_ref(&draw), 100, &flags, true)
            .unwrap();
        for node in 0..excluded.node_count {
            assert_eq!(excluded.vectors[node_base + node * NODE_STRIDE + 2][1], 0.0);
        }
    }
    #[test]
    fn casting_glass_tree_remaps_only_casting_transmission_without_removing_reflection_geometry() {
        let mut draws = Vec::new();
        let mut flags = HashMap::new();
        for index in 0..14 {
            let mut draw = cube_draw();
            draw.instance_key.actor_id = format!("opaque-{index}");
            draw.params.actor[2] = index as f32 * 3.0;
            draw.params.material11[0] = 0.0;
            flags.insert(draw.instance_key.actor_id.clone(), [true, true]);
            draws.push(draw);
        }
        let mut glass = cube_draw();
        glass.instance_key.actor_id = "glass".into();
        glass.params.actor[2] = 46.0;
        glass.params.material11[0] = 0.0;
        glass.params.material6[0] = 1.0;
        flags.insert("glass".into(), [true, true]);
        draws.push(glass);
        let mut cache = HybridSceneCache::default();
        let first = cache.update_with_shadow_flags(&draws, 1000, &flags, true).unwrap();
        assert_eq!(first.vectors[0][0], HEADER_VECTORS as f32);
        assert_eq!(first.triangle_count, 15 * 12);
        assert_eq!(first.vectors[3][3], 12.0);
        assert!(first.vectors[3][1] > 0.0);
        let remap_base = first.vectors[3][2] as usize;
        let glass_id = first.object_id(&draws[14]);
        let mut mapped = HashSet::new();
        for index in 0..12 {
            let triangle = first.vectors[remap_base + index / 4][index % 4] as usize;
            assert!(triangle < first.triangle_count);
            assert!(mapped.insert(triangle), "caster triangle was duplicated");
            let base = first.vectors[0][2] as usize + triangle * TRIANGLE_STRIDE;
            assert_eq!(first.vectors[base + 1][3], glass_id as f32);
        }
        let signature = first.signature;
        let same = cache.update_with_shadow_flags(&draws, 1000, &flags, true).unwrap();
        assert!(same.cache_hit);
        assert_eq!(same.signature, signature);
        draws[14].params.actor[0] = 5.0;
        let moved = cache.update_with_shadow_flags(&draws, 1000, &flags, true).unwrap();
        assert!(moved.refit);
        assert_eq!(moved.vectors[moved.vectors[3][0] as usize][0], 4.0);
        flags.insert("glass".into(), [false, true]);
        let excluded = cache.update_with_shadow_flags(&draws, 1000, &flags, true).unwrap();
        assert!(excluded.refit);
        assert_eq!(excluded.triangle_count, 15 * 12);
        assert_eq!(excluded.vectors[3][1], 0.0);
        assert_eq!(excluded.vectors[3][3], 0.0);
        assert_eq!(excluded.vectors[2][0], 0.0);
    }
    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn hybrid_complete_coverage_specialization_preserves_edges_and_uncertified_cutoffs() {
        use wgpu::util::DeviceExt;
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&Default::default());
            let adapter = request_adapter_async(&instance, &Default::default()).await
                .expect("complete coverage contract adapter");
            let (device, queue) = request_device_async(&adapter, &wgpu::DeviceDescriptor {
                label: Some("complete-coverage-contract"),
                required_features: wgpu::Features::empty(), required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance, trace: wgpu::Trace::Off,
            }).await.expect("complete coverage contract device");
            let device = Arc::new(device);
            let poller = DevicePoller::start(Arc::clone(&device));
            let mut covered = cube_draw();
            covered.params.material6[0] = 1.0;
            let mut high_cutoff = covered.clone();
            high_cutoff.params.material7[3] = 1.0;
            let mut partial_vertex = covered.clone();
            for vertex in Arc::make_mut(&mut partial_vertex.vertices) { vertex.color[3] = 0.5; }
            partial_vertex.vertex_signature += 1;
            partial_vertex.params.material7[3] = 0.75;
            let mut partial_texture = covered.clone();
            partial_texture.texture = Arc::new(GpuWorldTexture::new(1, 1, vec![255, 255, 255, 128]));
            partial_texture.params.material7[3] = 0.75;
            let mut cutoff_reserve = covered.clone();
            cutoff_reserve.params.material7[3] = 0.9999;
            let transport = include_str!("shaders/hybrid_transport.wgsl");
            let prefix = &transport[..transport.find("fn hybrid_environment").unwrap()];
            let ownership = &transport[transport.find("// The nearest solid").unwrap()..];
            const CASES: u32 = 6;
            const RESULT_BYTES: u64 = CASES as u64 * 32;
            for (draw, expected_evaluation) in [(covered, false), (cutoff_reserve, false),
                (high_cutoff, true), (partial_vertex, true), (partial_texture, true)]
            {
                let needs_alpha = super::super::shader_specialization::requires_alpha_evaluation(
                    std::slice::from_ref(&draw));
                assert_eq!(needs_alpha, expected_evaluation);
                let mut cache = HybridSceneCache::default();
                let scene = cache.update(std::slice::from_ref(&draw), 100).unwrap();
                let modes = if needs_alpha { vec![true] } else { vec![true, false] };
                let mut reference = None;
                for alpha_enabled in modes {
                    let source = format!("{prefix}\n{ownership}\n{}", r#"
struct TestLighting { surface0: vec4<f32>, surface1: vec4<f32> };
var<private> lighting: TestLighting;
@group(0) @binding(0) var<storage,read_write> output: array<vec4<f32>>;
const CASE_ORIGINS: array<vec3<f32>,6> = array<vec3<f32>,6>(
    vec3<f32>(0.0,0.0,-3.0), vec3<f32>(1.0,0.0,-3.0), vec3<f32>(0.0,1.0,-3.0),
    vec3<f32>(1.0,1.0,-3.0), vec3<f32>(-1.0,-1.0,-3.0), vec3<f32>(0.99999988079071,1.0,-3.0));
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let origin = CASE_ORIGINS[id.x];
    let direction = vec3<f32>(0.0,0.0,1.0);
    let hit = hybrid_intersect(origin,direction,100.0);
    output[id.x*2u] = vec4<f32>(hit.distance,select(0.0,1.0,hit.valid),
        select(0.0,1.0,hit.exhausted),f32(hybrid_work_remaining));
    let owner = primary_solid_interface_visible(origin,origin+direction*2.0,direction,0u);
    var shaded_alpha = 0.0;
    if (hit.valid) { shaded_alpha = hybrid_surface(hit,origin,direction).alpha; }
    output[id.x*2u+1u] = vec4<f32>(select(0.0,1.0,owner),f32(hybrid_work_remaining),
        shaded_alpha,f32(hit.triangle));
}"#).replace("override HYBRID_ALPHA_EVALUATION_ENABLED: bool = true;",
                        &format!("const HYBRID_ALPHA_EVALUATION_ENABLED: bool = {alpha_enabled};"));
                    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("actual-complete-coverage-functions"),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    });
                    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some("complete-coverage-contract"), layout: None, module: &module,
                        entry_point: Some("main"), compilation_options: Default::default(), cache: None,
                    });
                    let output = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("complete-coverage-output"), size: RESULT_BYTES,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    });
                    let readback = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("complete-coverage-readback"), size: RESULT_BYTES,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let output_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("complete-coverage-output"), layout: &pipeline.get_bind_group_layout(0),
                        entries: &[wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() }],
                    });
                    let scene_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("complete-coverage-scene"), contents: &scene.bytes(),
                        usage: wgpu::BufferUsages::STORAGE,
                    });
                    let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("complete-coverage-scene"), layout: &pipeline.get_bind_group_layout(1),
                        entries: &[wgpu::BindGroupEntry { binding: 20, resource: scene_buffer.as_entire_binding() }],
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                            label: Some("complete-coverage-contract"), timestamp_writes: None,
                        });
                        pass.set_pipeline(&pipeline);
                        pass.set_bind_group(0, &output_group, &[]);
                        pass.set_bind_group(1, &scene_group, &[]);
                        pass.dispatch_workgroups(CASES, 1, 1);
                    }
                    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, RESULT_BYTES);
                    queue.submit([encoder.finish()]);
                    BufferMapAsyncFuture::new(&poller, &readback).await.unwrap();
                    let bits = {
                        let mapped = readback.slice(..).get_mapped_range();
                        mapped.chunks_exact(4).map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
                            .collect::<Vec<_>>()
                    };
                    readback.unmap();
                    let value = |index: usize| f32::from_bits(bits[index]);
                    if needs_alpha {
                        assert_eq!(value(1), 0.0, "uncertified centre hit must retain strict alpha rejection");
                        assert_eq!(value(4), 0.0, "uncertified solid must retain ownership alpha rejection");
                    } else {
                        assert_eq!(value(0), 2.0);
                        assert_eq!(value(1), 1.0);
                        assert_eq!(value(4), 1.0);
                        assert_eq!(value(6), 1.0, "full shaded alpha remains exactly evaluated");
                    }
                    if let Some(expected) = &reference {
                        assert_eq!(&bits, expected,
                            "static coverage changed edge hits, triangle ties, ownership, budgets or shaded alpha");
                    } else { reference = Some(bits); }
                }
            }
        });
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn hybrid_primary_glass_visibility_skips_opaque_and_noncasting_interfaces() {
        use crate::common::gpu_async::{
            BufferMapAsyncFuture, DevicePoller, request_adapter_async, request_device_async,
        };
        use wgpu::util::DeviceExt;

        pollster::block_on(async {
            let instance = wgpu::Instance::new(&Default::default());
            let adapter = request_adapter_async(&instance, &Default::default())
                .await
                .expect("native visibility adapter");
            let (device, queue) = request_device_async(
                &adapter,
                &wgpu::DeviceDescriptor {
                    label: Some("filtered-glass-visibility-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: adapter.limits(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                },
            )
            .await
            .expect("native visibility device");
            let device = Arc::new(device);
            let poller = DevicePoller::start(Arc::clone(&device));
            let transport = include_str!("shaders/hybrid_transport.wgsl");
            // Compile the actual intersection, material, Fresnel/Beer and
            // visibility functions, isolated from unrelated lighting shaders.
            let prefix = &transport[..transport.find("fn hybrid_environment").unwrap()];
            let optics = &transport[transport.find("fn hybrid_fresnel").unwrap()
                ..transport.find("fn hybrid_light_visibility").unwrap()];
            let visibility = &transport[transport.find("fn hybrid_transmission_visibility").unwrap()
                ..transport.find("// The nearest solid").unwrap()];
            let ownership = &transport[transport.find("// The nearest solid").unwrap()..];
            let source = format!(
                "{prefix}\n{optics}\n{visibility}\n{ownership}\n{}",
                r#"struct TestLighting { surface0: vec4<f32>, surface1: vec4<f32> };
var<private> lighting: TestLighting;
@group(0) @binding(0) var<storage, read_write> output: array<vec4<f32>>;
@compute @workgroup_size(1) fn main() {
    let origin = vec3<f32>(0.0,0.0,-3.0);
    let direction = vec3<f32>(0.0,0.0,1.0);
    // The production header uses zero/one for the opaque shadow-map proof.
    // Fixture budget modes deliberately occupy values above that flag.
    if (hybrid_scene[2].w > 3.5) {
        // Isolate two per-ray checks from the ordinary multi-query fixture.
        // Two complete 2,048-node traversals fit the 4,096 invocation budget;
        // unrelated queries must not exhaust it before the assertion below.
        let visibility = hybrid_transmission_visibility(origin,direction,100.0);
        let hit = hybrid_intersect_filtered(origin,direction,100.0,1u);
        output[0] = vec4<f32>(visibility,1.0);
        output[1] = vec4<f32>(0.0);
        output[2] = vec4<f32>(0.0);
        output[3] = vec4<f32>(0.0,0.0,0.0,select(0.0,1.0,hit.exhausted));
        return;
    }
    // A separate invocation branch starts its cumulative budget untouched by
    // the ordinary visibility/nearest-hit assertions below.
    if (hybrid_scene[2].w > 1.5) {
        if (hybrid_scene[2].w > 2.5) {
            // No solid nodes are present: ownership consumes exactly the root
            // visit, then skips its subtree. That visit shares the ray budget.
            _ = primary_solid_interface_visible(origin,vec3<f32>(0.0,0.0,1.0),direction,0u);
        }
        var completed = 0u;
        for (var query = 0u; query < 8u; query += 1u) {
            let hit = hybrid_intersect_filtered(origin,direction,100.0,1u);
            if (hit.exhausted) { break; }
            completed += 1u;
        }
        let pending = hybrid_intersect_filtered(origin,direction,100.0,1u);
        output[0] = vec4<f32>(f32(completed),select(0.0,1.0,pending.exhausted),
            f32(hybrid_work_remaining),select(0.0,1.0,hybrid_work_initialized));
        output[1] = vec4<f32>(hybrid_trace_visibility(origin,direction,100.0,false),1.0);
        let owner = primary_solid_interface_visible(origin,vec3<f32>(0.0,0.0,1.0),direction,0u);
        output[2] = vec4<f32>(select(0.0,1.0,owner),f32(hybrid_work_remaining),
            select(0.0,1.0,hybrid_work_exhausted),1.0);
        output[3] = vec4<f32>(0.0);
        return;
    }
    output[0] = vec4<f32>(hybrid_transmission_visibility(origin,direction,100.0),1.0);
    output[1] = vec4<f32>(hybrid_trace_visibility(origin,direction,100.0,true),1.0);
    output[2] = vec4<f32>(hybrid_intersect(origin,direction,100.0).distance,
        hybrid_intersect_filtered(origin,direction,100.0,1u).distance,
        hybrid_intersect_filtered(origin,direction,100.0,4u).distance,1.0);
    output[3] = vec4<f32>(hybrid_trace_visibility(origin,direction,49.0,true),
        select(0.0,1.0,hybrid_intersect_filtered(origin,direction,100.0,1u).exhausted));
}"#
            );
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("actual-filtered-glass-visibility"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("filtered-glass-visibility"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("visibility-results"),
                size: 64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("visibility-readback"),
                size: 64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let result_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("visibility-output"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: output.as_entire_binding(),
                }],
            });
            let mut draws = Vec::new();
            let mut flags = HashMap::new();
            for index in 0..14 {
                let mut draw = cube_draw();
                draw.instance_key.actor_id = format!("opaque-{index}");
                draw.params.actor[2] = index as f32 * 3.0;
                draw.params.material11[0] = 0.0;
                flags.insert(draw.instance_key.actor_id.clone(), [true, true]);
                draws.push(draw);
            }
            // A noncasting tinted sheet must also be excluded from the
            // interface budget and absorption, even though it is transparent.
            let mut noncaster = cube_draw();
            noncaster.instance_key.actor_id = "noncasting-glass".into();
            noncaster.params.actor[2] = 43.0;
            noncaster.params.material11[0] = 0.0;
            noncaster.params.material6 = [1.0, 1.5, 0.5, 1.0];
            noncaster.params.material7 = [0.01, 0.01, 0.01, 0.0];
            flags.insert(noncaster.instance_key.actor_id.clone(), [false, true]);
            draws.push(noncaster);
            let mut glass = cube_draw();
            glass.instance_key.actor_id = "casting-glass".into();
            glass.params.actor[2] = 46.0;
            glass.params.material11[0] = 0.0;
            glass.params.material6 = [1.0, 1.5, 0.5, 1.0];
            glass.params.material7 = [0.25, 0.64, 1.0, 0.0];
            flags.insert(glass.instance_key.actor_id.clone(), [true, true]);
            draws.push(glass);
            // This opaque blocker lies farther than the tinted glass. The
            // shadow any-hit path must not shorten its interval to that glass,
            // and an emitter before this blocker must still see the tint.
            let mut far_opaque = cube_draw();
            far_opaque.instance_key.actor_id = "far-opaque".into();
            far_opaque.params.actor[2] = 49.0;
            far_opaque.params.material11[0] = 0.0;
            draws.push(far_opaque);
            let mut cache = HybridSceneCache::default();
            for (opaque_casts, far_opaque_casts) in [(true, false), (false, false), (false, true)] {
                for index in 0..14 {
                    flags.insert(format!("opaque-{index}"), [opaque_casts, true]);
                }
                flags.insert("far-opaque".into(), [far_opaque_casts, true]);
                let scene = cache
                    .update_with_shadow_flags(&draws, 1000, &flags, true)
                    .unwrap();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("visibility-scene"),
                    contents: &scene.bytes(),
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("visibility-scene"),
                    layout: &pipeline.get_bind_group_layout(1),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 20,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("visibility-contract"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &result_group, &[]);
                    pass.set_bind_group(1, &scene_group, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 64);
                queue.submit([encoder.finish()]);
                BufferMapAsyncFuture::new(&poller, &readback).await.unwrap();
                let values = {
                    let mapped = readback.slice(..).get_mapped_range();
                    std::array::from_fn::<_, 16, _>(|i| {
                        f32::from_ne_bytes(mapped[i * 4..i * 4 + 4].try_into().unwrap())
                    })
                };
                readback.unmap();
                for (channel, expected) in [0.4608, 0.73728, 0.9216].into_iter().enumerate() {
                    assert!(
                        (values[channel] - expected).abs() < 0.0002,
                        "primary shadow included unrelated interfaces: {values:?}"
                    );
                    let secondary = if opaque_casts || far_opaque_casts { 0.0 } else { expected };
                    assert!(
                        (values[4 + channel] - secondary).abs() < 0.0002,
                        "secondary casting semantics changed: {values:?}"
                    );
                    let finite = if opaque_casts { 0.0 } else { expected };
                    assert!(
                        (values[12 + channel] - finite).abs() < 0.0002,
                        "opaque caster beyond the emitter changed finite visibility: {values:?}"
                    );
                }
                assert!(
                    (values[8] - 2.0).abs() < 0.0002,
                    "unrestricted reflection lost opaque geometry: {values:?}"
                );
                assert!(
                    (values[9] - 48.0).abs() < 0.0002,
                    "filtered nearest hit was not casting glass: {values:?}"
                );
                if far_opaque_casts {
                    assert!(
                        (values[10] - 51.0).abs() < 0.0002,
                        "secondary any-hit missed the opaque blocker behind nearer glass: {values:?}"
                    );
                }
            }
            // A completed empty traversal is visible even when its exact node
            // count consumes the entire budget. One additional pending node
            // must instead be reported as unresolved and block visibility.
            let base_vectors = cache.scene.as_ref().unwrap().vectors.clone();
            for (node_count, cumulative_mode) in [(2048, 3), (2049, 3), (2048, 1), (2048, 2)] {
                let mut vectors = base_vectors.clone();
                let node_base = vectors.len();
                for _ in 0..node_count {
                    vectors.push([-1.0, -1.0, 0.0, 0.0]);
                    vectors.push([1.0, 1.0, 1.0, 0.0]);
                    vectors.push([node_count as f32, QUERY_CAST_TRANSMISSION as f32, 0.0, 0.0]);
                }
                vectors[2][0] = 1.0;
                vectors[2][3] = (cumulative_mode + 1) as f32;
                vectors[3] = [node_base as f32, node_count as f32, vectors.len() as f32, 0.0];
                let bytes: Vec<_> = vectors.iter().flatten().flat_map(|v| v.to_ne_bytes()).collect();
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("visibility-node-budget-scene"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                });
                let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("visibility-node-budget-scene"),
                    layout: &pipeline.get_bind_group_layout(1),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 20,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("visibility-node-budget-contract"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &result_group, &[]);
                    pass.set_bind_group(1, &scene_group, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, 64);
                queue.submit([encoder.finish()]);
                BufferMapAsyncFuture::new(&poller, &readback).await.unwrap();
                let values = {
                    let mapped = readback.slice(..).get_mapped_range();
                    std::array::from_fn::<_, 16, _>(|i| {
                        f32::from_ne_bytes(mapped[i * 4..i * 4 + 4].try_into().unwrap())
                    })
                };
                readback.unmap();
                if cumulative_mode < 3 {
                    let completed = if cumulative_mode == 1 { 2.0 } else { 1.0 };
                    assert_eq!(&values[..4], &[completed, 1.0, 0.0, 1.0],
                        "cumulative budget reset or failed to include ownership work: {values:?}");
                    assert_eq!(&values[4..7], &[0.0; 3],
                        "cumulative exhaustion leaked shadow light: {values:?}");
                    assert_eq!(&values[8..11], &[0.0, 0.0, 1.0],
                        "later ownership query reset the exhausted budget: {values:?}");
                    continue;
                }
                let expected = if node_count == 2048 { 1.0 } else { 0.0 };
                for channel in 0..3 {
                    assert!((values[channel] - expected).abs() < 0.00001,
                        "glass visibility confused a completed traversal with node exhaustion ({node_count}): {values:?}");
                }
                assert!((values[15] - (1.0 - expected)).abs() < 0.00001,
                    "intersection did not preserve node-budget exhaustion ({node_count}): {values:?}");
            }
        });
    }
    #[test]
    fn color_tile_downsampling_preserves_linear_light_and_texture_coordinates_transform_once() {
        let texture = GpuWorldTexture::new(
            512,
            1,
            (0..512)
                .flat_map(|i| {
                    if i % 2 == 0 {
                        [0, 0, 0, 255]
                    } else {
                        [255, 255, 255, 255]
                    }
                })
                .collect::<Vec<u8>>(),
        );
        let (w, h, p) = downsample_texture(&texture, true);
        assert_eq!((w, h), (256, 1));
        assert_eq!(p[0].to_le_bytes(), [188, 188, 188, 255]);
        let p = GpuWorldParams {
            material5: [2., 3., 0.1, 0.2],
            material3: [0.4, 0.5, 0., 1.],
            ..Default::default()
        };
        let uv = transformed_uv([0.25, 0.5], p);
        assert!((uv[0] + 1.0).abs() < 1e-6 && (uv[1] - 1.2).abs() < 1e-6);
    }
}
