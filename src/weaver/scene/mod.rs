// =========================================
// =========================================
// crates/motionloom/src/weaver/scene/mod.rs

use crate::experimental::geometry::ResolvedMesh;
use crate::scene::compositor::{ResolvedCompositionLayer, SceneCompositionPlan};
use crate::world::{WorldCamera, WorldLighting};

// Hide sequences own [start, end), avoiding coincident geometry at hard cuts.
pub(crate) fn hidden_sequence_ends_here(
    sequence: &crate::scene::model::SceneSequenceNode,
    sec: f32,
) -> bool {
    sequence.out.trim().eq_ignore_ascii_case("hide")
        && (sec.max(0.0) * 1000.0).round() as u64
            == sequence.from_ms as u64 + sequence.duration_ms.max(1) as u64
}

// Preserve unaffected checkpoints when fixing a sequence boundary export.
pub(crate) fn has_hidden_boundary(nodes: &[crate::scene::model::SceneNode], sec: f32) -> bool {
    use crate::scene::model::SceneNode;
    nodes.iter().any(|node| match node {
        SceneNode::Timeline(v) => has_hidden_boundary(&v.children, sec),
        SceneNode::Track(v) => has_hidden_boundary(&v.children, sec),
        SceneNode::Group(v) => has_hidden_boundary(&v.children, sec),
        SceneNode::Sequence(v) => {
            hidden_sequence_ends_here(v, sec)
                || crate::scene::timeline::scene_sequence_local_time(v, None, sec)
                    .is_some_and(|(_, local)| has_hidden_boundary(&v.children, local))
        }
        _ => false,
    })
}

/// Evaluated world-space data; no GPU preview buffers are retained.
pub(crate) struct Snapshot {
    pub meshes: Vec<ResolvedMesh>,
    /// Parallel to `meshes`; false affects primary rays only, so shadows and
    /// reflections retain the authored object.
    pub primary_camera_visibility: Vec<bool>,
    pub camera: WorldCamera,
    pub lighting: WorldLighting,
    pub time_seconds: f32,
    pub diagnostics: Vec<String>,
    pub composition: SceneCompositionPlan,
    /// Evaluated 2D runs, converted at the Raster boundary into the canonical
    /// linear-premultiplied working representation without flattening them.
    pub composition_layers: Vec<ResolvedCompositionLayer>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_cut_invalidates_only_the_shared_boundary_frame() {
        // Old checkpoints are still valid before and after the exact cut.
        let graph = crate::parse_graph_script(
            r#"<Graph fps={24} duration="6s" size={[64,64]}>
          <Scene id="cut">
            <Timeline>
              <Track id="stage" space="3d">
                <Sequence from="2s" duration="2s" out="hide">
                  <CompositeGroup id="gray" space="3d">
                  </CompositeGroup>
                </Sequence>
                <Sequence from="4s" duration="2s" out="hide">
                  <CompositeGroup id="white" space="3d">
                  </CompositeGroup>
                </Sequence>
              </Track>
            </Timeline>
          </Scene>
          <Present from="cut" />
        </Graph>"#,
        )
        .unwrap();
        let nodes = &graph.scenes[0].children;
        assert!(!has_hidden_boundary(nodes, 95.0 / 24.0));
        assert!(has_hidden_boundary(nodes, 96.0 / 24.0));
        assert!(!has_hidden_boundary(nodes, 97.0 / 24.0));
    }
}
