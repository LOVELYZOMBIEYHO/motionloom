# MotionLoom CLI

The native `motionloom` executable provides `fmt`, `render` and `export`.
Rendering uses `motionloom::api::weaver`; the CLI does not implement a separate
renderer or require an Anica Host. The same command adapter is used by the
`weaver_frame` and `weaver_sequence` examples.

## Build or install

From the `motionloom` repository:

```sh
cargo build -p motionloom --release --bin motionloom --features weaver
cargo install --path . --bin motionloom --features weaver
```

Installation makes `motionloom` available on the Cargo binary path. A build
without `weaver` still supports `fmt` and rendering command help; attempting to
render prints the feature/build instructions. The CLI is native only and does
not enter the browser WASM build.

## Render one frame

```sh
motionloom render main.motionloom \
  --renderer weaver --frame 1 --samples 128
```

Frame numbers are zero based. The default frame is `0`; the default fixed sample
count is `128`. The full Scene composition, including its background and 2D
content supported by Weaver, is written to `display.png`; linear EXRs are kept.
The completion report prints the actual PNG, scene-composite EXR and job paths.
If denoised composition is available, the printed image path selects it.

## Export a movie

```sh
motionloom export main.motionloom \
  --renderer weaver --samples 64 --out output/
```

Export defaults to `64` fixed samples and the complete authored timeline:
`0..ceil(duration_seconds * fps)-1`. Use `--frames 0:47` for an inclusive range.
The API writes a signature-specific directory under `output/sequences/`, keeping
master EXRs, per-frame resume records and `sequence-manifest.json`. By default,
this also produces `preview.mp4` (H.264) and
`display-master-prores4444xq.mov` (ProRes 4444 XQ), with authored audio when present.
Use `--no-prores`, `--no-preview` or `--no-audio` to disable those outputs.
`--no-scene-composite` omits the additional scene-linear EXR sequence; the display
masters needed for encoding and resume are retained.

MP4 and ProRes require FFmpeg and FFprobe. The existing API resolves the configured
runtime environment, the bundled Anica runtime when available, then the executable
path. No encoding or color-transform implementation is duplicated in the CLI.

## Shared settings

- `--renderer weaver` explicitly selects the supported offline renderer.
- Resolution comes from `Graph.renderSize`, or `Graph.size` when absent.
  `--size 1080x1920` overrides it for this job without editing the DSL.
- FPS, timeline, RenderStyle and camera optics come from the DSL.
- `--scene-id` and `--style` default to `auto`.
- `--f-stop`, `--focus`, `--focal-length`, `--dof` and `--no-dof` override only
  supplied camera fields. `--focus` uses scene units.
- `--mips` enables Weaver texture mipmaps.
- Physical dielectric reflection/refraction and material layers are enabled by
  default. `refractionMode="slab"` uses authored sheet thickness; `"solid"` traces
  actual closed-geometry entry/exit paths. See [hybrid/glass](HYBRID_REFLECTIONS.md).
- `--transmission-stopgap` explicitly disables physical glass and selects an
  opaque/alpha PBR migration fallback.
- Samples must be `2..1000000`. `--samples N` sets both minimum and maximum to `N`;
  the Rust API's adaptive quality presets remain available to API callers.
  Native rendering uses the Ultra path-depth preset. GPU work completes in
  groups of at most eight tiles and four samples per pixel before readback;
  this limits submission duration without lowering the requested sample count.
- `--out` names an output root directory. Without it, the CLI searches upward
  from the current working directory for the existing workspace output location,
  falling back to `.render-output/weaver` in that working directory.
- Relative asset paths resolve beside the source DSL, not beside the CLI binary.

For example, S98's `20s`, `24 fps`, `1080x1920` DSL produces frames `0:479`
without needing `--frames` or `--size`.

## Progress, cancel and resume

Progress goes to stderr and reports frame, tile, sample round and elapsed time.
Sample changes are reported even when no whole tile has finished. Reports and
actual output paths go to stdout.

Reducing total samples changes the number of rounds, not the cost of the first
round. High-resolution dense scenes therefore use bounded tile groups rather
than placing a complete frame into a single long GPU command. Each group writes
durable checkpoints; cancellation and resume can occur within a sample round.

The first `Ctrl+C` requests cancellation using the existing API token. An
in-flight GPU batch or audio-preparation/validation step may need to finish before
the safe checkpoint is reached. Video encoding subprocesses are cancellable.
A second `Ctrl+C` aborts the process immediately, retaining only durable output
already written.

Rerun the same command, source and settings with the same output root. The API
reuses signature-matched completed frames and available tile checkpoints; it does
not merely skip files by name. Keep referenced assets unchanged when resuming.
`--temporal-denoise` currently rerenders the selected range because temporal
history sidecars are not persisted. A completed sequence rerun can reuse rendered
frames while rebuilding its movies.

## Help and exit codes

```sh
motionloom --help
motionloom fmt --help
motionloom render --help
motionloom export --help
```

| Code | Meaning |
| --- | --- |
| `0` | Successful completion or help |
| `1` | `fmt --check` found differences, or rendering/runtime failure |
| `2` | Invalid command, input, DSL or settings |
| `130` | Cancelled by Ctrl+C |

Arguments, frame ranges, source parsing and job budgets are checked before GPU
allocation. Errors retain typed causes until the terminal boundary prints them.
For formatting rules and file-write behavior, see [FORMATTING.md](FORMATTING.md).
