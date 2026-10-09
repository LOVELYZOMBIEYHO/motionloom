# Indoor lighting and reflections

## Approved DSL extension and compatibility contract

The indoor-lighting implementation adds two optional declarations inside a
Scene's Composite3D. Existing scenes do not need either declaration. No Graph
asset kind, material syntax, camera syntax, or timeline syntax is removed.

Before, an EnvironmentLight illuminates surfaces and glass samples a global
environment. A bathroom mirror authored with transmission behaves as glass.

After, a scene can bind a portable CPU-baked lighting asset and name an opaque
mirror Model for a reflected-camera capture:

```xml
<BakedLighting id="room_bounce" src="assets/lighting/rooms.json"
              blend="0" intensity="1" specularIntensity="1" />
<PlanarReflection id="bathroom_reflection" target="mirror"
                  resolutionScale="0.5" clipBias="0.01" />
```

`src` is resolved with the existing local asset-root resolver. The JSON schema
stores two camera-independent states (day and dusk), room-bounded irradiance
probe grids, directional depth moments, validity, and linear HDR local
reflection images. `blend` interpolates the two states in [0,1]; `intensity`
and `specularIntensity` are nonnegative animated multipliers. An intensity of
zero disables the baked contribution during a construction reveal. Missing,
malformed, or unsupported assets are typed rendering errors, not silent fake
ambient light. Asset changes invalidate their retained GPU cache.

`target` names an existing opaque, planar Model. A target cannot be used twice.
The reflected camera is clipped against the mirror plane and excludes its own
target; captures have one recursion level. `resolutionScale` is in (0,1] and
`clipBias` is nonnegative. Host preview profiles cap resolution and capture
count without changing authored scene semantics.

The normal GPU path uses SH9 diffuse irradiance, GGX environment filtering and a
split-sum BRDF LUT. A baked room replaces global diffuse IBL there, with
visibility-weighted interpolation to limit light leaking. Local reflection
probes use room box projection; planar capture handles actual mirror imagery.
Glass uses a camera-consistent slab refraction and successive background
snapshots for bounded multi-pane composition. These are real-time approximations,
not a full path-traced renderer.

Validation includes constant-environment energy, roughness filtering, bake
fingerprints excluding cameras, wall occlusion, two-state interpolation,
parser/animation compatibility, overlapping glass, reflected offscreen objects,
and native GPU shader validation. Showcase99 verification uses still images and
short consecutive-frame samples; it does not export a full video.

## Bake and verify a scene

Use `motionloom::api::lighting_bake` with an `Arc<dyn AssetResolver>`. The solver
extracts settled geometry at `day_frame` and `dusk_frame`, builds a CPU BVH,
samples material textures, and traces two to four diffuse surface bounces.
Rays include geometric occlusion, emissive surfaces, fixed analytic lights,
and environment visibility through straight thin glass sheets with Fresnel and
Beer attenuation. This solver does not depend on Weaver's glass stopgap mode.

`LightingBakeOptions` has camelCase JSON fields `sceneId`, `dayFrame`,
`duskFrame`, `volumes`, `raysPerProbe`, `specularResolution`,
`specularSamplesPerPixel`, `maxBounces`, `seed`, and `maxRayDistance`.
Each volume has `id`, `boundsMin`, `boundsMax`, `counts` (2–32 per axis),
and `reflectionPositions` (zero or one capture for schema v1).
Probe order is x fastest, then y, then z. Inset grid endpoints keep samples
away from solids; invalid probes are excluded. A small receiver shell extends
0.12 m horizontally and 0.30 m vertically to reach nearby walls, floors and
ceilings; depth moments still reject occluded interpolation.

```sh
cargo run --release --example bake_scene_lighting -- \
  room.motionloom options.json assets/lighting/rooms.json
# Validate evaluated dependencies without tracing or writing another bake.
cargo run --release --example bake_scene_lighting -- \
  room.motionloom options.json assets/lighting/rooms.json --validate
```

The public API returns a `LightingBakeBundle`; `save_lighting_bake` is the
explicit native writer. The result includes two states of SH9 irradiance E,
8×8 octahedral first/second distance moments, validity, and linear RGBE local
reflection captures. Diffuse shading applies albedo/PI. Irradiance includes
visibility-filtered sky and bounced surface light, while analytic direct lights
remain in the GPU renderer. Local HDR captures trace the room's diffuse-lit
surfaces; GGX prefiltering supplies their specular response at runtime.

The stored terminal straight-ray transmittance is diagnostic. It is not
transmittance to an arbitrary receiver: a wall farther down the ray can make
it zero even when the receiver is in front of the wall. Receiver interpolation
uses directional depth moments; thin-glass attenuation is already included in
baked radiance. This is not a distance-conditioned volumetric visibility field.

## Retention and source changes

