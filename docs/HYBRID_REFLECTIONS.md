# Hybrid reflections and closed glass

The default native and WASM immediate renderer uses raster PBR, bounded
screen-space reflection/refraction, local baked probes, environment IBL, and
specialized planar captures. It does not build or traverse a full-scene reflection
BVH. Physical shading describes material appearance, not a path-tracing request.
`LightingStyle` remains optional; no `fast` shading mode or `reflectionMode`
selector is introduced.

Existing authoring properties remain accepted:

| Attribute | Accepted values | Default | Immediate interpretation |
| --- | --- | --- | --- |
| `LightingStyle.reflectionBounces` | integer `1` or `2` | `1` | Reflection intent; repeated paths are approximated by screen/probe evidence, not recursive geometry queries |
| `MaterialAsset.refractionMode` | `slab` or `solid` | `slab` | Authored optical model; Preview uses slab displacement and absorption with authored `thickness`, without mesh-exit queries |

The frame profile reports `geometry_transport_enabled`,
`recursive_reflection_approximated`, `solid_refraction_approximated`, and
`baked_lighting_fallback`. Native hosts print a capability warning once per
renderer lifetime. These approximations do not change the parsed materials or
scene. Weaver independently traces slab/solid optics; its offline bounce limits
remain RenderJob `light_paths` settings. `reflectionBounces` is not silently
mapped into that different offline budget.

For native A/B investigations only, setting
`MOTIONLOOM_REFERENCE_GEOMETRY_TRANSPORT=1` restores the former geometry-backed
implementation described below. This is an internal reference/diagnostic path,
not a DSL quality selector, a browser feature, or the default export renderer.
Weaver remains the separate high-quality offline renderer.

In normal Preview, offscreen reflection detail comes from retained environment
or baked room probes, or an explicitly authored `PlanarReflection`. There are no
automatic dynamic probe captures or automatic planar-face selection in this
path. Screen-space evidence cannot reconstruct hidden objects, and a solid's
true interior path, total internal reflection and nested-media transitions are
not solved by its Preview approximation. A zero authored thickness gives no slab
displacement or distance-based absorption; it does not estimate the mesh volume.

SSR-enabled profiles also trace a bounded inline reflection from sharp glass
(roughness below 0.5) using its actual interface position and mapped normal.
This uses the current underlay snapshot, at most 24 steps plus four refinements,
and no additional capture or target. A miss retains the RGB room/environment
response; direct light, transmission and distinct coating/sheen lobes remain
intact. Snapshot depth describes opaque geometry, so several transparent panes
still provide an approximate path. Glass SSR is disabled within planar captures.

```xml
<RenderStyle id="glass_studio">
  <SurfaceStyle shading="physical" />
  <!-- Omit this child when one reflection is sufficient. -->
  <LightingStyle reflectionBounces="2" />
</RenderStyle>

<MaterialAsset id="window" baseColor="#FFFFFF" roughness="0.06"
  transmission="0.96" ior="1.52" thickness="0.008"
  attenuationColor="#DCEBE7" attenuationDistance="3"
  refractionMode="slab" depthWrite="auto" doubleSided="true" />

<MaterialAsset id="glass_sphere" baseColor="#FFFFFF" roughness="0.06"
  transmission="0.96" ior="1.52"
  attenuationColor="#DCEBE7" attenuationDistance="1.5"
  refractionMode="solid" depthWrite="auto" doubleSided="true" />
```

RenderStyle is declared under Graph and referenced by Scene. MaterialAsset
declarations belong under Assets; the fragment above shows their controls,
not a complete Graph. The self-contained
[hybrid review scene](../examples/hybrid_reflections.motionloom) contains slab
and solid spheres, a coated surface, metal panels, a checker background and a
red emissive panel behind the main camera. It requires no external assets.

## Geometry transport reference implementation

The sections from here through "Reference scene updates and platform boundaries"
describe only the native diagnostic path enabled by the environment variable
above. Their BVH, recursive-ray, automatic-capture and reflection-history costs
are absent from normal native and WASM Preview.

