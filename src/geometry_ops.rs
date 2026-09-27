// =========================================
// =========================================
// crates/motionloom/src/geometry_ops.rs

//! Shared geometry kernels for DSL compilation and GeometryRecipe authoring.
use crate::mesh_authoring::{Axis, MeshAuthoringError, UvProjection};
use crate::{
    ControlCageNode, CurveInterpolation, GeometryUvNode, PrimitiveAxis, SweepProfilePointNode,
};
use std::f32::consts::TAU;

pub(crate) fn axis_index(axis: PrimitiveAxis) -> usize {
    match axis {
        PrimitiveAxis::X => 0,
        PrimitiveAxis::Y => 1,
        PrimitiveAxis::Z => 2,
    }
}
fn recipe_axis(axis: PrimitiveAxis) -> Axis {
    match axis {
        PrimitiveAxis::X => Axis::X,
        PrimitiveAxis::Y => Axis::Y,
        PrimitiveAxis::Z => Axis::Z,
    }
}

/// Parameter sampling stays stable when an authored profile point moves.
pub(crate) fn profile_sample(
    points: &[SweepProfilePointNode],
    interpolation: CurveInterpolation,
    t: f32,
    closed: bool,
) -> [f32; 2] {
    let intervals = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    let position = t.clamp(0.0, 1.0) * intervals as f32;
    let index = (position.floor() as usize).min(intervals - 1);
    let f = position - index as f32;
    let at = |i: isize| {
        points[if closed {
            i.rem_euclid(points.len() as isize) as usize
        } else {
            i.clamp(0, points.len() as isize - 1) as usize
        }]
        .position
    };
    let a = at(index as isize);
    let b = at(index as isize + 1);
    if interpolation == CurveInterpolation::Linear {
        return std::array::from_fn(|k| a[k] + (b[k] - a[k]) * f);
    }
    let before = at(index as isize - 1);
    let after = at(index as isize + 2);
    std::array::from_fn(|k| {
        0.5 * (2.0 * a[k]
            + (-before[k] + b[k]) * f
            + (2.0 * before[k] - 5.0 * a[k] + 4.0 * b[k] - after[k]) * f * f
            + (-before[k] + 3.0 * a[k] - 3.0 * b[k] + after[k]) * f * f * f)
    })
}

pub(crate) fn revolve(
    axis: PrimitiveAxis,
    segments: u32,
    samples: u32,
    interpolation: CurveInterpolation,
    points: &[SweepProfilePointNode],
) -> Result<ControlCageNode, MeshAuthoringError> {
    if !(3..=256).contains(&segments)
        || !(2..=256).contains(&samples)
        || points.len() < 2
        || points
            .iter()
            .any(|p| p.position.iter().any(|v| !v.is_finite()) || p.position[0] < 0.0)
    {
        return Err(MeshAuthoringError::Operation(
            "invalid revolve profile or tessellation".into(),
        ));
    }
    if (segments as usize + 1) * samples as usize > 30_000 {
        return Err(MeshAuthoringError::Operation(
            "revolve exceeds geometry vertex budget".into(),
        ));
    }
    let mut cage = ControlCageNode {
        positions: vec![],
        uvs: vec![],
        pinned: vec![],
        faces: vec![],
        subdivision: 0,
    };
    let mut rings = vec![];
    let axis = axis_index(axis);
    let a = (axis + 2) % 3;
    let b = (axis + 1) % 3;
    for row in 0..samples {
        let t = row as f32 / (samples - 1) as f32;
        let [radius, height] = profile_sample(points, interpolation, t, false);
        let radius = radius.max(0.0);
        let pole = radius <= 1.0e-7;
        let mut ring = vec![];
        for col in 0..if pole { 1 } else { segments + 1 } {
            let u = if pole {
                0.5
            } else {
                col as f32 / segments as f32
            };
            let theta = if col == segments { 0.0 } else { u * TAU };
            let mut position = [0.0; 3];
            position[axis] = height;
            position[a] = radius * theta.cos();
            position[b] = radius * theta.sin();
            ring.push(cage.positions.len() as u32);
            cage.positions.push(position);
            cage.uvs.push([u, 1.0 - t]);
            cage.pinned.push(false);
        }
        rings.push(ring);
    }
    for pair in rings.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for col in 0..segments as usize {
            let face = if a.len() == 1 && b.len() == 1 {
                continue;
            } else if a.len() == 1 {
                vec![a[0], b[col], b[col + 1]]
            } else if b.len() == 1 {
                vec![a[col], b[0], a[col + 1]]
            } else {
                vec![a[col], b[col], b[col + 1], a[col + 1]]
            };
            cage.faces.push(face);
        }
    }
    Ok(cage)
}

