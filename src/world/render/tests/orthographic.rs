// =========================================
// =========================================
// src/world/render/tests/orthographic.rs

use super::super::*;

#[test]
fn orthographic_scale_is_depth_independent_and_perspective_is_unchanged() {
    let source = r##"<Graph size={[100,100]} fps="24" duration="1s"><World id="w"><Camera projection="orthographic" orthographicScale="4" /></World><Present from="w" /></Graph>"##;
    let graph = crate::world::parse_world_graph_script(source).unwrap();
    let camera = perspective_camera_view(
        &graph.worlds[0],
        100,
        100,
        WorldTime {
            frame: 0,
            fps: 24.,
            duration_ms: 1000,
        },
    )
    .unwrap();
    assert!(camera.orthographic);
    assert_eq!(camera.focal_px, 25.);
    assert_eq!(
        camera.focal_px / camera.projection_divisor(2.),
        camera.focal_px / camera.projection_divisor(20.)
    );
    let graph = crate::world::parse_world_graph_script(
        &source.replace("projection=\"orthographic\"", "projection=\"perspective\""),
    )
    .unwrap();
    let camera = perspective_camera_view(
        &graph.worlds[0],
        100,
        100,
        WorldTime {
            frame: 0,
            fps: 24.,
            duration_ms: 1000,
        },
    )
    .unwrap();
    assert!(!camera.orthographic);
    assert!(
        camera.focal_px / camera.projection_divisor(2.)
            > camera.focal_px / camera.projection_divisor(20.)
    );
}

#[test]
fn orthographic_visibility_does_not_expand_with_depth() {
    let mut p = GpuWorldParams::default();
    p.canvas = [100., 100., 50., 50.];
    p.camera0 = [0., 0., 0., 50.];
    p.camera1 = [1., 0., 0., 0.1];
    p.camera2 = [0., 1., 0., 100.];
    p.camera3 = [0., 0., 1., 1.];
    p.model[3] = 1.;
    p.actor_rotation[3] = 1.;
    p.actor[0] = 10.;
    p.actor[2] = 50.;
    assert!(!rigid_draw_visible(Some(([-0.1; 3], [0.1; 3])), p));
    p.actor[0] = 0.;
    assert!(rigid_draw_visible(Some(([-0.1; 3], [0.1; 3])), p));
}
