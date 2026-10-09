# Immediate preview: material foundation

## Physical raster policy

All normal native Preview and WASM WebGPU rendering uses raster PBR with
profile-bounded SSR/SSGI, retained environment and room-probe IBL, shadow maps,
and bounded screen-space glass. `shading="physical"`, filmic physical and
unstyled defaults use this same policy; no Showcase source migration is needed.
Full-scene reflection BVH preparation, coarse geometry query passes, secondary
hit lighting/shadow rays, and geometry-reflection history are absent from this
path. Primary material texture filtering, alpha, PBR layers, HDR and authored AA
remain intact. Nonmatching clearcoat/sheen lobes retain their probe/environment
response while SSR replaces only its compatible base lobe.

`Scene3DFrameProfile.geometry_transport_enabled` is false and hybrid scene
bytes/triangles/nodes are zero for normal preview. The frame profile exposes
solid/repeated-reflection approximation and stale-bake fallback flags. A stale
bake is omitted with a deduplicated warning; malformed or missing assets remain
errors. Native-only `MOTIONLOOM_REFERENCE_GEOMETRY_TRANSPORT=1` exists for A/B
regression investigation, not as a public scene or browser quality setting.
Weaver uses a separate path integrator and is unaffected by this policy.

## Host quality contract

`ImmediatePreviewSettings` separates editor performance from authored
`RenderStyle` and offline output. Hosts can select `Portable`, `Balanced`,
`Cinematic`, or `Ultra`; each profile maps to concrete shadow-map,
texture-filtering, light-count, DoF-sampling, HDR, screen-space lighting, and
antialiasing budgets. The parsed graph and requested output dimensions are
never rewritten by selecting a profile.

```rust
use motionloom::{
    ImmediatePreviewProfile, ImmediatePreviewSettings, WgpuPreviewEngine,
};

let mut preview = WgpuPreviewEngine::new_with_cpu_fallback().await;
preview.set_settings(ImmediatePreviewSettings {
    profile: ImmediatePreviewProfile::Cinematic,
    target_fps: 30.0,
    dynamic_resolution: true,
    min_resolution_scale: 0.5,
});
```

`WgpuPreviewEngine::capabilities()` reports the actual immediate features,
including whether the host uses the browser WebGPU tier, its bounded SSR/SSGI
sample limits, local reflection probes, baked diffuse lighting, and planar
reflections when a GPU renderer is available.
`last_frame_metrics()` combines GPU timing,
Scene CPU timing, triangle/light counts, retained-cache counts, shadow-map
resolution, and estimated render-target memory.

Native timestamp scopes are separate. The existing `gpu_ms` metric measures
the final Scene compositor, not the preceding World 3D passes.
`Scene3DFrameProfile.gpu_ms` and `gpu_frame_index` identify the latest completed
3D queue span. `gpu_stages_ms` orders its six intervals as shadows/planar and
visibility preparation, reflection evidence, route classification, opaque
surface shading, reflection history plus ordered glass, and postprocessing.
The live-preview title labels these as `3D GPU` and `compositor GPU`.
`ImmediatePreviewFrameMetrics::estimated_frame_ms()` takes the maximum of
CPU work and both available GPU scopes, so a cheap compositor cannot hide a
slow 3D workload. It does not sum independently completed asynchronous scopes.
Timestamps are read asynchronously with at most three pending readbacks; an
unfinished measurement never adds a frame wait. A span can include queue idle
gaps between markers and does not measure a utilization percentage.
Portable pass markers can overlap independent graphics work on some backends;
their stage intervals are queue observations, not exclusive per-pass costs.
Do not attribute an entire delayed raster operation to the following marker.

The title's `CPU/driver` value is preparation and submission wall time after
subtracting separately reported explicit waits. Driver calls can still block
when the GPU queue is full, so this value is not CPU execution time or a CPU
utilization percentage. Readback duration and first-run shader compilation
also cannot establish sustained playback FPS; use completed presentations.

The native CLI preview requests the active adapter's supported limits, matching
the headless native renderer. A host that requests only minimum WebGPU limits
can disable optional renderer features even on a capable adapter. The native
geometry reference's coarse reflection evidence has additional limit guards;
that evidence path is not part of normal Preview or WASM rendering.
`MOTIONLOOM_MAX_BUFFER_MIB` still caps individual buffer allocations without
discarding the adapter's other capabilities.

The quality degradation order is resolution (respecting the configured floor),
optical samples, then profile-controlled shadow and filtering cost. Authored
materials, animation, camera, colors, and style identity remain unchanged.

The improvements are renderer internals shared by native and WASM WebGPU.
Anti-aliasing is additionally an explicit RenderStyle policy:

```xml
<AntiAliasingStyle method="taa" quality="high" fallback="smaa" sharpness="0.15" />
```

