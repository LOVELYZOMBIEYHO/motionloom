// =========================================
// =========================================
// src/character_authoring/templates.rs

use super::{CharacterDocument, CharacterError};
use crate::api::mesh_authoring::{
    Axis, GeometryOperation, GeometryRecipe, LoopSpec, MESH_AUTHORING_SCHEMA_VERSION,
    execute_geometry_recipe, mesh_asset_element,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ProportionPreset {
    Anime,
    Chibi,
    Neutral,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CharacterParameters {
    pub preset: ProportionPreset,
    pub height: f32,
    pub head_ratio: f32,
    pub shoulder_width: f32,
    pub rib_width: f32,
    pub rib_depth: f32,
    pub waist_width: f32,
    pub waist_depth: f32,
    pub pelvis_width: f32,
    pub pelvis_depth: f32,
    pub arm_length: f32,
    pub limb_radius: f32,
    pub sclera_width: f32,
    pub iris_radius: f32,
}
impl CharacterParameters {
    pub fn preset(preset: ProportionPreset) -> Self {
        Self {
            preset,
            height: 3.5,
            head_ratio: match preset {
                ProportionPreset::Anime => 4.5,
                ProportionPreset::Chibi => 3.,
                ProportionPreset::Neutral => 7.,
            },
            shoulder_width: 0.8,
            rib_width: 0.68,
            rib_depth: 0.58,
            waist_width: 0.55,
            waist_depth: 0.43,
            pelvis_width: 0.76,
            pelvis_depth: 0.6,
            arm_length: 1.1,
            limb_radius: 0.12,
            sclera_width: 0.19,
            iris_radius: 0.065,
        }
    }
    pub fn validate(&self) -> Result<(), CharacterError> {
        let values = [
            self.height,
            self.head_ratio,
            self.shoulder_width,
            self.rib_width,
            self.rib_depth,
            self.waist_width,
            self.waist_depth,
            self.pelvis_width,
            self.pelvis_depth,
            self.arm_length,
            self.limb_radius,
            self.sclera_width,
            self.iris_radius,
        ];
        if values.iter().any(|x| !x.is_finite() || *x <= 0.)
            || self.height > 10.
            || self.head_ratio < 2.
            || self.head_ratio > 10.
            || self.limb_radius >= self.pelvis_width * 0.45
        {
            return Err(CharacterError::Invalid(
                "Invalid body dimensions or proportion profile".into(),
            ));
        }
        Ok(())
    }
}

fn loft(
    id: &str,
    axis: Axis,
    sections: &[([f32; 3], f32, f32)],
    material: &str,
) -> Result<String, CharacterError> {
    let mut operations = vec![];
    let mut loops = vec![];
    for (i, (center, a, b)) in sections.iter().enumerate() {
        let loop_id = format!("{id}_{i}");
        loops.push(loop_id.clone());
        operations.push(GeometryOperation::CreateLoop {
            spec: LoopSpec {
                id: loop_id,
                center: *center,
                radius_a: *a,
                radius_b: *b,
                segments: 20,
                normal_axis: axis,
                rotation_degrees: 0.,
            },
        });
    }
    operations.push(GeometryOperation::LoftLoops {
        id: id.into(),
        loops,
        cap_start: true,
        cap_end: true,
    });
    let result = execute_geometry_recipe(&GeometryRecipe {
        schema_version: MESH_AUTHORING_SCHEMA_VERSION.into(),
        id: id.into(),
        subdivision: 0,
        operations,
    })?;
    if !result.topology.valid {
        return Err(CharacterError::Geometry(format!(
            "Template component {id} failed topology validation"
        )));
    }
    Ok(mesh_asset_element(id, material, &result.cage))
}

/// Templates emit ordinary editable assets; no character syntax enters the public DSL.
pub fn build_template(
    id: String,
    p: CharacterParameters,
) -> Result<CharacterDocument, CharacterError> {
    p.validate()?;
    let h = p.height;
    let head_h = h / p.head_ratio;
    let neck_top = h - head_h;
    let shoulder = neck_top - 0.10 * h;
    let torso_bottom = h * 0.39;
    let waist = h * 0.50;
    let rib = (h * 0.60).min(shoulder - h * 0.04);
    let mut assets = String::from(
        "<MaterialAsset id=\"skin\" shading=\"pbr\" baseColor=\"#FFE2C9\" roughness=\"0.9\" />\n<MaterialAsset id=\"white\" shading=\"pbr\" baseColor=\"#FFF8E7\" />\n<MaterialAsset id=\"iris_color\" shading=\"pbr\" baseColor=\"#80A5AF\" />\n",
    );
    let mut ids = vec![];
    let mut insert = |name: &str,
                      axis,
                      sections: Vec<([f32; 3], f32, f32)>,
                      mat: &str|
     -> Result<(), CharacterError> {
        assets.push_str(&loft(name, axis, &sections, mat)?);
        ids.push(name.to_string());
        Ok(())
    };
    insert(
        "torso",
        Axis::Y,
        vec![
            (
                [0., torso_bottom, 0.],
                p.pelvis_width / 2.,
                p.pelvis_depth / 2.,
            ),
            ([0., h * 0.43, 0.], p.pelvis_width / 2., p.pelvis_depth / 2.),
            ([0., waist, 0.], p.waist_width / 2., p.waist_depth / 2.),
            ([0., rib, 0.], p.rib_width / 2., p.rib_depth / 2.),
            (
                [0., (rib + shoulder) * 0.5, 0.],
                p.rib_width / 2.,
                p.rib_depth / 2.,
            ),
            (
                [0., shoulder, 0.],
                p.shoulder_width / 2.,
                p.rib_depth * 0.44,
            ),
            ([0., neck_top, 0.], p.limb_radius * 0.7, p.limb_radius * 0.7),
        ],
        "skin",
    )?;
    let mut head = vec![];
    for i in 0..=10 {
        let t = 0.04 + (std::f32::consts::PI - 0.08) * i as f32 / 10.;
        head.push((
            [0., neck_top + head_h * 0.5 - head_h * 0.5 * t.cos(), 0.],
            head_h * 0.42 * t.sin(),
            head_h * 0.36 * t.sin(),
        ));
    }
    insert("head", Axis::Y, head, "skin")?;
    for (sign, side) in [(-1., "l"), (1., "r")] {
        let hip = sign * p.pelvis_width * 0.25;
        insert(
            &format!("leg_{side}"),
            Axis::Y,
            vec![
                (
                    [hip, h * 0.055, 0.],
                    p.limb_radius * 0.52,
                    p.limb_radius * 0.65,
                ),
                (
                    [hip, h * 0.13, -0.03],
                    p.limb_radius * 0.65,
                    p.limb_radius * 0.9,
                ),
                ([hip, h * 0.24, -0.025], p.limb_radius, p.limb_radius * 1.2),
                ([hip, h * 0.29, 0.025], p.limb_radius * 0.85, p.limb_radius),
                (
                    [hip, h * 0.38, 0.],
                    p.limb_radius * 1.3,
                    p.limb_radius * 1.4,
                ),
                (
                    [hip, torso_bottom + 0.08, 0.],
                    p.limb_radius * 1.35,
                    p.limb_radius * 1.5,
                ),
            ],
            "skin",
        )?;
        insert(
            &format!("foot_{side}"),
            Axis::Y,
            vec![
                (
                    [hip, h * 0.015, 0.08],
                    p.limb_radius * 0.8,
                    p.limb_radius * 1.7,
                ),
                (
                    [hip, h * 0.035, 0.08],
                    p.limb_radius * 0.85,
                    p.limb_radius * 1.7,
                ),
                (
                    [hip, h * 0.07, 0.],
                    p.limb_radius * 0.5,
                    p.limb_radius * 0.6,
                ),
            ],
            "skin",
        )?;
        let start = p.shoulder_width * 0.44;
        let end = start + p.arm_length;
        let mut sections = vec![
            (
                [sign * start, shoulder, 0.],
                p.limb_radius * 1.05,
                p.limb_radius * 1.1,
            ),
            (
                [sign * (start + p.arm_length * 0.3), shoulder, 0.],
                p.limb_radius,
                p.limb_radius,
            ),
            (
                [sign * (start + p.arm_length * 0.52), shoulder, 0.],
                p.limb_radius * 0.8,
                p.limb_radius * 0.85,
            ),
            (
                [sign * (start + p.arm_length * 0.75), shoulder, 0.],
                p.limb_radius * 0.72,
                p.limb_radius * 0.85,
            ),
            (
                [sign * end, shoulder, 0.],
                p.limb_radius * 0.45,
                p.limb_radius * 0.5,
            ),
        ];
        if sign < 0. {
            sections.reverse();
        }
        insert(&format!("arm_{side}"), Axis::X, sections, "skin")?;
        let mut sections = vec![
            (
                [sign * end, shoulder, 0.],
                p.limb_radius * 0.35,
                p.limb_radius * 0.6,
            ),
            (
                [sign * (end + 0.10), shoulder, 0.],
                p.limb_radius * 0.3,
                p.limb_radius * 0.8,
            ),
            (
                [sign * (end + 0.20), shoulder, 0.],
                p.limb_radius * 0.25,
                p.limb_radius * 0.65,
            ),
        ];
        if sign < 0. {
            sections.reverse();
        }
        insert(&format!("hand_{side}"), Axis::X, sections, "skin")?;
        let eye_x = sign * head_h * 0.19;
        let eye_y = neck_top + head_h * 0.55;
        let eye_z = head_h * 0.34;
        insert(
            &format!("eye_white_{side}"),
            Axis::Z,
            vec![
                (
                    [eye_x, eye_y, eye_z],
                    p.sclera_width / 2.,
                    p.sclera_width * 0.34,
                ),
                (
                    [eye_x, eye_y, eye_z + 0.015],
                    p.sclera_width / 2.,
                    p.sclera_width * 0.34,
                ),
            ],
            "white",
        )?;
        insert(
            &format!("iris_{side}"),
            Axis::Z,
            vec![
                ([eye_x, eye_y, eye_z + 0.017], p.iris_radius, p.iris_radius),
                ([eye_x, eye_y, eye_z + 0.022], p.iris_radius, p.iris_radius),
            ],
            "iris_color",
        )?;
    }
    let models=ids.iter().map(|name|format!("<Model id=\"character_{name}\" asset=\"{name}\"><MaterialBinding modelSourceMaterial=\"*\" celRole=\"skin\" outlineWidth=\"0\" /></Model>")).collect::<String>();
    let source = format!(
        "<Graph fps=\"24\" duration=\"1s\" size={{[800,800]}}><Background color=\"#EAE6E4\" /><RenderStyle id=\"toon\"><SurfaceStyle shading=\"cel\" shadingSteps=\"3\" /><AntiAliasingStyle method=\"ssaa\" quality=\"medium\" /></RenderStyle><Assets>{assets}</Assets><Scene id=\"character_scene\" renderStyle=\"toon\"><Timeline><Track space=\"3d\"><Sequence from=\"0s\" duration=\"1s\"><CompositeGroup space=\"3d\" depth=\"true\"><Camera3D id=\"review\" position={{[0,{},15]}} target={{[0,{},0]}} fov=\"20\" /><DirectionalLight direction={{[-0.6,-0.9,-1]}} intensity=\"2\" castShadow=\"false\" />{models}</CompositeGroup></Sequence></Track></Timeline></Scene><Present from=\"character_scene\" /></Graph>",
        h / 2.,
        h / 2.
    );
    let mut document = CharacterDocument::import(id, source)?;
    document.current.parameters = Some(p);
    let torso = super::cages(&document.current.source)?["torso"].clone();
    for (name, lo, hi) in [
        ("body.ribcage", waist + 0.02, shoulder + 0.01),
        ("body.waist", waist - 0.05, waist + 0.05),
        ("body.pelvis", torso_bottom, h * 0.44),
    ] {
        let mut s = document.current.selections["torso"][0].clone();
        s.vertices = (0..torso.positions.len())
            .filter(|i| torso.positions[*i][1] >= lo && torso.positions[*i][1] <= hi)
            .collect();
        document.current.selections.insert(name.into(), vec![s]);
    }
    for base in ["arm", "leg", "hand", "foot", "eye_white", "iris"] {
        for (side, other) in [("l", "r"), ("r", "l")] {
            let name = format!("{base}_{side}");
            if let Some(items) = document.current.selections.get_mut(&name) {
                for item in items {
                    item.pair = Some(format!("{base}_{other}"));
                }
            }
        }
    }
    document.current.assumptions=vec!["Template proportions are a design prior, not an image reconstruction.".into(),"Body components have intentional joint overlap; skin welding and rig weights require a separate production pass.".into()];
    Ok(document)
}
