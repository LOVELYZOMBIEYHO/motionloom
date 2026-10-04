<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/MESH_REFERENCE_FITTING.md -->

# MeshAsset image-reference fitting API 1.0

`motionloom::api::mesh_reference` provides filesystem-free image analysis,
frozen-view evaluation and atomic position proposals for explicit MeshAssets.
Reference analyses, proposals and reports are sidecar evidence; the accepted
DSL remains the playback source of truth. Construction, session revisions and
topology proposals are documented in [Mesh authoring](MESH_AUTHORING.md).

The contract retains `schemaVersion: "1.0"`, including semantic features,
objective components, quality gates and evaluation-context fingerprints.

## Rust and WASM adapters

| Operation | Rust (`api::mesh_reference`) | WASM |
| --- | --- | --- |
| Discover the contract | `mesh_reference_schema_json` | `meshReferenceSchema` |
| Analyze image bytes | `analyze_image_reference` | `analyzeImageReference` |
| Evaluate a source/reference set | `evaluate_mesh_asset_reference` (async) | `evaluateMeshAssetReference` |
| Apply a position proposal | `apply_mesh_asset_proposal` | `applyMeshAssetProposal` |

Analysis, evaluation and apply also have Rust `_json` wrappers. Native examples
below are optional command adapters; API callers do not need the CLI.

## Host boundary and evidence class

The host resolves attachments or remote content to original image bytes,
assigns an opaque `imageId`, and stores the returned SHA-256 `contentHash`.
MotionLoom never fetches a reference URL or receives credentials. Images are
limited to 40 million pixels. Coordinates use original image pixels with the
origin at the upper-left and Y down; images are never mirrored implicitly.
An evaluation accepts one through eight reference views.

Crop a turnaround sheet into separate reference images in the host. Record the
crop coordinates and keep each resulting byte sequence fixed during fitting.
One view establishes `camera_match`; two consistent orthogonal views support
`partial_multiview_fit`; consistent front, side and back views support
`multiview_fit`. These are evidence classes, not dimensionality restrictions.
A user-requested complete 3D asset can contain inferred volume, but unobserved
depth and surfaces must remain explicit assumptions. Missing observations do
not prove full reconstruction or a percentage likeness.

## Image analysis

Run the analysis example once for every byte-exact reference:

```sh
cargo run -p motionloom --example analyze_image_reference -- \
  reference.png request.json /tmp/reference-analysis
```

The output directory contains:

- `analysis.json`: reusable analysis contract, including `contentHash`;
- `mask.png`: segmented foreground;
- `internal-edges.png`: image gradients inside the foreground;
- `overlay.png`: mask, contour, landmark, and feature inspection image.

Inspect the PNG artifacts before fitting. Correct a bad mask or misplaced hint
instead of compensating for it by deforming the mesh.

### Complete analysis request

Feature coordinates and bindings below illustrate the request shape. Use the
actual reference crop and current cage indices; leave unknown features unbound.

```json
{
  "schemaVersion": "1.0",
  "imageId": "attachment-front-sha-prefix",
  "view": "front",
  "segmentation": {
    "mode": "guided",
    "analysisRegion": [20, 20, 900, 900],
    "foregroundPoints": [[450, 450]],
    "backgroundPoints": [[25, 25], [915, 25]],
    "backgroundColor": null,
    "threshold": 42,
    "suppliedMask": null,
    "minComponentPixels": 64,
    "maxContourPoints": 512
  },
  "requestedLandmarks": [
    "topmost",
    "bottommost",
    "leftmost",
    "rightmost"
  ],
  "analysisProfile": "human_head_v1",
  "featureHints": [
    {
      "id": "nose_tip",
      "kind": "point",
      "points": [[452, 326]],
      "semanticLabel": "nose_tip",
      "binding": {"type": "vertex", "vertex": 624},
      "confidence": 0.95,
      "snapRadius": 5
    },
    {
      "id": "mouth_line",
      "kind": "polyline",
      "points": [[420, 370], [426, 372], [432, 371]],
      "semanticLabel": "mouth_line",
      "binding": {
        "type": "vertexChain",
        "vertices": [520, 528, 536],
        "closed": false
      },
      "confidence": 0.82,
      "snapRadius": 4
    }
  ],
  "depthHints": [
    {
      "relation": "inFrontOf",
      "subject": "nose_tip",
      "object": "mouth_line",
      "evidence": "consistentSideView",
      "value": null,
      "confidence": 0.75
    }
  ]
}
```

`analysisProfile` is an optional diagnostic profile. The current
`human_head_v1` profile requires host- or LLM-supplied feature hints; MotionLoom
does not invent facial semantics.

### Segmentation modes

