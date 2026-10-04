// =========================================
// =========================================
// src/cli/rendering.rs

use super::{
    CliError,
    options::{CommandKind, RenderOptions, default_output_dir},
};
use crate::api::{
    parse_graph_script,
    weaver::{
        CancellationToken, LensOverrides, MasterSequenceSettings, QualityPreset, RenderJob,
        RenderProgress, SceneOutputMode, SequenceDenoiseMode, render, render_master_sequence,
    },
};
use std::{ops::RangeInclusive, path::Path};

struct PreparedRender {
    job: RenderJob,
    frames: RangeInclusive<u32>,
    settings: MasterSequenceSettings,
}

// Job construction is shared by still, movie and example commands, without changing the DSL.
fn prepare(options: &RenderOptions, cwd: &Path) -> Result<PreparedRender, CliError> {
    let script = std::fs::read_to_string(&options.scene).map_err(|source| CliError::Io {
        path: options.scene.clone(),
        source,
    })?;
    let graph = parse_graph_script(&script).map_err(|source| CliError::Parse {
        path: options.scene.clone(),
        source,
    })?;
    let count = (graph.duration_ms as f64 * graph.fps as f64 / 1000.0).ceil();
    if !graph.fps.is_finite() || graph.fps <= 0.0 || !(1.0..=u32::MAX as f64).contains(&count) {
        return Err(CliError::Arguments(
            "DSL must have a positive FPS and a nonempty, representable timeline.".into(),
        ));
    }
    let frame_count = count as u32;
    let frames = match options.kind {
        CommandKind::Render => options.frame..=options.frame,
        CommandKind::Export => options.frames.clone().unwrap_or(0..=frame_count - 1),
    };
    if *frames.end() >= frame_count {
        return Err(CliError::Arguments(format!(
            "Frame {} is outside the DSL timeline 0:{}.",
            frames.end(),
            frame_count - 1
        )));
    }
    let size = graph.render_size.unwrap_or(graph.size);
    let mut job = RenderJob::new(&options.scene, QualityPreset::Ultra);
    job.scene_id = options.scene_id.clone();
    job.render_style = options.style.clone();
    job.frame = *frames.start();
    job.resolution = options.size.unwrap_or([size.0, size.1]);
    job.output_mode = SceneOutputMode::CompositeScene;
    job.output = options
        .out
        .clone()
        .unwrap_or_else(|| default_output_dir(cwd));
    job.memory_budget_mib = 8192;
    job.texture_mips = options.mips;
    job.allow_transmission_stopgap = options.transmission_stopgap;
    job.lens_overrides = LensOverrides {
        enabled: options.dof,
        focal_length_mm: options.focal_length,
        f_stop: options.f_stop,
        focus_distance: options.focus,
    };
    job.sampling.min_samples = options.samples;
    job.sampling.max_samples = options.samples;
    job.sampling.batch_samples = options.samples.min(4);
    job.validate().map_err(CliError::Settings)?;
    let settings = MasterSequenceSettings {
        encode_prores: options.prores,
        encode_preview: options.preview,
        include_audio: options.audio,
        write_scene_composite: options.scene_composite,
        denoise_mode: if options.temporal_denoise {
            SequenceDenoiseMode::Temporal
        } else {
            SequenceDenoiseMode::Independent
        },
        ..Default::default()
    };
    Ok(PreparedRender {
        job,
        frames,
        settings,
    })
}

// Include sample rounds: fixed-budget renders can keep completed_tiles at zero for a long time.
#[derive(Default)]
struct ProgressPrinter {
    last: Option<(u32, u32, u32)>,
}
impl ProgressPrinter {
    fn print(&mut self, frame: u32, completed: u32, total: u32, p: &RenderProgress, samples: u32) {
        let current = (frame, p.completed_tiles, p.tile_min_samples);
        if self.last != Some(current) {
            self.last = Some(current);
            eprintln!(
                "frame {frame} ({completed}/{total}) tile {}/{} samples {}/{} ({:.1}s)",
                p.completed_tiles, p.total_tiles, p.tile_min_samples, samples, p.elapsed_seconds
            );
        }
    }
}

