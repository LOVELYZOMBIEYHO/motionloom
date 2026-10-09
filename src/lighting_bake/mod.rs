//! Camera-independent room irradiance and local HDR reflection baking.
//!
//! This deterministic CPU solver traces two to four diffuse surface bounces
//! with actual scene occlusion. Transmissive windows use a bounded straight
//! thin-sheet approximation with Fresnel loss and Beer attenuation; solid
//! refraction, caustics, animated-object relighting and participating media are
//! outside this bake. V1 permits one local reflection capture per room volume.
//! No preview/Weaver GPU state is created.
mod schema;
mod trace;
pub use schema::*;

use crate::experimental::geometry::{
    GeometryError, SceneGeometryOptions, extract_scene_geometry_with_resolver,
};
use crate::scene::model::SceneNode;
use crate::{AssetResolver, AssetSource, GraphScript};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Component, Path};
use std::sync::Arc;
use trace::*;

#[derive(Debug, thiserror::Error)]
pub enum LightingBakeError {
    #[error("Invalid lighting bake: {0}")]
    Invalid(String),
    #[error("Lighting bake limit: {0}")]
    Limit(String),
    #[error("Lighting evaluation: {0}")]
    Evaluation(String),
    #[error("Unsupported lighting bake: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Geometry(#[from] GeometryError),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Environment(#[from] crate::lighting_ibl::IblPreprocessError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct LightingBakeProgress {
    pub state: String,
    pub volume: String,
    pub completed_probes: usize,
    pub total_probes: usize,
}