- `alpha`: use the supplied image alpha channel.
- `mask`: use `suppliedMask`, encoded as `MaskData` RLE.
- `guided`: use ROI plus foreground/background seed points.
- `backgroundColor`: use an RGB `backgroundColor`, for example `[240,240,240]`.
- `auto`: initial fallback only; inspect its artifacts carefully.

`analysisRegion` is `[x, y, width, height]`. Pixel coordinates use image space
with the origin at the upper-left. `threshold`, seed points, component size, and
contour-point limit are segmentation controls, not mesh-fitting controls.

A supplied mask has this form:

```json
{
  "width": 1024,
  "height": 1024,
  "startsForeground": false,
  "runs": [120, 18, 100, 22]
}
```

### Feature kinds and bindings

Feature kinds are `point`, `polyline`, `closedContour`, and `region`.
`closedContour` and `region` require at least three points; `point` requires
exactly one.

Available bindings are listed below, one independent object per line:

```jsonl
{"type":"vertex","vertex":18}
{"type":"edge","vertices":[18,19],"t":0.4}
{"type":"face","face":7,"barycentric":[0.2,0.3,0.5]}
{"type":"vertexChain","vertices":[18,19,20],"closed":false}
{"type":"nearestContour"}
```

Use `nearestContour` only while analyzing an unbound hint. Bind it to actual
mesh topology before expecting an internal feature residual. A face binding is
evaluated with three barycentric weights.

The analyzer snaps each hint toward a nearby internal edge within
`snapRadius`. It lowers confidence when no credible edge is found. Feature IDs
must be unique within a view and are also used by depth relations.

Supported ordinal depth relations are:

- `inFrontOf` or `closerThan`;
- `behind` or `fartherThan`.

Both `subject` and `object` must name bound features. Unknown or zero-confidence
depth hints are retained as uncertainty but do not add an objective penalty.

## Author and freeze the comparison setup

Use the canonical [GeometryAsset structure](GEOMETRY_ASSETS.md): authored
vertices/faces belong to `GeometryAsset/Mesh`, and `MeshAsset.geometry` references
that geometry. No reference-fitting tags enter the DSL. Keep subdivision off
while fitting the editable control cage.

Calibrate a fixed camera/frame and object transform per reference before fitting
geometry. Keep reference bytes, dimensions, camera snapshots, object transforms,
render dimensions, metric weights and quality gates unchanged within a comparison
setup. Use `cameraPolicy: "frozen"` and neutral review conditions without DoF,
temporal jitter, motion blur, animation, wind or outlines. Changing the setup
requires a new baseline; do not attribute camera/metric changes to geometry gains.

## Mesh evaluation

Evaluate a complete multiview set:

```sh
cargo run -p motionloom --example evaluate_mesh_reference -- \
  scene.motionloom references.json /tmp/mesh-evaluation
```

For one existing analysis:

```sh
cargo run -p motionloom --example evaluate_mesh_reference -- \
  scene.motionloom --analysis /tmp/reference-analysis/analysis.json \
  object_mesh object_model 0 /tmp/mesh-evaluation
```

### Complete multiview request

Replace each `{}` analysis value below with the full corresponding
`analysis.json` object:

```json
{
  "schemaVersion": "1.0",
  "targetAssetId": "object_mesh",
  "targetModelId": "object_model",
  "references": [
    {"id": "front", "frame": 0, "analysis": {}},
    {"id": "right", "frame": 48, "analysis": {}},
    {"id": "back", "frame": 96, "analysis": {}},
    {"id": "left", "frame": 144, "analysis": {}}
  ],
  "options": {
    "cameraPolicy": "frozen",
    "allowBoundary": false,
    "allowMultipleComponents": false,
    "maxVertexResiduals": 512,
    "metricWeights": {
      "silhouette": 0.25,
      "boundary": 0.20,
      "landmarks": 0.25,
      "features": 0.20,
      "depth": 0.10
    },
    "qualityGates": {
      "minimumMaskIou": 0.90,
      "maximumP95EdgeDistancePx": 16,
      "maximumLandmarkMeanDistancePx": 6,
      "maximumFeatureMeanDistancePx": 6,
      "maximumDepthViolations": 0
    }
  }
}
```

Weights are normalized over active components. Feature and depth weights become
inactive when no evaluable feature or confident two-feature depth relation is
present. Keep the same weights through an accepted/rejected proposal sequence.

### Evaluation report

`evaluation.json` includes aggregate `objective`, `passesQualityGates`, and one
entry per view. Inspect:

- `maskIou`;
- `meanEdgeDistancePx`, `p95EdgeDistancePx`, and
  `maximumEdgeDistancePx`;
- `landmarks` and `features`, including candidate points and residual distances;
- `depthViolations`;
- `objectiveComponents` for silhouette, boundary, landmarks, features, depth;
- ranked visible `vertexResiduals`;
- per-view and aggregate quality-gate results.