The main scene remains rasterized. Geometry reflection rays can intersect
objects outside the camera view. Screen-space evidence remains a fallback
when a base-lobe geometry ray has no confident hit. Opaque and coating reflection lobes retain
their own roughness and energy response; transparent reflection uses its
surface, rather than the opaque object seen through it. Existing planar
captures remain available as a specialized reflected-camera path.
Valid front-facing planar evidence replaces both reflection lobes before
geometry rays are evaluated. The reflected-camera capture spends the first
authored reflection bounce; geometry transport inside that capture uses the
remaining bounce budget.

Geometry-backed secondary hits evaluate direct lighting and available
environment or baked indirect light. Their base-color, emissive and
metallic/roughness maps use the authored UV transform and channel remapping.
Secondary maps are packed into a shared buffer; their longest edge is capped
at 256 pixels using linear-light area downsampling. This limits memory and
upload cost, and deliberately loses fine reflected texture detail.

Primary colored glass shadows query only transmissive shadow casters; opaque
occlusion remains in the authored shadow maps. A separate compact caster BVH
remaps its triangles into the complete geometry buffer, so opaque walls do not
consume glass traversal or the twelve interface iterations. Visibility evaluates
alpha coverage and boundary optics without sampling unrelated color,
metallic/roughness or emission maps. An unchanged scene reuses both trees; changed
scenes update the complete geometry and rebuild the compact caster tree.

Each primary area-light sample traces the exact finite segment to its sampled
emitter position. A fully opaque-occluded sample contributes no direct energy
and skips the colored-glass query. For unwrapped physical shading, visibility
also skips lights behind every active base and clearcoat lobe.

Secondary hits reuse a retained opaque shadow map only when conservative
coverage checks prove that its clipped intervals cannot omit an opaque caster.
Every nontransmissive shadow caster must belong to the opaque render phase.
Directional views fit their depth interval to the evaluated opaque-caster bounds;
each receiver and its complete shadow-filter footprint must remain inside the
selected view. A local-light face additionally requires a BVH check proving
that its clipped near pyramid contains no opaque caster. These checks use the
evaluated scene, including deformation, and the selected area-emitter sample.
Certified maps provide opaque visibility, while the compact transmissive-caster
BVH supplies colored glass attenuation. An uncertified face or receiver falls
back to full-geometry visibility. That query includes opaque casters and can
stop at any valid opaque blocker before the emitter, even when nearer glass
has already been found. Secondary hits retain their own shadow-receiving flag
and apply the selected light's shadow strength once.

Reflection rays retain all evaluated geometry regardless of an object's
shadow-casting flag. Colored shadow rays remain straight and do not focus or
bend illumination.

Recursive specular paths are bounded by `reflectionBounces`. Broad rough
reflections retain environment or room-probe support: geometry confidence
fades across roughness 0.5 through 0.75. Above roughness 0.12, the renderer uses
a bounded deterministic cone of two rays on Balanced and four rays on
Cinematic. Roughness at least 0.75 retains probe support. This is a bounded hybrid renderer,
not unrestricted path tracing or a full stochastic rough-surface integral.

Static shader selection removes unreachable transport code. A slab-only
specialization is selected when analysis of all evaluated draws proves that
solid transport is unnecessary; off-camera solids retain solid transport.
Opaque main-view batches can use a smaller reflection shader when every instance is provably
outside geometry reflection's active range and has no clearcoat. The proof
uses retained texture-channel extrema, authored channel remapping and inversion,
roughness factors and the lighting roughness bias; uncertain bounds keep the
complete shader. Clearcoat remains an independently evaluated reflection lobe.
These selections preserve the existing material samples, full-resolution
rendering, optical behavior and reflection budgets, and introduce no DSL setting.

When all evaluated transmissive draws are proven to cast no primary glass
shadows, the primary surface shader also removes that unreachable traversal.
Missing per-light flags remain conservative, and off-camera shadow casters
participate in this proof. The same proof removes transmissive interfaces from
reflection-hit shadow visibility, which retains its opaque occlusion query and
certified shadow-map reuse. Non-shadow-casting glass still participates in
reflection and refraction transport.

