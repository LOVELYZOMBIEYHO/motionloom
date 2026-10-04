<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/MESH_AUTHORING.md -->

# Mesh authoring API

`motionloom::api::mesh_authoring` constructs and revises universal MeshAssets
through deterministic, filesystem-free GeometryRecipes and authoring sessions.
Image-analysis/evaluation/position contracts and JSON examples live in
[Mesh reference fitting](MESH_REFERENCE_FITTING.md). Skills select and execute
workflows; these docs maintain their API contracts.

## Workflow

1. Classify available reference evidence and record missing-view assumptions.
2. Analyze images independently with `api::mesh_reference::analyze_image_reference`
   and review their masks/hints. This does not require a session. Keep feature
   hints unbound until the cage's actual vertices and faces are available.
3. For a new cage, query the authoring schema and run `execute_geometry_recipe`.
   An existing explicit cage can be reused without a new recipe.
4. Insert `mesh_asset_element` output into the caller's Scene. It emits a
   `GeometryAsset/Mesh` and a material-bound `MeshAsset`, not inline MeshAsset
   vertices. Parse the full source and validate the cage.
5. Calibrate camera/object placement, then `create_mesh_authoring_session` from
   that source. Revision 0 is the initial accepted source, not a fit certificate.
6. Populate the session's `analyses` with reviewed results, or call
   `analyze_references(&mut session, inputs)` now. This function needs an existing
   session and rejects duplicate image IDs and byte-identical references. Bind
   semantic observations to the current cage before establishing the baseline.
7. `evaluate_session_revision` establishes frozen camera, reference-set and metric
   fingerprints for the accepted revision.
8. Apply a bounded `apply_position_proposal_to_session`, or use
   `apply_topology_proposal_to_session` when topology or UV layout must change.
9. Re-evaluate the candidate under the same comparison setup. For topology edits,
   follow feature rebinding below first.
10. Review semantic/visual evidence, then `decide_candidate_revision` or
    `reject_candidate_revision`. Rejected candidates preserve the accepted source.
11. Repeat until the configured stopping condition; review subdivision separately.

Session handles are valid only for their revision ID and topology signature.
`revision_handles` and `validate_handle` support rejecting stale selections.
Candidates do not replace the accepted revision until a decision accepts them.

## GeometryRecipe operations and emitted DSL

Supported operations include cross-section loops/lofts, profile revolution,
point-grid surfaces, path sweeps, extrusion/thickening, semantic regions and
transforms, mirror/weld/caps, shared geometry modifiers and UV projection.
`mesh_authoring_schema_json` reports the current operation vocabulary.
Every operation has a unique ID. Regions use revision-local vertex/face indices;
results include cage, regions, operation results, topology and correspondence.
Recipes and topology results are capped at 30,000 vertices and 30,000 faces each.

