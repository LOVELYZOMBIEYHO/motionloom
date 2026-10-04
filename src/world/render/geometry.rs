// =========================================
// =========================================
// src/world/render/geometry.rs

use super::*;
use crate::experimental::geometry::{GeometryError, ResolvedMesh};

impl WorldFrameRenderer {
    /// CPU review uses the same resolved draw geometry and projection as the GPU path.
    pub(crate) fn draw_orthographic_diagnostic(
        &mut self,
        canvas: &mut RgbaImage,
        graph: &WorldGraph,
        frame: u32,
        root: &Path,
    ) -> Result<(), WorldRenderError> {
        let world = graph
            .presented_world()
            .ok_or_else(|| WorldRenderError::MissingWorld(graph.present.from.clone()))?;
        let (width, height) = canvas.dimensions();
        let (draws, _, _, _) = build_actor_gpu_draws(
            None,
            width,
            height,
            false,
            false,
            graph,
            world,
            root,
            self.asset_resolver.as_ref(),
            WorldTime {
                frame,
                fps: graph.fps,
                duration_ms: graph.duration_ms,
            },
            &mut self.mesh_cache,
            &mut self.primitive_texture_cache,
            &mut self.effective_bounds_cache,
            &mut self.gpu_static_draw_cache,
            &mut self.skinning_strategy_cache,
            &[],
        )?;
        let mut depth = vec![f32::INFINITY; (width * height) as usize];
        for draw in &draws {
            let p = draw.params;
            let mut ignored = 0;
            let projected: Vec<[f32; 3]> = draw
                .vertices
                .iter()
                .map(|v| {
                    let local = simulate_gpu_vertex_skinning(v, &draw.bone_matrices, &mut ignored);
                    let local = std::array::from_fn(|i| (local[i] - p.model[i]) * p.model[3]);
                    let world = quat_rotate_vec3(quat_normalize_xyzw(p.actor_rotation), local);
                    let rel =
                        std::array::from_fn::<_, 3, _>(|i| world[i] + p.actor[i] - p.camera0[i]);
                    let view = [p.camera1, p.camera2, p.camera3]
                        .map(|basis| (0..3).map(|i| rel[i] * basis[i]).sum::<f32>());
                    [
                        width as f32 * 0.5 + view[0] * p.camera0[3],
                        height as f32 * 0.5 - view[1] * p.camera0[3],
                        view[2],
                    ]
                })
                .collect();
            for ids in draw.indices.chunks_exact(3) {
                let points = [ids[0], ids[1], ids[2]].map(|i| projected[i as usize]);
                if points
                    .iter()
                    .any(|p| p[2] <= draw.params.camera1[3] || p[2] >= draw.params.camera2[3])
                {
                    continue;
                }
                let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
                    (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
                };
                let area = edge(points[0], points[1], points[2][0], points[2][1]);
                if area.abs() < 1e-8 {
                    continue;
                }
                let min_x = points
                    .iter()
                    .map(|p| p[0])
                    .fold(f32::INFINITY, f32::min)
                    .floor()
                    .max(0.) as u32;
                let max_x = points
                    .iter()
                    .map(|p| p[0])
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil()
                    .min(width as f32 - 1.) as u32;
                let min_y = points
                    .iter()
                    .map(|p| p[1])
                    .fold(f32::INFINITY, f32::min)
                    .floor()
                    .max(0.) as u32;
                let max_y = points
                    .iter()
                    .map(|p| p[1])
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil()
                    .min(height as f32 - 1.) as u32;
                for y in min_y..=max_y {
                    for x in min_x..=max_x {
                        let a = edge(points[1], points[2], x as f32 + 0.5, y as f32 + 0.5) / area;
                        let b = edge(points[2], points[0], x as f32 + 0.5, y as f32 + 0.5) / area;
                        let c = 1. - a - b;
                        if a < 0. || b < 0. || c < 0. {
                            continue;
                        }
                        let z = a * points[0][2] + b * points[1][2] + c * points[2][2];
                        let index = (y * width + x) as usize;
                        if z < depth[index] {
                            depth[index] = z;
                            let weights = [a, b, c];
                            let color = std::array::from_fn::<_, 4, _>(|channel| {
                                (0..3)
                                    .map(|i| {
                                        draw.vertices[ids[i] as usize].color[channel] * weights[i]
                                    })
                                    .sum::<f32>()
                            });
                            // This diagnostic path intentionally omits the GPU cel/texture shader.
                            canvas.put_pixel(
                                x,
                                y,
                                Rgba([
                                    (color[0].clamp(0., 1.).powf(1. / 2.2) * 255.) as u8,
                                    (color[1].clamp(0., 1.).powf(1. / 2.2) * 255.) as u8,
                                    (color[2].clamp(0., 1.).powf(1. / 2.2) * 255.) as u8,
                                    (color[3].clamp(0., 1.) * 255.) as u8,
                                ]),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) fn extract_asset_meshes(
        &mut self,
        graph: &WorldGraph,
        frame: u32,
        root: &Path,
        overrides: &[WorldMaterialTextureOverride],
        selected_model_ids: Option<&[String]>,
        allow_terrain_vegetation: bool,
    ) -> Result<Vec<ResolvedMesh>, GeometryError> {
        let world = graph
            .presented_world()
            .ok_or_else(|| GeometryError::Invalid("missing world".into()))?;
        // Offline callers opt in so the snapshot API keeps rejecting
        // camera-dependent terrain/vegetation until it has explicit tooling.
        if !allow_terrain_vegetation
            && world
                .actor_slice()
                .iter()
                .any(|a| a.terrain.is_some() || a.vegetation.is_some())
        {
            return Err(GeometryError::Unsupported(
                "terrain/vegetation snapshot".into(),
            ));
        }
        let (draws, _, _, _) = build_actor_gpu_draws(
            None,
            1,
            1,
            false,
            false,
            graph,
            world,
            root,
            self.asset_resolver.as_ref(),
            WorldTime {
                frame,
                fps: graph.fps,
                duration_ms: graph.duration_ms,
            },
            &mut self.mesh_cache,
            &mut self.primitive_texture_cache,
            &mut self.effective_bounds_cache,
            &mut self.gpu_static_draw_cache,
            &mut self.skinning_strategy_cache,
            overrides,
        )
        .map_err(|e| GeometryError::Evaluation(e.to_string()))?;
        let mut result = Vec::new();
        for d in draws {
            if d.params.style[0] <= 0.0 {
                continue;
            }
            if selected_model_ids
                .is_some_and(|ids| !ids.iter().any(|id| id == &d.instance_key.actor_id))
            {
                continue;
            }
            let original = &self.mesh_cache[&d.resource_key.model_path];
            let mut material = d
                .resource_key
                .draw_key
                .material
                .and_then(|i| original.materials.get(i))
                .cloned()
                .unwrap_or_default();
            // Factors already in vertex colors must not be applied twice.
            material.base_color_factor = [1.0; 4];
            material.occlusion_strength = 1.0;
            material.base_color_texture = Some(0);
            material.normal_texture = Some(1);
            material.metallic_roughness_texture = Some(2);
            material.emissive_texture = Some(3);
            material.occlusion_texture = Some(4);
            if d.params.style[0] < 1.0 {
                material.alpha_mode = crate::world::gltf_loader::GlbAlphaMode::Blend;
            }
            let textures = [
                &d.texture,
                &d.normal_texture,
                &d.metallic_roughness_texture,
                &d.emissive_texture,
                &d.occlusion_texture,
            ]
            .into_iter()
            .map(|t| GlbTextureData {
                width: t.width,
                height: t.height,
                rgba: Arc::clone(&t.rgba),
            })
            .collect::<Vec<_>>();
            let actor = world
                .actor_slice()
                .iter()
                .find(|a| a.id == d.instance_key.actor_id)
                .unwrap();
            // `definition` replaces the PBR scalars; `tint` leaves them alone.
            // Base color already flows through the draw texture and color factor.
            let material_override = actor
                .material_color_overrides
                .iter()
                .find(|value| {
                    material
                        .name
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&value.material))
                })
                .or_else(|| {
                    actor
                        .material_color_overrides
                        .iter()
                        .find(|value| value.material == "*")
                });
            if let Some(value) = material_override {
                if let Some(metallic) = value.metallic {
                    material.metallic_factor = metallic;
                }
                if let Some(roughness) = value.roughness {
                    material.roughness_factor = roughness;
                }
                if let Some(specular) = value.specular {
                    material.specular_factor = specular;
                }
                if let Some(normal_scale) = value.normal_scale {
                    material.normal_scale = normal_scale;
                }
            }
            let fallback = actor.primitive.as_ref().is_some_and(|primitive| {
                crate::world::primitive::generated_control_cage(primitive).is_some_and(|cage| {
                    cage.positions
                        .iter()
                        .zip(&cage.uvs)
                        .all(|(position, uv)| *uv == [position[0], position[1]])
                })
            });
            let mut mesh = ResolvedMesh {
                name: format!("{}:{}", actor.id, result.len()),
                positions: Vec::new(),
                normals: Vec::new(),
                tangents: Vec::new(),
                uvs: Vec::new(),
                colors: Vec::new(),
                indices: d.indices.as_ref().clone(),
                material,
                textures,
                uv_source: if fallback {
                    "fallback_xy"
                } else if original.texcoords.iter().any(Option::is_none) {
                    "missing_or_partial"
                } else {
                    "mesh_uv"
                }
                .into(),
            };
            let rotation = mat4_from_quat(d.params.actor_rotation);
            for v in d.vertices.iter() {
                let mut bad = 0;
                let p = simulate_gpu_vertex_skinning(v, &d.bone_matrices, &mut bad);
                if bad > 0 {
                    return Err(GeometryError::Invalid("invalid skin joint".into()));
                }
                let local = std::array::from_fn(|i| (p[i] - d.params.model[i]) * d.params.model[3]);
                let rotated = mat4_transform_point(rotation, local);
                mesh.positions
                    .push(std::array::from_fn(|i| rotated[i] + d.params.actor[i]));
                let direction = |input: [f32; 3]| {
                    let mut output = input;
                    let sum: f32 = v.weights.iter().sum();
                    if sum > 0.000001 {
                        output = [0.0; 3];
                        for j in 0..4 {
                            if v.weights[j] > 0.0 {
                                let m = d.bone_matrices[(v.joints[j] + 0.5) as usize];
                                let transformed = transform_vector_mat4(m, input);
                                for k in 0..3 {
                                    output[k] += transformed[k] * v.weights[j] / sum;
                                }
                            }
                        }
                    }
                    normalize3(transform_vector_mat4(rotation, output))
                };
                let normal = direction(v.normal);
                let tangent = direction(v.tangent);
                let bitangent = direction(v.bitangent);
                mesh.normals.push(normal);
                mesh.tangents.push([
                    tangent[0],
                    tangent[1],
                    tangent[2],
                    if dot3(cross3(normal, tangent), bitangent) < 0.0 {
                        -1.0
                    } else {
                        1.0
                    },
                ]);
                let u = v.uv[0] * d.params.material5[0];
                let vv = v.uv[1] * d.params.material5[1];
                mesh.uvs.push([
                    u * d.params.material3[2] - vv * d.params.material3[3]
                        + d.params.material5[2]
                        + d.params.material3[0],
                    u * d.params.material3[3]
                        + vv * d.params.material3[2]
                        + d.params.material5[3]
                        + d.params.material3[1],
                ]);
                // glTF vertex colors are linear. Preserve the renderer's factor convention.
                mesh.colors.push(std::array::from_fn(|i| {
                    if i == 3 {
                        v.color[i] * d.params.material4[i] * d.params.style[0]
                    } else {
                        (v.color[i] * d.params.material4[i]).max(0.0).powf(2.2)
                    }
                }));
            }
            result.push(mesh);
        }
        Ok(result)
    }
}

// Use the actual draw uniforms so editing follows normalization and camera animation.
impl WorldFrameRenderer {
    pub(crate) fn mesh_edit_snapshot(
        &mut self,
        graph: &WorldGraph,
        frame: u32,
        root: &Path,
        overrides: &[WorldMaterialTextureOverride],
        model_id: &str,
        size: [u32; 2],
    ) -> Result<serde_json::Value, GeometryError> {
        let world = graph
            .presented_world()
            .ok_or_else(|| GeometryError::Invalid("missing world".into()))?;
        let actor = world
            .actor_slice()
            .iter()
            .find(|a| a.id == model_id)
            .ok_or_else(|| GeometryError::Invalid("missing model".into()))?;
        let asset = actor
            .primitive
            .as_ref()
            .ok_or_else(|| GeometryError::Unsupported("MeshAsset only".into()))?;
        let crate::dsl::PrimitiveGeometry::Mesh { cage } = &asset.geometry else {
            return Err(GeometryError::Unsupported("MeshAsset only".into()));
        };
        let (draws, _, _, _) = build_actor_gpu_draws(
            None,
            size[0],
            size[1],
            false,
            false,
            graph,
            world,
            root,
            self.asset_resolver.as_ref(),
            WorldTime {
                frame,
                fps: graph.fps,
                duration_ms: graph.duration_ms,
            },
            &mut self.mesh_cache,
            &mut self.primitive_texture_cache,
            &mut self.effective_bounds_cache,
            &mut self.gpu_static_draw_cache,
            &mut self.skinning_strategy_cache,
            overrides,
        )
        .map_err(|e| GeometryError::Evaluation(e.to_string()))?;
        let draw = draws
            .iter()
            .find(|d| d.instance_key.actor_id == model_id)
            .ok_or_else(|| GeometryError::Invalid("model is not visible at this frame".into()))?;
        let p = draw.params;
        let view = |pos: [f32; 3]| -> [f32; 3] {
            let local = std::array::from_fn(|i| (pos[i] - p.model[i]) * p.model[3]);
            let rotated = quat_rotate_vec3(quat_normalize_xyzw(p.actor_rotation), local);
            let rel: [f32; 3] = std::array::from_fn(|i| rotated[i] + p.actor[i] - p.camera0[i]);
            [p.camera1, p.camera2, p.camera3].map(|basis| (0..3).map(|i| rel[i] * basis[i]).sum())
        };
        let origin = view([0.0; 3]);
        let basis: Vec<[f32; 3]> = (0..3)
            .map(|axis| {
                let mut pos = [0.0; 3];
                pos[axis] = 1.0;
                let v = view(pos);
                std::array::from_fn(|i| v[i] - origin[i])
            })
            .collect();
        Ok(
            serde_json::json!({"modelId":model_id,"assetId":asset.id,"positions":cage.positions,
            "faces":cage.faces,"origin":origin,"basis":basis,"size":size,
            "center":[p.canvas[2],p.canvas[3]],"focal":p.camera0[3],"near":p.camera1[3],"orthographic":p.camera3[3]>0.5}),
        )
    }
}
