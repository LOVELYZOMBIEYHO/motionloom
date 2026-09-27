# Canonical geometry assets

## Asset design

1. Separate `GeometryAsset`, material-bound `MeshAsset`, and `CompoundAsset`.
2. Keep one location for modifiers, subdivision, and UV generation.
3. Use the existing `Primitive`, `Mesh`, `Sweep`, `Loft`, `Ribbon`, `Profile`,
   `Vertex`, and `Face` concepts; add `Revolve`, `RadialWave`, and `DisplaceNoise`.
4. Share geometry operations between the DSL, authoring API, native, WASM, and
   Weaver; separate geometry compilation from material assembly and caching.
5. Use the same asset structure in examples, templates, schema discovery,
   reference fitting, and editor tools.
6. Verify canonical parsing, deterministic geometry, UV seams,
   shared geometry, authoring proposals, native/WASM builds, and every showcase.

UV generation belongs only to `GeometryAsset/UV`. Material texture scale,
offset, and rotation are sampling controls. Model and material IDs provide
stable targets for scene animation references.

## Canonical structure

```xml
<GeometryAsset id="ball_geometry">
  <Primitive shape="sphere" radius="1" />
</GeometryAsset>
<MaterialAsset id="red" baseColor="#D92820" roughness="0.5" />
<MeshAsset id="ball" geometry="ball_geometry" material="red" />
```

An authored polygon cage uses `GeometryAsset/Mesh/Vertex` and `Face`. Each
geometry has exactly one generator, or a `source` geometry for derivation.
`MeshAsset` is a material-bound reference and never owns inline vertices.
Subdivision uses `Modifiers/Subdivision` with an explicit algorithm `scheme`.
Spatial curves remain `CurveAsset/CurvePoint`; sweep and revolution profiles
share `Profile/ProfilePoint` parsing. Image noise remains a separate image
resource; `DisplaceNoise` modifies geometry.

Asset compilation preserves authored positions, UVs, winding, primitive defaults,
collision settings, and scene IDs. Procedural fitting is a separate operation:
an approved explicit cage is not silently replaced with an approximate profile.

## Procedural profile and reusable material variants

```xml
<GeometryAsset id="fruit_body">
  <Revolve axis="y" segments="32" samples="24">
    <Profile interpolation="catmullRom">
      <ProfilePoint position={[0,-0.95]} />
      <ProfilePoint position={[0.75,-0.85]} />
      <ProfilePoint position={[1.02,-0.35]} />
      <ProfilePoint position={[1.04,0.25]} />
      <ProfilePoint position={[0.90,0.70]} />
      <ProfilePoint position={[0.25,0.58]} />
      <ProfilePoint position={[0,0.54]} />
    </Profile>
  </Revolve>
  <Modifiers>
    <RadialWave axis="y" cycles="5" amplitude="0.025"
                heightRange={[0.25,0.85]} falloff="0.12" />
    <DisplaceNoise amplitude="0.003" frequency="12" seed="98" />
    <WeightedNormals strength="1" keepSharpEdges="false" />
  </Modifiers>
  <UV mode="profileParameter" />
</GeometryAsset>
<MeshAsset id="fruit_red" geometry="fruit_body" material="red_skin" />
<MeshAsset id="fruit_gray" geometry="fruit_body" material="gray" />
<MeshAsset id="fruit_white" geometry="fruit_body" material="white" />

<GeometryAsset id="fruit_wire" source="fruit_body">
  <Modifiers>
    <Wireframe radius="0.002" segments="4" />
  </Modifiers>
</GeometryAsset>
<MeshAsset id="fruit_wire_model" geometry="fruit_wire" material="gray" />
```

This example illustrates the generator; it does not replace the approved S98
explicit cage. Profile points are `[radius,height]`. Their parameter order fixes
UV rows, so changing a radius preserves corresponding texels. Revolve duplicates
the angular UV seam and collapses zero-radius rows to poles. Positive-radius
endpoints remain open. CatmullRom can overshoot; sampled radii are clamped at zero.

Use a modest tessellation for wire tubes to stay within the topology budget.

`source` references support forward declarations and reject cycles. Source
operations run first. Derived operations run afterward. One `UV` node is allowed;
its position before or after `Modifiers` controls when projection is applied.
`Partition` selects faces by UV centroid in half-open `uRange`/`vRange` intervals.
`Wireframe` creates tubes along unique edges. `ThickenSurface` reuses the authoring
API's existing extrusion kernel. Topology limits fail during parsing as errors.

## API and source editing

`GeometryRecipe` adds `revolveProfile` and `applyModifier`. It uses the same
revolution, deformation, subdivision, surface and UV kernels as the DSL.
`mesh_asset_element` emits a `GeometryAsset/Mesh` plus a referencing `MeshAsset`.
Image analysis, evaluation, vertex proposals and topology proposals resolve
`MeshAsset.geometry`; proposals edit its explicit `Mesh` only. Parametric or
derived generators require an explicit conversion to a Mesh before source vertex
editing. `bake_mesh_asset_geometry` performs this conversion with a required source
fingerprint, replaces the geometry pipeline, and preserves all material bindings. Editing shared geometry changes all of its material variants.

Native, WGPU, WASM and Weaver consume the same resolved rendering IR. Geometry cache keys
exclude materials; material assembly and texture loading retain their own keys.

Specialized `TerrainAsset` and `VegetationAsset` provide their own scene-specific
systems.

## Verification

- All 48 affected DSL files across 22 showcase directories use the canonical
  asset structure, including alternate scene files.
- Parsed all 146 showcase scene and action-library files successfully. Regenerated
  all 99 main showcase schemas: 98 clean, one existing tessellation review warning,
  no repair or unrenderable results.
- Preserved S98's 32 original explicit cages, including vertex positions, UVs,
  pinned values and face data. Its 38 material bindings now share 30 geometry
  definitions. The scene remains 1080 by 1920, 20 seconds and 480 frames.
- Passed 712 Rust tests and four mesh editor tests. Existing gates ignored 16
  tests; one unrelated HTTP resolver test was excluded because the sandbox
  cannot bind its local test server.
- Rebuilt the MotionLoom WASM package and landing site successfully. Rendered
  and inspected S98 frame zero using the native GPU at 1080 by 1920.

These checks verify the asset structure and S98 geometry preservation. They do
not constitute a full Weaver video render of every showcase.