Scenes without a RenderStyle retain the host profile policy. A referenced
RenderStyle without `AntiAliasingStyle` deliberately resolves to `off`; this is
the compatibility break that makes AA cost explicit. Immediate preview supports
`off`, FXAA, compact spatial morphology AA, and history-rejected TAA. Portable
`msaa` and `ssaa` intents use the requested safe fallback until a backend offers
their required multisample targets or internal-resolution path. The last-frame
profile reports requested/effective methods and whether a fallback occurred.

- Material textures receive retained mip chains and 8x anisotropic filtering.
  Color filtering uses linear light with alpha-weighted mip generation; normal
  mip vectors are normalized; metallic, roughness and AO remain linear data.
  Texture caching includes the semantic role, including when a GLB reuses one
  image for multiple material slots.
- Material AO is sampled separately and attenuates indirect diffuse and specular
  light. It no longer stains base color or direct lighting. Primitive materials,
  GLB materials and blended terrain carry independent AO; missing AO is neutral.
- The 3D intermediate target and transmission snapshot use RGBA16Float. Tone
  mapping and output gamma run once after transparency and camera depth of field.
  Exposure, white balance and contrast remain before blending. SceneCompositor
  receives and blends linear-premultiplied RGBA16F; display readback is encoded
  only after the HDR composition is complete.
- The primary opaque geometry pass uses MRT to retain material normal, velocity,
  roughness, metallic, AO and a temporal reactive mask while it writes shaded
  HDR plus the indirect specular lobe used by reflection resolve. Per-object motion comes from previous model transforms and previous bone
  palettes. This removes the former extra geometry submission and fullscreen
  normal-reconstruction pass.
- Temporal resolve reprojects the prior display frame. Host-controlled unstyled
  scenes retain the profile's stable projection. An explicit `taa` style uses a
  bounded Halton phase count selected by `quality`; depth, normal, velocity and
  reactive evidence reject invalid history so thin geometry does not accumulate
  the former long trails. History is
  rejected after a non-sequential seek, camera cut, size change or
  preview-profile change. Motion vectors use
  interpolated current and previous clip positions instead of a quantized
  fragment pixel centre; history confidence and motion blur use de-jittered
  physical velocity.
- Material-aware SSR uses profile-bounded ray marching, binary
  hit refinement, back-face rejection, and roughness-dependent filtering. The
  room-local HDR probe or global environment is the off-screen fallback. SSR
  replaces only indirect specular radiance, retaining diffuse/direct light.
  Planar mirrors and glass pixels skip this opaque-depth post approximation.
  Sharp glass instead uses a separate inline ray from its actual interface
  normal into the current transmission snapshot when the profile enables SSR.
  It has at most 24 march steps and four refinements, retains its RGB BRDF,
  and leaves probe/environment reflection intact on a miss. It shares the
  existing transmission-layer budget and does not run within planar captures.
  Opaque tinted GGX lobes retain RGB probe/environment reflection because the
  current opaque reflection payload stores only a scalar response.
- Cinematic and Ultra add bounded diffuse screen-space GI. It samples visible
  neighbouring radiance with normal, range, metallic, and AO rejection, then
  keeps environment IBL as the non-screen fallback. A bound active BakedLighting
  asset disables this extra gather to avoid adding the same bounce twice.
  Room baking traces camera-independent diffuse transport with actual geometry;
  the screen-space gather itself remains a visible-neighbour approximation.
- Shadow filtering uses a rotated Poisson kernel and slope-aware bias instead
  of an axis-aligned 3x3 kernel. The same algorithm is compiled for native and
  browser WebGPU.
  Opt-in `LightingStyle shadowMode="perLight"` masks each direct emitter
  independently: the primary directional map is retained, spots use one view,
  points use six cube faces, and each area sample uses six faces. Portable,
  Balanced and Cinematic/Ultra request one, two and four area samples; total
  view limits can reduce that count. Local faces are 256² on Portable and
  512² on the other profiles, with local view caps of 64/64/96/96. Source-size
  PCSS uses static `DirectionalLight angularDiameter` (0–90 degrees) and
  point/spot `sourceRadius` (0–1000 scene units), both omitted defaults zero.
  A sun diameter of `0.5` and fixture radius of `0.025` are useful room examples.
  `RectAreaLight castShadow` is a literal true/false, default false. See
  [per-light shadows](PER_LIGHT_SHADOWS.md) for full defaults and limits.
- Native and WASM consume the same graph, atmosphere plan, material data, and
  WGSL. Browser defaults to Portable and caps the expensive Cinematic/Ultra
  screen-space loops at 20 SSR steps and 4 GI taps; native caps them at 40 and
  8. This changes implementation quality and cost, not authored semantics or
  requested output dimensions.

[Indoor lighting and reflections](BAKED_LIGHTING.md) documents directional SH9
irradiance, GGX-filtered specular environments and the split-sum BRDF LUT. Original
background mips are separate. Room grids carry directional depth moments and
validity; local captures use box projection, while mirrors render a reflected
camera including offscreen opaque objects and bounded transparent layers.
Glass uses camera-consistent thin-slab refraction and successive HDR snapshots.
Authored solid glass uses that same bounded slab approximation in Preview,
including its authored thickness and transmission-layer budget. Actual solid
entry/exit tracing belongs to Weaver or the native diagnostic reference.
These real-time methods do not provide unrestricted path tracing, caustics,
animated-object rebaking, or pixel parity with an offline renderer.

