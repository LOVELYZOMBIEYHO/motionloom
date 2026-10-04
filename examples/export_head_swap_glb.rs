// =========================================
// =========================================
// examples/export_head_swap_glb.rs

use motionloom::{export_scene_head_swap_glb, parse_graph_script, set_scene_asset_roots};
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 4 {
        return Err(
            "usage: export_head_swap_glb <scene.motionloom> <asset-id> <output.glb>".into(),
        );
    }
    let scene = Path::new(&args[1]);
    if let Some(parent) = scene.parent() {
        set_scene_asset_roots(vec![parent.to_path_buf()]);
    }
    let graph = parse_graph_script(&fs::read_to_string(scene)?)?;
    let bytes = pollster::block_on(export_scene_head_swap_glb(&graph, &args[2]))?;
    fs::write(&args[3], &bytes)?;
    println!("exported {} bytes to {}", bytes.len(), args[3]);
    Ok(())
}
