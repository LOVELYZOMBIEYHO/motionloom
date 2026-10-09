//! CPU-only two-state room lighting baker. No GPU or video export is involved.
use motionloom::api::lighting_bake::{
    BakedLightingAsset, LightingBakeOptions, bake_scene_lighting_with_progress, save_lighting_bake,
    validate_lighting_bake_fingerprint,
};
use motionloom::api::{AssetResolver, AssetSource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct RelativeAssets(PathBuf);
impl AssetResolver for RelativeAssets {
    fn resolve(&self, src: &str) -> Result<AssetSource, String> {
        let path = Path::new(src);
        if src.starts_with("https://") || src.starts_with("http://") {
            return Err("preload remote assets or use local files before baking".into());
        }
        Ok(AssetSource::Path(if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.0.join(path)
        }))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let validate_only = args.len() == 5 && args[4] == "--validate";
    if args.len() != 4 && !validate_only {
        return Err("usage: bake_scene_lighting <script.motionloom> <options.json> <output/baked-lighting.json> [--validate]; options use LightingBakeOptions camelCase serde fields".into());
    }
    let source = Path::new(&args[1]);
    let root = source
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    motionloom::api::set_scene_asset_roots(vec![root.clone()]);
    let graph = motionloom::api::parse_graph_script(&std::fs::read_to_string(source)?)?;
    let options: LightingBakeOptions = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    if validate_only {
        let asset: BakedLightingAsset = serde_json::from_slice(&std::fs::read(&args[3])?)?;
        pollster::block_on(validate_lighting_bake_fingerprint(
            &graph,
            &options,
            &asset,
            Arc::new(RelativeAssets(root)),
        ))?;
        println!("Lighting dependencies match {}", asset.fingerprint.combined);
        return Ok(());
    }
    let mut last = 0;
    let bundle = pollster::block_on(bake_scene_lighting_with_progress(
        &graph,
        &options,
        Arc::new(RelativeAssets(root)),
        |p| {
            if p.completed_probes == 1
                || p.completed_probes >= last + 16
                || p.completed_probes == p.total_probes
            {
                eprintln!(
                    "{} / {}: {} of {} probes",
                    p.state, p.volume, p.completed_probes, p.total_probes
                );
                last = p.completed_probes;
            }
        },
    ))?;
    save_lighting_bake(&bundle, &args[3])?;
    println!(
        "Saved two states, {} local HDR captures; fingerprint {}",
        bundle.reflection_images.len(),
        bundle.asset.fingerprint.combined
    );
    for message in &bundle.asset.diagnostics {
        println!("{message}");
    }
    Ok(())
}
