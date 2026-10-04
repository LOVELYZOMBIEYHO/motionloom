<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/RIG_AUTHORING.md -->

# Mesh-only humanoid rig authoring

The headless public API is `motionloom::api::character_authoring::rig`.
MotionLoom contains the fitter, binder, verifier and embedded reference data;
the host supplies GLB bytes, anatomy hints and action sources. No UI or transport
is required, and existing MotionLoom parser syntax remains unchanged.

## Reference contract

`humanoid_rig_standard()` returns `humanoid65_v1`: a fixed 65-node core hierarchy,
canonical names, one-to-one Character1/Character2 names, global reference rotations,
mesh-height-normalized joint positions, source hashes and CC0 provenance.

The hierarchy has one root, 22 body nodes, 30 finger articulation nodes and 12
finger/toe endpoints. `index_end_l` maps to `index_04_leaf_l`; `toe_end_l` maps to
`ball_leaf_l`. Names already used by MotionLoom remain unchanged. Extra joints
follow an existing core or extension parent and cannot replace or insert themselves
between core joints. Endpoints and root do not receive deformation weights.
Extensions receive weights only when explicitly selected by a weight region.

Mesh space is right-handed, +Y up, +Z facing and +X anatomical left, matching the
CC0 reference GLBs. Joint positions and landmark hints use the baked mesh space.
Rotations are **global normalized XYZW quaternions**; local transforms and inverse
bind matrices are derived. Different target proportions never change core parents.

The embedded JSON contains only reference joint data. It does not contain meshes,
textures, animation clips or code from external riggers. Runtime use requires no
Character1/Character2 asset files. `tools/extract_rig_reference.py` reproduces this
data from the two known reference files; it rejects files with different hashes
and is not used to inspect unknown models. Each reference carries its own source
URL and license metadata.

