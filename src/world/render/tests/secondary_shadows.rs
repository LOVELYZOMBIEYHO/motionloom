//! Native image contracts for opaque shadow-map reuse at reflected receivers.
use crate::api::{SceneRenderProfile, SceneRenderer, parse_graph_script};
use crate::preview::{ImmediatePreviewProfile, ImmediatePreviewSettings};

fn reflected_receiver(caster_material: &str, caster_model: &str) -> String {
    format!(
        r##"<Graph fps="24" duration="1s" size={{[128,128]}}>
<RenderStyle id="style"><SurfaceStyle shading="physical" />
<LightingStyle ambientIntensity="0" shadowMode="perLight" reflectionBounces="1" />
<PostStyle toneMapping="none" exposure="1" />
<AntiAliasingStyle method="off" /></RenderStyle>
<Assets>
<MaterialAsset id="mirror" baseColor="#FFFFFF" metallic="1" roughness="0.04" />
<MaterialAsset id="receiver" baseColor="#FFFFFF" roughness="1" specular="0" />
{caster_material}
<GeometryAsset id="mirror_g"><Primitive shape="box" size={{[2,2,0.04]}} /></GeometryAsset>
<MeshAsset id="mirror" material="mirror" geometry="mirror_g" />
<GeometryAsset id="receiver_g"><Primitive shape="box" size={{[8,8,0.05]}} /></GeometryAsset>
<MeshAsset id="receiver" material="receiver" geometry="receiver_g" />
<GeometryAsset id="caster_g"><Primitive shape="box" size={{[0.7,1.2,0.2]}} /></GeometryAsset>
<MeshAsset id="caster" material="caster" geometry="caster_g" />
</Assets><Background color="#000000" />
<Scene id="stage" renderStyle="style"><Timeline><Track id="track" space="3d">
<Sequence duration="1s"><CompositeGroup id="room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="35" />
<PointLight position={{[2,0,3]}} intensity="20" range="12" castShadow="true" sourceRadius="0" />
<Model id="mirror" asset="mirror" castShadow="false" receiveShadow="false" />
<Model id="secondary" asset="receiver" position={{[0,0,6]}} castShadow="false" receiveShadow="true" />
{caster_model}
</CompositeGroup></Sequence></Track></Timeline></Scene>
<Present from="stage" /></Graph>"##
    )
}

const OPAQUE_CASTER: &str =
    r##"<MaterialAsset id="caster" baseColor="#000000" roughness="1" specular="0" />"##;
const ORDINARY_BLOCKER: &str =
    r#"<Model id="blocker" asset="caster" position={[1,0,4.5]} castShadow="true" />"#;

fn receiver_flag_sources() -> [String; 3] {
    // The primary mirror and secondary receiver intentionally have opposite
    // receive flags. A reflected hit must use its own receiver's flag.
    let lit = reflected_receiver(OPAQUE_CASTER, "");
    let blocked = reflected_receiver(OPAQUE_CASTER, ORDINARY_BLOCKER);
    let disabled = blocked.replace(
        r#"castShadow="false" receiveShadow="true""#,
        r#"castShadow="false" receiveShadow="false""#,
    );
    [lit, blocked, disabled]
}

fn blended_caster_sources() -> [String; 2] {
    // A blend material has no transmission but is excluded from the opaque
    // depth maps. Its valid alpha coverage still blocks secondary BVH shadows.
    let material = OPAQUE_CASTER.replace("roughness=", "alphaMode=\"blend\" roughness=");
    [
        reflected_receiver(&material, ""),
        reflected_receiver(&material, ORDINARY_BLOCKER),
    ]
}

fn near_clip_caster_sources() -> [String; 2] {
    // Every vertex is less than 0.01 along every point-light face axis, so all
    // six shadow views clip this caster. It remains on the finite light ray.
    let blocker = r#"<Model id="near_clipped" asset="caster" position={[1.997,0,3.004]} castShadow="true" />"#;
    [
        reflected_receiver(OPAQUE_CASTER, ""),
        reflected_receiver(OPAQUE_CASTER, blocker),
    ]
    .map(|source| source.replace("[0.7,1.2,0.2]", "[0.004,0.008,0.004]"))
}

