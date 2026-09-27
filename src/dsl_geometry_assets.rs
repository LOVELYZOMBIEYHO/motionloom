// =========================================
// =========================================
// crates/motionloom/src/dsl_geometry_assets.rs

use super::*;

/// A geometry declaration retains its independent source and authored operation order.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GeometryAssetNode {
    pub id: String,
    pub source: Option<String>,
    pub generator: String,
    pub geometry: Option<PrimitiveGeometry>,
    pub modifiers: Vec<PrimitiveModifierNode>,
    pub uv: GeometryUvNode,
    pub uv_operation_index: usize,
    pub mesh_build: PrimitiveMeshBuildNode,
    pub mesh_build_authored: bool,
    pub lod: PrimitiveLodNode,
    pub lod_authored: bool,
    pub bevel_radius: f32,
    pub bevel_segments: u32,
}

/// Geometry UV controls are independent of material texture sampling transforms.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct GeometryUvNode {
    pub mode: String,
    pub axis: PrimitiveAxis,
    pub u_axis: PrimitiveAxis,
    pub v_axis: PrimitiveAxis,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}
impl Default for GeometryUvNode {
    fn default() -> Self {
        Self {
            mode: "authored".into(),
            axis: PrimitiveAxis::Y,
            u_axis: PrimitiveAxis::X,
            v_axis: PrimitiveAxis::Y,
            scale: [1.0; 2],
            offset: [0.0; 2],
        }
    }
}

pub(super) struct PendingMeshAsset {
    pub(super) asset_index: usize,
    id: String,
    geometry: String,
    material: String,
    tag: String,
    line: usize,
}

pub(super) fn nonempty_children(lines: &[&str], start: usize, end: usize) -> Vec<usize> {
    let mut result = vec![];
    let mut i = start;
    while i < end {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with("<!--") {
            i += 1;
            continue;
        }
        result.push(i);
        let Ok((tag, open)) = collect_tag_block(lines, i, '>', false) else {
            break;
        };
        if is_self_closing_tag(&tag) {
            i = open + 1;
        } else {
            let name = tag
                .trim_start()
                .trim_start_matches('<')
                .split(|c: char| c.is_whitespace() || c == '>')
                .next()
                .unwrap_or("");
            match find_matching_close_tag(lines, open + 1, name) {
                Ok(close) => i = close + 1,
                Err(_) => break,
            }
        }
    }
    result
}

pub(super) fn parse_profile(
    lines: &[&str],
    start: usize,
    id: &str,
) -> Result<(Vec<SweepProfilePointNode>, bool, CurveInterpolation, usize), GraphParseError> {
    let (tag, open) = collect_tag_block(lines, start, '>', false)?;
    if !starts_open_tag(tag.trim(), "Profile") || is_self_closing_tag(&tag) {
        return Err(GraphParseError {
            line: start + 1,
            message: "Geometry requires a Profile block.".into(),
        });
    }
    validate_hair_attributes(&tag, &["closed", "interpolation"], "Profile", start + 1)?;
    let closed = parse_optional_primitive_bool(&tag, "closed", id, start + 1)?.unwrap_or(false);
    let interpolation = match primitive_string_attribute(&tag, "interpolation", "linear").as_str() {
        "linear" => CurveInterpolation::Linear,
        "catmullrom" => CurveInterpolation::CatmullRom,
        _ => {
            return Err(GraphParseError {
                line: start + 1,
                message: "Profile interpolation must be linear or catmullRom.".into(),
            });
        }
    };
    let end = find_matching_close_tag(lines, open + 1, "Profile")?;
    let mut points = vec![];
    for i in nonempty_children(lines, open + 1, end) {
        let (point, _) = collect_self_closing_block(lines, i)?;
        if !starts_open_tag(point.trim(), "ProfilePoint") {
            return Err(GraphParseError {
                line: i + 1,
                message: "Profile only accepts ProfilePoint.".into(),
            });
        }
        validate_hair_attributes(&point, &["position"], "ProfilePoint", i + 1)?;
        let position = parse_optional_primitive_vec::<2>(&point, "position", id, i + 1, false)?
            .ok_or_else(|| GraphParseError {
                line: i + 1,
                message: "ProfilePoint requires position.".into(),
            })?;
        points.push(SweepProfilePointNode { position });
    }
    if points.len() < if closed { 3 } else { 2 } || points.windows(2).any(|p| p[0] == p[1]) {
        return Err(GraphParseError {
            line: start + 1,
            message: "Profile needs distinct points (at least two open or three closed).".into(),
        });
    }
    Ok((points, closed, interpolation, end))
}

