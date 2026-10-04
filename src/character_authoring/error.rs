// =========================================
// =========================================
// src/character_authoring/error.rs

use super::rig;

#[derive(Debug, thiserror::Error)]
pub enum CharacterError {
    #[error("Character operation is unavailable on this platform: {0}")]
    Unsupported(&'static str),
    #[error("Invalid character request: {0}")]
    Invalid(String),
    #[error("Stale character revision: expected {expected}, current {current}")]
    Stale { expected: u64, current: u64 },
    #[error("Source changed outside the authoring session; re-import before editing")]
    SourceChanged,
    #[error("Locked selection: {0}")]
    Locked(String),
    #[error("Attachment requires rebinding: {0}")]
    Rebind(String),
    #[error("Character not found: {0}")]
    Missing(String),
    #[error("Geometry validation failed: {0}")]
    Geometry(String),
    #[error("Rendering failed: {0}")]
    Render(String),
    #[error(transparent)]
    Mesh(#[from] crate::api::mesh_authoring::MeshAuthoringError),
    #[error(transparent)]
    Proposal(#[from] crate::api::mesh_reference::MeshReferenceError),
    #[error("Asset {asset_id}: {error}")]
    AssetEdit {
        asset_id: String,
        #[source]
        error: crate::api::mesh_reference::MeshReferenceError,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Rig(#[from] rig::RigError),
}