Native reference rendering automatically contracts the physical style and
ray-hit alpha cutoff paths together when the exact evaluated style is physical and every
evaluated draw, including off-camera geometry, has
unit packed material/actor alpha, unit vertex alpha, a valid fully opaque base
texture, and a finite cutoff no greater than 0.9999. The cutoff reserve covers
floating-point interpolation near triangle edges. Any uncertain member restores
both complete shader paths. Shaded surface alpha, texture color, transmission,
finite shadow queries and solid-interface ownership remain unchanged.
The physical contraction retains material controls, coating, sheen, lighting
and transport. Renderer reuse includes both selections, so changing coverage or
style restores the required shader and resets retained history. Normal native
and WASM rendering uses the separate realtime shader instead. Native diagnostics
`MOTIONLOOM_TRACE_COMPLETE_COVERAGE_QUERY`
and `MOTIONLOOM_TRACE_PHYSICAL_SHADER` accept exact `0`/`1` values to disable or
independently request each contraction; a request still requires its proof.
Other values follow the automatic policy. These are internal comparison controls,
not DSL options. `MOTIONLOOM_TRACE_BATCHES=1` reports the actual shader selections.

Reflected and transmitted radiance stays in linear HDR through composition;
display tone mapping happens at the final resolve. A material's alpha coverage
and physical transmission remain separate concepts. Prefer transmission for
glass and keep `depthWrite="auto"` for ordinary glass scenes.
Display-locked colors and host 2D canvases cancel invertible scene grading before
this resolve. Filmic ACES inversion remains approximate; clipped colors and
zero exposure, contrast or saturation cannot be recovered exactly by inversion.

## Reference rough reflection evidence

Large profiled scenes can trace broad opaque base and clearcoat incident
radiance on a smaller raster grid, then reconstruct that radiance during the
full-resolution surface pass. This is a spatial approximation of the incoming
reflection light. Primary material textures, normals, direct lighting, AO,
BRDF response, layer energy, alpha coverage and final output resolution remain
evaluated at full resolution. It does not remove off-camera geometry or reduce
the authored reflection bounce count.

The grid activates only with at least 32,768 transport triangles, more than
16,384 output pixels and fewer than 65,535 transport objects. Dimensions round
up to cover odd output sizes:

| Immediate preview profile | Evidence width and height | At 1920 × 1080 |
| --- | --- | --- |
| Portable | Each output dimension divided by eight | 240 × 135 |
| Balanced | Each output dimension divided by eight | 240 × 135 |
| Cinematic | Each output dimension divided by four | 480 × 270 |
| Ultra | No evidence grid; existing per-pixel transport | Full-resolution queries |

The direct World path without a host preview profile also retains per-pixel
transport. The grid requires device limits of at least twenty-one sampled
textures per shader stage, four bind groups, five color attachments and
thirty-two color attachment bytes per sample. Devices that do not meet every limit retain the
existing per-pixel path without constructing the evidence and routing pipelines.

Only fully covered, nontransmissive, physically shaded primary surfaces can
reuse grid evidence. An eligible lobe must have roughness strictly between
0.12 and 0.75. The base lobe uses its mapped shading normal and sampled
roughness. Clearcoat uses its own geometric-normal reflection direction and
authored roughness. Several imported material chunks can share one transport
object ID, so clearcoat reuse additionally requires proof that every chunk of
that object has positive finite clearcoat and exactly the same finite coating
roughness. Uncertain objects retain the complete coating query. Glass, sharp
lobes without compatible planar evidence and reflected-camera captures keep
their existing transport. Valid planar projection evidence still owns both
lobes before geometry queries.

The evidence pass retains the full camera projection and uses explicitly
scaled texture gradients to preserve primary material mip selection. All
opaque depth-writing occluders participate, including surfaces ineligible for
reuse; independent grid depth and the full-resolution nearest-depth/coverage
prepass reject hidden samples. Each grid fragment traces the original bounded
cone against the complete scene BVH, including geometry outside the main
camera. Base and coating queries within that invocation share the existing
4,096-node budget. Grid fragments and full-resolution shading fragments have
independent budgets; this is not one shared allowance for a complete output
pixel or frame.

