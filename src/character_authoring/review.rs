// =========================================
// =========================================
// src/character_authoring/review.rs

use super::{CharacterError, CharacterSnapshot};
use crate::api::{SceneRenderProfile, SceneRenderer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewView {
    pub name: String,
    pub yaw: f32,
    pub pitch: f32,
    pub target: [f32; 3],
    pub vertical_scale: f32,
    pub projection: String,
    pub requested_fov: f32,
    pub effective_fov: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewReport {
    pub revision: u64,
    pub source_fingerprint: String,
    pub bounds: [[f32; 3]; 2],
    pub views: Vec<ReviewView>,
    pub images: Vec<String>,
    pub diagnostics: Vec<String>,
    pub reference_scope: String,
}

/// Named pixels are returned without choosing filesystem or transport ownership.
#[derive(Clone, Debug)]
pub struct ReviewImage {
    pub name: String,
    pub pixels: image::RgbaImage,
}

#[derive(Clone, Debug)]
pub struct RenderedReview {
    pub report: ReviewReport,
    pub images: Vec<ReviewImage>,
}

/// Resolve frame-zero world bounds without blocking the host executor.
pub async fn bounds_async(snapshot: &CharacterSnapshot) -> Result<[[f32; 3]; 2], CharacterError> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    // Frame-zero world geometry includes actor transforms and excludes unused assets.
    let graph = crate::api::parse_graph_script(&snapshot.source)
        .map_err(|e| CharacterError::Invalid(e.to_string()))?;
    let scene = graph
        .scenes
        .first()
        .ok_or_else(|| CharacterError::Invalid("No scene to review".into()))?;
    let geometry = crate::experimental::extract_scene_geometry(
        &graph,
        &crate::experimental::SceneGeometryOptions {
            scene_id: scene.id.clone(),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CharacterError::Geometry(e.to_string()))?;
    for mesh in geometry.meshes {
        for point in mesh.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(point[i]);
                hi[i] = hi[i].max(point[i]);
            }
        }
    }
    if lo.iter().chain(&hi).any(|x| !x.is_finite()) {
        return Err(CharacterError::Geometry(
            "Review scene has no finite geometry".into(),
        ));
    }
    Ok([lo, hi])
}

/// Shared framing prevents an A/B comparison from concealing a size change.
pub async fn review_report_async(
    snapshot: &CharacterSnapshot,
    revision: u64,
    compare: Option<&CharacterSnapshot>,
) -> Result<ReviewReport, CharacterError> {
    let mut b = bounds_async(snapshot).await?;
    if let Some(other) = compare {
        let c = bounds_async(other).await?;
        for i in 0..3 {
            b[0][i] = b[0][i].min(c[0][i]);
            b[1][i] = b[1][i].max(c[1][i]);
        }
    }
    let center = std::array::from_fn(|i| (b[0][i] + b[1][i]) * 0.5);
    let vertical_scale =
        ((b[1][0] - b[0][0]).hypot(b[1][2] - b[0][2])).max(b[1][1] - b[0][1]) * 1.18;
    let mut views = vec![];
    for (name, yaw, pitch) in [
        ("front", 0., 0.),
        ("back", 180., 0.),
        ("left", -90., 0.),
        ("right", 90., 0.),
        ("quarter", 35., 5.),
        ("portrait", 0., 0.),
    ] {
        let mut target = center;
        let mut scale = vertical_scale;
        if name == "portrait" {
            target[1] = b[1][1] - (b[1][1] - b[0][1]) * 0.16;
            scale = (b[1][1] - b[0][1]) * 0.40;
        }
        views.push(ReviewView {
            name: name.into(),
            yaw,
            pitch,
            target,
            vertical_scale: scale,
            projection: "orthographic".into(),
            requested_fov: 35.,
            effective_fov: 35.,
        });
    }
    let mut diagnostics = snapshot.assumptions.clone();
    if let Some(p) = &snapshot.parameters {
        if p.rib_depth / p.rib_width < 0.4 {
            diagnostics.push("Rib cage is unusually shallow for this template.".into());
        }
        if p.pelvis_depth / p.pelvis_width < 0.4 {
            diagnostics.push("Pelvis is unusually shallow for this template.".into());
        }
    }
    Ok(ReviewReport {revision,source_fingerprint:snapshot.source_fingerprint.clone(),bounds:b,views,images:vec![],diagnostics,reference_scope:"Design review only; generated views are not reference images or reconstruction evidence.".into()})
}

