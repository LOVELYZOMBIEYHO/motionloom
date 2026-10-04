// =========================================
// =========================================
// src/character_authoring/export.rs

use super::{CharacterError, CharacterSnapshot};

/// Export static geometry as bytes so the host can choose storage or transport.
pub async fn export_character_glb(snapshot: &CharacterSnapshot) -> Result<Vec<u8>, CharacterError> {
    if crate::mesh_reference::mesh_source_fingerprint(&snapshot.source)
        != snapshot.source_fingerprint
    {
        return Err(CharacterError::SourceChanged);
    }
    let graph = crate::api::parse_graph_script(&snapshot.source)
        .map_err(|e| CharacterError::Invalid(e.to_string()))?;
    let scene = graph
        .scenes
        .first()
        .ok_or_else(|| CharacterError::Invalid("No scene to export".into()))?;
    let geometry = crate::experimental::extract_scene_geometry(
        &graph,
        &crate::experimental::SceneGeometryOptions {
            scene_id: scene.id.clone(),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| CharacterError::Geometry(e.to_string()))?;
    crate::experimental::export_scene_glb(&geometry)
        .map_err(|e| CharacterError::Geometry(e.to_string()))
}