Reconstruction examines a 3 × 3 grid neighborhood and requires the exact
transport object ID, compatible geometric normals and the same local world
plane. Base-lobe samples additionally require compatible mapped normals and
roughness within 0.03. Coating samples use the certified coating roughness
instead of the base roughness. Camera-relative positions and packed normals
retain bounded precision; finite-range checks reject unsupported positions.
Accepted samples blend incident HDR radiance by confidence, with a packed
reflection-hit distance retained for temporal rejection. A compatible sample
with zero geometry confidence retains the existing environment/probe fallback.
If no compatible sample exists, the full-resolution fragment performs its
original geometry query. Thin objects, normal changes and depth boundaries
therefore retain a fallback instead of borrowing a neighboring object's light.
Incident radiance outside finite half-float range invalidates the sample;
lookup rejects nonfinite retained values as well. Without finite matching
evidence, pixels use the full query instead of clamping incoming light. A weak
material response can produce finite output even when its incident radiance
exceeds the evidence texture's range, so clamping that radiance would incorrectly
remove reflected energy.

A separate full-resolution `R8Uint` texture records whether each output pixel
can use the cached surface shader. This is the sixth texture in evidence bind
group 3, separate from the five grid color attachments. The classifier uses
the primary material derivatives, channel remapping and normal construction,
but performs no lighting or BVH queries. Its strict `Greater` depth test records
the first nearest winner in original draw order, including coplanar triangles
and instances. A fully covered winner can select the cached route only when
every active lobe has valid evidence or needs no geometry query. Fractional
coverage and uncertain evidence select the full route.

For each opaque draw requiring geometry reflection in an active grid frame,
the renderer submits its cached and full variants together before advancing
to the next draw. Their complementary route
tests preserve the original depth and blending sequence, including coplanar
and fractional contributions. The cached shader module replaces only primary
base/coating geometry-query calls with nontracing stubs. It still uses
cached incident radiance with the full-resolution BRDF and retains all material,
direct-light and colored-shadow behavior. The full shader keeps original query
fallbacks. Ordinary glass and reflected-camera modules stub evidence/route
reads and retain their established three-bind-group layouts; the classifier
binds a distinct default route texture while writing the active route target
to avoid attachment feedback.

The grid is regenerated for the current frame, independently of reflection
history. Spatial reconstruction can smooth changes in incident radiance and
uses approximate positions, normals and hit distances; it is not pixel-identical
to Ultra. The allocation costs 36 bytes per grid pixel, one byte per
full-resolution output pixel for routing, and 37 bytes for valid inactive
default textures. `Scene3DFrameProfile.rough_reflection_size` reports
the active grid dimensions or `None`, and `rough_reflection_bytes` reports these
allocations separately from reflection-history memory. These are renderer and
host-profile controls, with no new DSL tag or attribute.

## Reference automatic planar face captures

Portable, Balanced and Cinematic can use remaining profile capture slots for
large, sharp planar faces without an authored `PlanarReflection`. This activates
under the same large-scene triangle/output thresholds as the rough grid.
Visible authored captures take priority; automatic targets fill only the slots
left afterward. Ultra and the direct World path without a host preview profile
retain the original geometry-query path instead of adding automatic captures.

Candidates must be visible rigid opaque depth-writing surfaces belonging to an
actor with exactly one draw. Alpha coverage, transmission, clearcoat, unlit
materials, wind and skin deformation prevent automatic selection. Conservative
channel bounds must permit roughness at or below 0.12 and prove that sampled
normal maps deviate by less than 0.3 degrees. The fitted visible face must have
coplanar positions and compatible vertex normals, and every part of the target
must stay behind that plane so excluding the target actor cannot remove a
foreground reflection. Beveled edges can remain on the actor but do not count
toward the certified flat face.

Materials without a normal texture use their geometric normal exactly,
including when a scalar normal strength is authored. The neutral RGBA8 fallback
texel cannot encode a zero tangent-space offset exactly, so its strength is
disabled for missing maps. Actual normal textures retain their authored strength.

