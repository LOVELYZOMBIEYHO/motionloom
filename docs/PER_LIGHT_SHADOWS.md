# Per-light shadows

The immediate GPU renderer can compute independent visibility for every
supported authored light. Enable it with `LightingStyle shadowMode="perLight"`.
Each visibility term multiplies only that emitter's direct radiance; environment
IBL, baked diffuse bounce, emission and other lights retain their own energy.
This is an opt-in extension using existing tags. Omission or `"legacy"` keeps
the previous selected-owner shadow behavior.

Per-light mode also decodes authored light colors in canonical RGB order using
the exact sRGB transfer, matching `filmic_physical_v1` and `pbr_npr_soft_v1`.
For example, `#FF0000` emits red. Legacy non-filmic lighting retains its
historical reversed red/blue convention and gamma-2.2 conversion for compatible
old scene output. Review colored fixtures when opting such a scene into
per-light mode. The bake source fingerprint includes the active shadow mode,
so an old bake becomes stale when this color/lighting contract changes; rebake
the accepted scene. See [indoor lighting](BAKED_LIGHTING.md).

```xml
<RenderStyle id="room_physical">
  <SurfaceStyle shading="filmic_physical_v1" />
  <LightingStyle shadowMode="perLight" shadowStyle="soft" />
</RenderStyle>
<!-- Inside the Scene's existing CompositeGroup space="3d": -->
<DirectionalLight id="sun" direction={[-0.4,-1,-0.3]} intensity="3"
  castShadow="true" shadowStrength="1" angularDiameter="0.5" />
<PointLight id="ceiling_bulb" position={[0,2.8,0]} intensity="10" range="8"
  castShadow="true" sourceRadius="0.025" />
<SpotLight id="reading_lamp" position={[1,2,0]} direction={[-0.3,-1,0]}
  innerCone="25" outerCone="35" intensity="6" range="5"
  castShadow="true" sourceRadius="0.025" />
<RectAreaLight id="window_softbox" position={[-2,2,0]} direction={[1,-0.4,0]}
  width="1.2" height="0.8" intensity="8" castShadow="true" />
```

Reference the style with `Scene renderStyle="room_physical"`. `angularDiameter`
is the **full diameter in degrees**; `0.5` is a sun-sized example.
`sourceRadius` is a **radius in scene units**, normally metres in room scenes;
`0.025` describes a 25 mm source radius. It controls shadow softness without
changing a point or spot light's analytic attenuation. Area lights use their
existing `width` and `height` for emitter sampling.

| Existing tag / attribute | Accepted values | Omitted default |
| --- | --- | --- |
| `LightingStyle.shadowMode` | `legacy`, `perLight`, case-sensitive | `legacy` behavior |
| `DirectionalLight.angularDiameter` | finite literal 0–90 degrees | 0 |
| `PointLight.sourceRadius` | finite literal 0–1000 scene units | 0 |
| `SpotLight.sourceRadius` | finite literal 0–1000 scene units | 0 |
| `RectAreaLight.castShadow` | literal `true` or `false` | false |

Directional `castShadow` already defaults to true; point and spot
`castShadow` default to false. Author those flags explicitly for room fixtures.
Existing `Model castShadow` and `receiveShadow` flags default to true and control
caster participation and receiver visibility independently.
Directional `shadowStrength` retains its existing default of 0.8 and runtime
0–1 clamp. Per-light point, spot and area visibility uses full occlusion; these
tags do not gain a `shadowStrength` attribute. New source-size controls are
static literals, not expressions or `AnimationTarget` channels. Existing light
position, direction, intensity and supported numeric channels remain animatable.
RenderStyle parameters are static resources; a Scene can switch style references.

## Depth views and preview budgets

The primary selected directional light uses the existing fitted directional
shadow map. Each additional directional or spot light uses one depth view.
A point light always uses six cube faces, so receivers above, below and on
both sides of the source are covered. Cube-face selection and filtered samples
cross face boundaries rather than treating a point light as one spotlight.
Each area-emitter sample also uses six faces: one central sample, a diagonal
pair, or four rectangle samples, with matching radiance and visibility weights.