The output directory also includes `{view}-overlay.png` and
`{view}-difference.png`. In an overlay, white agrees, green is missing candidate
area, and magenta is candidate overflow.

Copy all six fingerprints from the current evaluation before constructing a
proposal:

- `sourceFingerprint` binds exact source text;
- `topologySignature` binds vertex/face topology;
- `cameraFingerprint` binds all evaluated camera snapshots;
- `referenceSetFingerprint` binds image hashes, analyses, features, and depth;
- `metricProfileFingerprint` binds weights and gates;
- `evaluationFingerprint` binds the preceding five values together.

Any source, topology, camera, reference, or metric change makes the proposal
context stale.

## Safe position proposals

Apply one bounded proposal to the exact source that produced its evaluation:

```sh
cargo run -p motionloom --example apply_mesh_proposal -- \
  accepted.motionloom proposal.json candidate.motionloom
```

### Complete fingerprint-safe proposal

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
  "reason": "both profile views place the nose tip farther forward",
  "changes": [
    {
      "vertex": 624,
      "before": [0.0, -0.1188374, 0.8993575],
      "after": [0.0, -0.1188374, 0.9093575],
      "confidence": 0.84,
      "evidenceViews": ["left", "right"]
    }
  ],
  "validation": {
    "allowBoundary": false,
    "allowMultipleComponents": false,
    "maxMoveRelativeToBounds": 0.02,
    "maxChangedVertices": 16,
    "maxEdgeLengthRatio": 1.25,
    "maxLaplacianDeltaRelativeToBounds": 0.04
  }
}
```

All four evaluation-context fingerprints are optional only for compatibility
with older schema-1.0 requests. If any one is supplied, all four must be
supplied. New LLM workflows must always supply all six fingerprints shown
above.

The standalone apply function checks exact source/topology and the internal
consistency of supplied context fingerprints. It has no current camera or
reference set to compare against. `apply_position_proposal_to_session` additionally
requires all six values to match the accepted revision's stored evaluation.
Hosts using standalone Rust, JSON or WASM calls must retain and check that
accepted evaluation themselves.

Every `before` value must exactly match the current source. The apply operation
is atomic and rejects:

- a stale source/topology or internally inconsistent context fingerprints;
- duplicate, missing, pinned, or out-of-range vertices;
- non-finite coordinates or moves above the relative bound;
- batches above `maxChangedVertices`;
- edge stretch above `maxEdgeLengthRatio`;
- local deformation spikes above `maxLaplacianDeltaRelativeToBounds`;
- boundary, component, winding, non-manifold, degenerate-face, collapsed-volume,
  or detected self-intersection failures.

The operation edits the explicit geometry referenced by the target MeshAsset,
formats the source and reparses it. Shared geometry changes all material variants
that reference it. The candidate has a new `sourceFingerprint` and unchanged
topology. Position proposals preserve authored UV values; they do not generate
or pack UVs.

## Acceptance and limits

Immediately evaluate the candidate with the same frozen reference set. In a
Rust session, `decide_candidate_revision` checks aggregate improvement against
`minimum_improvement` and maximum fitted-view regression against
`primary_view_tolerance`. A zero minimum permits an equal objective. It does
not enforce feature-by-feature non-regression, visual plausibility or final
quality gates; the host reviews those separately before calling the decision.

Reject a candidate when important features or available held-out evidence
regress, even if the aggregate objective improves. Apply success proves a valid
mutation, not an improved match. `passesQualityGates` records the configured
measurement checks and is distinct from accepting an intermediate revision.

Position proposals cannot add/remove faces, edit UVs or change topology. Use
[MeshTopologyProposal](MESH_AUTHORING.md#topology-proposals) when necessary, and
follow its feature-rebinding and baseline rules. Repeat analysis only when
images, masks, hints or bindings change; normally repeat evaluate, propose,
apply and evaluate. `stop_reason` and `artifact_manifest` are described in
[Mesh authoring](MESH_AUTHORING.md#decisions-stopping-and-artifacts).

Built-in segmentation is deterministic alpha/color/guided analysis, not a neural
semantic detector. Features need host-supplied semantics and credible bindings;
monocular depth remains a hint. Heavy occlusion, reflections, transparent hair,
blur or inconsistent views can prevent convergence.

Evaluation measures authored control faces, not the final subdivided surface or
texture likeness. Review the requested subdivided/styled render separately.
Topology validation detects invalid indices, degeneracy, winding, non-manifold
edges, disallowed boundaries/components, collapsed closed volume and detected
non-adjacent triangle intersections. Coplanar overlaps remain a known limitation
and need visual review. Browser hosts should run authoring evaluation in a worker;
it is independent of immediate playback.