The projected certified face must cover at least the larger of 4,096 pixels
and 0.5% of the output. Selection ranks projected coverage multiplied by a
Schlick-style grazing-angle reflection weight, allowing a strongly reflective
horizontal slab to outrank a larger face with weak visible reflection.
Automatic resources are retained only while their targets remain selected;
authored resources retain their existing lifetime.

Automatic faces with a proven common plane, reflected camera and canvas can
share one capture slot. Every member must pass the same material and geometry
proofs, and all of their geometry must stay behind the canonical plane. The
capture excludes every member and covers their combined lookup regions.
Parallel offset planes and authored targets remain separate captures.

Each automatic reflected-camera capture uses one quarter of each output
dimension, limited further by the current profile and a longest edge of
512 pixels. This is a bounded spatial approximation for a sharp base lobe,
not a full-resolution mirror render. At each full-resolution shading point,
reuse additionally requires roughness at or below 0.12, no transmission or
coating, normal-map deviation below 0.1 degrees, a matching geometric normal,
and position on the certified face plane. The plane tolerance is one millionth
of the object's largest world span, with a minimum of 0.000005 world units.
Rough texels, bevels, displaced normals and failed projections retain geometry
queries. Accepted capture radiance receives the same base BRDF response as the
existing planar path. The primary reflected-camera view spends the first
authored bounce and retains the remaining budget for transport inside that
capture. Explicit `PlanarReflection` controls retain their existing behavior;
automatic selection introduces no DSL setting.

### Native investigation paths

`MOTIONLOOM_TRACE_PLANAR_GLASS=1` additionally investigates fully covered,
rigid, sharp slab-glass faces in automatic capture selection. It is disabled by
default and on WASM. Solids, coating, alpha masks, uncertain normals and
deformation retain their established transport. Glass groups spend the same
profile capture slots; their experimental ranking estimates saved reflection
queries rather than weighting a weak normal-incidence Fresnel response down.
Quarter-size capture radiance changes fine reflected detail and is an
approximation. Slab refraction, per-pane absorption, underlay snapshots and
authored bounce budgets retain their existing implementation.

When complete-scene proofs rule out solids and casting-glass shadows, selected
automatic slab draws can split their original triangle order into certified
and ordinary runs. Certification requires rigid identity transforms, constant
axis-aligned frames, bounds on maximum mapped roughness and normal deviation,
plane error reserves and strictly interior reflected projection bounds. Only
certified front triangles use the light module, which has no reachable BVH
query. Uncertain triangles retain ordinary full shading. Proven closed-slab
back faces skip main-view rasterization but remain in the optical BVH.
All runs retain one pre-pane snapshot and original primitive order; multi-chunk
draws and candidates without certified front triangles retain the ordinary
path. `Scene3DFrameProfile.planar_slab_cached_triangles` and
`planar_slab_discarded_triangles` count unique main-view triangles, while
`planar_slab_extra_draw_calls` reports additional submitted runs across tiles.
These paths require native image and timing validation before default activation.

`MOTIONLOOM_TRACE_COMPACT_REFLECTION_QUERIES=1` investigates a GPU work list for
full-resolution opaque reflection fallbacks. It is also disabled by default,
on WASM, and when solids or casting-glass shadow transport is required. Queries
retain unique draw ownership and completion checks before reuse; invalid or
uncompleted records retain the original shader. It adds approximately 84 bytes
per output pixel plus a four-byte counter (174,182,404 bytes at 1920 × 1080),
excluding dispatch uniforms. Positions and incoming radiance use float32;
normal directions are quantized to oct16. The current measured gain does not
justify promoting that memory and direction approximation to the default path.

## Reference slabs, solids and nested media

`slab` uses `thickness` as an optical approximation for thin windows and sheets.
It does not require a closed mesh. `solid` traces entry and exit intersections,
uses the exit normal for Snell refraction, evaluates dielectric Fresnel and
total internal reflection, and attenuates light by the actual distance inside
the material. Changing `thickness` does not change a solid's optical path;
changing its geometry or scale does.

