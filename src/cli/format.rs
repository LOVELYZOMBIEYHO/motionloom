// =========================================
// =========================================
// src/cli/format.rs

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::api::format_dsl;

use super::CliError;

fn io_error(path: &Path, source: io::Error) -> CliError {
    CliError::Io {
        path: path.into(),
        source,
    }
}

fn collect(path: &Path, files: &mut BTreeSet<PathBuf>, explicit: bool) -> Result<(), CliError> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error(path, e))?;
    if metadata.file_type().is_symlink() {
        if explicit {
            return Err(CliError::Arguments(
                "Explicit symlink paths are not supported.".into(),
            ));
        }
        return Ok(());
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path).map_err(|e| io_error(path, e))? {
            let entry = entry.map_err(|e| io_error(path, e))?;
            collect(&entry.path(), files, false)?;
        }
    } else if metadata.is_file() && path.extension().is_some_and(|e| e == "motionloom") {
        files.insert(fs::canonicalize(path).map_err(|e| io_error(path, e))?);
    } else if explicit {
        return Err(CliError::Arguments(
            "Expected a .motionloom file or directory.".into(),
        ));
    }
    Ok(())
}

// Prepare every output before replacing files, retaining permissions and checking revisions.
struct PreparedFile {
    path: PathBuf,
    original: String,
    formatted: String,
    temporary: Option<PathBuf>,
}

impl PreparedFile {
    fn prepare(&mut self, index: usize) -> Result<(), CliError> {
        let name = self.path.file_name().unwrap().to_string_lossy();
        let temp = self
            .path
            .with_file_name(format!(".{name}.fmt-{}-{index}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| io_error(&temp, e))?;
        self.temporary = Some(temp.clone());
        let permissions = fs::metadata(&self.path)
            .map_err(|e| io_error(&self.path, e))?
            .permissions();
        file.write_all(self.formatted.as_bytes())
            .map_err(|e| io_error(&temp, e))?;
        file.set_permissions(permissions)
            .map_err(|e| io_error(&temp, e))?;
        file.sync_all().map_err(|e| io_error(&temp, e))?;
        Ok(())
    }

    fn verify_original(&self) -> Result<(), CliError> {
        if fs::read_to_string(&self.path).map_err(|e| io_error(&self.path, e))? != self.original {
            return Err(CliError::Changed(self.path.clone()));
        }
        Ok(())
    }

    fn replace(&mut self) -> Result<(), CliError> {
        self.verify_original()?;
        fs::rename(self.temporary.as_ref().unwrap(), &self.path)
            .map_err(|e| io_error(&self.path, e))?;
        self.temporary = None;
        println!("Formatted {}", self.path.display());
        Ok(())
    }
}

impl Drop for PreparedFile {
    fn drop(&mut self) {
        if let Some(path) = &self.temporary {
            let _ = fs::remove_file(path);
        }
    }
}

pub(super) fn run(arguments: &[std::ffi::OsString]) -> Result<u8, CliError> {
    let mut check = false;
    let mut positional = false;
    let mut paths = Vec::new();
    for argument in &arguments[1..] {
        if !positional && (argument == "--help" || argument == "-h") {
            println!("{}", super::FMT_HELP);
            return Ok(0);
        } else if !positional && argument == "--check" {
            check = true;
        } else if !positional && argument == "--" {
            positional = true;
        } else if !positional && argument.to_string_lossy().starts_with('-') {
            return Err(CliError::Arguments(
                "Unknown option. Run 'motionloom fmt --help'.".into(),
            ));
        } else {
            paths.push(PathBuf::from(argument));
        }
    }
    if paths.is_empty() {
        return Err(CliError::Arguments(
            "Specify a .motionloom file or directory.".into(),
        ));
    }
    let mut files = BTreeSet::new();
    for path in paths {
        collect(&path, &mut files, true)?;
    }
    if files.is_empty() {
        return Err(CliError::Arguments("No .motionloom files found.".into()));
    }
    let count = files.len();
    let mut changed = Vec::new();
    for path in files {
        let original = fs::read_to_string(&path).map_err(|e| io_error(&path, e))?;
        let result = format_dsl(&original).map_err(|source| CliError::Format {
            path: path.clone(),
            source,
        })?;
        if result.changed {
            changed.push(PreparedFile {
                path,
                original,
                formatted: result.source,
                temporary: None,
            });
        }
    }
    if check {
        for file in &changed {
            println!("Needs formatting: {}", file.path.display());
        }
        println!("Checked {count} files; {} need formatting.", changed.len());
        return Ok(u8::from(!changed.is_empty()));
    }
    for (index, file) in changed.iter_mut().enumerate() {
        file.prepare(index)?;
    }
    for file in &changed {
        file.verify_original()?;
    }
    for file in &mut changed {
        file.replace()?;
    }
    println!("Checked {count} files; formatted {}.", changed.len());
    Ok(0)
}