fn validate(options: &LightingBakeOptions) -> Result<(), LightingBakeError> {
    if options.scene_id.is_empty() || options.volumes.is_empty() || options.volumes.len() > 16 {
        return Err(LightingBakeError::Invalid(
            "sceneId and 1–16 room volumes are required".into(),
        ));
    }
    if !(2..=4).contains(&options.max_bounces)
        || !(32..=8192).contains(&options.rays_per_probe)
        || !(1..=64).contains(&options.specular_samples_per_pixel)
        || options
            .specular_resolution
            .iter()
            .any(|&v| v == 0 || v > 1024)
        || !options.max_ray_distance.is_finite()
        || !(1.0..=10000.0).contains(&options.max_ray_distance)
    {
        return Err(LightingBakeError::Invalid("use 2–4 bounces, 32–8192 rays, 1–64 reflection samples, dimensions 1–1024, and finite distance 1–10000".into()));
    }
    let mut names = HashSet::new();
    let mut probes = 0u64;
    let mut reflections = 0u64;
    for v in &options.volumes {
        if v.id.is_empty()
            || !v
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || !names.insert(v.id.clone())
            || v.bounds_min
                .iter()
                .chain(v.bounds_max.iter())
                .any(|x| !x.is_finite())
            || (0..3).any(|k| v.bounds_min[k] >= v.bounds_max[k])
            || v.counts.iter().any(|&n| n < 2 || n > 32)
            || v.reflection_positions.len() > 1
        {
            return Err(LightingBakeError::Invalid(format!(
                "invalid volume {} bounds/grid/id (counts must be 2–32, maximum one reflection per room)",
                v.id
            )));
        }
        if v.reflection_positions.iter().any(|p| {
            (0..3).any(|k| !p[k].is_finite() || p[k] < v.bounds_min[k] || p[k] > v.bounds_max[k])
        }) {
            return Err(LightingBakeError::Invalid(format!(
                "reflection must be inside volume {}",
                v.id
            )));
        }
        probes += v.counts.iter().map(|&x| x as u64).product::<u64>();
        reflections += v.reflection_positions.len() as u64;
    }
    let pixels = options.specular_resolution[0] as u64 * options.specular_resolution[1] as u64;
    let work = 2
        * (probes * options.rays_per_probe as u64
            + reflections * pixels * options.specular_samples_per_pixel as u64)
        * options.max_bounces as u64;
    if probes > 4096 || work > 25_000_000 || 2 * reflections * pixels * 12 > 256_000_000 {
        return Err(LightingBakeError::Limit("maximum 4096 probes, 25 million path segments and 256 MB reflection output; reduce grids/rays/resolution".into()));
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn digest_json<T: serde::Serialize>(value: &T) -> Result<String, LightingBakeError> {
    Ok(digest(&serde_json::to_vec(value)?))
}

// Runtime source compatibility is a schema lineage, not a transport version.
// Changing the solver version still invalidates explicit dependency checks.
const AUTHORING_FINGERPRINT_LINEAGE: &str = "diffuse-probes-v1";

/// Remove neutral additions and preview-only recursion without reordering legacy
/// struct fields or rewriting float tokens. A Value round-trip would change
/// the byte stream used by already published source fingerprints.
fn canonical_authoring_json(encoded: &[u8]) -> Result<Vec<u8>, LightingBakeError> {
    fn string_end(bytes: &[u8], cursor: &mut usize) {
        *cursor += 1;
        while *cursor < bytes.len() {
            match bytes[*cursor] {
                b'\\' => *cursor += 2,
                b'"' => {
                    *cursor += 1;
                    break;
                }
                _ => *cursor += 1,
            }
        }
    }
    fn value(bytes: &[u8], cursor: &mut usize) -> Result<Vec<u8>, serde_json::Error> {
        let start = *cursor;
        match bytes[*cursor] {
            b'{' => {
                *cursor += 1;
                let mut fields = Vec::new();
                while bytes[*cursor] != b'}' {
                    let key_start = *cursor;
                    string_end(bytes, cursor);
                    let key_raw = &bytes[key_start..*cursor];
                    let key: String = serde_json::from_slice(key_raw)?;
                    *cursor += 1; // Colon; input is serde_json's compact output.
                    let value_start = *cursor;
                    let filtered = value(bytes, cursor)?;
                    fields.push((key, key_raw, &bytes[value_start..*cursor], filtered));
                    if bytes[*cursor] == b',' {
                        *cursor += 1;
                    } else {
                        break;
                    }
                }
                *cursor += 1;
                let zero = |name: &str| {
                    fields.iter().find(|f| f.0 == name).is_some_and(|f| {
                        serde_json::from_slice::<serde_json::Value>(f.2)
                            .ok()
                            .and_then(|v| v.as_f64())
                            == Some(0.)
                    })
                };
                let sheen_off = zero("sheen");
                let coat_off = zero("clearcoat");
                let rect_area = fields
                    .iter()
                    .any(|f| f.0 == "kind" && f.2 == b"\"rectAreaLight\"");
                let mut output = vec![b'{'];
                for (key, raw_key, raw_value, filtered) in &fields {
                    let neutral = (sheen_off
                        && matches!(key.as_str(), "sheen" | "sheenColor" | "sheenRoughness"))
                        || (coat_off && matches!(key.as_str(), "clearcoat" | "clearcoatRoughness"))
                        || (matches!(
                            key.as_str(),
                            "angularDiameter"
                                | "sourceRadius"
                                | "angular_diameter"
                                | "source_radius"
                        ) && zero(key))
                        || (key == "shadowMode"
                            && (raw_value == &b"null".as_slice()
                                || raw_value == &b"\"legacy\"".as_slice()))
                        || (key == "perLightShadows" && raw_value == &b"false".as_slice())
                        // Preview recursion does not change the baked diffuse solve.
                        || key == "reflectionBounces"
                        || (key == "refractionMode" && raw_value == &b"\"slab\"".as_slice())
                        || (rect_area && key == "castShadow" && raw_value == &b"false".as_slice());
                    if neutral {
                        continue;
                    }
                    if output.len() > 1 {
                        output.push(b',');
                    }
                    output.extend_from_slice(raw_key);
                    output.push(b':');
                    output.extend_from_slice(filtered);
                }
                output.push(b'}');
                Ok(output)
            }
            b'[' => {
                *cursor += 1;
                let mut output = vec![b'['];
                while bytes[*cursor] != b']' {
                    if output.len() > 1 {
                        output.push(b',');
                    }
                    output.extend(value(bytes, cursor)?);
                    if bytes[*cursor] == b',' {
                        *cursor += 1;
                    } else {
                        break;
                    }
                }
                *cursor += 1;
                output.push(b']');
                Ok(output)
            }
            b'"' => {
                string_end(bytes, cursor);
                Ok(bytes[start..*cursor].to_vec())
            }
            _ => {
                while *cursor < bytes.len() && !matches!(bytes[*cursor], b',' | b']' | b'}') {
                    *cursor += 1;
                }
                Ok(bytes[start..*cursor].to_vec())
            }
        }
    }
    // This private walker receives serde_json's serialized typed data, so
    // compact formatting and complete delimiters are guaranteed.
    let mut cursor = 0;
    Ok(value(encoded, &mut cursor)?)
}

struct Prepared {
    name: String,
    frame: u32,
    scene: TraceScene,
    lighting_signature: String,
    environment_signature: String,
}

fn collect_lighting(
    graph: &GraphScript,
    scene_id: &str,
    frame: u32,
) -> Result<crate::world::WorldLighting, LightingBakeError> {
    let evaluated = crate::scene::render::apply_animation_targets_at_frame(graph, frame)
        .map_err(|e| LightingBakeError::Evaluation(e.to_string()))?;
    let graph = evaluated.as_ref().unwrap_or(graph);
    let scene = graph
        .scenes
        .iter()
        .find(|s| s.id == scene_id)
        .ok_or_else(|| LightingBakeError::Invalid(format!("missing Scene {scene_id}")))?;
    let style = crate::render_style::resolve_scene_render_style(graph, scene_id)
        .map_err(|e| LightingBakeError::Evaluation(e.to_string()))?;
    let images = graph
        .assets
        .iter()
        .filter_map(|a| a.external_src().map(|src| (a.id.clone(), src.to_string())))
        .collect();
    let mut result = crate::world::WorldLighting::default();
    fn visit(
        nodes: &[SceneNode],
        norm: f32,
        sec: f32,
        images: &HashMap<String, String>,
        style: &crate::render_style::ResolvedSceneRenderStyle,
        result: &mut crate::world::WorldLighting,
    ) -> Result<(), LightingBakeError> {
        for node in nodes {
            match node {
                SceneNode::Timeline(v) => visit(&v.children, norm, sec, images, style, result)?,
                SceneNode::Track(v) => visit(&v.children, norm, sec, images, style, result)?,
                SceneNode::Sequence(v) => {
                    if let Some((local_norm, local_sec)) =
                        crate::scene::timeline::scene_sequence_local_time(v, None, sec)
                    {
                        visit(&v.children, local_norm, local_sec, images, style, result)?;
                    }
                }
                SceneNode::Chain(v) => {
                    let mut cursor = v.from_ms as i64;
                    for child in &v.children {
                        if let SceneNode::Sequence(s) = child {
                            if let Some((local_norm, local_sec)) =
                                crate::scene::timeline::scene_sequence_local_time(
                                    s,
                                    Some(cursor),
                                    sec,
                                )
                            {
                                visit(&s.children, local_norm, local_sec, images, style, result)?;
                            }
                            cursor += s.duration_ms as i64 + v.gap_ms;
                        }
                    }
                }
                SceneNode::Group(v) => {
                    if crate::scene::render::eval_scene_number(&v.opacity, norm, sec)
                        .map_err(|e| LightingBakeError::Evaluation(e.to_string()))?
                        <= 0.
                    {
                        continue;
                    }
                    if let Some(composite) = &v.composite {
                        if composite.space == "3d" {
                            let mut c = composite.clone();
                            c.render_style = Some(style.clone());
                            let lowered =
                                crate::scene::render::scene_world_lighting(&c, images, norm, sec)
                                    .map_err(|e| LightingBakeError::Evaluation(e.to_string()))?;
                            if lowered.atmosphere_medium.is_some() {
                                return Err(LightingBakeError::Unsupported(
                                    "participating-medium bake".into(),
                                ));
                            }
                            if let Some(e) = lowered.environment {
                                if result.environment.as_ref().is_some_and(|other| other != &e) {
                                    return Err(LightingBakeError::Unsupported(
                                        "different environments in merged 3D islands".into(),
                                    ));
                                }
                                result.environment = Some(e);
                            }
                            result.lights.extend(lowered.lights);
                        }
                    }
                    visit(&v.children, norm, sec, images, style, result)?;
                }
                SceneNode::Layer(v) => visit(&v.children, norm, sec, images, style, result)?,
                _ => {}
            }
        }
        Ok(())
    }
    let sec = frame as f32 / graph.fps;
    visit(
        &scene.children,
        sec / (graph.duration_ms as f32 / 1000.).max(0.001),
        sec,
        &images,
        &style,
        &mut result,
    )?;
    Ok(result)
}

fn asset_bytes(resolver: &dyn AssetResolver, src: &str) -> Result<Vec<u8>, LightingBakeError> {
    let source = resolver
        .resolve(src)
        .map_err(LightingBakeError::Evaluation)?;
    let bytes = match source {
        AssetSource::Bytes(bytes) => bytes,
        AssetSource::Path(path) => std::fs::read(path)?,
        AssetSource::Url(_) => {
            return Err(LightingBakeError::Unsupported(
                "preload remote environment bytes in a resolver".into(),
            ));
        }
    };
    if bytes.len() > 256_000_000 {
        return Err(LightingBakeError::Limit(
            "environment source exceeds 256 MB".into(),
        ));
    }
    Ok(bytes)
}

async fn prepare(
    graph: &GraphScript,
    options: &LightingBakeOptions,
    resolver: Arc<dyn AssetResolver>,
) -> Result<Vec<Prepared>, LightingBakeError> {
    validate(options)?;
    if !graph.fps.is_finite() || graph.fps <= 0. {
        return Err(LightingBakeError::Invalid("invalid Graph fps".into()));
    }
    let mut states = Vec::new();
    let mut shared_textures = HashMap::<(u32, u32, String), Arc<Vec<u8>>>::new();
    let mut unique_texture_bytes = 0usize;
    for (name, frame) in [("day", options.day_frame), ("dusk", options.dusk_frame)] {
        let mut snapshot = extract_scene_geometry_with_resolver(
            graph,
            &SceneGeometryOptions {
                scene_id: options.scene_id.clone(),
                frame,
                include_hidden: false,
                selected_model_ids: None,
            },
            Arc::clone(&resolver),
        )
        .await?;
        // Geometry extraction is independent for the two light states. Share
        // identical pixels across them (and material variants), rather than
        // counting/retaining another full set of decoded 2K maps per state.
        for mesh in &mut snapshot.meshes {
            for texture in &mut mesh.textures {
                let key = (texture.width, texture.height, digest(&texture.rgba));
                if let Some(pixels) = shared_textures.get(&key) {
                    texture.rgba = Arc::clone(pixels);
                } else {
                    unique_texture_bytes += texture.rgba.len();
                    if unique_texture_bytes > 512_000_000 {
                        return Err(LightingBakeError::Limit(
                            "unique decoded material texture data exceeds512MB".into(),
                        ));
                    }
                    shared_textures.insert(key, Arc::clone(&texture.rgba));
                }
            }
        }
        let mut lighting = collect_lighting(graph, &options.scene_id, frame)?;
        // Exposure/tone/AO are not illumination and must never enter a bake.
        let lighting_signature = digest_json(&(&lighting.lights, &lighting.environment))?;
        let (environment, environment_signature) = if let Some(e) = lighting.environment.take() {
            let bytes = asset_bytes(resolver.as_ref(), &e.src)?;
            let signature = digest(&bytes);
            let map = crate::lighting_ibl::LinearEnvironment::from_encoded_bytes(&bytes)?;
            if map.width as u64 * map.height as u64 > 16_777_216 {
                return Err(LightingBakeError::Limit(
                    "environment exceeds 16 million pixels".into(),
                ));
            }

            (
                Some(Environment {
                    map,
                    intensity: e.intensity,
                    diffuse: e.diffuse_intensity,
                    specular: e.specular_intensity,
                    rotation: e.rotation_y_degrees.to_radians(),
                }),
                signature,
            )
        } else {
            (None, digest(b"no environment"))
        };
        let scene = TraceScene::new(
            snapshot.meshes,
            lighting.lights,
            environment,
            options.max_ray_distance,
        )?;
        states.push(Prepared {
            name: name.into(),
            frame,
            scene,
            lighting_signature,
            environment_signature,
        });
    }
    Ok(states)
}

fn fingerprint(
    states: &[Prepared],
    options: &LightingBakeOptions,
) -> Result<BakeFingerprint, LightingBakeError> {
    let mut geometry = Sha256::new();
    let mut materials = Sha256::new();
    let mut textures = Sha256::new();
    let mut seen = HashMap::<usize, String>::new();
    let mut total_texture = 0usize;
    for state in states {
        for mesh in &state.scene.meshes {
            geometry.update((mesh.positions.len() as u64).to_le_bytes());
            geometry.update((mesh.indices.len() as u64).to_le_bytes());
            for p in mesh
                .positions
                .iter()
                .flatten()
                .chain(mesh.normals.iter().flatten())
                .chain(mesh.uvs.iter().flatten())
                .chain(mesh.tangents.iter().flatten())
            {
                geometry.update(p.to_le_bytes());
            }
            for &i in &mesh.indices {
                geometry.update(i.to_le_bytes());
            }
            materials.update(material_fingerprint_bytes(&mesh.material));
            for value in mesh.colors.iter().flatten() {
                materials.update(value.to_le_bytes());
            }
            for texture in &mesh.textures {
                let pointer = Arc::as_ptr(&texture.rgba) as usize;
                let hash = if let Some(hash) = seen.get(&pointer) {
                    hash.clone()
                } else {
                    total_texture += texture.rgba.len();
                    if total_texture > 512_000_000 {
                        return Err(LightingBakeError::Limit(
                            "decoded material texture data exceeds 512 MB".into(),
                        ));
                    }
                    let hash = digest(&texture.rgba);
                    seen.insert(pointer, hash.clone());
                    hash
                };
                textures.update(texture.width.to_le_bytes());
                textures.update(texture.height.to_le_bytes());
                textures.update(hash.as_bytes());
            }
        }
    }
    let geometry = format!("{:x}", geometry.finalize());
    let materials = format!("{:x}", materials.finalize());
    let textures = format!("{:x}", textures.finalize());
    let lighting = digest_json(
        &states
            .iter()
            .map(|s| &s.lighting_signature)
            .collect::<Vec<_>>(),
    )?;
    let environments = digest_json(
        &states
            .iter()
            .map(|s| &s.environment_signature)
            .collect::<Vec<_>>(),
    )?;
    let settings = digest_json(options)?;
    let combined = digest_json(&(
        LIGHTING_BAKER_VERSION,
        &geometry,
        &materials,
        &textures,
        &lighting,
        &environments,
        &settings,
    ))?;
    Ok(BakeFingerprint {
        geometry,
        materials,
        textures,
        lighting,
        environments,
        settings,
        combined,
    })
}

/// Cheap source-level stale-bake guard, independent of camera and 2D overlays.
/// Evaluated graph clones retain raw_script; use the authored source so an
/// animated playback frame cannot change this revision fingerprint.
pub fn scene_lighting_authoring_fingerprint(
    graph: &GraphScript,
) -> Result<String, LightingBakeError> {
    let parsed;
    let authored = if let Some(source) = graph.raw_script.as_deref() {
        parsed = crate::parse_graph_script(source)
            .map_err(|e| LightingBakeError::Evaluation(e.to_string()))?;
        &parsed
    } else {
        graph
    };
    fn prune(value: serde_json::Value, ids: &mut HashSet<String>) -> Option<serde_json::Value> {
        let mut object = value.as_object()?.clone();
        let composite = object
            .get("composite")
            .and_then(|c| c.get("space"))
            .and_then(|s| s.as_str())
            == Some("3d");
        let children = object
            .remove("children")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| prune(v, ids))
            .collect::<Vec<_>>();
        if !composite && children.is_empty() {
            return None;
        }
        if composite {
            if let Some(c) = object.get_mut("composite").and_then(|c| c.as_object_mut()) {
                c.remove("activeCamera");
                if let Some(nodes) = c.get_mut("nodes3d").and_then(|v| v.as_array_mut()) {
                    nodes.retain(|n| {
                        !matches!(
                            n.get("kind").and_then(|k| k.as_str()),
                            Some(
                                "camera"
                                    | "bakedLighting"
                                    | "planarReflection"
                                    | "ambientOcclusion"
                                    | "contactShadow"
                                    | "colorManagement"
                                    | "debug"
                            )
                        )
                    });
                    for n in nodes {
                        if let Some(id) = n.get("id").and_then(|i| i.as_str()) {
                            ids.insert(id.into());
                        }
                    }
                }
            }
        }
        if let Some(id) = object.get("id").and_then(|i| i.as_str()) {
            ids.insert(id.into());
        }
        for field in [
            "effects",
            "postEffects",
            "processEffects",
            "filter",
            "blend",
            "opacity",
            "depth",
            "format",
            "size",
        ] {
            object.remove(field);
        }
        object.insert("children".into(), serde_json::Value::Array(children));
        Some(serde_json::Value::Object(object))
    }
    let mut ids = HashSet::new();
    let scenes = authored
        .scenes
        .iter()
        .filter_map(|s| prune(serde_json::to_value(s).ok()?, &mut ids))
        .collect::<Vec<_>>();
    let scene_nodes = authored
        .scene_nodes
        .iter()
        .filter_map(|s| prune(serde_json::to_value(s).ok()?, &mut ids))
        .collect::<Vec<_>>();
    let channels = authored
        .animation_targets
        .iter()
        .filter(|a| ids.contains(&a.node) && a.property != "activeCamera")
        .collect::<Vec<_>>();
    let assets = authored
        .assets
        .iter()
        .filter(|a| {
            matches!(
                a.kind,
                crate::dsl::GraphAssetKind::Image
                    | crate::dsl::GraphAssetKind::Model
                    | crate::dsl::GraphAssetKind::Animation
            )
        })
        .collect::<Vec<_>>();
    let mut styles = serde_json::to_value(&authored.render_styles)?;
    if let Some(styles) = styles.as_array_mut() {
        for style in styles {
            if let Some(s) = style.as_object_mut() {
                for field in [
                    "post",
                    "depthOfField",
                    "antiAliasing",
                    "outline",
                    "universal",
                ] {
                    s.remove(field);
                }
            }
        }
    }
    let encoded = serde_json::to_vec(&(
        AUTHORING_FINGERPRINT_LINEAGE,
        authored.fps,
        authored.duration_ms,
        assets,
        &authored.material_assets,
        &authored.geometry_assets,
        &authored.curve_assets,
        scenes,
        scene_nodes,
        channels,
        styles,
        &authored.actions,
        &authored.apply_actions,
        &authored.model_profiles,
        &authored.attachments,
        &authored.scene_constraints,
    ))?;
    Ok(digest(&canonical_authoring_json(&encoded)?))
}

/// Validate every evaluated geometry/material/light/environment/texture
/// dependency without tracing a new lighting solution. Use for build checks
/// after asset bytes change; source-level stale guards run in GPU preview.
pub async fn validate_lighting_bake_fingerprint(
    graph: &GraphScript,
    options: &LightingBakeOptions,
    asset: &BakedLightingAsset,
    resolver: Arc<dyn AssetResolver>,
) -> Result<(), LightingBakeError> {
    let current = scene_lighting_bake_fingerprint(graph, options, resolver).await?;
    if current != asset.fingerprint {
        return Err(LightingBakeError::Invalid(
            "stale lighting asset: bake dependency fingerprint differs".into(),
        ));
    }
    Ok(())
}

/// Compute the same dependency fingerprint without tracing probe rays.
/// Camera, screen artwork and post-processing changes do not invalidate it.
pub async fn scene_lighting_bake_fingerprint(
    graph: &GraphScript,
    options: &LightingBakeOptions,
    resolver: Arc<dyn AssetResolver>,
) -> Result<BakeFingerprint, LightingBakeError> {
    fingerprint(&prepare(graph, options, resolver).await?, options)
}

/// Return an in-memory two-state bake. No files are written implicitly.
pub async fn bake_scene_lighting(
    graph: &GraphScript,
    options: &LightingBakeOptions,
    resolver: Arc<dyn AssetResolver>,
) -> Result<LightingBakeBundle, LightingBakeError> {
    bake_scene_lighting_with_progress(graph, options, resolver, |_| {}).await
}

pub async fn bake_scene_lighting_with_progress<F: FnMut(LightingBakeProgress)>(
    graph: &GraphScript,
    options: &LightingBakeOptions,
    resolver: Arc<dyn AssetResolver>,
    mut progress: F,
) -> Result<LightingBakeBundle, LightingBakeError> {
    let prepared = prepare(graph, options, resolver).await?;
    let fingerprint = fingerprint(&prepared, options)?;
    let mut states = Vec::new();
    let mut reflection_images = Vec::new();
    let total_probes = options
        .volumes
        .iter()
        .map(|v| v.counts.iter().map(|&n| n as usize).product::<usize>())
        .sum::<usize>()
        * 2;
    let mut completed_probes = 0;
    for state in &prepared {
        let mut volumes = Vec::new();
        for (volume_index, v) in options.volumes.iter().enumerate() {
            let mut probes = Vec::new();
            for z in 0..v.counts[2] {
                for y in 0..v.counts[1] {
                    for x in 0..v.counts[0] {
                        let grid = [x, y, z];
                        let position = std::array::from_fn(|k| {
                            v.bounds_min[k]
                                + (v.bounds_max[k] - v.bounds_min[k])
                                    * if v.counts[k] == 1 {
                                        0.5
                                    } else {
                                        grid[k] as f32 / (v.counts[k] - 1) as f32
                                    }
                        });
                        // Common samples across day/dusk reduce blend noise.
                        let seed = options.seed
                            ^ (volume_index as u64).wrapping_mul(0x85ebca6b)
                            ^ probes.len() as u64;
                        probes.push(bake_probe(&state.scene, position, options, seed));
                        completed_probes += 1;
                        progress(LightingBakeProgress {
                            state: state.name.clone(),
                            volume: v.id.clone(),
                            completed_probes,
                            total_probes,
                        });
                    }
                }
            }
            let mut reflections = Vec::new();
            for (index, &position) in v.reflection_positions.iter().enumerate() {
                if !state.scene.valid_probe(position) {
                    return Err(LightingBakeError::Invalid(format!(
                        "reflection probe {}:{index} is inside/too close to geometry",
                        v.id
                    )));
                }
                let src = format!("reflections/{}-{}-{index:03}.hdr", state.name, v.id);
                let [width, height] = options.specular_resolution;
                let mut pixels = Vec::with_capacity((width * height) as usize);
                let mut rng = Rng::new(options.seed ^ ((volume_index as u64) << 32) ^ index as u64);
                for y in 0..height {
                    for x in 0..width {
                        let mut color = [0.; 3];
                        for _ in 0..options.specular_samples_per_pixel {
                            let theta = ((x as f32 + rng.unit()) / width as f32 - 0.5)
                                * std::f32::consts::TAU;
                            let phi =
                                (y as f32 + rng.unit()) / height as f32 * std::f32::consts::PI;
                            let d = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
                            color = add(
                                color,
                                state.scene.radiance(
                                    position,
                                    d,
                                    options.max_bounces,
                                    &mut rng,
                                    true,
                                ),
                            );
                        }
                        pixels.push(scale(color, 1. / options.specular_samples_per_pixel as f32));
                    }
                }
                reflection_images.push(BakedReflectionImage {
                    src: src.clone(),
                    width,
                    height,
                    pixels,
                });
                reflections.push(LocalReflectionProbe {
                    position,
                    bounds_min: v.bounds_min,
                    bounds_max: v.bounds_max,
                    src,
                });
            }
            volumes.push(BakedLightingVolume {
                id: v.id.clone(),
                bounds_min: v.bounds_min,
                bounds_max: v.bounds_max,
                counts: v.counts,
                probes,
                reflections,
            });
        }
        states.push(BakedLightingState {
            name: state.name.clone(),
            frame: state.frame,
            volumes,
        });
    }
    let diagnostics=vec![format!("Deterministic CPU BVH; {} diffuse bounces. SH stores cosine-convolved irradiance E; Lambert use E/PI.",options.max_bounces),
        "Actual geometry occludes every light/environment ray. Visibility/depth moments are camera independent. Straight thin-sheet transmission includes Fresnel and Beer attenuation; solid refraction and caustics are not solved.".into(),
        "Irradiance includes visibility-filtered environment and bounced surface radiance, but no directly sampled delta lights at probes. Keep analytic direct lighting separate. Local reflection images are linear radiance, without exposure/tone mapping.".into()];
    Ok(LightingBakeBundle {
        asset: BakedLightingAsset {
            schema_version: LIGHTING_BAKE_SCHEMA_VERSION,
            baker_version: LIGHTING_BAKER_VERSION.into(),
            scene_id: options.scene_id.clone(),
            authoring_fingerprint: scene_lighting_authoring_fingerprint(graph)?,
            fingerprint,
            states,
            diagnostics,
        },
        reflection_images,
    })
}

/// Explicitly save the JSON and its untonemapped Radiance HDR captures.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_lighting_bake(
    bundle: &LightingBakeBundle,
    path: impl AsRef<Path>,
) -> Result<(), LightingBakeError> {
    let path = path.as_ref();
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(root)?;
    for image in &bundle.reflection_images {
        let relative = Path::new(&image.src);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(LightingBakeError::Invalid(
                "reflection path must be relative without parent traversal".into(),
            ));
        }
        if image.width == 0
            || image.height == 0
            || image.pixels.len() != image.width as usize * image.height as usize
            || image
                .pixels
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || *v < 0.)
        {
            return Err(LightingBakeError::Invalid(
                "invalid reflection image".into(),
            ));
        }
        let out = root.join(relative);
        std::fs::create_dir_all(out.parent().unwrap())?;
        let pixels = image
            .pixels
            .iter()
            .copied()
            .map(image::Rgb)
            .collect::<Vec<_>>();
        image::codecs::hdr::HdrEncoder::new(std::fs::File::create(out)?).encode(
            &pixels,
            image.width as usize,
            image.height as usize,
        )?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(&bundle.asset)?)?;
    Ok(())
}