```rust
use motionloom::api::mesh_authoring::{
    GeometryRecipe, execute_geometry_recipe, mesh_asset_element,
};

let recipe: GeometryRecipe = serde_json::from_str(recipe_json)?;
let result = execute_geometry_recipe(&recipe)?;
let asset_declarations = mesh_asset_element("subject", "neutral", &result.cage);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Insert both returned asset declarations into `<Assets>` and provide the referenced
material and Model. Subdivision belongs to `GeometryAsset/Modifiers/Subdivision`.
A procedural or derived pipeline must be explicitly converted with
`bake_mesh_asset_geometry` before source-vertex editing; that operation requires
the exact source fingerprint and preserves its material bindings. See
[Geometry assets](GEOMETRY_ASSETS.md).

Example recipe:

```json
{
  "schemaVersion": "1.0",
  "id": "initial-control-cage",
  "subdivision": 0,
  "operations": [
    {
      "type": "createLoop",
      "spec": {
        "id": "section-a",
        "center": [-1, 0, 0],
        "radiusA": 0.2,
        "radiusB": 0.15,
        "segments": 8,
        "normalAxis": "x",
        "rotationDegrees": 0
      }
    },
    {
      "type": "createLoop",
      "spec": {
        "id": "section-b",
        "center": [1, 0, 0],
        "radiusA": 0.1,
        "radiusB": 0.08,
        "segments": 8,
        "normalAxis": "x",
        "rotationDegrees": 0
      }
    },
    {
      "type": "loftLoops",
      "id": "body-volume",
      "loops": ["section-a", "section-b"],
      "cap_start": true,
      "cap_end": true
    }
  ]
}
```

## Rust, JSON and WASM adapters

| Operation | Rust (`api::mesh_authoring`) | WASM |
| --- | --- | --- |
| Discover operations | `mesh_authoring_schema_json` | `meshAuthoringSchema` |
| Execute a recipe | `execute_geometry_recipe` / `execute_geometry_recipe_json` | `executeGeometryRecipe` |
| Apply standalone topology edits | `apply_mesh_topology_proposal` / `apply_mesh_topology_proposal_json` | `applyMeshTopologyProposal` |

Session creation, analysis, evaluation, apply and decision helpers are public Rust
APIs. CamelCase names in the schema describe those operations; they are not all
JavaScript exports. The WASM topology adapter takes four arguments: source text,
analyses JSON, proposal JSON and expected evaluation JSON. A WASM host owns revision
tracking and acceptance when using standalone adapters.

### Current JSON field names

Structs such as `GeometryRecipe` and `LoopSpec` serialize fields in camelCase.
`GeometryOperation` and `UvProjection` serialize variant tags in camelCase, but
their variant fields currently keep Rust's snake_case spelling:

| Variant | JSON fields |
| --- | --- |
| `loftLoops` | `cap_start`, `cap_end` |
| `createSurface` | `close_columns` |
| `mirrorRegion` | `weld_center` |
| `capBoundary` | `loop_id` |
| UV `planar` | `u_axis`, `v_axis` |
| UV `referenceCamera` | `image_size` |

The discovery schema's `operationInputs` currently lists camelCase hints for
some of these fields. Use the actual serialized keys above: unknown enum fields
may be ignored, and an omitted optional field takes its default. For example,
`capStart: true` is ignored and leaves the loft uncapped; `cap_start: true` caps
it. These names document the existing contract, not a parser migration.

## Topology proposals

Use this phase only when moving the existing cage cannot represent required
geometry. A topology proposal uses GeometryRecipe operations on semantic regions:

The indices, target and fingerprint placeholders below are illustrative; replace
them with the current cage selections and evaluation before applying the proposal.

```json
{
  "schemaVersion": "1.0",
  "sourceFingerprint": "FROM_CURRENT_EVALUATION",
  "topologySignature": "FROM_CURRENT_EVALUATION",
  "cameraFingerprint": "FROM_CURRENT_EVALUATION",
  "referenceSetFingerprint": "FROM_CURRENT_EVALUATION",
  "metricProfileFingerprint": "FROM_CURRENT_EVALUATION",
  "evaluationFingerprint": "FROM_CURRENT_EVALUATION",
  "targetAssetId": "object_mesh",
  "reason": "the observed appendage needs its own connected extrusion",
  "operations": [
    {
      "type": "extrudeRegion",
      "id": "appendage-extrusion",
      "region": "appendage-root",
      "vector": [0, 0.18, 0],
      "segments": 1
    }
  ],
  "regions": [
    {"id": "appendage-root", "vertices": [], "faces": [42]}
  ],
  "validation": {
    "allowBoundary": false,
    "allowMultipleComponents": false
  },
  "evidenceViews": ["front", "left"],
  "confidence": 0.82
}
```

Call the Rust function `apply_mesh_topology_proposal` or WASM
`applyMeshTopologyProposal`. The operation validates the complete candidate and
atomically returns formatted source with the target's explicit geometry rewritten.
Shared GeometryAsset edits affect every material-bound MeshAsset that references
it. The result includes old-to-new vertex/face correspondence, created/removed
handles, invalidated regions and `invalidatedFeatureIds`.

Topology application enforces operation validation, size limits and the complete
result's topology policy. The shared validation type also contains position-edit
movement, batch, stretch and Laplacian fields; this path does not apply those
position-delta budgets to topology operations. The host must bound operation
scope and review the returned geometry.

The current implementation conservatively invalidates all supplied bindings to
vertices, edges, faces and vertex chains; the list is not limited to features
near the edit. Rebind every listed feature before relying on its residual.

## Feature rebinding and new baselines

`evaluate_session_revision` freezes the full reference-set fingerprint, including
feature bindings. It rejects changes to references, cameras or metrics after the
baseline. Therefore changing a bound vertex index is not a continuation under
the old frozen setup, even if the image bytes are unchanged.

If confirmed bindings remain byte-identical and meaningful after the edit, the
candidate can use the existing comparison setup. Otherwise:

1. Retain the accepted source, pending topology proposal and old reports. Do not
   reuse invalidated indices or treat different objectives as comparable.
2. Build a common binding-independent reference set for the old and new topology,
   for example silhouette and contour observations with affected bound features
   omitted. Preserve image bytes/cameras and record this as a new comparison setup.
3. Create a fresh session from the last accepted source, analyze/store that common
   set and evaluate its baseline. Reissue the same reviewed topology operations
   with the new six fingerprints; evaluate and decide the candidate under this
   shared setup. No unverified geometry becomes accepted merely by resetting.
4. Independently evaluate the candidate with correctly rebound semantic features
   and check the original required feature goals. A silhouette improvement alone
   cannot replace a required facial or anatomical check. If a trustworthy common
   comparison or feature validation is unavailable, keep the result pending.
5. After acceptance, start a new session from the accepted source with its rebound
   features for subsequent edits. Preserve both sessions and their relationship
   in host-owned history. Do not compare old/new setup objectives as one fit score.

## Decisions, stopping and artifacts

`decide_candidate_revision` compares evaluated revisions with identical camera,
reference and metric fingerprints and view IDs. Acceptance requires objective
improvement of at least `minimum_improvement` and maximum per-view regression no
larger than `primary_view_tolerance`; zero minimum allows equality. The host checks
individual semantic features, overlays, held-out evidence and final quality gates
separately before the decision. `reject_candidate_revision` records explicit
rejection without replacing the accepted source.

`stop_reason` reports configured quality gates, contradictory views, exhausted
movement budget, insufficient topology or negligible recent objective progress.
It uses the caller's `StopConditions`; three negligible iterations are a workflow
choice, not a hard-coded API rule.

`artifact_manifest(root)` returns suggested paths for source, cage, recipe,
session, references, analyses, evaluations, proposals and fit summary. It does
not create directories, persist sessions or render artifacts. The host owns
storage and any requested export. Preserve editable DSL alongside derived GLB
outputs; exporting does not establish fit quality.

## Optional native command adapters

From the `motionloom` checkout:

```text
cargo run -p motionloom --example mesh_authoring -- schema
cargo run -p motionloom --example mesh_authoring -- \
  build recipe.json mesh.motionloom subject neutral
cargo run -p motionloom --example mesh_authoring -- \
  apply-topology accepted.motionloom analyses.json proposal.json \
  evaluation.json candidate.motionloom
```

`build` emits two asset declarations for insertion into `<Assets>`, not a complete
playable Scene. Core APIs do not fetch image URLs, call an external model generator
or require a UI, ACP transport or CLI.