pub(super) fn parse_geometry(
    lines: &[&str],
    start: usize,
) -> Result<(GeometryAssetNode, usize), GraphParseError> {
    let (tag, open) = collect_tag_block(lines, start, '>', false)?;
    validate_hair_attributes(&tag, &["id", "source"], "GeometryAsset", start + 1)?;
    if is_self_closing_tag(&tag) {
        return Err(GraphParseError {
            line: start + 1,
            message: "GeometryAsset requires a generator or source plus operations.".into(),
        });
    }
    let id = strip_wrappers(&required_attr_value(&tag, "id", start + 1)?).to_string();
    let source = attr_value(&tag, "source").map(|v| strip_wrappers(&v).to_string());
    let end = find_matching_close_tag(lines, open + 1, "GeometryAsset")?;
    let mut result = GeometryAssetNode {
        id: id.clone(),
        source,
        generator: String::new(),
        geometry: None,
        modifiers: vec![],
        uv: GeometryUvNode::default(),
        uv_operation_index: 0,
        mesh_build: PrimitiveMeshBuildNode::default(),
        mesh_build_authored: false,
        lod: PrimitiveLodNode::default(),
        lod_authored: false,
        bevel_radius: 0.0,
        bevel_segments: 0,
    };
    let mut seen = HashSet::new();
    let mut saw_uv = false;
    for i in nonempty_children(lines, open + 1, end) {
        let (child, _) = collect_tag_block(lines, i, '>', false)?;
        let name = child
            .trim_start()
            .trim_start_matches('<')
            .split(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .next()
            .unwrap_or("");
        if !seen.insert(name.to_string()) {
            return Err(GraphParseError {
                line: i + 1,
                message: format!("GeometryAsset {id} repeats <{name}>."),
            });
        }
        match name {
            "Modifiers" => result.modifiers = parse_primitive_modifiers(lines, i, &id)?.0,
            "MeshBuild" => {
                result.mesh_build = parse_primitive_mesh_build(&child, &id, i + 1)?;
                result.mesh_build_authored = true;
            }
            "LOD" => {
                result.lod = parse_primitive_lod(&child, &id, i + 1)?;
                result.lod_authored = true;
            }
            "UV" => {
                result.uv = parse_uv(&child, &id, i + 1)?;
                result.uv_operation_index = result.modifiers.len();
                saw_uv = true;
            }
            "Primitive" | "Mesh" | "Sweep" | "Loft" | "Ribbon" | "Revolve" | "Head" | "Hair" => {
                if result.source.is_some() || result.geometry.is_some() {
                    return Err(GraphParseError {
                        line: i + 1,
                        message:
                            "GeometryAsset accepts exactly one generator OR source, never both."
                                .into(),
                    });
                }
                result.generator = name.into();
                let geometry = match name {
                    "Primitive" => {
                        if !is_self_closing_tag(&child) {
                            return Err(GraphParseError {
                                line: i + 1,
                                message:
                                    "Primitive is self-closing; put operations on GeometryAsset."
                                        .into(),
                            });
                        }
                        let (geometry, bevel_radius, bevel_segments) =
                            parse_primitive_geometry(&child, &id, i + 1)?;
                        result.bevel_radius = bevel_radius;
                        result.bevel_segments = bevel_segments;
                        geometry
                    }
                    "Head" => parse_head_asset_block(lines, i, &id)?.0.geometry,
                    "Hair" => parse_hair_asset_block(lines, i, &id)?.0.geometry,
                    "Mesh" => PrimitiveGeometry::Mesh {
                        cage: control_cage_parser::parse_mesh_geometry(lines, i, &id)?.0,
                    },
                    "Sweep" => parse_sweep_geometry(lines, i, &id)?.0,
                    "Loft" => parse_primitive_loft(lines, i, &id)?.0,
                    "Ribbon" => parse_primitive_ribbon(lines, i, &id)?.0,
                    "Revolve" => parse_revolve(lines, i, &id)?,
                    _ => unreachable!(),
                };
                result.geometry = Some(geometry);
            }
            _ => {
                return Err(GraphParseError {
                    line: i + 1,
                    message: format!("GeometryAsset does not accept <{name}>."),
                });
            }
        }
    }
    if result.geometry.is_none() && result.source.is_none() {
        return Err(GraphParseError {
            line: start + 1,
            message: "GeometryAsset requires exactly one generator or source.".into(),
        });
    }
    if !saw_uv {
        result.uv_operation_index = result.modifiers.len();
        if result.generator == "Sweep" {
            result.uv.mode = "distance".into();
        }
        if result.generator == "Revolve" {
            result.uv.mode = "profileparameter".into();
        }
    }
    Ok((result, end))
}

fn parse_revolve(
    lines: &[&str],
    start: usize,
    id: &str,
) -> Result<PrimitiveGeometry, GraphParseError> {
    let (tag, open) = collect_tag_block(lines, start, '>', false)?;
    validate_hair_attributes(&tag, &["axis", "segments", "samples"], "Revolve", start + 1)?;
    let axis = parse_primitive_axis(&tag, id, start + 1)?;
    let segments = parse_primitive_segments(&tag, "segments", 64, id, start + 1)?;
    let samples = parse_optional_primitive_u32(&tag, "samples", id, start + 1)?.unwrap_or(56);
    if !(2..=256).contains(&samples) {
        return Err(GraphParseError {
            line: start + 1,
            message: "Revolve samples must be 2..256.".into(),
        });
    }
    let end = find_matching_close_tag(lines, open + 1, "Revolve")?;
    let children = nonempty_children(lines, open + 1, end);
    if children.len() != 1 {
        return Err(GraphParseError {
            line: start + 1,
            message: "Revolve accepts one Profile.".into(),
        });
    }
    let (points, closed, interpolation, _) = parse_profile(lines, children[0], id)?;
    if closed || points.iter().any(|p| p.position[0] < 0.0) {
        return Err(GraphParseError {
            line: start + 1,
            message: "Revolve requires an open profile of nonnegative radii.".into(),
        });
    }
    let cage = crate::geometry_ops::revolve(axis, segments, samples, interpolation, &points)
        .map_err(|e| GraphParseError {
            line: start + 1,
            message: e.to_string(),
        })?;
    if cage.faces.is_empty() {
        return Err(GraphParseError {
            line: start + 1,
            message: "Revolve profile produces no surface.".into(),
        });
    }
    Ok(PrimitiveGeometry::Mesh { cage })
}

fn parse_uv(tag: &str, id: &str, line: usize) -> Result<GeometryUvNode, GraphParseError> {
    validate_hair_attributes(
        tag,
        &["mode", "axis", "uAxis", "vAxis", "scale", "offset"],
        "UV",
        line,
    )?;
    let mode = strip_wrappers(&required_attr_value(tag, "mode", line)?).to_ascii_lowercase();
    if !matches!(
        mode.as_str(),
        "authored"
            | "profileparameter"
            | "distance"
            | "normalized"
            | "planar"
            | "cylindrical"
            | "spherical"
            | "box"
    ) {
        return Err(GraphParseError { line, message: "UV mode must be authored, profileParameter, distance, normalized, planar, cylindrical, spherical, or box.".into() });
    }
    let axis_value = |name: &str, fallback| -> Result<PrimitiveAxis, GraphParseError> {
        match attr_value(tag, name).map(|v| strip_wrappers(&v).to_ascii_lowercase()) {
            None => Ok(fallback),
            Some(v) => match v.as_str() {
                "x" => Ok(PrimitiveAxis::X),
                "y" => Ok(PrimitiveAxis::Y),
                "z" => Ok(PrimitiveAxis::Z),
                _ => Err(GraphParseError {
                    line,
                    message: format!("UV {name} must be x, y, or z."),
                }),
            },
        }
    };
    let result = GeometryUvNode {
        mode,
        axis: axis_value("axis", PrimitiveAxis::Y)?,
        u_axis: axis_value("uAxis", PrimitiveAxis::X)?,
        v_axis: axis_value("vAxis", PrimitiveAxis::Y)?,
        scale: parse_optional_primitive_vec::<2>(tag, "scale", id, line, true)?.unwrap_or([1.0; 2]),
        offset: parse_optional_primitive_vec::<2>(tag, "offset", id, line, false)?
            .unwrap_or([0.0; 2]),
    };
    if result.mode == "planar" && result.u_axis == result.v_axis {
        return Err(GraphParseError {
            line,
            message: "Planar UV axes must differ.".into(),
        });
    }
    Ok(result)
}

pub(super) fn parse_new_modifier(
    tag: &str,
    id: &str,
    line: usize,
) -> Result<Option<PrimitiveModifierNode>, GraphParseError> {
    let modifier = if starts_open_tag(tag.trim(), "RadialWave") {
        validate_hair_attributes(
            tag,
            &["axis", "cycles", "amplitude", "heightRange", "falloff"],
            "RadialWave",
            line,
        )?;
        let cycles = parse_optional_primitive_u32(tag, "cycles", id, line)?.unwrap_or(5);
        let height_range = parse_optional_primitive_vec::<2>(tag, "heightRange", id, line, false)?;
        if cycles == 0 || cycles > 128 || height_range.is_some_and(|r| r[0] >= r[1]) {
            return Err(GraphParseError {
                line,
                message: "RadialWave requires cycles 1..128 and ascending heightRange.".into(),
            });
        }
        PrimitiveModifierNode::RadialWave {
            axis: parse_primitive_axis(tag, id, line)?,
            cycles,
            amplitude: parse_optional_nonnegative_primitive_number(tag, "amplitude", id, line)?
                .unwrap_or(0.0),
            height_range,
            falloff: parse_optional_nonnegative_primitive_number(tag, "falloff", id, line)?
                .unwrap_or(0.0),
        }
    } else if starts_open_tag(tag.trim(), "DisplaceNoise") {
        validate_hair_attributes(
            tag,
            &["amplitude", "frequency", "seed"],
            "DisplaceNoise",
            line,
        )?;
        PrimitiveModifierNode::DisplaceNoise {
            amplitude: parse_optional_nonnegative_primitive_number(tag, "amplitude", id, line)?
                .unwrap_or(0.0),
            frequency: parse_optional_positive_primitive_number(tag, "frequency", id, line)?
                .unwrap_or(1.0),
            seed: parse_optional_primitive_u64(tag, "seed", id, line)?.unwrap_or(0),
        }
    } else if starts_open_tag(tag.trim(), "ThickenSurface") {
        validate_hair_attributes(tag, &["thickness"], "ThickenSurface", line)?;
        PrimitiveModifierNode::ThickenSurface {
            thickness: parse_positive_primitive_number(tag, "thickness", id, line)?,
        }
    } else if starts_open_tag(tag.trim(), "Wireframe") {
        validate_hair_attributes(tag, &["radius", "segments"], "Wireframe", line)?;
        PrimitiveModifierNode::Wireframe {
            radius: parse_positive_primitive_number(tag, "radius", id, line)?,
            segments: parse_primitive_segments(tag, "segments", 4, id, line)?,
        }
    } else if starts_open_tag(tag.trim(), "Partition") {
        validate_hair_attributes(tag, &["uRange", "vRange"], "Partition", line)?;
        let u_range = parse_optional_primitive_vec::<2>(tag, "uRange", id, line, false)?
            .ok_or_else(|| GraphParseError {
                line,
                message: "Partition requires uRange.".into(),
            })?;
        let v_range = parse_optional_primitive_vec::<2>(tag, "vRange", id, line, false)?
            .ok_or_else(|| GraphParseError {
                line,
                message: "Partition requires vRange.".into(),
            })?;
        if u_range[0] >= u_range[1] || v_range[0] >= v_range[1] {
            return Err(GraphParseError {
                line,
                message: "Partition ranges must ascend.".into(),
            });
        }
        PrimitiveModifierNode::Partition { u_range, v_range }
    } else {
        return Ok(None);
    };
    Ok(Some(modifier))
}

pub(super) fn parse_mesh_reference(
    lines: &[&str],
    start: usize,
) -> Result<(PendingMeshAsset, usize), GraphParseError> {
    let (tag, end) = collect_tag_block(lines, start, '>', false)?;
    if !is_self_closing_tag(&tag) {
        return Err(GraphParseError { line: start + 1, message: "MeshAsset only references geometry/material. Move Vertex/Face into GeometryAsset/Mesh.".into() });
    }
    validate_hair_attributes(
        &tag,
        &[
            "id",
            "geometry",
            "material",
            "color",
            "materialSeed",
            "collision",
            "collider",
            "colliderSize",
            "colliderRadius",
            "colliderHeight",
            "colliderScale",
            "colliderOffset",
            "colliderRotation",
            "colliderMargin",
            "collisionGroup",
            "collisionMask",
            "friction",
            "restitution",
            "density",
        ],
        "MeshAsset",
        start + 1,
    )?;
    Ok((
        PendingMeshAsset {
            asset_index: 0,
            id: strip_wrappers(&required_attr_value(&tag, "id", start + 1)?).into(),
            geometry: strip_wrappers(&required_attr_value(&tag, "geometry", start + 1)?).into(),
            material: strip_wrappers(&required_attr_value(&tag, "material", start + 1)?).into(),
            tag,
            line: start + 1,
        },
        end,
    ))
}

pub(super) fn resolve_assets(
    assets: &mut Vec<GraphAssetNode>,
    geometries: &[GeometryAssetNode],
    meshes: Vec<PendingMeshAsset>,
    curves: &[CurveAssetNode],
    line: usize,
) -> Result<(), GraphParseError> {
    let mut ids: HashSet<String> = assets
        .iter()
        .map(|a| a.id.clone())
        .chain(curves.iter().map(|c| c.id.clone()))
        .collect();
    let mut definitions = HashMap::new();
    for geometry in geometries {
        if geometry.id.is_empty() || !ids.insert(geometry.id.clone()) {
            return Err(GraphParseError {
                line,
                message: format!("Duplicate or empty geometry id: {}", geometry.id),
            });
        }
        definitions.insert(geometry.id.clone(), geometry);
    }
    let mut resolved = HashMap::new();
    for geometry in geometries {
        resolve_geometry(
            &geometry.id,
            &definitions,
            curves,
            &mut resolved,
            &mut vec![],
            line,
        )?;
    }
    for mesh in meshes {
        if mesh.id.is_empty() || !ids.insert(mesh.id.clone()) {
            return Err(GraphParseError {
                line: mesh.line,
                message: format!("Duplicate or empty asset id: {}", mesh.id),
            });
        }
        let mut primitive =
            resolved
                .get(&mesh.geometry)
                .cloned()
                .ok_or_else(|| GraphParseError {
                    line: mesh.line,
                    message: format!(
                        "MeshAsset {} references unknown GeometryAsset {}",
                        mesh.id, mesh.geometry
                    ),
                })?;
        primitive.id = mesh.id.clone();
        primitive.material = Some(mesh.material);
        primitive.color = attr_value(&mesh.tag, "color")
            .map(|v| parse_primitive_color(&v, &mesh.id, mesh.line))
            .transpose()?
            .unwrap_or([1.0; 4]);
        primitive.material_seed =
            parse_optional_primitive_u64(&mesh.tag, "materialSeed", &mesh.id, mesh.line)?;
        primitive.collision =
            parse_primitive_collision(&mesh.tag, &mesh.id, &primitive.geometry, mesh.line)?;
        assets.insert(
            mesh.asset_index.min(assets.len()),
            GraphAssetNode {
                id: mesh.id,
                kind: GraphAssetKind::Model,
                source: GraphAssetSource::Primitive(primitive),
                decoder: None,
                color_space: None,
                profile: None,
                clip: None,
            },
        );
    }
    for asset in assets.iter() {
        if let Some(compound) = asset.compound() {
            for instance in &compound.instances {
                if assets
                    .iter()
                    .find(|a| a.id == instance.asset)
                    .and_then(GraphAssetNode::primitive)
                    .is_none()
                {
                    return Err(GraphParseError {
                        line,
                        message: format!(
                            "CompoundAsset {} must reference a generated MeshAsset; unknown/non-generated model asset {}",
                            compound.id, instance.asset
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

fn resolve_geometry(
    id: &str,
    definitions: &HashMap<String, &GeometryAssetNode>,
    curves: &[CurveAssetNode],
    resolved: &mut HashMap<String, PrimitiveAssetNode>,
    visiting: &mut Vec<String>,
    line: usize,
) -> Result<PrimitiveAssetNode, GraphParseError> {
    if let Some(value) = resolved.get(id) {
        return Ok(value.clone());
    }
    if visiting.iter().any(|v| v == id) {
        return Err(GraphParseError {
            line,
            message: format!(
                "Cyclic GeometryAsset reference: {} -> {id}",
                visiting.join(" -> ")
            ),
        });
    }
    let node = definitions.get(id).ok_or_else(|| GraphParseError {
        line,
        message: format!("Unknown GeometryAsset {id}"),
    })?;
    visiting.push(id.into());
    let mut value = if let Some(source) = &node.source {
        resolve_geometry(source, definitions, curves, resolved, visiting, line)?
    } else {
        PrimitiveAssetNode {
            id: id.into(),
            geometry: node.geometry.clone().ok_or_else(|| GraphParseError {
                line,
                message: format!("GeometryAsset {id} has no generator"),
            })?,
            color: [1.0; 4],
            material: None,
            material_definition: None,
            bevel_radius: node.bevel_radius,
            bevel_segments: node.bevel_segments,
            material_seed: None,
            collision: PrimitiveCollisionNode::default(),
            modifiers: vec![],
            mesh_build: PrimitiveMeshBuildNode::default(),
            lod: PrimitiveLodNode::default(),
        }
    };
    if let PrimitiveGeometry::Sweep { curve, uv_mode, .. } = &mut value.geometry {
        if curve.points.is_empty() {
            *curve = curves
                .iter()
                .find(|c| c.id == curve.id)
                .cloned()
                .ok_or_else(|| GraphParseError {
                    line,
                    message: format!(
                        "GeometryAsset {id} references unknown CurveAsset {}",
                        curve.id
                    ),
                })?;
        }
        if matches!(node.uv.mode.as_str(), "distance" | "normalized") {
            *uv_mode = node.uv.mode.clone();
        }
    } else if matches!(node.uv.mode.as_str(), "distance" | "normalized") {
        return Err(GraphParseError {
            line,
            message: "distance/normalized UV requires Sweep geometry.".into(),
        });
    }
    if node.uv.mode == "profileparameter" && node.generator != "Revolve" && node.source.is_none() {
        return Err(GraphParseError {
            line,
            message: "profileParameter UV requires Revolve geometry.".into(),
        });
    }
    value.id = id.into();
    let mut operations = node.modifiers.clone();
    if node.uv != GeometryUvNode::default() {
        operations.insert(
            node.uv_operation_index.min(operations.len()),
            PrimitiveModifierNode::Uv {
                settings: node.uv.clone(),
            },
        );
    }
    value.modifiers.extend(operations);
    if node.source.is_none() || node.mesh_build_authored {
        value.mesh_build = node.mesh_build.clone();
    }
    if node.source.is_none() || node.lod_authored {
        value.lod = node.lod.clone();
    }
    validate_primitive_build_budget(&value, line)?;
    if value.modifiers.iter().any(|m| {
        matches!(
            m,
            PrimitiveModifierNode::ThickenSurface { .. }
                | PrimitiveModifierNode::Wireframe { .. }
                | PrimitiveModifierNode::Partition { .. }
        )
    }) {
        crate::world::primitive::validate_geometry_operations(&value).map_err(|e| {
            GraphParseError {
                line,
                message: e.to_string(),
            }
        })?;
    }
    visiting.pop();
    resolved.insert(id.into(), value.clone());
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph(assets: &str) -> Result<GraphScript, GraphParseError> {
        parse_graph_script(&format!(r##"<Graph fps={{24}} duration="1s" size={{[64,64]}}><Assets><MaterialAsset id="red" baseColor="#E33828" /><MaterialAsset id="gray" baseColor="#AAAAAA" />{assets}</Assets><Background color="#000000" /><Present from="scene" /></Graph>"##).replace("><", ">\n<"))
    }
    const PROFILE: &str = r#"<Revolve axis="y" segments="16" samples="12"><Profile interpolation="catmullRom"><ProfilePoint position={[0,-1]} /><ProfilePoint position={[0.85,-0.8]} /><ProfilePoint position={[1.05,0]} /><ProfilePoint position={[0.9,0.7]} /><ProfilePoint position={[0,0.8]} /></Profile></Revolve>"#;
    #[test]
    fn one_geometry_can_bind_multiple_materials_with_one_cache_identity() {
        let g = graph(&format!(r#"<MeshAsset id="apple" geometry="body" material="red" /><MeshAsset id="clay" geometry="body" material="gray" /><GeometryAsset id="body">{PROFILE}</GeometryAsset>"#)).unwrap();
        assert_eq!(g.geometry_assets.len(), 1);
        let a = g.assets[0].primitive().unwrap();
        let b = g.assets[1].primitive().unwrap();
        assert_eq!(a.geometry, b.geometry);
        assert_eq!(
            crate::world::primitive::primitive_geometry_cache_key(a),
            crate::world::primitive::primitive_geometry_cache_key(b)
        );
        assert_ne!(
            crate::world::primitive::primitive_material_cache_key(a),
            crate::world::primitive::primitive_material_cache_key(b)
        );
    }
    #[test]
    fn profile_edit_keeps_uv_correspondence_and_revolution_seam() {
        let parse = |profile: &str| {
            graph(&format!(
                r#"<GeometryAsset id="body">{profile}</GeometryAsset>"#
            ))
            .unwrap()
        };
        let a = parse(PROFILE);
        let b = parse(&PROFILE.replace("0.85,-0.8", "1.1,-0.8"));
        let PrimitiveGeometry::Mesh { cage: ac } = a.geometry_assets[0].geometry.as_ref().unwrap()
        else {
            panic!()
        };
        let PrimitiveGeometry::Mesh { cage: bc } = b.geometry_assets[0].geometry.as_ref().unwrap()
        else {
            panic!()
        };
        assert_eq!(ac.uvs, bc.uvs);
        assert_eq!(ac.faces, bc.faces);
        assert_ne!(ac.positions, bc.positions);
        assert_eq!(ac.positions[1], ac.positions[17]);
        assert_eq!(ac.uvs[1][0], 0.0);
        assert_eq!(ac.uvs[17][0], 1.0);
        assert!(
            ac.faces
                .iter()
                .all(|f| f.len() >= 3 && f.iter().all(|&v| (v as usize) < ac.positions.len()))
        );
    }
    #[test]
    fn rejects_old_asset_forms_and_duplicate_uv_ownership() {
        for assets in [
            r#"<PrimitiveAsset id="p" shape="sphere" radius="1" />"#,
            r#"<SweepAsset id="s" curve="c" />"#,
            r#"<HeadAsset id="h" material="red" />"#,
            r#"<HairAsset id="h" material="red" />"#,
            r#"<MeshAsset id="m" material="red"><Vertex position={[0,0,0]} /></MeshAsset>"#,
            r#"<MaterialAsset id="old" mapping="box" />"#,
            r#"<GeometryAsset id="p"><Primitive shape="sphere" radius="1" material="red" /></GeometryAsset>"#,
            r#"<GeometryAsset id="p"><Primitive shape="sphere" radius="1" /><Modifiers><Subdivision levels="1" /></Modifiers></GeometryAsset>"#,
        ] {
            assert!(graph(assets).is_err(), "accepted removed form: {assets}");
        }
    }
    #[test]
    fn rejects_missing_and_cyclic_sources_and_multiple_generators() {
        for assets in [
            r#"<GeometryAsset id="a" source="missing"></GeometryAsset>"#,
            r#"<GeometryAsset id="a" source="b"></GeometryAsset><GeometryAsset id="b" source="a"></GeometryAsset>"#,
            r#"<GeometryAsset id="a"><Primitive shape="sphere" radius="1" /><Primitive shape="box" size={[1,1,1]} /></GeometryAsset>"#,
            r#"<MeshAsset id="a" geometry="missing" material="red" />"#,
        ] {
            assert!(graph(assets).is_err());
        }
    }
    #[test]
    fn noise_recipe_and_dsl_share_the_same_modifier_kernel() {
        let g=graph(&format!(r#"<GeometryAsset id="body">{PROFILE}<Modifiers><RadialWave axis="y" amplitude="0.03" cycles="5" /><DisplaceNoise amplitude="0.004" frequency="8" seed="98" /></Modifiers></GeometryAsset><MeshAsset id="apple" geometry="body" material="red" />"#)).unwrap();
        let node = &g.geometry_assets[0];
        let PrimitiveGeometry::Mesh { cage } = node.geometry.as_ref().unwrap() else {
            panic!()
        };
        let mut result = cage.clone();
        for modifier in &node.modifiers {
            result =
                crate::world::primitive::apply_control_cage_modifier(&result, modifier).unwrap();
        }
        let rendered =
            crate::world::primitive::generate_primitive_mesh(g.assets[0].primitive().unwrap());
        assert_eq!(rendered.positions.len(), result.positions.len());
        assert!(
            rendered
                .positions
                .iter()
                .zip(&result.positions)
                .all(|(a, b)| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-7))
        );
        let repeated =
            crate::world::primitive::generate_primitive_mesh(g.assets[0].primitive().unwrap());
        assert_eq!(rendered.positions, repeated.positions);
        assert_eq!(result.uvs, cage.uvs);
    }
    #[test]
    fn topology_modifier_failure_is_a_parse_error() {
        let error=graph(r#"<GeometryAsset id="a"><Primitive shape="plane" size={[1,1]} /><Modifiers><Partition uRange={[4,5]} vRange={[4,5]} /></Modifiers></GeometryAsset>"#).unwrap_err();
        assert!(error.message.contains("selects no faces"));
    }
}