| Host profile | Direct light cap | Primary directional map | Local face size | Requested area samples | Local view cap |
| --- | --- | --- | --- | --- | --- |
| Portable | 4 | 1024² | 256² | 1 | 64 |
| Balanced | 8 | 1536² | 512² | 2 | 64 |
| Cinematic | 8 | 2048² | 512² | 4 | 96 |
| Ultra | 8 | 4096² | 512² | 4 | 96 |

These are immediate renderer implementation budgets, not new DSL settings.
When all requested area samples exceed the local view cap, the renderer reduces
the shared area sample count from four to two to one until it fits. Point lights
keep all six faces. The primary directional map is separate from the local
array; profiles also retain their existing overall direct-light cap. Lights
beyond that cap contribute neither direct radiance nor shadow views.

Soft filtering uses bounded PCSS: eight blocker-search samples estimate
source/receiver separation and twelve Poisson comparison samples filter the
result. Larger sources and larger blocker-to-receiver gaps produce wider
penumbras, while contact stays harder. Zero source size retains a compact
filtered delta-source shadow. `shadowStyle="hard"` bypasses the soft search.
Finite view resolution, sparse blocker search and the capped filter radius
remain approximations; area quadrature can show discrete samples. This path
does not trace caustics, colored transparent shadows or unrestricted area-light
transport. Alpha-masked opaque casters respect texture opacity and cutoff;
ordinary blended transmission is not an opaque depth caster.

## Retained cache and frame evidence

Static depth views are reused in the retained renderer. Changes to a view's
light projection/origin, caster geometry, transforms, skeletal pose, vegetation
deformation or opacity inputs invalidate the affected cached evidence. A
profile change that alters face resolution or array layout recreates the
resources. Light color, intensity and source radius can change shading without
requiring a new depth image. The directional fit is based on scene bounds;
camera movement alone does not intentionally invalidate static world shadows.
Depth caches are renderer state and are not persisted as authored scene assets.

`Scene3DFrameProfile` exposes:

- `shadow_map_size`: primary directional resolution selected by the profile.
- `shadow_view_count`: active views, including the primary directional view.
- `shadow_rendered_views`: views submitted again for this frame.
- `shadow_cache_hits`: retained views reused for this frame.
- `per_light_shadow_bytes`: estimated local depth-array and uniform storage.

`render_target_bytes` includes the local array in addition to the existing
targets. A 512² six-face point map holds roughly 6 MiB of Depth32Float texels;
an area light at four samples needs 24 local faces. Cache hits reduce depth
submission work, while visibility filtering still runs for lit receivers.
Inspect these fields through the host's last 3D frame profile; submission
timings alone are not measured GPU time or playback FPS.

## Caster shape and physical contact

Shadow quality depends on actual geometry and contact. Curved chair backs,
inflated sofa cushions, pillow seams and thin draped fabric use existing
`GeometryAsset`, `Mesh`/`Vertex`/`Face`, `Loft` and supported `Modifiers`.
There is no `Cushion`, `Cloth` or shadow-only replacement tag. Keep metre-scale
UVs, correct normals, shell thickness and the intended furniture footprint;
a seat separated from its base still casts a detached shadow. See
[geometry assets](GEOMETRY_ASSETS.md) and [mesh authoring](MESH_AUTHORING.md).
Material fabric/varnish controls are documented in
[material layers](MATERIAL_LAYERS.md); room bounce and local reflections are
documented in [indoor lighting](BAKED_LIGHTING.md).

## Regression fixtures

Nonignored parser tests cover typed values, omitted defaults, old serialized
nodes, finite literal bounds, strict mode names and area shadow toggles.
Ignored native 128×128 GPU fixtures isolate colored-light ownership, opposite
point cube faces, source-radius/contact-distance penumbra width, area-light
ownership, opacity-mask holes and retained-depth reuse/invalidation. The cache
fixture checks repeated frames, radius-only changes, caster movement and the
six changed faces of a moved point source. Run them serially on a suitable native GPU:

```sh
cargo test --test per_light_shadows
cargo test --test per_light_shadows -- --ignored --test-threads=1
```

Native and browser GPU paths share the shader implementation. Native fixture
results and browser compilation do not establish browser pixel parity or
target-device performance.
