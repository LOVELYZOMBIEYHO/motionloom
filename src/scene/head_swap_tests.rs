// =========================================
// =========================================
// src/scene/head_swap_tests.rs

use super::*;
use crate::parse_graph_script;
use serde_json::{Value, json};

// Minimal self-contained skin exercises the composer without project sample assets.
fn body_glb() -> Vec<u8> {
    let mut bin = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bytes: Vec<u8>, component: u32, count: usize, kind: &str| {
        bin.resize((bin.len() + 3) & !3, 0);
        views.push(json!({"buffer":0,"byteOffset":bin.len(),"byteLength":bytes.len()}));
        bin.extend(bytes);
        accessors.push(
            json!({"bufferView":views.len()-1,"componentType":component,"count":count,"type":kind}),
        );
        accessors.len() - 1
    };
    let floats = |values: &[f32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    };
    let position = add(
        floats(&[-0.2, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.5, 0.0]),
        5126,
        3,
        "VEC3",
    );
    let normals = add(
        floats(&[0., 0., 1., 0., 0., 1., 0., 0., 1.]),
        5126,
        3,
        "VEC3",
    );
    let uv = add(floats(&[0., 0., 1., 0., 0.5, 1.]), 5126, 3, "VEC2");
    let joints = add(vec![2, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0], 5121, 3, "VEC4");
    let weights = add(
        floats(&[1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.]),
        5126,
        3,
        "VEC4",
    );
    let indices = add(vec![0, 0, 1, 0, 2, 0], 5123, 3, "SCALAR");
    let identity = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    let bind = add(floats(&identity.repeat(3)), 5126, 3, "MAT4");
    let doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":bin.len()}],
        "bufferViews":views,"accessors":accessors,"materials":[{}],
        "nodes":[{"name":"Head"},{"name":"neck_01"},{"name":"spine_03"},{"mesh":0,"skin":0}],
        "skins":[{"joints":[0,1,2],"inverseBindMatrices":bind}],"scenes":[{"nodes":[3]}],"scene":0,
        "meshes":[{"primitives":[{"attributes":{"POSITION":position,"NORMAL":normals,"TEXCOORD_0":uv,"JOINTS_0":joints,"WEIGHTS_0":weights},"indices":indices,"material":0}]}]});
    let mut json = serde_json::to_vec(&doc).unwrap();
    json.resize((json.len() + 3) & !3, b' ');
    bin.resize((bin.len() + 3) & !3, 0);
    let mut glb = b"glTF".to_vec();
    for value in [
        2,
        (28 + json.len() + bin.len()) as u32,
        json.len() as u32,
        0x4e4f534a,
    ] {
        glb.extend(value.to_le_bytes());
    }
    glb.extend(json);
    glb.extend((bin.len() as u32).to_le_bytes());
    glb.extend(b"BIN\0");
    glb.extend(bin);
    glb
}
fn head_scene() -> String {
    r##"<Graph fps="30" duration="1s" size={[64,64]}>
<Assets>
<ImageAsset id="texture" src="face.png" />
<MaterialAsset id="paint" baseColorTexture="texture" />
<GeometryAsset id="geometry"><Primitive shape="sphere" radius="0.5" segments="8" /></GeometryAsset>
<MeshAsset id="mesh" geometry="geometry" material="paint" />
</Assets>
<Scene id="head"><Timeline><Track space="3d"><Sequence duration="1s">
<CompositeGroup space="3d"><Model id="head" asset="mesh" /></CompositeGroup>
</Sequence></Track></Timeline></Scene><Present from="head" /></Graph>"##
        .into()
}
fn graph(head: &str, extra: &str) -> GraphScript {
    parse_graph_script(&format!(r#"<Graph fps="30" duration="1s" size={{[64,64]}}>
<Assets><ModelAsset id="body" src="https://example.test/body.glb" />
<HeadSwapAsset id="hero" body="body" {head} scale="0.2" />{extra}</Assets>
<Scene id="main"><Timeline><Track space="3d"><Sequence duration="1s"><CompositeGroup space="3d"><Model asset="hero" /></CompositeGroup></Sequence></Track></Timeline></Scene><Present from="main" /></Graph>"#)).unwrap()
}
fn texture(color: [u8; 4]) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(2, 2, image::Rgba(color))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn document(glb: &[u8]) -> Value {
    serde_json::from_slice(
        &glb[20..20 + u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize],
    )
    .unwrap()
}

#[test]
fn head_scene_urls_and_scoped_texture_changes_rebuild_the_memory_asset() {
    let resolver = Arc::new(crate::asset::MemoryAssetResolver::new());
    resolver.insert("https://example.test/body.glb".into(), body_glb());
    resolver.insert(
        "https://example.test/heads/main.motionloom".into(),
        head_scene().into_bytes(),
    );
    resolver.insert(
        "https://example.test/heads/face.png".into(),
        texture([255, 0, 0, 255]),
    );
    let graph = graph(
        r#"headScene="https://example.test/heads/main.motionloom" headObject="head""#,
        "",
    );
    pollster::block_on(async {
        let mut renderer = SceneFrameRenderer::new_for_profile_with_resolver(
            SceneRenderProfile::Cpu,
            resolver.clone(),
        )
        .await;
        renderer.prepare_frame_caches(&graph);
        renderer.ensure_head_swap_assets().await.unwrap();
        let first = renderer.head_swap_cache["hero"].1.clone();
        let bytes = load_binary_asset_source(&first, renderer.asset_resolver.as_ref()).unwrap();
        assert!(document(&bytes)["images"].as_array().unwrap().len() > 0);
        renderer.ensure_head_swap_assets().await.unwrap();
        assert_eq!(renderer.head_swap_cache["hero"].1, first);
        resolver.insert(
            "https://example.test/heads/face.png".into(),
            texture([0, 255, 0, 255]),
        );
        renderer.ensure_head_swap_assets().await.unwrap();
        assert_ne!(renderer.head_swap_cache["hero"].1, first);
    });
}
#[test]
fn prebuilt_url_fallback_uses_preloaded_bytes_without_a_filesystem() {
    let resolver = Arc::new(crate::asset::MemoryAssetResolver::new());
    let bytes = body_glb();
    resolver.insert("https://example.test/prebuilt.glb".into(), bytes.clone());
    let graph = graph(
        r#"head="missing.glb" src="https://example.test/prebuilt.glb""#,
        "",
    );
    let actual = pollster::block_on(export_scene_head_swap_glb_with_resolver(
        &graph, "hero", resolver,
    ))
    .unwrap();
    assert_eq!(actual, bytes);
}