Profile limits on underlay snapshots apply only to slabs. Both the main camera
and planar captures retain solid geometry transport after that slab limit is
exhausted; a solid is not downgraded to ordinary alpha blending.

Solid meshes must be closed, consistently wound manifolds with outward
orientation after coincident vertex positions are welded for inspection.
Split vertices at UV or normal seams are allowed. Open boundaries,
nonmanifold edges, inconsistent orientation and invalid volume are diagnosed
before accepting solid transport. A hollow bottle needs coherent inner and
outer walls; marking an open surface solid cannot generate its missing wall.
Multi-material meshes need each traced volume to meet the closure contract.

The native reference path tracks up to four overlapping or nested media and performs
at most twelve interface iterations. BVH traversal visits at most 2,048 nodes
per ray. All shadow, solid-ownership, reflection and transmission queries in one
fragment invocation also share a total budget of 4,096 node visits. This total
does not restart between lobes, lights, samples, bounces or interface crossings,
and applies to planar captures as well as the main view. Missing exits, exhausted
geometry budgets and medium-stack overflow
produce zero geometry confidence rather than pretending that the object is a
slab. Environment/probe fallback remains bounded approximate evidence.
Shadow visibility has a separate bounded result: exhausted interface, medium
or BVH traversal budgets conservatively block the sample rather than leaking
light through untested boundaries.
These implementation budgets are renderer controls, not additional DSL tags.
Initial camera placement inside one solid is supported; the full stack is not
initialized when the camera starts inside several nested media. Solid geometry
must retain outward winding after its transform; a nonpositive uniform scale
is not a supported solid-volume transform.

GLB geometry export preserves a solid material through
`materials[].extras.motionloom.refractionMode="solid"`; import validates
`slab` or `solid` explicitly. Ordinary GLB materials default to slab. This
MotionLoom extra is separate from standard glTF transmission/volume factors.

## Reference scene updates and platform boundaries

The BVH contains evaluated geometry, including objects outside the main camera.
Rigid motion refits retained topology; a topology change rebuilds it. Evaluated
vertex caches include skin and vegetation deformation so secondary geometry
follows the same sampled pose as the main view. Unchanged geometry and material
signatures reuse retained GPU resources rather than uploading every frame.

Both geometry and transmissive-caster trees use a sixteen-bin surface-area
heuristic to group triangles. Construction bounds tree depth to thirty levels.
Closest-hit traversal visits the nearer child first and prunes branches beyond
the current hit distance. Parent and right-child indices accompany the existing
escape and query-mask metadata; two ancestor bitmasks track pending siblings
without a per-fragment array stack. Finite shadow queries retain the complete
emitter interval so a nearer glass hit cannot hide a farther opaque blocker.
The per-ray and shared fragment node-visit budgets remain unchanged.

The reference implementation uses storage buffers and software traversal through
Rust/WGSL. It does not depend on hardware ray-tracing extensions, and is selected
only on native builds. WASM keeps the same authored materials but always uses
the realtime approximation. Native verification does not establish browser
pixel parity or performance.
The reference scene has a one-million-triangle transport budget and must fit
the device's storage-buffer limit. Exceeding either reports an explicit error;
objects are not silently removed from reflection rays. `Scene3DFrameProfile`
reports transport bytes, triangle/node/object counts, prepare time, cache hits,
refits, reflection-history bytes and rough-reflection grid dimensions/bytes
separately from playback FPS.

Large immediate scenes split expensive surface work into bounded scissor
tiles while retaining the full-resolution viewport, coordinates, derivatives,
depth and authored draw order. This activates from 32,768 transport triangles
when the output exceeds 16,384 pixels. Cached rigid local bounds are transformed
and conservatively projected into screen bounds, including raster and jitter
padding. A draw is replayed only in intersecting tiles, and empty opaque tiles
are skipped. Skinned or wind-deformed geometry, near-plane crossings and
uncertain transforms retain all tiles. This screen-bound selection affects
raster replay only; off-camera geometry remains in the transport BVH and shadow
passes. Each glass pane takes one underlay snapshot before its intersecting tile
sequence; opaque reflection history resolves after all opaque tiles. Up to
sixteen bounded command buffers share one queue submission, preserving queue
order without an explicit device poll at each tile. These controls limit
individual command work and amortize submission overhead; they do not establish
real-time playback or equivalence with Blender's renderer.