fn configure_cameras(value: &mut Value, view: &ReviewView) -> usize {
    let mut count = 0;
    match value {
        Value::Object(map) => {
            if map.contains_key("position") && map.contains_key("target") && map.contains_key("fov")
            {
                let yaw = view.yaw.to_radians();
                let pitch = view.pitch.to_radians();
                let distance = view.vertical_scale * 3.;
                let position = [
                    view.target[0] + yaw.sin() * pitch.cos() * distance,
                    view.target[1] + pitch.sin() * distance,
                    view.target[2] + yaw.cos() * pitch.cos() * distance,
                ];
                map.insert(
                    "position".into(),
                    Value::String(serde_json::to_string(&position).unwrap()),
                );
                map.insert(
                    "target".into(),
                    Value::String(serde_json::to_string(&view.target).unwrap()),
                );
                map.insert("projection".into(), Value::String("orthographic".into()));
                map.insert("fov".into(), Value::String(view.requested_fov.to_string()));
                map.insert(
                    "orthographicScale".into(),
                    Value::String(view.vertical_scale.to_string()),
                );
                map.insert("depthOfField".into(), Value::Null);
                count += 1;
            } else {
                for value in map.values_mut() {
                    count += configure_cameras(value, view);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                count += configure_cameras(value, view);
            }
        }
        _ => {}
    }
    count
}

fn neutralize_bindings(value: &mut Value) {
    // Cel roles, per-model tints and colored outlines must not conceal surface shape in gray review.
    match value {
        Value::Object(map) => {
            if map.contains_key("cel") && map.contains_key("material") {
                map.insert("cel".into(), serde_json::json!({}));
                map.insert("tint".into(), serde_json::json!([0.65, 0.65, 0.65, 1.]));
                map.insert("tintAmount".into(), serde_json::json!(1.));
                map.insert("texture".into(), Value::Null);
            }
            for value in map.values_mut() {
                neutralize_bindings(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                neutralize_bindings(value);
            }
        }
        _ => {}
    }
}

fn refresh_compiled_style(value: &mut Value, style: &Value) {
    // Scene parsing lowers styles into composite islands; changing only resource definitions is insufficient.
    match value {
        Value::Object(map) => {
            if map.contains_key("nodes3d") || map.contains_key("nodes3D") {
                map.insert("renderStyle".into(), style.clone());
            }
            for value in map.values_mut() {
                refresh_compiled_style(value, style);
            }
        }
        Value::Array(values) => {
            for value in values {
                refresh_compiled_style(value, style);
            }
        }
        _ => {}
    }
}

pub fn review_graph(
    snapshot: &CharacterSnapshot,
    view: &ReviewView,
    size: u32,
    gray: bool,
) -> Result<crate::GraphScript, CharacterError> {
    if !(64..=2048).contains(&size) {
        return Err(CharacterError::Invalid(
            "Review size must be 64..2048".into(),
        ));
    }
    let mut graph = crate::api::parse_graph_script(&snapshot.source)
        .map_err(|e| CharacterError::Invalid(e.to_string()))?;
    let mut value = serde_json::to_value(&graph)?;
    if configure_cameras(&mut value, view) == 0 {
        return Err(CharacterError::Invalid("Scene has no Camera3D".into()));
    }
    graph = serde_json::from_value(value)?;
    graph.size = (size, size);
    graph.render_size = None;
    if gray {
        for material in &mut graph.material_assets {
            material.base_color = [0.65, 0.65, 0.65, 1.];
            material.base_color_texture = None;
            material.metallic = 0.;
            material.roughness = 1.;
        }
        // Parsed primitives retain resolved material copies, so neutralize both representations.
        for asset in &mut graph.assets {
            if let crate::GraphAssetSource::Primitive(primitive) = &mut asset.source {
                primitive.color = [0.65, 0.65, 0.65, 1.];
                if let Some(material) = &mut primitive.material_definition {
                    material.base_color = [0.65, 0.65, 0.65, 1.];
                    material.base_color_texture = None;
                    material.metallic = 0.;
                    material.roughness = 1.;
                }
            }
        }
        for style in &mut graph.render_styles {
            style.surface.get_or_insert_with(Default::default).shading = Some("clay".into());
            style.color.get_or_insert_with(Default::default).saturation = Some(0.);
            if let Some(surface) = &mut style.surface {
                surface.shadow_color = Some("#666666".into());
            }
            if let Some(outline) = &mut style.outline {
                outline.color = Some("#505050".into());
            }
            if let Some(lighting) = &mut style.lighting {
                lighting.ambient_color = Some("#FFFFFF".into());
            }
            if let Some(post) = &mut style.post {
                post.saturation = Some(0.);
            }
        }
        let mut value = serde_json::to_value(&graph)?;
        neutralize_bindings(&mut value);
        for (i, scene) in graph.scenes.iter().enumerate() {
            let style = crate::api::resolve_scene_render_style(&graph, &scene.id)
                .map_err(|e| CharacterError::Invalid(e.to_string()))?;
            refresh_compiled_style(&mut value["scenes"][i], &serde_json::to_value(style)?);
        }
        graph = serde_json::from_value(value)?;
    }
    Ok(graph)
}

/// Render images in memory with one renderer and shared before/after framing.
pub async fn render_review_images(
    snapshot: &CharacterSnapshot,
    revision: u64,
    compare: Option<&CharacterSnapshot>,
    size: u32,
    gray: bool,
    gpu: bool,
) -> Result<RenderedReview, CharacterError> {
    let mut report = review_report_async(snapshot, revision, compare).await?;
    let mut images = Vec::new();
    let mut renderer = if gpu {
        Some(
            SceneRenderer::new(SceneRenderProfile::Gpu)
                .await
                .map_err(|e| CharacterError::Render(e.to_string()))?,
        )
    } else {
        None
    };
    let mut sheet = image::RgbaImage::new(size * 3, size * 2);
    for (i, view) in report.views.iter().enumerate() {
        let graph = review_graph(snapshot, view, size, gray)?;
        let pixels = if let Some(renderer) = &mut renderer {
            renderer
                .render_frame(&graph, 0)
                .await
                .map_err(|e| CharacterError::Render(e.to_string()))?
        } else {
            super::render_cpu_review(&graph, view, gray).await?
        };
        let name = format!("{}.png", view.name);
        report.images.push(name.clone());
        images.push(ReviewImage {
            name,
            pixels: pixels.clone(),
        });
        image::imageops::replace(
            &mut sheet,
            &pixels,
            (i as i64 % 3) * size as i64,
            (i as i64 / 3) * size as i64,
        );
        if let Some(previous) = compare {
            let graph = review_graph(previous, view, size, gray)?;
            let before = if let Some(renderer) = &mut renderer {
                renderer
                    .render_frame(&graph, 0)
                    .await
                    .map_err(|e| CharacterError::Render(e.to_string()))?
            } else {
                super::render_cpu_review(&graph, view, gray).await?
            };
            let mut pair = image::RgbaImage::new(size * 2, size);
            image::imageops::replace(&mut pair, &before, 0, 0);
            image::imageops::replace(&mut pair, &pixels, size as i64, 0);
            images.push(ReviewImage {
                name: format!("{}-comparison.png", view.name),
                pixels: pair,
            });
        }
    }
    images.push(ReviewImage {
        name: "review.png".into(),
        pixels: sheet,
    });
    report.diagnostics.push(if gpu {
        "Native GPU cel/material rendering.".into()
    } else {
        "CPU diagnostic geometry rendering; GPU is required for final cel appearance.".into()
    });
    Ok(RenderedReview { report, images })
}