The bake records separate geometry, material, texture-byte, light, environment
and settings fingerprints. Camera changes and 2D artwork do not enter them.
`validate_lighting_bake_fingerprint` recomputes dependencies without tracing a
new solution, and rejects changed asset bytes/settings. Use it in asset builds.
A cheap `authoring_fingerprint` also excludes cameras and overlay channels;
it retains the published source-signature lineage, including SurfaceStyle.
A style-only change can therefore invalidate this guard even when the evaluated
transport dependencies are unchanged. Existing assets are not silently assigned
a different fingerprint interpretation.

Immediate preview omits a stale bake and reports a diagnostic rather than
stopping scene rendering or applying outdated lighting. This also permits
physical/toon comparison without a mandatory rebake. With the bake omitted,
diffuse/specular illumination uses the ordinary environment fallback, and
screen-space GI can run when the selected preview profile permits it. The
comparison consequently does not retain the stale room's baked illumination.
Restore the original source or regenerate the bake to restore that contribution.
Missing JSON/HDR files, malformed data and unsupported schemas remain typed
errors; stale source does not hide these failures. Strict source loading still
rejects a changed authoring signature; explicit dependency validation still
rejects changed evaluated geometry, material, texture, light or bake settings.

`LightingStyle shadowMode="perLight"` uses canonical RGB light colors and exact
sRGB decoding, matching filmic shading. Legacy non-filmic scenes retain their
historical red/blue channel convention and gamma-2.2 decoding. The authoring
fingerprint includes the active shadow mode and emitter controls, so enabling
per-light mode invalidates an earlier bake; regenerate it from the accepted
source. See [per-light shadows](PER_LIGHT_SHADOWS.md) for this opt-in contract.
The guard uses the raw authored graph, so evaluated animation clones do not
invalidate it every frame. Cached JSON and HDR file changes invalidate retained
CPU/GPU data; moving a camera or changing blend does not decode the assets again.

At strengths 0–1 the surface fades from global IBL to the room result; values
above one amplify room lighting. Day/dusk share deterministic ray directions
and blend linearly without camera-dependent sample changes. Disable the bake
until a construction reveal has reached the settled geometry used for baking.

## Approximation and budgets

The solver handles diffuse transport, not glossy multi-surface interreflection,
caustics, participating media or automatic dynamic-object relighting. Room
probes approximate parallax with a box; a planar mirror uses its actual reflected
camera instead. A rigid planar/thin target is required; nonplanar targets fail.
The mirror excludes itself, clips geometry behind its plane, captures bounded
alpha/glass layers, and uses one recursion level. Multiple mirror recursion is
not supported.

| Profile | Visible mirror captures | Capture dimension cap | Transmission layers |
| --- | ---: | ---: | ---: |
| Portable | 1 | 512 | 16 |
| Balanced | 1 | 768 | 24 |
| Cinematic | 2 | 1024 | 32 |
| Ultra | 2 | 1536 | 48 |

Glass uses projected Snell slab displacement with opaque-depth foreground and
offscreen checks. Parallel slab exit direction matches the incident direction;
absorption uses authored thickness and is applied once per pane. Authored solid
glass also follows this Preview approximation and consumes the same snapshot
budget; no actual mesh exit or interior distance is traced. Per-pane snapshots
compose depth-sorted layers without reading a render attachment while writing it. Intersecting
transparent meshes and arbitrary concave refractive solids remain approximate.
Excess layers fall back to ordinary alpha rather than reusing a stale snapshot.
Uncovered snapshot pixels use the HDR environment for transmission even when
the camera background is hidden. Previously composed panes retain their alpha
coverage. Planar sky radiance follows environment intensity/specular energy;
the visible camera sky keeps its separately authored background controls.

SSR uses an additional indirect-specular MRT and replaces that lobe only. Glass
and exact planar pixels reject the opaque-depth SSR path. Baked diffuse replaces
global diffuse IBL within valid room coverage and suppresses extra SSGI. The
renderer reports main draw calls and capture draw calls separately, plus probe,
local reflection, IBL and render-target memory estimates.
`planar_capture_count` counts retained capture targets; a target may remain
allocated across camera cuts while `planar_capture_draw_calls` is zero when its
mirror is outside the current frustum.

## Validation commands

```sh
cargo test --lib
cargo test --test indoor_lighting
cargo test --lib world::render::tests::glass_planar -- --ignored --test-threads=1
cargo check --lib --target wasm32-unknown-unknown
```

The native GPU fixtures verify camera/world rotation invariance, absorption
through two closed panes, authoring-order independence, and a mirror reflecting
an object behind the main camera. A camera-hidden environment remains visible
through glass while the uncovered camera background stays hidden. CPU fixtures verify directional irradiance,
GGX filtering/energy, colored second bounce, wall occlusion, thin-sheet losses,
HDR retention, invalid data/work limits, deterministic dependencies, and stale
source rejection. Preview-loader tests additionally verify stale-source fallback,
restored-source cache reuse and rejection of missing/malformed assets even when
the source is stale. Dependency tests separate a physical/toon style change
from actual material changes without altering published source hashes.
Browser compilation is distinct from a browser GPU run.