fn material_fingerprint_bytes(material: &crate::world::gltf_loader::GlbMaterialData) -> Vec<u8> {
    let mut encoded = format!("{material:?}");
    // The retained fingerprint predates this field. Keep default slab materials
    // byte-identical without changing the established ordering of other fields.
    if material.refraction_mode == crate::dsl::MaterialRefractionMode::Slab {
        const DEFAULT_FIELD: &str = ", refraction_mode: Slab";
        if let Some(start) = encoded.rfind(DEFAULT_FIELD) {
            encoded.replace_range(start..start + DEFAULT_FIELD.len(), "");
        }
    }
    encoded.into_bytes()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod authoring_compatibility_tests {
    use super::*;

    #[test]
    fn material_fingerprint_preserves_default_slab_field_lineage() {
        let mut material = crate::world::gltf_loader::GlbMaterialData::default();
        let slab = String::from_utf8(material_fingerprint_bytes(&material)).unwrap();
        assert!(!slab.contains("refraction_mode:"));
        assert_eq!(
            slab,
            format!("{material:?}").replace(", refraction_mode: Slab", "")
        );
        material.refraction_mode = crate::dsl::MaterialRefractionMode::Solid;
        let solid = String::from_utf8(material_fingerprint_bytes(&material)).unwrap();
        assert!(solid.contains("refraction_mode: Solid"));
        assert_ne!(digest(slab.as_bytes()), digest(solid.as_bytes()));
    }

    #[test]
    fn preview_bounce_budget_and_default_slab_do_not_invalidate_static_bakes() {
        let old = br#"{"material":{"ior":1.5},"lighting":{}}"#;
        for modern in [
            br#"{"material":{"ior":1.5,"refractionMode":"slab"},"lighting":{"reflectionBounces":null}}"#.as_slice(),
            br#"{"material":{"ior":1.5,"refractionMode":"slab"},"lighting":{"reflectionBounces":1}}"#.as_slice(),
            br#"{"material":{"ior":1.5,"refractionMode":"slab"},"lighting":{"reflectionBounces":2}}"#.as_slice(),
        ] {
            assert_eq!(canonical_authoring_json(modern).unwrap(), old);
        }
        let solid = br#"{"material":{"ior":1.5,"refractionMode":"solid"},"lighting":{"reflectionBounces":2}}"#;
        assert_ne!(canonical_authoring_json(solid).unwrap(), old);
    }

    #[test]
    fn neutral_new_fields_recover_exact_legacy_json_bytes() {
        let legacy = br#"{"number":0.000001,"materials":[{"id":"m","roughness":0.5}],"lights":[{"kind":"directionalLight","intensity":"1"}],"style":{"lighting":{"ambientIntensity":0.1}},"literal":"\\\"sheen\\\":0"}"#;
        let modern = br#"{"number":0.000001,"materials":[{"id":"m","roughness":0.5,"sheen":0.0,"sheenColor":[0.2,0.3,0.4],"sheenRoughness":0.7,"clearcoat":0.0,"clearcoatRoughness":0.6}],"lights":[{"kind":"directionalLight","intensity":"1","angularDiameter":0.0,"sourceRadius":0.0}],"style":{"lighting":{"ambientIntensity":0.1,"shadowMode":"legacy"}},"literal":"\\\"sheen\\\":0"}"#;
        serde_json::from_slice::<serde_json::Value>(modern).unwrap();
        assert_eq!(canonical_authoring_json(modern).unwrap(), legacy);
        assert_eq!(
            digest(&canonical_authoring_json(modern).unwrap()),
            digest(legacy)
        );
        assert_eq!(
            canonical_authoring_json(br#"{"shadowMode":null}"#).unwrap(),
            b"{}"
        );
        assert_eq!(
            canonical_authoring_json(
                br#"{"kind":"rectAreaLight","castShadow":false,"intensity":"1"}"#
            )
            .unwrap(),
            br#"{"kind":"rectAreaLight","intensity":"1"}"#
        );
        for unchanged in [
            br#"{"kind":"rectAreaLight","castShadow":true}"#.as_slice(),
            br#"{"kind":"pointLight","castShadow":false}"#.as_slice(),
            br#"{"kind":"directionalLight","castShadow":false}"#.as_slice(),
            br#"{"kind":"spotLight","castShadow":false}"#.as_slice(),
        ] {
            assert_eq!(canonical_authoring_json(unchanged).unwrap(), unchanged);
        }
        let active = br#"{"sheen":0.4,"sheenColor":[0.2,0.3,0.4],"sheenRoughness":0.7,"clearcoat":0.6,"clearcoatRoughness":0.2,"angularDiameter":0.53,"sourceRadius":0.15,"shadowMode":"perLight"}"#;
        assert_eq!(
            canonical_authoring_json(active).unwrap(),
            active,
            "active metadata must remain intact"
        );
    }

    #[test]
    fn resolved_legacy_shadow_default_preserves_published_source_bytes() {
        // CompositeGroup embeds the resolved style, independently of the
        // authored LightingStyle. Its new default must also be removed.
        let legacy = br#"{"composite":{"renderStyle":{"sceneId":"room","hardShadows":false,"lightingPreset":null},"nodes3d":[]},"lighting":{"shadowStyle":"soft"}}"#;
        let modern = br#"{"composite":{"renderStyle":{"sceneId":"room","hardShadows":false,"perLightShadows":false,"lightingPreset":null},"nodes3d":[]},"lighting":{"shadowStyle":"soft","shadowMode":null}}"#;
        let canonical = canonical_authoring_json(modern).unwrap();
        assert_eq!(canonical, legacy);
        assert_eq!(
            digest(&canonical),
            "b42d584db2e368a5d01e9a59ee58279cbfd527c5a4a7d4923fb9fd8b94cc5037"
        );
        let active = br#"{"composite":{"renderStyle":{"perLightShadows":true}},"lighting":{"shadowMode":"perLight"}}"#;
        assert_eq!(canonical_authoring_json(active).unwrap(), active);
        assert_ne!(
            digest(&canonical_authoring_json(active).unwrap()),
            digest(legacy)
        );
    }

    fn scene(
        extra_material: &str,
        extra_sun: &str,
        extra_lamp: &str,
        shadow_mode: &str,
    ) -> GraphScript {
        crate::parse_graph_script(&format!(r##"<Graph fps={{24}} duration="3s" size={{[64,64]}}>
        <Assets><MaterialAsset id="paint" baseColor="#DDAA88" roughness="0.8" {extra_material}/>
        <GeometryAsset id="box_g"><Primitive shape="box" size={{[1,1,1]}} /></GeometryAsset>
        <MeshAsset id="box" geometry="box_g" material="paint" /></Assets>
        <RenderStyle id="style"><LightingStyle ambientIntensity="0.1" {shadow_mode}/></RenderStyle>
        <Scene id="room" renderStyle="style"><Timeline><Track space="3d"><Sequence from="0s" duration="3s" out="hold"><CompositeGroup space="3d">
        <Camera3D position={{[0,2,4]}} target={{[0,0,0]}} />
        <DirectionalLight id="sun" direction={{[0,-1,0]}} intensity="1" {extra_sun}/>
        <PointLight id="lamp" position={{[0,2,0]}} intensity="1" {extra_lamp}/>
        <Model id="box_model" asset="box" /></CompositeGroup></Sequence></Track></Timeline></Scene>
        <Present from="room" /></Graph>"##)).unwrap()
    }

    #[test]
    fn source_guard_includes_active_emitters_layers_and_shadow_mode() {
        let base = scene("", "", "", "");
        let neutral = scene(
            "sheen=\"0\" sheenColor=\"#234567\" sheenRoughness=\"0.7\" clearcoat=\"0\" clearcoatRoughness=\"0.6\"",
            "angularDiameter=\"0\"",
            "sourceRadius=\"0\"",
            "shadowMode=\"legacy\"",
        );
        let expected = scene_lighting_authoring_fingerprint(&base).unwrap();
        assert_eq!(
            scene_lighting_authoring_fingerprint(&neutral).unwrap(),
            expected
        );
        for changed in [
            scene("", "angularDiameter=\"0.53\"", "", ""),
            scene("", "", "sourceRadius=\"0.15\"", ""),
            scene("sheen=\"0.4\"", "", "", ""),
            scene("clearcoat=\"0.4\"", "", "", ""),
            scene("", "", "", "shadowMode=\"perLight\""),
        ] {
            assert_ne!(
                scene_lighting_authoring_fingerprint(&changed).unwrap(),
                expected
            );
        }
        let a = scene("sheen=\"0.4\" sheenRoughness=\"0.3\"", "", "", "");
        let b = scene("sheen=\"0.4\" sheenRoughness=\"0.7\"", "", "", "");
        assert_ne!(
            scene_lighting_authoring_fingerprint(&a).unwrap(),
            scene_lighting_authoring_fingerprint(&b).unwrap()
        );
    }

    #[test]
    fn dependency_validation_rejects_changed_finite_source_and_material_layer() {
        let base = scene("", "", "", "");
        let options = LightingBakeOptions {
            scene_id: "room".into(),
            day_frame: 24,
            dusk_frame: 48,
            volumes: vec![BakeVolumeOptions {
                id: "room".into(),
                bounds_min: [-2., 0.8, -2.],
                bounds_max: [2., 2., 2.],
                counts: [2; 3],
                reflection_positions: vec![],
            }],
            rays_per_probe: 32,
            specular_resolution: [8, 4],
            specular_samples_per_pixel: 1,
            ..Default::default()
        };
        let bundle = pollster::block_on(bake_scene_lighting(
            &base,
            &options,
            Arc::new(crate::PathAssetResolver),
        ))
        .unwrap();
        for changed in [
            scene("", "angularDiameter=\"0.53\"", "", ""),
            scene("", "", "sourceRadius=\"0.15\"", ""),
            scene("sheen=\"0.4\"", "", "", ""),
            scene("clearcoat=\"0.4\"", "", "", ""),
        ] {
            assert!(matches!(
                pollster::block_on(validate_lighting_bake_fingerprint(
                    &changed,
                    &options,
                    &bundle.asset,
                    Arc::new(crate::PathAssetResolver)
                )),
                Err(LightingBakeError::Invalid(_))
            ));
        }
    }
}
