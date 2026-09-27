// =========================================
// =========================================
// crates/motionloom/examples/verify_dsl_formatting.rs

//! Verify lossless edits, idempotence and parsed semantics across a DSL corpus.

use std::{env, fs, path::Path};

fn collect(path: &Path, paths: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    if path.is_symlink() {
        return Ok(());
    }
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect(&entry?.path(), paths)?;
        }
    } else if path.extension().is_some_and(|v| v == "motionloom") {
        paths.push(path.into());
    }
    Ok(())
}

// Original DSL snapshots are intentionally not semantic fields in this comparison.
fn remove_source_snapshots(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("raw_script");
            map.remove("rawScript");
            for child in map.values_mut() {
                remove_source_snapshots(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                remove_source_snapshots(child);
            }
        }
        _ => {}
    }
}

fn semantics(source: &str) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut value = if source.contains("<Graph") {
        serde_json::to_value(motionloom::api::parse_graph_script(source)?)?
    } else {
        serde_json::to_value(motionloom::parse_action_library_document(source)?)?
    };
    remove_source_snapshots(&mut value);
    Ok(value)
}

fn first_difference(
    before: &serde_json::Value,
    after: &serde_json::Value,
    path: &str,
) -> Option<String> {
    if before == after {
        return None;
    }
    match (before, after) {
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                let next = format!("{path}/{key}");
                if let (Some(left), Some(right)) = (a.get(key), b.get(key)) {
                    if let Some(diff) = first_difference(left, right, &next) {
                        return Some(diff);
                    }
                } else {
                    return Some(next);
                }
            }
        }
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) if a.len() == b.len() => {
            for (index, (left, right)) in a.iter().zip(b).enumerate() {
                if let Some(diff) = first_difference(left, right, &format!("{path}/{index}")) {
                    return Some(diff);
                }
            }
        }
        _ => {}
    }
    Some(path.into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = Vec::new();
    for argument in env::args().skip(1) {
        collect(Path::new(&argument), &mut paths)?;
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err("Specify DSL files or directories.".into());
    }
    for path in &paths {
        let source = fs::read_to_string(path)?;
        let result = motionloom::api::format_dsl(&source)?;
        if motionloom::api::format_dsl(&result.source)?.changed {
            return Err(format!("{}: formatting is not idempotent", path.display()).into());
        }
        let mut edited = source.clone();
        for edit in result.edits.iter().rev() {
            if !source[edit.start_byte..edit.end_byte]
                .chars()
                .all(char::is_whitespace)
                || !edit.replacement.chars().all(char::is_whitespace)
            {
                return Err(
                    format!("{}: formatting changes non-whitespace", path.display()).into(),
                );
            }
            edited.replace_range(edit.start_byte..edit.end_byte, &edit.replacement);
        }
        if edited != result.source {
            return Err(format!("{}: edits do not reproduce output", path.display()).into());
        }
        let before = semantics(&source).map_err(|e| format!("{} before: {e}", path.display()))?;
        let after =
            semantics(&result.source).map_err(|e| format!("{} after: {e}", path.display()))?;
        if let Some(field) = first_difference(&before, &after, "") {
            return Err(format!("{}: parsed semantics differ at {field}", path.display()).into());
        }
    }
    println!(
        "Verified {} files: whitespace-only edits, identical parsed semantics, reproducible edits and idempotence.",
        paths.len()
    );
    Ok(())
}
