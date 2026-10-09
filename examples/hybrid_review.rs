// examples/hybrid_review.rs
//! Capture review stills while retaining the actual native temporal history.
//! Usage: hybrid_review SCRIPT ASSET_ROOT OUTPUT_DIR FRAMES_COMMA_LIST [WIDTH]
//!                      [--warm=N] [--profile=cinematic|balanced|portable|ultra]
//!                      [--device-control]
use motionloom::api::{
    SceneRenderProfile, SceneRenderer, motionloom_analyze_script_json, parse_graph_script,
    set_scene_asset_roots,
};
use motionloom::{ImmediatePreviewProfile, ImmediatePreviewSettings};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, time::Instant};

const USAGE: &str = "usage: hybrid_review SCRIPT ASSET_ROOT OUTPUT_DIR FRAMES_COMMA_LIST [WIDTH] [--warm=N] [--profile=cinematic|balanced|portable|ultra] [--device-control]";

fn pixel_evidence(image: &image::RgbaImage) -> serde_json::Value {
    let mut minimum = [u8::MAX; 4];
    let mut maximum = [u8::MIN; 4];
    let mut nontransparent_pixels = 0_u64;
    for pixel in image.pixels() {
        for channel in 0..4 {
            minimum[channel] = minimum[channel].min(pixel[channel]);
            maximum[channel] = maximum[channel].max(pixel[channel]);
        }
        nontransparent_pixels += u64::from(pixel[3] != 0);
    }
    json!({"channelMinimum":minimum,"channelMaximum":maximum,
        "nontransparentPixels":nontransparent_pixels,
        "fullyTransparent":nontransparent_pixels == 0})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 5 {
        return Err(USAGE.into());
    }
    let mut requested_width = None;
    let mut warm_count = 8_u32;
    let mut profile = ImmediatePreviewProfile::Cinematic;
    let mut device_control = false;
    for arg in &args[5..] {
        if let Some(count) = arg.strip_prefix("--warm=") {
            warm_count = count.parse::<u32>()?;
        } else if let Some(name) = arg.strip_prefix("--profile=") {
            profile = match name {
                "cinematic" => ImmediatePreviewProfile::Cinematic,
                "balanced" => ImmediatePreviewProfile::Balanced,
                "portable" => ImmediatePreviewProfile::Portable,
                "ultra" => ImmediatePreviewProfile::Ultra,
                _ => return Err(format!("unrecognized preview profile {name:?}; {USAGE}").into()),
            };
        } else if arg == "--device-control" {
            device_control = true;
        } else if arg.starts_with('-') || requested_width.is_some() {
            return Err(format!("unrecognized argument {arg:?}; {USAGE}").into());
        } else {
            requested_width = Some(arg.parse::<u32>()?);
        }
    }
    let source_path = PathBuf::from(&args[1]);
    let source = fs::read_to_string(&source_path)?;
    let mut graph = parse_graph_script(&source)?;
    let frames = args[4]
        .split(',')
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    let frame_count = (graph.duration_ms as f64 * graph.fps as f64 / 1000.0).ceil() as u32;
    if frames.is_empty() || frames.iter().any(|frame| *frame >= frame_count) {
        return Err("capture frames must belong to the authored timeline".into());
    }
    let authored_dimensions = graph.render_size.unwrap_or(graph.size);
    let mut review_dimensions = authored_dimensions;
    if let Some(width) = requested_width {
        if width == 0 || authored_dimensions.0 == 0 {
            return Err("review width must be positive".into());
        }
        let height = ((authored_dimensions.1 as f64 * width as f64 / authored_dimensions.0 as f64)
            .round() as u32)
            .max(1);
        review_dimensions = (width, height);
        // Change only the review output size. Logical DSL coordinates/cameras
        // and every authored frame remain intact; the aspect ratio is retained.
        graph.render_size = Some(review_dimensions);
    }
    let output = PathBuf::from(&args[3]);
    fs::create_dir_all(&output)?;
    let asset_root = PathBuf::from(&args[2]);
    set_scene_asset_roots(vec![asset_root.clone()]);
    fs::write(
        output.join("authoring-report.json"),
        motionloom_analyze_script_json(&source),
    )?;
    let settings = ImmediatePreviewSettings {
        profile,
        target_fps: 30.0,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
    };
    let mut renderer = pollster::block_on(SceneRenderer::new(SceneRenderProfile::Gpu))?;
    renderer.set_immediate_preview_settings(settings);
    let mut warm_submissions = Vec::new();
    for submission in 0..warm_count {
        let start = Instant::now();
        let _ = pollster::block_on(renderer.render_frame_to_wgpu_texture(&graph, frames[0]))?;
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        warm_submissions.push(json!({"submission":submission + 1,
            "frame":frames[0],"submissionWallMs":wall_ms}));
        println!(
            "warm submission {}/{warm_count}, {wall_ms:.1} ms",
            submission + 1
        );
    }
    let mut captures = Vec::new();
    let mut previous: Option<u32> = None;
    for frame in frames {
        let start = Instant::now();
        let image = pollster::block_on(renderer.render_frame_gpu_readback(&graph, frame))?;
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        let filename = format!("frame-{frame:04}.png");
        image.save(output.join(&filename))?;
        captures.push(json!({"frame":frame,"timeSeconds":frame as f64/graph.fps as f64,
            "file":filename,"dimensions":[image.width(),image.height()],
            "readbackRenderWallMs":wall_ms,"consecutiveHistory":previous.is_some_and(|p|p.checked_add(1)==Some(frame)),
            "pixels":pixel_evidence(&image),
            "scene3d":renderer.last_3d_frame_profile()}));
        previous = Some(frame);
        // Preserve review evidence even when a later requested frame fails.
        fs::write(
            output.join("review-report.json"),
            serde_json::to_vec_pretty(&json!({
                "sourcePath":source_path,"sourceSha256":format!("{:x}",Sha256::digest(source.as_bytes())),
                "assetRoot":asset_root,"settings":settings,"authoredDimensions":authored_dimensions,
                "reviewDimensions":review_dimensions,"warmSubmissionsAtFirstFrame":warm_count,
                "warmSubmissions":warm_submissions,"deviceControl":null,
                "capturePolicy":"One retained native renderer. Only the first frame is warmed. Consecutive requested frames retain normal temporal history; seeks/cuts use engine invalidation.",
                "timingMeaning":"Capture timing is render plus GPU readback wall time. Warm timing is CPU preparation and submission wall time without waiting for GPU completion. Neither is sustained playback FPS.",
                "captures":captures,"scope":"Selected still frames only; no movie export."
            }))?,
        )?;
        println!("captured frame {frame}, {wall_ms:.1} ms");
    }
    if device_control {
        // Use the retained renderer and device after the heavy scene, so this
        // isolates a persistent device/queue failure from scene-specific pixels.
        let control_source = r##"<Graph fps="24" duration="1s" size={[64,64]}>
  <Background color="#20C080" />
  <Scene id="device_control">
    <Timeline><Track space="screen"><Sequence duration="1s"><Layer>
      <Rect x="0" y="0" width="64" height="64" color="#20C080" />
    </Layer></Sequence></Track></Timeline>
  </Scene>
  <Present from="device_control" />
</Graph>"##;
        let control_graph = parse_graph_script(control_source)?;
        let start = Instant::now();
        let image = pollster::block_on(renderer.render_frame_gpu_readback(&control_graph, 0))?;
        let wall_ms = start.elapsed().as_secs_f64() * 1000.0;
        image.save(output.join("device-control.png"))?;
        fs::write(output.join("device-control.motionloom"), control_source)?;
        let expected = [32_u8, 192, 128, 255];
        let matches_background = image.pixels().all(|pixel| {
            pixel
                .0
                .iter()
                .zip(expected)
                .all(|(actual, wanted)| actual.abs_diff(wanted) <= 1)
        });
        let control_report = json!({"file":"device-control.png","sourceFile":"device-control.motionloom",
            "dimensions":[image.width(),image.height()],"readbackRenderWallMs":wall_ms,
            "pixels":pixel_evidence(&image),"expectedRgba":expected,
            "matchesExpectedBackground":matches_background,
            "policy":"Cheap 2D control on the same retained renderer and GPU device, submitted after all requested scene captures."});
        fs::write(
            output.join("device-control-report.json"),
            serde_json::to_vec_pretty(&control_report)?,
        )?;
        let report_path = output.join("review-report.json");
        let mut report: serde_json::Value = serde_json::from_slice(&fs::read(&report_path)?)?;
        report["deviceControl"] = control_report;
        fs::write(report_path, serde_json::to_vec_pretty(&report)?)?;
        println!(
            "device control captured, {wall_ms:.1} ms, expected background: {matches_background}"
        );
    }
    Ok(())
}
