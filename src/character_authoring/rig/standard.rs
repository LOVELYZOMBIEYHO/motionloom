// =========================================
// =========================================
// src/character_authoring/rig/standard.rs

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const HUMANOID65_STANDARD_ID: &str = "humanoid65_v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HumanoidRigStandard {
    pub schema_version: u32,
    pub standard_id: String,
    pub license: String,
    pub source: String,
    pub source_url: String,
    pub coordinate_system: String,
    pub position_units: String,
    pub references: Vec<RigReference>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigReference {
    pub id: String,
    pub source: String,
    pub source_url: String,
    pub license: String,
    pub sha256: String,
    pub source_height: f32,
    pub joints: Vec<ReferenceJoint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceJoint {
    pub id: String,
    pub reference_name: String,
    pub parent: Option<String>,
    pub endpoint: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

/// Embedded, versioned CC0 reference data makes verification independent of local GLBs.
pub fn humanoid_rig_standard() -> &'static HumanoidRigStandard {
    static STANDARD: OnceLock<HumanoidRigStandard> = OnceLock::new();
    STANDARD.get_or_init(|| {
        serde_json::from_str(include_str!("data/humanoid65.json"))
            .expect("compiled humanoid65 reference data must be valid")
    })
}
