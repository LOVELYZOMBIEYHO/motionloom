// =========================================
// =========================================
// examples/validate_geometry_assets.rs
//! Validate every DSL file beneath the supplied files or directories.
use std::{
    env, fs,
    path::{Path, PathBuf},
};
fn collect(path: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(&entry?.path(), files)?;
        }
    } else if path.extension().is_some_and(|value| value == "motionloom") {
        files.push(path.to_owned());
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut files = vec![];
    for path in env::args().skip(1) {
        collect(Path::new(&path), &mut files)?;
    }
    files.sort();
    files.dedup();
    let mut failures = 0;
    let mut geometries = 0;
    let mut models = 0;
    for path in &files {
        let source = fs::read_to_string(path)?;
        if !source.contains("<Graph") {
            if let Err(error) = motionloom::parse_action_library_document(&source) {
                failures += 1;
                eprintln!("{}: {error}", path.display());
            }
            continue;
        }
        match motionloom::parse_graph_script(&source) {
            Ok(graph) => {
                geometries += graph.geometry_assets.len();
                models += graph
                    .assets
                    .iter()
                    .filter(|a| a.primitive().is_some())
                    .count();
            }
            Err(error) => {
                failures += 1;
                eprintln!("{}: {error}", path.display());
            }
        }
    }
    println!(
        "Validated {} files, {geometries} geometry definitions, {models} generated models; {failures} failures.",
        files.len()
    );
    if failures > 0 {
        return Err("DSL validation failed".into());
    }
    Ok(())
}