Native reference slab-only scenes without primary glass-shadow casters use
512-pixel main-view tiles to reduce repeated vertex processing. Reference solid
scenes and primary glass-shadow scenes retain 128-pixel tiles. Reflected-camera
captures and the rough evidence grid keep their own 128-pixel bounds. Native
Scene queue admission occurs before embedded World rendering starts, so an
already full three-frame queue applies backpressure before another frame's
3D command buffers are submitted.

The main view first records nearest opaque depth and alpha coverage using the
same vertex deformation, clipping and alpha cutoff as surface shading. Fully
covered nearer surfaces reject hidden fragments before expensive transport;
fractional coverage keeps the original blending behavior. This prepass reuses
the independent transmission depth/coverage textures. Its coverage is replaced
by each slab's HDR underlay only after opaque shading has finished.

Planar captures use the same nearest-depth and coverage prepass in their own
reflected camera, with conservative reflected-view bounds. Hidden reflected
geometry is rejected before transport, and fractional coverage preserves the
original composition. Their snapshot depth and coverage remain independent
of the active shading attachments and are reused for ordered glass only after
opaque capture shading has finished. Large reflected views use the same
128-pixel scissor tiles and conservative reflected-view draw bounds; all tiles
retain the complete reflected viewport and material derivatives. A slab takes
one complete underlay snapshot before its intersecting tiles, and every tile
of that pane finishes before the next pane snapshots. Planar work joins the
same batches of up to sixteen bounded command buffers per queue submission;
solid transport remains independent of the slab snapshot limit.

Opaque indirect specular radiance has a separate HDR history pass before
transparent/volume composition, depth of field and display grading, independent
of display TAA. It reprojects
physical velocity with the previous/current raster jitter, and rejects history
after camera cuts, seeks or transport revisions. Local rejection checks
surface depth, normals, roughness and encoded reflection-hit distance; current
neighbors clamp the retained radiance. Only the indirect specular delta changes
the HDR color, preserving direct-light energy and alpha coverage. Three
RGBA16Float targets are double-buffered, costing 48 bytes per rendered pixel.

Transparent glass does not currently write an independent reflection lobe to
this history buffer. Its pixels reject ordinary display TAA history because the
opaque G-buffer behind the glass cannot identify the glass surface. Animated
glass also records a previous-coverage marker so newly uncovered opaque pixels
cannot retain glass color through matching underlay depth/normal. This marker
costs eight additional bytes per pixel while display TAA is enabled. Animated
disocclusions, rapidly changing glossy highlights and thin glass
remain important temporal-review cases; a still image is insufficient evidence
of stable video behavior.

## Weaver output

Weaver uses its path tracer rather than Preview's raster approximations or the
native reference's screen/BVH blend. Its
`RenderJob::light_paths` limits total, glossy and transmission path depth;
the DSL `reflectionBounces` limit does not cap offline paths at one or two.

The material path supports dielectric reflection/transmission, exact Fresnel,
total internal reflection and Beer attenuation. Solid paths follow actual
mesh exits, with a bounded stack of eight mesh-identified media. Slab paths
use paired parallel interfaces, their absorption/internal-reflection series,
and a thickness-dependent lateral offset. Slab roughness affects reflection;
independent rough entry/exit scattering is not represented by this slab model.
GGX clearcoat and Charlie/Neubelt sheen retain layer energy accounting.

Solid closure diagnostics also apply to offline output. Initial camera
placement inside one solid is supported; starting inside several nested media
does not yet initialize the full medium stack. Shadow rays use straight-ray
colored transmission; this is not a bent-light caustic solver. The explicit
job transmission stopgap remains a deliberately opaque fallback when selected.

## Current limits and verification

