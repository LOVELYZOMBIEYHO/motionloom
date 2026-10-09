//! Small native still-image fixtures; no video or external assets are required.
#![cfg(not(target_arch = "wasm32"))]
use base64::Engine;
use motionloom::{SceneRenderProfile, SceneRenderer, parse_graph_script};

fn fixture(attributes: &str, environment: bool) -> String {
    let image = image::RgbaImage::from_pixel(8, 4, image::Rgba([255, 255, 255, 255]));
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).unwrap();
    let sky = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let intensity = if environment { 0 } else { 3 };
    let specular = if environment { 1 } else { 0 };
    format!(
        r##"<Graph fps="24" duration="1s" size={{[128,128]}}>
<RenderStyle id="fixture"><SurfaceStyle shading="physical"/><LightingStyle ambientIntensity="0"/></RenderStyle>
<Assets>
<ImageAsset id="white_sky" src="data:image/png;base64,{sky}"/>
<MaterialAsset id="black" baseColor="#000000" metallic="0" roughness="0.8" specular="0" {attributes}/>
<GeometryAsset id="sphere"><Primitive shape="sphere" radius="0.8" segments="48" rings="24"/></GeometryAsset>
<MeshAsset id="ball" geometry="sphere" material="black"/>
</Assets>
<Background color="#000000"/>
<Scene id="main" renderStyle="fixture"><Timeline><Track><Sequence duration="1s"><CompositeGroup space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="35"/>
<EnvironmentLight asset="white_sky" visible="false" intensity="1" diffuseIntensity="0" specularIntensity="{specular}"/>
<DirectionalLight direction={{[0.4,-0.3,-1]}} intensity="{intensity}" castShadow="false"/>
<Model id="sphere_model" asset="ball"/>
</CompositeGroup></Sequence></Track></Timeline></Scene><Present from="main"/>
</Graph>"##
    )
}

#[test]
fn material_layer_image_fixtures_parse() {
    for environment in [false, true] {
        parse_graph_script(&fixture("", environment)).unwrap();
        parse_graph_script(&fixture(
            "sheen=\"1\" sheenColor=\"#FF0000\" sheenRoughness=\"0.6\"",
            environment,
        ))
        .unwrap();
        parse_graph_script(&fixture(
            "clearcoat=\"1\" clearcoatRoughness=\"0.15\"",
            environment,
        ))
        .unwrap();
    }
}

#[test]
#[ignore = "requires a real native GPU"]
fn native_layer_lobes_light_black_base_and_zero_weights_preserve_pixels() {
    pollster::block_on(async {
        let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu).await.unwrap();
        for environment in [false, true] {
            let mut render = async |attrs: &str| {
                renderer
                    .render_frame_gpu_readback(
                        &parse_graph_script(&fixture(attrs, environment)).unwrap(),
                        0,
                    )
                    .await
                    .unwrap()
            };
            let plain = render("").await;
            let zero = render("sheen=\"0\" clearcoat=\"0\" sheenColor=\"#FF0000\" sheenRoughness=\"0.8\" clearcoatRoughness=\"0.9\"").await;
            assert_eq!(
                plain, zero,
                "inactive parameters changed pixels (environment={environment})"
            );
            let cloth = render("sheen=\"1\" sheenColor=\"#FF0000\" sheenRoughness=\"0.6\"").await;
            let coat = render("clearcoat=\"1\" clearcoatRoughness=\"0.15\"").await;
            let rough_coat = render("clearcoat=\"1\" clearcoatRoughness=\"0.7\"").await;
            assert_ne!(
                coat, rough_coat,
                "coat roughness did not change its independent lobe"
            );
            let energy = |img: &image::RgbaImage, k: usize| {
                img.pixels().map(|p| u64::from(p[k])).sum::<u64>()
            };
            assert!(
                energy(&cloth, 0) > energy(&plain, 0) + 500,
                "missing sheen lobe (environment={environment})"
            );
            assert!(
                energy(&cloth, 0) > energy(&cloth, 2) + 500,
                "sheen tint lost"
            );
            assert!(
                energy(&coat, 0) > energy(&plain, 0) + 500,
                "missing independent coat lobe (environment={environment})"
            );
        }
    });
}