pub(crate) fn radial_wave(
    positions: &mut [[f32; 3]],
    axis: PrimitiveAxis,
    cycles: u32,
    amplitude: f32,
    range: Option<[f32; 2]>,
    falloff: f32,
) {
    let axis = axis_index(axis);
    let a = (axis + 2) % 3;
    let b = (axis + 1) % 3;
    for p in positions {
        let weight = range.map_or(1.0, |r| {
            if p[axis] < r[0] || p[axis] > r[1] {
                0.0
            } else if falloff <= 0.0 {
                1.0
            } else {
                let t = ((p[axis] - r[0]).min(r[1] - p[axis]) / falloff).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        });
        let radius = p[a].hypot(p[b]);
        if radius < 1.0e-7 {
            continue;
        }
        let theta = p[b].atan2(p[a]);
        let offset = amplitude * weight * (cycles as f32 * theta).cos();
        let factor = (radius + offset).max(0.0) / radius;
        p[a] *= factor;
        p[b] *= factor;
    }
}

/// Seeded object-space value noise gives coincident seam vertices the same signal.
fn noise(p: [f32; 3], seed: u64) -> f32 {
    let cell = p.map(|v| v.floor() as i64);
    let f = std::array::from_fn::<_, 3, _>(|i| {
        let t = p[i] - p[i].floor();
        t * t * (3.0 - 2.0 * t)
    });
    let mut sum = 0.0;
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let mut h = seed ^ 0x9e3779b97f4a7c15;
                for (i, v) in [x, y, z].into_iter().enumerate() {
                    h ^= (cell[i] + v) as u64;
                    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
                    h ^= h >> 27;
                }
                let value = (h >> 40) as f32 / 16777215.0 * 2.0 - 1.0;
                let weight = [x, y, z]
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| if v == 0 { 1.0 - f[i] } else { f[i] })
                    .product::<f32>();
                sum += value * weight;
            }
        }
    }
    sum
}
pub(crate) fn displace_noise(
    positions: &mut [[f32; 3]],
    normals: &[Option<[f32; 3]>],
    amplitude: f32,
    frequency: f32,
    seed: u64,
) {
    for (i, p) in positions.iter_mut().enumerate() {
        let amount = noise(p.map(|v| v * frequency), seed) * amplitude;
        let n = normals.get(i).copied().flatten().unwrap_or([0.0, 1.0, 0.0]);
        for k in 0..3 {
            p[k] += n[k] * amount;
        }
    }
}

pub(crate) fn apply_uv(
    positions: &[[f32; 3]],
    normals: &[Option<[f32; 3]>],
    uvs: &mut [Option<[f32; 2]>],
    settings: &GeometryUvNode,
) {
    for (i, uv) in uvs.iter_mut().enumerate() {
        let point = positions[i];
        let normal = normals.get(i).copied().flatten().unwrap_or([0.0, 1.0, 0.0]);
        let projection = match settings.mode.to_ascii_lowercase().as_str() {
            "planar" => Some(UvProjection::Planar {
                u_axis: recipe_axis(settings.u_axis),
                v_axis: recipe_axis(settings.v_axis),
                scale: settings.scale,
                offset: settings.offset,
            }),
            "cylindrical" => Some(UvProjection::Cylindrical {
                axis: recipe_axis(settings.axis),
                scale: settings.scale,
                offset: settings.offset,
            }),
            "spherical" => Some(UvProjection::Spherical {
                scale: settings.scale,
                offset: settings.offset,
            }),
            "box" => Some(UvProjection::Box {
                scale: settings.scale,
                offset: settings.offset,
            }),
            _ => None,
        };
        *uv = Some(if let Some(projection) = projection {
            crate::mesh_authoring::geometry::project_uv(point, normal, &projection)
                .expect("validated geometry UV projection")
        } else {
            let old = uv.unwrap_or([0.0; 2]);
            std::array::from_fn(|k| old[k] * settings.scale[k] + settings.offset[k])
        });
    }
}

pub(crate) fn thicken(
    cage: &ControlCageNode,
    thickness: f32,
) -> Result<ControlCageNode, MeshAuthoringError> {
    use crate::mesh_authoring::{GeometryOperation, SemanticRegion};
    let region = SemanticRegion {
        id: "surface".into(),
        vertices: vec![],
        faces: (0..cage.faces.len() as u64).collect(),
    };
    Ok(crate::mesh_authoring::geometry::execute_operations_on_cage(
        cage.clone(),
        vec![region],
        &[GeometryOperation::ThickenSurface {
            id: "thickened".into(),
            region: "surface".into(),
            thickness,
        }],
        crate::mesh_reference::MeshProposalValidationOptions::default(),
    )?
    .cage)
}