pub(super) fn run(options: RenderOptions) -> Result<u8, CliError> {
    let cwd = std::env::current_dir().map_err(|source| CliError::Io {
        path: ".".into(),
        source,
    })?;
    let PreparedRender {
        job,
        frames,
        settings,
    } = prepare(&options, &cwd)?;
    let cancel = CancellationToken::default();
    let signal_token = cancel.clone();
    ctrlc::set_handler(move || {
        if signal_token.is_cancelled() {
            // A second interrupt lets the user stop a stuck GPU or external encoder.
            std::process::exit(130);
        }
        signal_token.cancel();
        eprintln!(
            "Cancellation requested; waiting for a safe checkpoint. Rerun the same command to resume."
        );
    })?;
    eprintln!(
        "weaver: {} frames={}:{} {}x{} samples={} -> {}",
        job.scene.display(),
        frames.start(),
        frames.end(),
        job.resolution[0],
        job.resolution[1],
        job.sampling.max_samples,
        job.output.display()
    );
    let mut printer = ProgressPrinter::default();
    let result = match options.kind {
        CommandKind::Render => pollster::block_on(render(&job, &cancel, |p| {
            printer.print(job.frame, 0, 1, &p, job.sampling.max_samples);
        }))
        .map(|report| {
            println!(
                "{} [{}] elapsed={:.1}s output={}",
                report.status,
                report.renderer,
                report.elapsed_seconds,
                report.output.display()
            );
            if report.status != "cancelled" {
                let source = if report.output.join("denoised/display.png").is_file() {
                    report.output.join("denoised")
                } else {
                    report.output.clone()
                };
                println!("PNG: {}", source.join("display.png").display());
                println!("EXR: {}", source.join("scene-composite.exr").display());
            }
            for diagnostic in &report.diagnostics {
                println!("diagnostic: {diagnostic}");
            }
            if report.status == "cancelled" { 130 } else { 0 }
        }),
        CommandKind::Export => pollster::block_on(render_master_sequence(
            &job,
            frames,
            &settings,
            &cancel,
            |p| {
                printer.print(
                    p.frame,
                    p.completed_frames,
                    p.total_frames,
                    &p.render,
                    job.sampling.max_samples,
                );
            },
        ))
        .map(|report| {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("render report is serializable")
            );
            if report.status == "cancelled" { 130 } else { 0 }
        }),
    };
    // External encoders may themselves receive SIGINT; still report cancellation consistently.
    if cancel.is_cancelled() {
        return Ok(130);
    }
    result.map_err(CliError::Weaver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::options::parse;
    use std::ffi::OsString;

    #[test]
    fn dsl_defaults_and_overrides_match_the_shared_api_job() {
        let scene = std::env::temp_dir().join(format!(
            "motionloom-cli-job-{}.motionloom",
            std::process::id()
        ));
        let source = include_str!("../../tests/fixtures/cli-render.motionloom")
            .replace("duration=\"0.083s\"", "duration=\"20s\"")
            .replacen(
                "size={[64,64]}",
                "size={[800,450]} renderSize={[1080,1920]}",
                1,
            );
        std::fs::write(&scene, source).unwrap();
        let args = vec![
            scene.clone().into_os_string(),
            OsString::from("--renderer"),
            OsString::from("weaver"),
        ];
        let export = parse(CommandKind::Export, &args).unwrap().unwrap();
        let prepared = prepare(&export, Path::new(".")).unwrap();
        assert_eq!(prepared.job.resolution, [1080, 1920]);
        assert_eq!(prepared.frames, 0..=479);
        assert_eq!(prepared.job.sampling.min_samples, 64);
        assert_eq!(prepared.job.sampling.max_samples, 64);
        assert_eq!(prepared.job.output_mode, SceneOutputMode::CompositeScene);
        assert_eq!(
            prepared.job.lens_source,
            crate::api::weaver::LensSource::AuthoredCamera
        );
        assert!(prepared.job.lens_overrides.enabled.is_none());
        let mut still = parse(CommandKind::Render, &args).unwrap().unwrap();
        still.frame = 1;
        still.size = Some([72, 128]);
        still.focus = Some(4.0);
        let prepared = prepare(&still, Path::new(".")).unwrap();
        let mut api_job = RenderJob::new(&still.scene, QualityPreset::Ultra);
        api_job.scene_id = "auto".into();
        api_job.render_style = "auto".into();
        api_job.frame = 1;
        api_job.resolution = [72, 128];
        api_job.output_mode = SceneOutputMode::CompositeScene;
        api_job.output = default_output_dir(Path::new("."));
        api_job.memory_budget_mib = 8192;
        api_job.sampling.min_samples = 128;
        api_job.sampling.max_samples = 128;
        api_job.lens_overrides.focus_distance = Some(4.0);
        assert_eq!(
            serde_json::to_value(prepared.job).unwrap(),
            serde_json::to_value(api_job).unwrap()
        );
        still.frame = 480;
        assert!(prepare(&still, Path::new(".")).is_err());
        std::fs::remove_file(scene).unwrap();
    }
}