async fn image(source: &str) -> image::RgbaImage {
    let graph = parse_graph_script(source).expect("secondary shadow fixture parse");
    let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu)
        .await
        .expect("native secondary shadow renderer");
    renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
        profile: ImmediatePreviewProfile::Cinematic,
        target_fps: 30.0,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
    });
    renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("secondary shadow GPU frame")
}

fn evidence(label: &str, image: &image::RgbaImage) {
    let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("create secondary shadow evidence directory");
    image
        .save(root.join(format!("secondary-shadow-{label}.png")))
        .expect("save secondary shadow evidence PNG");
}

fn center_mean(image: &image::RgbaImage) -> f64 {
    (60..68)
        .flat_map(|y| (60..68).map(move |x| (x, y)))
        .map(|(x, y)| image.get_pixel(x, y)[0] as u64)
        .sum::<u64>() as f64
        / 64.0
}

fn assert_shadow(lit: &image::RgbaImage, blocked: &image::RgbaImage, label: &str) {
    let lit_mean = center_mean(lit);
    let blocked_mean = center_mean(blocked);
    assert!(
        lit_mean > 80.0,
        "{label}: control must light the reflected receiver, got {lit_mean}"
    );
    assert!(
        blocked_mean + 30.0 < lit_mean,
        "{label}: reflected opaque shadow missing: lit={lit_mean}, blocked={blocked_mean}"
    );
    assert_eq!(
        lit.get_pixel(2, 2),
        blocked.get_pixel(2, 2),
        "{label}: blocker entered the direct camera background"
    );
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_secondary_shadow_uses_reflected_receiver_flag() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [lit, blocked, disabled] = receiver_flag_sources();
        let lit = image(&lit).await;
        let blocked = image(&blocked).await;
        let disabled = image(&disabled).await;
        evidence("receiver-lit", &lit);
        evidence("receiver-blocked", &blocked);
        evidence("receiver-disabled", &disabled);
        assert_shadow(&lit, &blocked, "secondary receive flag");
        assert!(
            (center_mean(&lit) - center_mean(&disabled)).abs() <= 4.0,
            "secondary receiveShadow=false must restore the lit receiver: lit={}, disabled={}",
            center_mean(&lit),
            center_mean(&disabled)
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_secondary_blended_opaque_caster_keeps_geometry_shadow() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [lit, blocked] = blended_caster_sources();
        let lit = image(&lit).await;
        let blocked = image(&blocked).await;
        evidence("blend-lit", &lit);
        evidence("blend-blocked", &blocked);
        assert_shadow(&lit, &blocked, "blended opaque caster");
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_secondary_point_near_clipped_caster_keeps_geometry_shadow() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [lit, blocked] = near_clip_caster_sources();
        let lit = image(&lit).await;
        let blocked = image(&blocked).await;
        evidence("point-near-lit", &lit);
        evidence("point-near-blocked", &blocked);
        assert_shadow(&lit, &blocked, "point-light near-clipped caster");
    });
}

#[test]
fn secondary_shadow_native_fixture_sources_parse_without_a_gpu() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    for source in receiver_flag_sources()
        .into_iter()
        .chain(blended_caster_sources())
        .chain(near_clip_caster_sources())
    {
        let graph = parse_graph_script(&source).expect("secondary shadow fixture parse");
        assert_eq!(graph.size, (128, 128));
        assert_eq!(graph.material_assets.len(), 3);
        assert_eq!(graph.geometry_assets.len(), 3);
        assert!(source.contains("shadowMode=\"perLight\""));
        assert!(source.contains(
            "id=\"mirror\" asset=\"mirror\" castShadow=\"false\" receiveShadow=\"false\""
        ));
    }
}
