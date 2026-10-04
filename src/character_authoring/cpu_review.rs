// =========================================
// =========================================
// src/character_authoring/cpu_review.rs

use super::{CharacterError, ReviewView};

/// Rasterize resolved world geometry for a GPU-independent modeling diagnostic.
/// Scene's CPU compositor does not render native 3D islands; this path uses its shared lowering.
pub async fn render_cpu_review(
    graph: &crate::GraphScript,
    view: &ReviewView,
    gray: bool,
) -> Result<image::RgbaImage, CharacterError> {
    if !(64..=2048).contains(&graph.size.0)
        || graph.size.0 != graph.size.1
        || !view.vertical_scale.is_finite()
        || view.vertical_scale <= 0.
        || !view.yaw.is_finite()
        || !view.pitch.is_finite()
        || view.target.iter().any(|v| !v.is_finite())
    {
        return Err(CharacterError::Invalid(
            "CPU review needs finite framing and a square 64..2048 image".into(),
        ));
    }
    let scene = graph
        .scenes
        .first()
        .ok_or_else(|| CharacterError::Invalid("No review scene".into()))?;
    let geometry = crate::experimental::extract_scene_geometry(
        graph,
        &crate::experimental::SceneGeometryOptions {
            scene_id: scene.id.clone(),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CharacterError::Geometry(e.to_string()))?;
    let size = graph.size.0;
    let mut image = image::RgbaImage::from_pixel(size, size, image::Rgba([235, 235, 235, 255]));
    let mut depths = vec![f32::NEG_INFINITY; (size * size) as usize];
    let (sy, cy) = view.yaw.to_radians().sin_cos();
    let (sp, cp) = view.pitch.to_radians().sin_cos();
    let right = [cy, 0., -sy];
    let up = [-sy * sp, cp, -cy * sp];
    let toward = [sy * cp, sp, cy * cp];
    let dot = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
    let project = |p: [f32; 3]| {
        let p = std::array::from_fn(|i| p[i] - view.target[i]);
        [
            size as f32 * (0.5 + dot(p, right) / view.vertical_scale),
            size as f32 * (0.5 - dot(p, up) / view.vertical_scale),
            dot(p, toward),
        ]
    };
    let edge = |a: [f32; 3], b: [f32; 3], x: f32, y: f32| {
        (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0])
    };
    for mesh in geometry.meshes {
        let points: Vec<_> = mesh.positions.iter().copied().map(project).collect();
        for ids in mesh.indices.chunks_exact(3) {
            let p = [ids[0], ids[1], ids[2]].map(|i| points[i as usize]);
            let area = edge(p[0], p[1], p[2][0], p[2][1]);
            if area.abs() < 1e-8 {
                continue;
            }
            let low = [0, 1].map(|axis| {
                p.iter()
                    .map(|p| p[axis])
                    .fold(f32::INFINITY, f32::min)
                    .floor()
                    .max(0.) as u32
            });
            let high = [0, 1].map(|axis| {
                p.iter()
                    .map(|p| p[axis])
                    .fold(f32::NEG_INFINITY, f32::max)
                    .ceil()
                    .min(size as f32 - 1.) as u32
            });
            if low[0] > high[0] || low[1] > high[1] {
                continue;
            }
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let a = edge(p[1], p[2], x as f32 + 0.5, y as f32 + 0.5) / area;
                    let b = edge(p[2], p[0], x as f32 + 0.5, y as f32 + 0.5) / area;
                    let weights = [a, b, 1. - a - b];
                    if weights.iter().any(|w| *w < 0.) {
                        continue;
                    }
                    let z = (0..3).map(|i| weights[i] * p[i][2]).sum::<f32>();
                    let index = (y * size + x) as usize;
                    if z <= depths[index] {
                        continue;
                    }
                    depths[index] = z;
                    let normal = std::array::from_fn(|axis| {
                        (0..3)
                            .map(|i| weights[i] * mesh.normals[ids[i] as usize][axis])
                            .sum::<f32>()
                    });
                    let shade =
                        (0.45 + dot(normal, [0.35, 0.65, 0.67]).max(0.) * 0.55).clamp(0., 1.);
                    let base = if gray {
                        [0.65; 4]
                    } else {
                        mesh.material.base_color_factor
                    };
                    image.put_pixel(
                        x,
                        y,
                        image::Rgba(std::array::from_fn(|channel| {
                            if channel == 3 {
                                255
                            } else {
                                (base[channel] * shade * 255.) as u8
                            }
                        })),
                    );
                }
            }
        }
    }
    Ok(image)
}
