<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/CHARACTER_AUTHORING.md -->

# Character authoring API

Character construction, semantic MeshAsset editing, attachments, immutable
candidates, revision checks, undo/redo, review and GLB export are included in the
public MotionLoom library. Import `motionloom::api::character_authoring` and its
nested `rig` module. The host owns each `CharacterDocument`, `CharacterService`
and `RigAuthoringSession`; no editor, transport or global service is required.
The DSL in each snapshot is the authoritative character description.

`CharacterService::execute` accepts typed JSON commands with existing camelCase
fields. Call `CharacterCommand::Schema` (JSON `{"operation":"schema"}`) for
operations, defaults and platform capabilities. The schema remains experimental.

## Workflow

1. `import` a self-contained Scene containing editable MeshAssets, or use
   `buildTemplate` with `anime`, `chibi` or `neutral` parameters.
2. `inspect` its semantic selections, attachment registry and revision.
   Use `registerSelection` to register explicit vertex regions for imported asset names.
3. `bindAttachment` for explicit garment/accessory dependencies. The binding uses
   a control triangle, barycentric coordinates and a local surface offset.
4. `proposeEdit` with `expectedRevision`, operations and locks. Candidates never
   replace the current source. `paired` follows explicit counterpart selections.
5. `review` a candidate in gray or native GPU cel style; each result records its
   revision. Before/after images use union bounds with identical framing.
6. `commitEdit`, `discardEdit`, `undo` or `redo` with revision checks.
7. `save`/`load` the authoring document or `export` the current static GLB.

Commands use camelCase field names:

```json
{
  "operation": "proposeEdit",
  "characterId": "s101",
  "request": {
    "expectedRevision": 0,
    "operations": [
      {"kind": "scale", "target": "eyes.sclera", "factors": [0.9, 1, 1]}
    ],
    "locks": ["eyes.iris"],
    "paired": true,
    "reason": "Narrow sclera while keeping iris geometry and placement."
  }
}
```

Locks protect the selected asset's geometry. Position-only proposals cannot
change its model transforms or material, and pinned vertices retain their
existing engine protection. S101's iris selection includes pupils and glints.
Imported geometry has no fabricated template parameters. Free-form edits clear
the reversible parameter metadata; `proposeParameters` is available only while
template parameters remain authoritative. Direct source changes require re-import.

Explicit design edits validate geometry and invariants; they do not claim a
reference-fit score improvement. Image fitting keeps the existing
`MeshAuthoringSession` acceptance rules and frozen reference cameras.

## Review cameras

Native typed Scene cameras support orthographic review with
`orthographic_scale` defined as the full vertical field in world units.
Legacy Scene DSL still parses as perspective. Existing World DSL orthographic
cameras now use the same projection in GPU geometry, ground grid, CPU diagnostic
rasterization and mesh-reference snapshots. Review cameras never alter the
reference-fitting setup. The host CPU review resolves world geometry independently
of Scene's native 3D compositor. CPU review is a geometry diagnostic; final cel styling
uses the native GPU shader. S101 has front/back/left/right/quarter/portrait views.

## Validation and limits

Each candidate checks source/topology versions, locks, pinned vertices, movement
budgets, degenerate/non-manifold geometry and self-intersections using MotionLoom's
validator. Bindings reject topology changes and cycles instead of silently
selecting a new surface. Optional minimum clearance checks vertex-to-surface
distance; they do not provide continuous cloth collision or a physics solver.
Expected surface overlap can be allowed by leaving clearance unset. Bindings use
shared asset-local coordinates; parent and child Model transforms must match.
Unchanged control triangles preserve child vertices exactly, and small props use
the parent movement budget while direct edits retain their own bounds.

Templates are editable static modeling starters with intentionally overlapping
body components at joints. They have no production skin welding, automatic rig
weights, facial rig or production UV atlas. Imported procedural FaceLayout nodes
are not yet editable cages; bake or author them as MeshAssets before this workflow.
Static GLB export retains geometry/materials and reports that skeletal animation
and portable MotionLoom cel shader appearance are not included.

## Mesh-only humanoid rig API

The independent `rig` module now supplies the CC0 Character1/Character2-aligned
65-node core, target-mesh-only inspection, explicit landmark fitting, immutable
skin candidates, existing Skeleton/ModelProfile DSL output and headless direction
and deformation verification. Input GLB skins, weights and animations are ignored.
This does not change the static template/editor workflow above. See
[Rig Authoring](RIG_AUTHORING.md) for the Rust API and JSON host operations.

## Portable and native integration

```rust,no_run
use motionloom::api::character_authoring::*;

# async fn example() -> Result<(), CharacterError> {
let mut document = build_template(
    "character".into(), CharacterParameters::preset(ProportionPreset::Anime),
)?;
let bytes = document.to_json_bytes()?;
let restored = CharacterDocument::from_json_bytes(&bytes)?;
let review = render_review_images(&restored.current, restored.revision, None, 128, true, false).await?;
let glb = export_character_glb(&restored.current).await?;
# Ok(())
# }
```

`to_json_bytes` / `from_json_bytes` preserve the existing saved format, revision,
history, selections, attachments and pending candidates. Restoration validates
all snapshots; source or topology tampering is rejected. Hosts choose storage.
`CharacterService::load_document_bytes` rejects replacing a registered ID.

`bounds_async`, `review_report_async` and `render_review_images` work without
filesystem writes or a blocking executor. `RenderedReview` returns typed metadata
and named RGBA images, including comparisons and a contact sheet when requested.
`CharacterService::review_images` freezes inputs at their checked revision.
`execute_async` supports review metadata; rendering images uses the typed method.

On native targets, `native::save_document`, `load_document`, `write_review` and
`render_review` provide path conveniences. The synchronous `bounds` and
`review_report` wrappers delegate to the asynchronous implementation. Existing
JSON `save`, `load`, `export`, `inspectRigMesh` and `exportRig` retain path payloads
and responses. These operations are unsupported on WASM; the schema omits them
from its available operations. WASM callers use bytes and asynchronous review.
Directory-writing review is native-only. Rust WASM compilation does not supply
new JavaScript bindings or promise a browser renderer adapter.

CPU review provides geometry diagnostics; GPU review requires a suitable GPU
adapter and uses the production cel shader. Static GLB export does not include
skeletal animation or a portable MotionLoom cel shader.

Run the engine regressions with `cargo test -p motionloom --test
character_authoring --test character_rig_authoring`. The S101 fitted workflow and
S103/S104 authoring drivers remain in the external `motionloom-example` repository;
no showcase asset is required for the portable API.
