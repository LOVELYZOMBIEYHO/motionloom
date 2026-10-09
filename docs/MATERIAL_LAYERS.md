# Material sheen and clearcoat

`MaterialAsset` accepts five optional constant attributes. Both lobe weights
default to zero, so omitting them does not add material layers. Reflections use
the [default hybrid renderer](HYBRID_REFLECTIONS.md); its scene-wide migration
can change appearance and cost independently of these material defaults.
No new asset tags or texture slots are introduced.

| Attribute | Range / format | Default | Meaning |
| --- | --- | --- | --- |
| `sheen` | finite 0–1 | 0 | Fabric sheen weight |
| `sheenColor` | `#RRGGBB` | `#FFFFFF` | sRGB tint, converted once to linear RGB |
| `sheenRoughness` | finite 0.04–1 | 0.5 | Charlie sheen spread |
| `clearcoat` | finite 0–1 | 0 | Dielectric coating weight |
| `clearcoatRoughness` | finite 0.04–1 | 0.1 | Independent GGX coating roughness |

```xml
<MaterialAsset id="upholstery" baseColor="#C9C1AF"
  roughness="0.82" normalTexture="weave_normal"
  sheen="0.35" sheenColor="#E4DDD0" sheenRoughness="0.65" />
<MaterialAsset id="varnished_wood" baseColorTexture="oak_color"
  metallicRoughnessTexture="oak_orm" roughness="0.85"
  clearcoat="0.25" clearcoatRoughness="0.18" />
```

Sheen is a Charlie lobe with Neubelt visibility. Its directional albedo is
numerically integrated using that same visibility and stored beside the
unchanged GGX split-sum coefficients. The base lobe loses the energy assigned to
sheen. Clearcoat uses IOR 1.5, its own roughness and the geometric surface normal;
it attenuates the base, sheen, emission and transmission with coating Fresnel.
The existing base normal, roughness and AO textures continue to work. Color
alpha, if present in an eight-digit `sheenColor`, does not change coverage.

Direct lighting evaluates both lobes. Environment and local room reflections
evaluate coating GGX independently; sheen uses 16 deterministic source-radiance
samples normalized to its directional-albedo LUT. This bounded approximation
preserves energy for constant lighting, but very small bright lights in an
environment can need higher sampling quality in a future renderer profile.
Hybrid scene geometry augments primary base and coating reflections
independently, including reflected objects outside the main camera. Environment
and local probes remain rough-surface/missed-ray fallback evidence. The existing
screen-space response applies only where valid layer evidence is available;
one G-buffer normal is not treated as a complete description of both lobes.
Material AO attenuates indirect lobes only.

The CPU room baker applies the same directional loss to diffuse throughput and
coating loss to emission/transmission. Its diffuse solver does **not** trace
glossy clearcoat or fabric sheen bounces. Weaver supports active GGX clearcoat
and Charlie/Neubelt sheen in its physical BSDF. It samples their own lobes,
retains independent coat/base roughness, and attenuates the underlying BSDF
using the shared Charlie directional-albedo energy table and coating Fresnel.
Its reflection, transmission and total path counts remain RenderJob budgets;
see [Weaver's optical limits](../src/weaver/README.md#explicit-limitations).

GLB imports and exports scalar/color factors for
[`KHR_materials_sheen`](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_sheen)
and
[`KHR_materials_clearcoat`](https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_clearcoat).
glTF sheen factors are already linear; exported `sheenColorFactor` includes the
DSL sheen weight. Imported factor roughness zero uses the numerical 0.04 floor.
Extension textures, including coat normals, fail with a specific import error
instead of silently losing their appearance. Sheen/coat texture authoring is
outside this first version.