pub(crate) fn partition(
    cage: &ControlCageNode,
    u_range: [f32; 2],
    v_range: [f32; 2],
) -> Result<ControlCageNode, MeshAuthoringError> {
    use std::collections::BTreeMap;
    let mut result = ControlCageNode {
        positions: vec![],
        uvs: vec![],
        pinned: vec![],
        faces: vec![],
        subdivision: 0,
    };
    let mut remap = BTreeMap::new();
    for face in &cage.faces {
        let uv = face.iter().fold([0.0; 2], |mut sum, &i| {
            for k in 0..2 {
                sum[k] += cage.uvs[i as usize][k] / face.len() as f32;
            }
            sum
        });
        if uv[0] < u_range[0] || uv[0] >= u_range[1] || uv[1] < v_range[0] || uv[1] >= v_range[1] {
            continue;
        }
        let next = face
            .iter()
            .map(|&i| {
                *remap.entry(i).or_insert_with(|| {
                    let n = result.positions.len() as u32;
                    result.positions.push(cage.positions[i as usize]);
                    result.uvs.push(cage.uvs[i as usize]);
                    result.pinned.push(cage.pinned[i as usize]);
                    n
                })
            })
            .collect();
        result.faces.push(next);
    }
    if result.faces.is_empty() {
        return Err(MeshAuthoringError::Operation(
            "Partition selects no faces".into(),
        ));
    }
    Ok(result)
}

pub(crate) fn wireframe(
    cage: &ControlCageNode,
    radius: f32,
    segments: u32,
) -> Result<ControlCageNode, MeshAuthoringError> {
    use crate::mesh_authoring::{GeometryOperation, GeometryRecipe, MESH_AUTHORING_SCHEMA_VERSION};
    use std::collections::BTreeSet;
    let mut edges = BTreeSet::new();
    let mut operations = vec![];
    for face in &cage.faces {
        for i in 0..face.len() {
            let a = cage.positions[face[i] as usize];
            let b = cage.positions[face[(i + 1) % face.len()] as usize];
            if a == b {
                continue;
            }
            let key = |p: [f32; 3]| p.map(|v| (v as f64 * 1e6).round() as i64);
            let (ka, kb) = (key(a), key(b));
            let edge = if ka < kb { (ka, kb) } else { (kb, ka) };
            if edges.insert(edge) {
                operations.push(GeometryOperation::SweepProfile {
                    id: format!("edge_{}", operations.len()),
                    path: vec![a, b],
                    radius,
                    segments: segments as usize,
                });
            }
        }
    }
    let vertices = operations.len().saturating_mul(segments as usize * 2 + 2);
    let faces = operations.len().saturating_mul(segments as usize * 4);
    if vertices > 30_000 || faces > 30_000 {
        return Err(MeshAuthoringError::LimitExceeded { vertices, faces });
    }
    let recipe = GeometryRecipe {
        schema_version: MESH_AUTHORING_SCHEMA_VERSION.into(),
        id: "wireframe".into(),
        subdivision: 0,
        operations,
    };
    Ok(crate::mesh_authoring::execute_geometry_recipe(&recipe)?.cage)
}

/// Keep recipe inputs and the canonical DSL on the same modifier contract.
pub(crate) fn validate_modifier(
    modifier: &crate::PrimitiveModifierNode,
) -> Result<(), MeshAuthoringError> {
    use crate::PrimitiveModifierNode::*;
    let positive = |x: f32| x.is_finite() && x > 0.0;
    let nonnegative = |x: f32| x.is_finite() && x >= 0.0;
    let range = |r: [f32; 2]| r.iter().all(|x| x.is_finite()) && r[0] < r[1];
    let valid = match modifier {
        Subdivision { levels, scheme } => {
            matches!(
                scheme.to_ascii_lowercase().as_str(),
                "linear" | "catmullclark"
            ) && *levels
                <= if scheme.eq_ignore_ascii_case("linear") {
                    3
                } else {
                    2
                }
        }
        RadialWave {
            cycles,
            amplitude,
            height_range,
            falloff,
            ..
        } => {
            (1..=128).contains(cycles)
                && nonnegative(*amplitude)
                && nonnegative(*falloff)
                && height_range.is_none_or(range)
        }
        DisplaceNoise {
            amplitude,
            frequency,
            ..
        } => nonnegative(*amplitude) && positive(*frequency),
        ThickenSurface { thickness } => positive(*thickness),
        Wireframe { radius, segments } => positive(*radius) && (3..=256).contains(segments),
        Partition { u_range, v_range } => range(*u_range) && range(*v_range),
        Uv { settings } => {
            matches!(
                settings.mode.as_str(),
                "authored"
                    | "profileparameter"
                    | "distance"
                    | "normalized"
                    | "planar"
                    | "cylindrical"
                    | "spherical"
                    | "box"
            ) && settings.scale.iter().all(|x| positive(*x))
                && settings.offset.iter().all(|x| x.is_finite())
                && (settings.mode != "planar" || settings.u_axis != settings.v_axis)
        }
        _ => true,
    };
    if !valid {
        return Err(MeshAuthoringError::Operation(
            "invalid geometry modifier parameters".into(),
        ));
    }
    Ok(())
}