Reference sources are Quaternius' CC0 Universal Animation Library 1 and Universal
Animation Library 2 Female Mannequin. Asset hashes are embedded in the standard.
See [Universal Animation Library](https://quaternius.com/packs/universalanimationlibrary.html)
and [Universal Animation Library 2](https://quaternius.com/packs/universalanimationlibrary2.html).

## Rust API workflow

```rust,no_run
use motionloom::api::character_authoring::rig::*;
use std::collections::BTreeMap;

# fn example(bytes: &[u8], library_source: String) -> Result<(), RigError> {
let mut session = RigAuthoringSession::from_glb_bytes(
    bytes,
    &MeshInspectionOptions {
        target_height: Some(1.8),
        ..Default::default()
    },
)?;

// Read vertices, triangles, bounds and part/node IDs before supplying anatomy.
let mesh = session.mesh();
let geometry_fingerprint = mesh.inspection.geometry_fingerprint.clone();
let nearby = nearest_rig_vertices(mesh, [0.35, 1.25, 0.0], 12)?;
// Each sample has named vertex, position and distance fields.

// Empty landmarks produce a complete draft with explicitly unconfirmed priors.
let request = RigBuildRequest {
    expected_geometry_fingerprint: geometry_fingerprint,
    reference: "character1".into(),
    landmarks: BTreeMap::new(),
    rotations: BTreeMap::new(),
    extensions: vec![],
};
let candidate = session.propose(0, &request, &RigBindingOptions::default())?;

let report = session.verify(
    &candidate.id,
    &RigVerificationOptions {
        actions: vec![RigActionTest {
            library_source,
            action_id: "sprint_standard_loop".into(),
            phases: vec![0.2, 0.5, 0.8],
        }],
        ..Default::default()
    },
)?;

// Refine landmarks, global joint frames, weight regions or semantic axes,
// then call propose again at the same revision and verify the new candidate.
if report.accepted {
    session.commit(0, &candidate.id)?;
    let glb = session.export_committed()?;
} else {
    let review_glb = session.export_candidate(&candidate.id)?;
}
# Ok(())
# }
```

`build_humanoid_skeleton`, `bind_humanoid_skin`, `export_rig_glb` and
`verify_humanoid_binding` are also available as typed functions. The stateful
session keeps immutable candidates, revisions, source checks and verification
receipts together. Sessions are in memory; returned typed candidate/report JSON
and DSL fragments can be stored by the host. Path-backed sessions reject source
file changes before proposing, verifying, committing or exporting.

## Target skeleton exclusion

Inspection removes `skins`, animation, morph targets, node skin assignments,
`JOINTS_*` and `WEIGHTS_*` before the MotionLoom mesh loader runs. Declared joint
node transforms are skipped even when they occur in mesh-object ancestry. Names,
bind matrices and poses from the target rig never seed the new skeleton.

Static non-joint object transforms are baked; geometry is centered in X/Z and
floored in Y, with optional uniform height normalization. `facingRotation` is
caller-supplied because symmetric meshes cannot reliably reveal front direction.
`meshNodes` selects a subset of mesh nodes; otherwise all mesh nodes are inspected.
Multiple instances of one source mesh are decoded independently.

Positions, transformed normals, UVs, indices, materials and embedded images are
retained in candidate exports. Tangents are omitted after baking. Compressed or
required extension geometry, reflected/singular object transforms, non-triangle
primitives and external buffer/image files need preprocessing and return typed
errors. None of these operations changes the supplied GLB.

## Fitting and weighting

The initial fit is a **reference proportion prior**, not anatomical detection.
Explicit landmark positions replace those priors. Unspecified descendants follow
edited parent positions; bone direction changes align the reference coordinate
frame to the target chain. `rotations` can refine roll and palm/finger orientation.
All 52 non-root, non-endpoint core positions must be confirmed with explicit
landmarks before acceptance. Unspecified endpoints follow the fitted terminal
segment direction, roll and proportional length; explicit endpoint hints can
override them. Endpoints are structurally checked.
Supplied left/right hints that contradict the reference basis are rejected.

Initial skin weights use inverse-square local bone-segment distances, opposite-side
filtering and smaller finger influence radii. Metric surface diffusion gives short
connected edges stronger coupling and normalizes restricted distributions on each
iteration. Sparse support changes approach zero continuously; diffusion operates on
the four influences that will actually be rendered. Reviewed rigid surfaces blend
into compatible neighboring regions over a geodesic transition of
`2 * falloff * mesh_height`. Loose surfaces are not joined by proximity. Each vertex
keeps at most four normalized influences. Explicit non-overlapping weight regions
constrain the allowed bones and remain constrained during smoothing. Their vertex
indices refer to the full inspected mesh, across all primitives. An optional
`axisMap` replaces the semantic/rest calibration proposal for AI refinement.

Use `RigWeightMethod::SurfaceGeodesic` in `RigBindingOptions.weight_method` (JSON
`"weightMethod": "surfaceGeodesic"`) when a resting arm or garment lies close to
another body surface. This native surface graph method seeds fitted bone capsules
on each connected component and propagates distances along actual mesh edges and
exact welded seams. It does not jump through empty space between an arm and waist.
Reviewed attachments use their declared anchors and rigid constraints remain exact.
`LocalDistance` is the default when the optional field is absent; existing options
and empty-field fingerprints remain readable. Geodesic output still requires
correct landmarks, reviewed ambiguous surfaces and the complete motion checks.

This is not a universal learned auto-rigger or a volumetric heat solver. Closely
touching limbs, garments and unusual anatomy can require explicit regions and
additional hints. Automatic output is always a candidate until verified.

## Weight diagnostics and AI refinement

Never assign unclassified vertices to the torso as a fallback. Use
`propose_rig_weight_regions(mesh, skeleton, checks)` (or the session method
`propose_weight_regions`) to obtain body-family regions plus `pendingVertices`.
The proposal compares fitted bone segments in baked mesh space. Ambiguous
boundaries and distant surfaces stay pending. Pending vertices receive provisional
distance weights for preview, have no confirmed region and prevent acceptance.
They must not overlap explicit regions. Resolve them with reviewed mesh evidence
and disjoint regions, not by clearing the list to make a check pass.

`inspect_rig_weights` / `session.inspect_weights` performs two headless checks:

- **Anatomy:** a confidently nearer fitted body family conflicts with more than
  50% of the vertex's weights. This catches hand vertices assigned to torso bones.
  The default confidence requires a 1.5% mesh-height separation between the two
  closest families and a nearest distance below 12% of height. These are geometric
  heuristics, not measured anatomy; incorrect landmarks require landmark review.
  Local hierarchy transitions (at most three edges, with the other pivot within
  14% of mesh height) remain compatible,
  allowing pelvis/thigh and chest/shoulder articulation without authorizing remote
  torso weights on hands. `anatomicallyCompatibleInfluences` exposes that support.
  For an explicit reviewed `RigidJoint`, expected anatomy follows the declared
  anchor; `geometricFamily` still records the closest geometric family and
  `anatomicalBasis` distinguishes `reviewedRigidConstraint` from
  `geometricProximity`. Hair and backpacks may extend beside unrelated limbs.
  This records caller intent, not independent proof of the attachment selection;
  constraint integrity, continuity and motion deformation gates still apply.
- **Continuity:** normalized bone-weight total variation exceeds 0.65 on a surface
  edge shorter than 2% of mesh height, and dominant bones are at least three
  hierarchy edges apart. Welded coincident UV-seam vertices are checked too.
  Ordinary parent/child transitions are not classified as distant-bone jumps.
  Separate surfaces are not connected merely because they are spatially close.

`RigWeightCheckOptions` exposes these thresholds and the diagnostic example cap.
Reports retain complete suspect indices and counts even when examples are capped.
Each example includes positions, named weights, candidate-local region indices,
allowed influences, expected family, distances, confidence and suggested bones.
The candidate retains `bindingOptions`; this provenance and pending state are
covered by its fingerprint. Older JSON without provenance remains readable.

Motion verification now also returns `anatomyStatus`, `weightContinuityStatus`
and `weightDiagnostics`. Retained severe edges carry both vertices' evidence,
`likelyCauses` and `nextStep`. Cause codes distinguish anatomical region conflicts,
adjacent influence discontinuities, unresolved vertices and cases that still need
pivot/axis/local-weight review. Cause codes describe likely contributors; a static
weight check does not prove that an axis or landmark is correct.

Use the following bounded AI loop after inspecting the GLB and fitting landmarks:

```rust,no_run
# use motionloom::api::character_authoring::rig::*;
# fn refine(session: &mut RigAuthoringSession, mut candidate: RigCandidate,
#           tests: RigVerificationOptions) -> Result<(), RigError> {
for _ in 0..3 {
    let report = session.verify(&candidate.id, &tests)?;
    if report.accepted {
        session.commit(session.revision(), &candidate.id)?;
        return Ok(());
    }
    // Inspect the report; revise incorrect landmarks before reselecting geometry.
    let correction = session.suggest_weight_refinement(&candidate.id, &tests.weight_checks)?;
    // The host may amend suggestions with reviewed regions and explicit pending indices.
    let next = session.refine_weights(session.revision(), &candidate.id, &correction)?;
    if next.binding.fingerprint == candidate.binding.fingerprint {
        session.discard(&next.id)?;
        break;
    }
    candidate = next;
}
// Unresolved results remain review drafts; never commit to bypass a failed gate.
# Ok(())
# }
```

Suggestions reselect suspect vertices and retained motion-edge endpoints using
confident fitted families; unresolved suggestions stay pending. `refine_weights`
checks the exact binding fingerprint and session revision, preserves untouched
regions and creates a new candidate with regenerated weights. It never changes
the failed candidate, inherits a verification receipt or accepts a repair without
new action tests. A direction failure may require a new `buildRig` with corrected
landmarks/rotations/axes instead of another weight-only iteration. Bound retries,
retain each receipt, and stop on no progress rather than changing tolerances.

## Persistent head and skin constraints

For reviewed continuous weight painting, provide `vertex_weights` in
`RigWeightRefinement` (JSON `vertexWeights`). Each `RigVertexWeight` names one
global mesh vertex and up to four positive `RigNamedInfluence` values summing to
1.0 within `1e-5`. Call `session.refine_weights()` with the exact candidate
fingerprint, then `inspect_weights()` and `verify()` on the new candidate.
These rows remain exact through smoothing and later refinements. Unchanged
rows and selections persist; the original candidate and its verification remain
unchanged. An empty refinement preserves the binding hash but creates a new
unverified candidate. Invalid indices, duplicate joints or rows, root/endpoints,
pending vertices and contradictions with allowed regions or rigid constraints
are rejected. Optional extension joints require an explicit allowed selection.

```json
{
  "expectedBindingFingerprint": "<current-candidate-fingerprint>",
  "regions": [], "pendingVertices": [],
  "vertexWeights": [
    {"vertex": 120, "influences": [
      {"bone": "neck", "weight": 0.35},
      {"bone": "head", "weight": 0.65}
    ]}
  ]
}
```

Painting is reviewed input, not an acceptance certificate. Independent checks
compare actual rows with the painted receipt (`PAINTED_WEIGHT_MISMATCH`) and keep
all anatomy, adjacency, head-rigidity and action-deformation gates enabled. Use
continuous throat and palm-web transitions; isolated finger shafts can enter
their own joint chain while the shared web follows the hand. Retain failed
receipts and correct their actual mesh evidence before accepting a candidate.

`RigBindingOptions.surface_regions` and `RigWeightRefinement.surface_regions`
accept optional caller-reviewed `RigSurfaceRegion` annotations. A
`RigSurfacePurpose::Body { family }` distinguishes actual neck/torso/limb surfaces
from a geometric proximity guess for anatomical validation.
`RigSurfacePurpose::Attachment { anchors }`
restricts deforming garments to their reviewed anchor joints. Both preserve
`geometricFamily`; `anatomicalBasis` and `surfaceRegionId` identify the provenance.
Missing annotations continue to use geometric evidence. Invalid/overlapping
indices, unknown or duplicate anchors, contradictory allowed regions and
pending/annotated overlaps are rejected. Empty annotations preserve old JSON and
fingerprints. Refinement preserves unrelated annotations, replaces explicitly
updated IDs and revokes annotations on vertices explicitly marked pending.

```json
{
  "surfaceRegions": [
    {"id": "body.throat", "vertices": [120, 121],
     "purpose": {"kind": "body", "family": "head"}},
    {"id": "garment.coat", "vertices": [900, 901],
     "purpose": {"kind": "attachment", "anchors": ["hips", "spine", "chest", "upper_chest"]}}
  ]
}
```

These annotations are reviewed input, not independently detected anatomy. Do not
label an uncertain body vertex as an attachment to clear a conflict. A valid
annotation does not waive constraint integrity, surface continuity, moving-head
rigidity or the original action strain gates. A narrow reviewed single-digit
weight region uses a wider finger support radius; measure its joints and review
the palm transition instead of blending neighboring finger chains indiscriminately.

Use `RigWeightConstraint::RigidJoint { id, vertices, joint }` to require a selected
surface to receive exactly one joint at weight 1.0. Constraints are optional in
`RigBindingOptions` and `RigWeightRefinement`; old JSON remains readable and empty
constraints do not change legacy binding fingerprints. Constraints persist during
distance weighting, smoothing, subsequent refinement and GLB export metadata.
They are included in the immutable binding fingerprint. Invalid indices,
root/endpoint targets, duplicate IDs, overlapping constraints, contradictory
regions and pending/constrained overlaps are rejected.

`propose_rig_head_constraints(mesh, binding, options)` and
`session.propose_head_constraints(candidate_id, options)` return a typed
`RigHeadConstraintProposal`. The JSON operation is `proposeRigHeadConstraints`.
The proposal uses static inspected geometry and fitted head/neck landmarks,
never source skins, joint names, weights or animations. By default, a geometric
head envelope returns `suggestedConstraints`, evidence and complete pending
indices. Suggestions do not become confirmed constraints automatically.

Review the mesh and pass `RigHeadConstraintOptions.reviewed_regions` with unique
IDs, global mesh vertex indices and `RigidHead` or `NeckTransition` kinds.
`search_vertices` optionally scopes remaining geometric suggestions. Keep face,
skull, eyes, teeth and attached beard together; select facial skin inside shared
body primitives per vertex. A rigid annotation creates a head constraint, while
a neck transition creates an ordinary allowed `neck`/`head` weight region.
Evidence records bounds, indices and caller review provenance. A reviewed flag
records the caller's annotation, not independent proof of anatomical completeness.
Unreviewed candidates remain pending and prevent acceptance.

```rust,no_run
# use motionloom::api::character_authoring::rig::*;
# fn head_review(session: &mut RigAuthoringSession, candidate: &RigCandidate,
#               reviewed_face_vertices: Vec<usize>, neck_vertices: Vec<usize>,
#               tests: RigVerificationOptions) -> Result<(), RigError> {
let mut scope = reviewed_face_vertices.clone();
scope.extend(&neck_vertices);
let mut reviewed_regions = vec![RigHeadRegion {
    id: "head.face".into(), vertices: reviewed_face_vertices,
    kind: RigHeadRegionKind::RigidHead,
}];
if !neck_vertices.is_empty() {
    reviewed_regions.push(RigHeadRegion {
        id: "head.neckTransition".into(), vertices: neck_vertices,
        kind: RigHeadRegionKind::NeckTransition,
    });
}
let proposal = session.propose_head_constraints(&candidate.id, &RigHeadConstraintOptions {
    reviewed_regions,
    search_vertices: Some(scope),
})?;
let next = session.refine_weights(session.revision(), &candidate.id, &proposal.refinement)?;
let diagnostics = session.inspect_weights(&next.id, &RigWeightCheckOptions::default())?;
let report = session.verify(&next.id, &tests)?;
// Only commit the exact candidate when the complete report accepts it.
if report.accepted { session.commit(session.revision(), &next.id)?; }
# Ok(())
# }
```

Refinement replaces incoming constraint IDs explicitly and preserves unrelated
constraints. Incoming correction regions, constraints and pending vertices must
be disjoint. Geometric automatic refinement cannot revoke rigid annotations;
review incorrect annotations explicitly instead of weakening validation.

`inspect_weights` adds `constraintStatus`, `constraintViolationCount` and capped
`constraintViolations`, retaining every suspect vertex index. Each violation
contains its constraint ID, target joint, actual/expected weight and named vertex
influences. `verify` adds `constraintStatus`, `headRigidityStatus`,
`distinctHeadMovingSamples` and per-action `headRigidity` measurements. Production
LBS positions are compared with the head matrix and inverse bind matrix, including
separate face components; selected surface edges are checked for shape changes.
Failures report action/phase, selected vertices, expected/actual positions, weights
and deviations, or the changed surface edge and constraint IDs.

Set `RigVerificationOptions.head_checks.required = true` for a head certificate.
Any explicit rigid head constraint also activates this acceptance gate. At least
three selected vertices, three measurable surface edges and two distinct frames
rotating head relative to its parent by five degrees are required. Missing head
surface or motion evidence is inconclusive. Defaults allow displacement up to
`height * 1e-5` and edge relative error 0.005; edges shorter than `height * 1e-4`
are excluded from relative-error measurements to avoid float quantization. All
selected vertices still receive the absolute displacement check. Old workflows
without head constraints may retain their existing acceptance behavior, but their
head status is inconclusive and is not a head certificate.

Rigid head constraints assume no facial articulation. Exclude articulated jaw,
facial expression bones, cloth and independently animated long hair. A head
certificate covers the caller-selected surfaces, not unannotated anatomy. GLB
export preserves source UV, material and image payloads; also audit UV accessors
and native rendered action frames when delivering a character showcase. Unchanged
UV data alone does not prove that the weighted facial surface keeps its shape.

## Verification and acceptance

Verification checks all 65 core nodes plus extensions, exact parents, unique
mapping, local/global transform consistency, inverse bind matrices, nonnegative
normalized weights and raw bind-pose vertex reconstruction.
Generated DSL is parsed using the existing MotionLoom parser and its ModelProfile
is compared with typed data. Both DSL fragments are covered by the binding fingerprint.

It uses MotionLoom's existing **production CPU pose evaluator**, rather than a
second animation implementation. Built-in +20-degree semantic probes exercise
available forward, side, bend, twist and turn channels independently. Both CC0
reference poses play the same probes and supplied Action Library poses at the
same phases. Model-global rotation deltas are measured relative to each rig's
calibrated neutral pose, so source proportions and bone rest rotations are not
compared as if they must be identical. Each reference result is retained, along
with per-bone errors and the worst error across both references.

Every sample evaluates the complete node hierarchy, including root and endpoints,
then skins the real target vertices and reports unique-edge stretch and vertex motion.
The eight worst severe edges include vertex indices and rest/posed lengths, so an
AI can locate the affected mesh region and constrain its influences on the next proposal.
Default tolerances are 10 degrees maximum rotation error, 4x maximum edge stretch,
and 1% severe edge fraction (outside 0.5x to 2x). These are explicit configurable
quality gates, not a claim of anatomical realism. Skin tests also run during the
single-joint probes. Action phases use a common 120 fps sample clock; each sample
records its actual evaluation frame.

The report separates `structureStatus`, `bindPoseStatus`, `directionStatus`,
`deformationStatus`, `anatomyStatus` and `weightContinuityStatus`.
Pending weight vertices also prevent acceptance. Missing explicit anatomy or fewer than three distinct moving
Action Library frames prevents acceptance. Each qualifying frame must move a
vertex by at least 1% of mesh height relative to the calibrated neutral pose.
Repeating a frame or supplying a static action does not add evidence.
Failures carry bone/action/phase and measured error; incomplete evidence remains
`inconclusive`. A commit requires a passing report for the exact candidate
fingerprint and revision. Exporting a draft does not mark it accepted.

Scene contacts, ground/foot IK, cloth, collisions, facial behavior and anatomical
volume are outside this certificate. Action contact metadata can be supplied,
but scene contact correction is not executed. Authored IK actions are rejected
by this verification path and require full Scene evaluation. Extensions have
structural/skin coverage but no extension-specific motion certificate.

## JSON dispatch

`CharacterService::execute` exposes the same API to a host without a CLI:

| Operation | Purpose |
| --- | --- |
| `rigStandard` | Return the full 65-node standard and both CC0 references |
| `inspectRigMesh` | Start a mesh-only rig session from a GLB path |
| `rigMeshData` | Read full mesh vertices, normals and triangles |
| `queryRigVertices` | Query nearby vertices for anatomical hints |
| `buildRig` | Generate immutable typed skeleton, skin and DSL candidate |
| `verifyRig` | Run probes, reference comparison and deformation checks |
| `inspectRigWeights` | Inspect anatomy conflicts and surface/UV-seam weight jumps |
| `proposeRigWeightRegions` | Suggest body-family regions while retaining ambiguous vertices as pending |
| `suggestRigWeightRefinement` | Return corrections from this candidate's stored verification receipt |
| `refineRigWeights` | Regenerate weights into a new candidate using fingerprint-checked corrections and persistent constraints |
| `proposeRigHeadConstraints` | Suggest mesh-only head regions; reviewed regions become rigid head constraints or neck transitions |
| `discardRig` | Drop a candidate and its verification receipt |
| `commitRig` | Accept an exactly verified candidate and advance revision |
| `exportRig` | Export a candidate for review or the accepted binding |

For example, after reviewing the named-weight evidence, a host can submit a
local correction. Replace the IDs, fingerprint and vertex index with values
from its actual candidate; this example does not authorize reusing S105 indices.

```json
{
  "operation": "refineRigWeights",
  "rigId": "hero",
  "candidateId": "rig-candidate-1",
  "expectedRevision": 0,
  "refinement": {
    "expectedBindingFingerprint": "from-the-exact-candidate",
    "regions": [{"vertices": [2571], "influences": ["forearm_l", "hand_l"]}],
    "pendingVertices": []
  }
}
```

Use the returned new candidate ID in `verifyRig`. To mark an affected vertex
uncertain, put it in `pendingVertices` and omit it from the correction's regions.
Untouched regions and pending vertices are preserved.

Call `schema` to discover coordinate rules, requests, operations and acceptance
policy. Returned candidate data includes all joint transforms, inverse bind
matrices, weights, complete Skeleton/BoneAxisMap DSL and ModelProfile DSL.
These use existing MotionLoom syntax; the standard version and fingerprints live
in typed authoring data and GLB extras, not newly invented DSL attributes.

`examples/rig_api_workflow.rs` shows a host exercising inspection, proposal,
existing Action Library verification and review export through the Rust API.
The regression suite includes poisoned input rigs, mesh instances/transforms,
65-node structure, optional extensions, GLB roundtrip, JSON dispatch, source
changes, candidate acceptance, static/repeated action rejection, vertex-level
skin tearing and intentionally reversed finger/arm axes. It requires no GPU or
external asset download.
