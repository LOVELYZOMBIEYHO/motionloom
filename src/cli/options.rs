// =========================================
// =========================================
// src/cli/options.rs

use super::CliError;
#[cfg(feature = "weaver")]
use std::path::Path;
use std::{ffi::OsString, ops::RangeInclusive, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CommandKind {
    Render,
    Export,
}

pub(super) fn help(kind: CommandKind) -> String {
    let (command, selection, samples) = match kind {
        CommandKind::Render => (
            "render",
            "  --frame N                  Zero-based frame (default: 0)",
            128,
        ),
        CommandKind::Export => (
            "export",
            "  --frames START:END         Inclusive range (default: full DSL timeline)\n  --no-prores                Skip ProRes 4444 XQ movie\n  --no-preview               Skip H.264 MP4 movie\n  --no-audio                 Skip authored audio\n  --no-scene-composite       Skip additional scene-linear sequence\n  --temporal-denoise         Temporal history; completed-frame reuse is disabled",
            64,
        ),
    };
    format!(
        "Usage: motionloom {command} <SCENE.motionloom> --renderer weaver [OPTIONS]\n\n  --renderer weaver          Native Weaver path tracer\n{selection}\n  --samples N                Fixed sample count, 2..1000000 (default: {samples})\n  --size WxH                 Override DSL renderSize, or size when absent\n  --out DIR                  Output root (default: workspace .render-output/weaver)\n  --scene-id ID              Scene selection (default: auto)\n  --style ID                 RenderStyle selection (default: auto)\n  --f-stop F                 Override DSL camera f-stop\n  --focus D                  Override focus distance in scene units\n  --focal-length MM          Override focal length\n  --dof / --no-dof           Override DSL depth of field\n  --mips                     Enable Weaver texture mipmaps\n  --transmission-stopgap     Explicit temporary transmission fallback\n  --composite-scene          Complete Scene composition (already the default)\n  -h, --help                 Show help\n\nRelative asset paths resolve beside the DSL file. Frame numbers start at 0.\nCtrl+C requests cancellation at a safe checkpoint; rerun the same command to resume.\nOutput paths are printed on completion; export retains EXRs and a sequence manifest."
    )
}

#[derive(Debug)]
#[cfg_attr(not(feature = "weaver"), allow(dead_code))]
pub(super) struct RenderOptions {
    pub kind: CommandKind,
    pub scene: PathBuf,
    pub frame: u32,
    pub frames: Option<RangeInclusive<u32>>,
    pub size: Option<[u32; 2]>,
    pub samples: u32,
    pub out: Option<PathBuf>,
    pub scene_id: String,
    pub style: String,
    pub f_stop: Option<f32>,
    pub focus: Option<f32>,
    pub focal_length: Option<f32>,
    pub dof: Option<bool>,
    pub mips: bool,
    pub transmission_stopgap: bool,
    pub prores: bool,
    pub preview: bool,
    pub audio: bool,
    pub scene_composite: bool,
    pub temporal_denoise: bool,
}

fn invalid(message: impl Into<String>) -> CliError {
    CliError::Arguments(message.into())
}

fn number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, CliError> {
    value
        .parse()
        .map_err(|_| invalid(format!("Invalid value for {flag}: `{value}`")))
}

// Both commands and examples use one parser; OS paths never require UTF-8.
pub(super) fn parse(
    kind: CommandKind,
    arguments: &[OsString],
) -> Result<Option<RenderOptions>, CliError> {
    if arguments.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(None);
    }
    let mut options = RenderOptions {
        kind,
        scene: PathBuf::new(),
        frame: 0,
        frames: None,
        size: None,
        samples: if kind == CommandKind::Render { 128 } else { 64 },
        out: None,
        scene_id: "auto".into(),
        style: "auto".into(),
        f_stop: None,
        focus: None,
        focal_length: None,
        dof: None,
        mips: false,
        transmission_stopgap: false,
        prores: true,
        preview: true,
        audio: true,
        scene_composite: true,
        temporal_denoise: false,
    };
    let mut renderer = None;
    let mut positional = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        index += 1;
        if !positional && argument == "--" {
            positional = true;
            continue;
        }
        if positional || !argument.to_string_lossy().starts_with('-') {
            if !options.scene.as_os_str().is_empty() {
                return Err(invalid("Specify exactly one .motionloom scene."));
            }
            options.scene = PathBuf::from(argument);
            continue;
        }
        let flag = argument
            .to_str()
            .ok_or_else(|| invalid("Option names must be UTF-8."))?;
        let mut value_os = || -> Result<&OsString, CliError> {
            let value = arguments
                .get(index)
                .filter(|value| !value.to_string_lossy().starts_with("--"))
                .ok_or_else(|| invalid(format!("Missing value for {flag}")))?;
            index += 1;
            Ok(value)
        };
        let mut value = || -> Result<&str, CliError> {
            value_os()?
                .to_str()
                .ok_or_else(|| invalid(format!("Value for {flag} must be UTF-8.")))
        };
        match flag {
            "--renderer" => renderer = Some(value()?.to_owned()),
            "--frame" if kind == CommandKind::Render => options.frame = number(value()?, flag)?,
            "--frames" if kind == CommandKind::Export => {
                let range = value()?;
                let (start, end) = range
                    .split_once(':')
                    .ok_or_else(|| invalid("--frames must be START:END."))?;
                let (start, end): (u32, u32) = (number(start, flag)?, number(end, flag)?);
                if start > end {
                    return Err(invalid("--frames START must not exceed END."));
                }
                options.frames = Some(start..=end);
            }
            "--size" => {
                let size = value()?;
                let (width, height) = size
                    .split_once(['x', 'X'])
                    .ok_or_else(|| invalid("--size must be WxH."))?;
                let size = [number(width, flag)?, number(height, flag)?];
                if size.iter().any(|&v| v == 0 || v > 16384) {
                    return Err(invalid("--size dimensions must be 1..16384."));
                }
                options.size = Some(size);
            }
            "--samples" => {
                options.samples = number(value()?, flag)?;
                if !(2..=1_000_000).contains(&options.samples) {
                    return Err(invalid("--samples must be 2..1000000."));
                }
            }
            "--out" => options.out = Some(PathBuf::from(value_os()?)),
            "--scene-id" => options.scene_id = value()?.to_owned(),
            "--style" => options.style = value()?.to_owned(),
            "--f-stop" => options.f_stop = Some(number(value()?, flag)?),
            "--focus" => options.focus = Some(number(value()?, flag)?),
            "--focal-length" => options.focal_length = Some(number(value()?, flag)?),
            "--dof" | "--no-dof" => {
                let enabled = flag == "--dof";
                if options.dof.is_some_and(|current| current != enabled) {
                    return Err(invalid("--dof and --no-dof conflict."));
                }
                options.dof = Some(enabled);
            }
            "--mips" => options.mips = true,
            "--transmission-stopgap" => options.transmission_stopgap = true,
            "--composite-scene" => {}
            "--no-prores" if kind == CommandKind::Export => options.prores = false,
            "--no-preview" if kind == CommandKind::Export => options.preview = false,
            "--no-audio" if kind == CommandKind::Export => options.audio = false,
            "--no-scene-composite" if kind == CommandKind::Export => {
                options.scene_composite = false
            }
            "--temporal-denoise" if kind == CommandKind::Export => options.temporal_denoise = true,
            _ => {
                return Err(invalid(format!(
                    "Unknown option for {}: `{flag}`. Run 'motionloom {} --help'.",
                    if kind == CommandKind::Render {
                        "render"
                    } else {
                        "export"
                    },
                    if kind == CommandKind::Render {
                        "render"
                    } else {
                        "export"
                    }
                )));
            }
        }
    }
    if options.scene.as_os_str().is_empty() {
        return Err(invalid("Specify a .motionloom scene."));
    }
    if options
        .scene
        .extension()
        .is_none_or(|ext| ext != "motionloom")
    {
        return Err(invalid("Expected a .motionloom scene file."));
    }
    match renderer.as_deref() {
        Some("weaver") => {}
        Some(other) => {
            return Err(invalid(format!(
                "Unsupported renderer `{other}`. Available: weaver."
            )));
        }
        None => return Err(invalid("Specify --renderer weaver.")),
    }
    Ok(Some(options))
}

// Search from the working directory, preserving the existing workspace output convention.
#[cfg(feature = "weaver")]
pub(super) fn default_output_dir(start: &Path) -> PathBuf {
    for dir in start.ancestors() {
        if (dir.join("anica").is_dir() && dir.join("motionloom-example").is_dir())
            || dir.join(".render-output").is_dir()
        {
            return dir.join(".render-output/weaver");
        }
    }
    start.join(".render-output/weaver")
}