The preview budget exposes `planar_capture_limit`, `planar_resolution_limit`
and `transmission_layer_limit`: Portable 1/512/16, Balanced 1/768/24,
Cinematic 2/1024/32, Ultra 2/1536/48. Resolution caps apply per capture;
transparent layers exceeding the limit use ordinary alpha coverage. Offscreen
mirror targets are skipped. `Scene3DFrameProfile` reports capture submissions
separately from the main scene, transmission layers, and probe/local-reflection/
IBL bytes. Target bytes include the specular MRT and transmission depth snapshot.
Native/browser use the same shader and asset semantics, but compile success
alone does not certify browser pixels or performance.

Per-light depth views retain caster/light signatures between frames. Geometry,
transform, skeletal/vegetation deformation and opacity-input changes invalidate
depth evidence; resolution or array-layout changes recreate resources.
`Scene3DFrameProfile` reports `shadow_view_count`, `shadow_rendered_views`,
`shadow_cache_hits` and `per_light_shadow_bytes` alongside `shadow_map_size`.
Target-byte estimates include the local depth array. Cache hits save depth
submissions, while receiver filtering remains part of shading cost. These
fields describe implementation work and memory, not certified GPU timings.

### Complex-island visibility and retention

Large native-geometry islands (at least 64 Models) may retain their lowered
actors and draw plans. The key includes the graph revision and each evaluated
transform, unit scale and exposure. A moving curve changes the key; a settled
curve can reuse geometry while cameras and lights continue to evaluate every
frame. External meshes, rigs, Scene-backed material textures, anchors, physics
and mixed scatters keep the original preparation path. The established large
Scatter cache remains separate. The cache holds at most four pose variants.

Physical islands with at least 256 prepared items and per-light shadows or
baked diffuse lighting use a current-frame opaque coverage/depth prepass.
Only a fully covering nearer surface can reject expensive hidden shading;
alpha, coplanar ordering and uncertain/deformed bounds remain conservative.
This is fragment filtering, not a hierarchical object-occlusion system.
Off-frustum resource assembly is skipped only when all per-light shadow
signatures are valid (or no shadow rendering is required) and no planar capture
is active. Shadow refreshes and reflected views retain off-camera candidates.
Small islands do not acquire the extra prepass or retention signature.

Large raster islands also limit each ordered glass color snapshot to a proved
slab read footprint. Copies preserve the original full-resolution coordinates
and include thickness displacement, blur taps and a filtering border. SSR,
deformation, unknown bounds and near-plane uncertainty use a full snapshot.
No optical layer, light, shadow filter or output pixel is removed by this gate.
Probe receivers share their SH basis across the eight grid corners while
retaining per-probe clamping and visibility weighting.

`camera_draw_batches` counts main-camera GPU batches, while `draw_calls` still
counts prepared items before batching. `frustum_rejected_items` counts proven
empty main-camera bounds; those items may still be needed for another pass.
`raster_visibility_prepass` identifies the fragment filter and
`transmission_copy_pixels` counts refreshed glass color texels per frame.
The latter excludes the independent full opaque-depth snapshot. These counts
are not visible-object counts or frame-rate measurements. All-zero native
encoder timestamps are discarded and switch back to pass markers; portable
marker intervals cannot reliably attribute all cost to a single named stage.

There is no mandatory offline rendering step. Mips are built on texture-cache
misses. Typical square texture mip storage adds about one third; HDR and
MRT G-buffer targets add frame-sized GPU memory, and the final resolve adds one
fullscreen pass even when depth of field is disabled.
Immediate frame rate still depends on device, render size and scene complexity.

## Validation observation

A representative 1280x720 frame was captured before and after on native Metal.
Its predominantly flat toon materials have no decoded detail
textures, so the visual change is small. This is a regression comparison, not
evidence that the renderer can automatically reproduce a textured horror image.

Twelve warmed debug submissions averaged 47.2 ms before and 47.8 ms after in the
initial comparison; a final-build capture averaged 49.9 ms. These are CPU
submission timings, not GPU frame timings or
playback FPS; they do not establish browser or release performance. First-run
shader compilation also incurs startup cost and is excluded from this average.

GPU regression fixtures isolate material AO from direct light and verify that
high emission retains energy through defocus, including partial alpha coverage.
CPU mip tests cover linear-light filtering, transparent colors, normal vectors
and odd image edges. WASM compilation checks the shared code; browser runtime
performance and pixel equivalence require separate testing on target devices.

Validation on 2026-09-12 covers the shared WGSL and Rust renderer contract. The
S90 Cinematic preview is also used as a native Metal smoke test for the MRT and
temporal pipeline.

Reproduce the native GPU fixtures with:

```sh
cargo test -p motionloom --test immediate_materials -- --ignored --test-threads=1
```