- No focused caustics or unrestricted mirror recursion.
- Normal Preview uses bounded SSR with probe/environment fallback for opaque
  reflections, and screen-space slabs for glass. Offscreen detail, curved-solid
  optics, repeated mirror paths and intersecting transparent meshes remain
  approximate. Layer budgets apply to glass, including authored solids.
- Normal Preview retains primary normal maps, sheen and clearcoat. SSR replaces
  its compatible base lobe; distinct coating and sheen responses retain their
  environment/probe support.
- The native geometry reference does not fully evaluate secondary-hit normal
  maps and sheen. Its rough surfaces and exhausted rays use environment/probes.
  Its spatial reconstruction, automatic sharp-face captures, bounded solid
  rays and reflection history have the specific limits described above; they
  are not normal Preview features or frame-rate guarantees.
- Weaver's physical GGX transmission has a separate sampling budget.
- Froxel volume composition suppresses the later SSR replacement, avoiding a
  pre-volume lobe subtraction. Environment/probe reflections remain available;
  geometry reflections are retained only in the native reference path.
- Model closure, wall thickness, normals and UV quality still determine the
  result; a rendering algorithm cannot supply missing object geometry.
- A native GPU check and a real browser run are distinct verification stages.
  Compilation alone proves neither visual quality nor sustained frame rate.

Focused source/schema tests cover omitted defaults, strict values, serialized
graphs, GLB extras and material cache invalidation. Native realtime image
fixtures in `src/world/render/tests/physical_realtime.rs` exercise PBR and glass
with zero BVH preparation, approximation diagnostics and authored AA. Native
geometry-reference image fixtures in
`src/world/render/tests/hybrid_glass.rs` cover behind-camera reflections,
solid-versus-slab checker distortion, actual-path absorption, intersection order
and a two-mirror path. They are ignored by ordinary CPU CI because they require
a native GPU adapter:

```sh
cargo test -p motionloom --lib physical_realtime -- --ignored --test-threads=1
cargo test -p motionloom --lib hybrid_ -- --ignored --test-threads=1
cargo test -p motionloom --lib reflection_history_gpu_sequence -- --ignored --test-threads=1
cargo test -p motionloom --lib secondary_shadows:: -- --ignored --test-threads=1
cargo test -p motionloom --lib rough_reflections:: -- --ignored --test-threads=1
cargo test -p motionloom --test geometry_export glb_roundtrip_preserves_solid_refraction
```

The native reference fixtures in `src/world/render/tests/secondary_shadows.rs`
additionally cover a reflected receiver's own shadow flag, a nontransmissive blended caster
that cannot use opaque-map certification, and an opaque caster inside a local
light's clipped near region. CPU tests cover conservative tile bounds,
depth-bounded closest-hit traversal, shadow-coverage certification and texture
channel bounds used for static shader selection.

The native reference fixtures in `src/world/render/tests/rough_reflections.rs`
compare Cinematic evidence reconstruction with Ultra per-pixel transport. They cover
offscreen reflection energy, a coating without an active geometry base lobe,
distinct coplanar object colors, and thin/depth/normal boundaries. CPU contracts
cover profile grid dimensions, device capability guards, allocation format
budgets and coating compatibility across shared-object material chunks. Further
native fixtures compare high incident HDR radiance and both authored orders of
coplanar cached/full surfaces, including fractional coverage and normal-map
fallback. Their CPU fixture contracts ensure those layers remain in the opaque
depth-writing phase so the ordering case is actually exercised.

Naga validation covers the realtime shader without full-scene BVH bindings and
the reference's ordinary, evidence, classification and cached/full
surface module variants. Reachability contracts require the cached surface
to omit primary reflection queries while retaining required shadow transport;
the classifier cannot reach lighting/BVH queries or read its own route
attachment. Automatic-capture CPU contracts cover visible transformed faces,
bounded resolution, remapped roughness and normal bounds, and rejection of
small, hidden, deformed or incompatible surfaces. These contracts describe
test coverage. Generated native and browser validation reports are retained
locally under `evidence/` and are not distributed with the source. Browser
runtime validation remains separate. Readback wall time is not GPU time or playback FPS.
