// =========================================
// =========================================
// src/cli/mod.rs

//! Native command adapters. Applications should use `motionloom::api` directly.
mod format;
mod options;
#[cfg(feature = "weaver")]
mod rendering;

use crate::api::FormatError;
use std::{ffi::OsString, io, path::PathBuf, process::ExitCode};
use thiserror::Error;

const HELP: &str = "MotionLoom\n\nUsage:\n  motionloom fmt [--check] <FILE_OR_DIRECTORY>...\n  motionloom render <SCENE.motionloom> --renderer weaver [OPTIONS]\n  motionloom export <SCENE.motionloom> --renderer weaver [OPTIONS]\n\nRun 'motionloom <COMMAND> --help' for command options.\nRendering requires a native build with --features weaver. No Host is required.\nExit codes: 0 success, 1 fmt check differs or rendering failed, 2 invalid input, 130 cancelled.";

const FMT_HELP: &str = "Usage: motionloom fmt [--check] <FILE_OR_DIRECTORY>...\n\nDirectories are searched recursively for .motionloom files. Symlinks are skipped.\nWithout --check, formatted files are written in place.\nExit codes: 0 success, 1 formatting differs (--check), 2 error.";

/// Preserve typed causes until the terminal boundary prints them.
#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Arguments(String),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {source}")]
    Format { path: PathBuf, source: FormatError },
    #[error("{0}: source changed while formatting; file was not replaced")]
    Changed(PathBuf),
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: crate::api::GraphParseError,
    },
    #[cfg(feature = "weaver")]
    #[error("Invalid render settings: {0}")]
    Settings(#[source] crate::api::weaver::WeaverError),
    #[cfg(feature = "weaver")]
    #[error(transparent)]
    Weaver(#[from] crate::api::weaver::WeaverError),
    #[cfg(feature = "weaver")]
    #[error("Unable to install Ctrl+C handler: {0}")]
    Signal(#[from] ctrlc::Error),
}

impl CliError {
    fn exit_code(&self) -> u8 {
        match self {
            #[cfg(feature = "weaver")]
            Self::Weaver(_) | Self::Signal(_) => 1,
            _ => 2,
        }
    }
}

/// Execute one native command, retaining typed errors for test and adapter callers.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<u8, CliError> {
    let arguments: Vec<_> = arguments.into_iter().collect();
    let Some(command) = arguments.first() else {
        println!("{HELP}");
        return Ok(0);
    };
    if command == "--help" || command == "-h" {
        println!("{HELP}");
        return Ok(0);
    }
    if command == "fmt" {
        return format::run(&arguments);
    }
    let kind = if command == "render" {
        options::CommandKind::Render
    } else if command == "export" {
        options::CommandKind::Export
    } else {
        return Err(CliError::Arguments(
            "Unknown command. Run 'motionloom --help'.".into(),
        ));
    };
    let Some(options) = options::parse(kind, &arguments[1..])? else {
        println!("{}", options::help(kind));
        return Ok(0);
    };
    #[cfg(feature = "weaver")]
    {
        rendering::run(options)
    }
    #[cfg(not(feature = "weaver"))]
    {
        let _ = options;
        Err(CliError::Arguments("Weaver is not enabled in this binary. Build with:\n  cargo build -p motionloom --release --bin motionloom --features weaver\nOr install with:\n  cargo install --path . --bin motionloom --features weaver".into()))
    }
}

/// Terminal entry point shared by the binary and the example command adapters.
pub fn main(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    match run(arguments) {
        Ok(code) => code.into(),
        Err(error) => {
            eprintln!("Error: {error}");
            error.exit_code().into()
        }
    }
}
