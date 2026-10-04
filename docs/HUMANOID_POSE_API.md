<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/HUMANOID_POSE_API.md -->

# Humanoid rest-pose and knee inference API design

**Status: proposed contract; not implemented or callable.** This document designs
headless additions for `motionloom::api::character_authoring::rig`. The existing
authoring implementation is public; these proposed additions are not implemented
by its migration and do not change MotionLoom DSL.
The implemented workflow is documented in [Humanoid binding](HUMANOID_BINDING.md).

## Existing support and the missing contract

The current `RigBuildRequest` accepts arbitrary mesh-space landmarks and global
joint rotations. It can fit both A-pose and T-pose geometry when callers supply
those data. The character templates generate T-pose geometry, but that is not a
dedicated rig rest-pose API. There is no explicit rest-pose classification,
pose-constraint report, A/T preview request, surface-section query or knee candidate
inference operation. `queryRigVertices` returns nearby vertices only.

Add explicit support for both poses. Keep existing `buildRig`, request structs,
serialized requests and acceptance behavior unchanged. New operations compose
the current fitter, binder and production pose evaluator instead of introducing
another animation implementation. No CLI, UI or ACP layer is required.

## Shared data contract

Every geometry request includes the rig session ID, expected revision and
`expectedGeometryFingerprint`. Every proposal includes an immutable proposal ID,
proposal fingerprint, algorithm version, parameters and selected-region fingerprint.
Candidate-dependent operations also require the binding fingerprint.

Positions are inspected mesh-space world units; +Y is up, +Z is forward and +X
is anatomical left. Quaternions are global normalized XYZW. Indices use the full
inspected mesh. `RigSurfaceSelection` contains explicit `triangleIndices`; its
fingerprint includes the ordered, deduplicated indices and geometry fingerprint.
No new selection-registration service is required. A caller without a selection
can supply leg seed vertices and request a region proposal in `proposeRigKnees`;
the returned mask is unconfirmed until selected by the AI for assessment.
Node/material names and discarded source skins are never
anatomical evidence. Front-direction ambiguity remains explicit.

Preserve typed outcomes `pass`, `fail` and `inconclusive`. Invalid numeric values,
stale fingerprints, invalid selections and unsupported input formats use a typed
`PoseAuthoringError`; valid but ambiguous anatomy returns an inconclusive report.
An anatomical score is a deterministic ranking score, not a calibrated probability.

## Proposed operations

| JSON operation | Rust function or session method | Result |
| --- | --- | --- |
| `inspectRigRestPose` | `inspect_rig_rest_pose` | Measured pose, frame evidence and ambiguity |
| `queryRigSections` | `query_rig_sections` | Triangle-intersected sections and surface evidence |
| `proposeRigKnees` | `propose_rig_knees` | Ranked internal pivot candidates per leg |
| `evaluateRigKnees` | `evaluate_rig_knees` | Comparative bend/skin reports for proposed candidates |
| `buildPosedRig` | `propose_posed` | Immutable rig candidate plus pose/landmark provenance |
| `evaluateRigPose` | `evaluate_rig_pose` | A/T/custom preview pose, skin data and existing Action DSL |
| `verifyPosedRig` | `verify_posed` | Existing verification plus pose, fit and coverage gates |
| `commitPosedRig` | `commit_posed` | Accept the exact fully verified posed candidate |

The Rust functions return `Result<TypedReport, PoseAuthoringError>`. Posed sessions
wrap the current authoring core and keep additional receipts. Posed candidates
must not be committed through the legacy `commitRig` path, which lacks those gates.
Draft export can reuse the existing GLB exporter with an explicit draft status.

## A-pose and T-pose

### Pose definitions

`SourcePoseSpec.kind` accepts `auto`, `aPose`, `tPose` or `custom`.
`auto` requests analysis and may remain inconclusive. A supplied label is a
constraint to check against geometry, not permission to overwrite it.

`PoseSpec` for a target preview accepts `aPose`, `tPose` or `custom`. For T-pose,
arms are horizontally extended in the canonical frame. A-pose previews carry
explicit left/right arm-lowering angles; the proposed default is 35 degrees.
That preview default is not an assertion that every source A-pose uses 35 degrees.
Custom previews carry canonical global joint frames or landmark targets.
The base A/T preview uses straight elbows and legs; palm roll must be declared
or remain an inspectable unresolved assumption.

The measured arm elevation is the signed angle between shoulder-to-elbow direction
and the horizontal plane: horizontal is 0 degrees, lowered arms are negative.
Return both arms separately. Default classification policy considers near-horizontal
arms within 5 degrees a T-pose and lowered arms between 10 and 75 degrees below
horizontal an A-pose, only when the remaining chain checks support that label.
These configurable bands are authoring policy, not anatomical facts.
Bent elbows, crossed arms or asymmetric measurements can produce `custom` or
`unknown`; they must not be coerced into a symmetric A-pose.

### `inspectRigRestPose`

Inputs are a geometry receipt, explicit anatomical landmarks and optional arm/leg
selections. Missing landmarks can use reference-scaled priors for search only;
the report identifies each inferred anchor and cannot claim confirmed classification
from those priors alone. Analysis checks:

- shoulder-to-elbow and elbow-to-wrist directions and per-side arm elevation;
- elbow straightness, bilateral differences and palm/roll evidence;
- pelvis/hip/knee/ankle ordering and segment degeneracy;
- supplied source-pose constraints and front-direction ambiguity.

`RigRestPoseReport` returns measured angles, per-joint frames, evidence vertex IDs,
assumed landmarks, mismatch diagnostics and separate classification/frame statuses.
`PoseMismatch` includes the measured and declared constraints. The operation is
read-only and does not rotate or bake mesh vertices.

### `buildPosedRig`

Use a new `PosedRigBuildRequest` envelope containing:

```text
build: existing RigBuildRequest
binding: existing RigBindingOptions
sourcePose: SourcePoseSpec
calibrationReference: character1 | character2
poseInspectionReceipt: exact rest-pose report ID and fingerprint
kneeSelection: optional exact proposal/assessment IDs and chosen candidate IDs
```

The `build` field contains the full landmark map, not just arm or knee edits.
Selected knee candidates map to `lower_leg_l` and `lower_leg_r`. Conflicting
explicit landmark values are rejected rather than silently overwritten.

Preserve geometry in its supplied bind pose. Fit shoulder/elbow/wrist and
hip/knee/ankle positions to that pose; derive frame directions and use an explicit
roll hint where necessary. Compute local transforms and inverse bind matrices
from those fitted global transforms. Separately derive calibration against the
chosen CC0 reference, retaining source pose and anatomical proportions.

The result contains the current `RigCandidate`, `poseFitStatus`, measured source
pose, per-joint frame provenance, selected landmark evidence and a posed-candidate
fingerprint covering all of them. Raw inverse-bind reconstruction must reproduce
the inspected mesh before calibration.

Emit existing `Skeleton` and `ModelProfile`/`BoneAxisMap` DSL with roundtrip checks
against typed rendering data. Do not add `sourcePose` to DSL. The final DSL and GLB
must retain all frames/calibration needed for playback without the authoring report.
Evidence and inference scores are authoring metadata, not hidden playback state.

### `evaluateRigPose`

Input is an exact posed candidate and a target `PoseSpec`. The production evaluator
returns local/global joint transforms, posed vertex positions, regional distortion
and an existing Action/Pose DSL fragment. A/T rest previews use reference-derived
canonical frames and requested arm lowering, not uncalibrated Euler-axis guesses.

The preview is transient: it does not change source geometry or inverse bind
matrices. Applying an A-pose preview to a T-pose-bound model and fitting an
original A-pose mesh are separate operations. Baking a preview into a new mesh
would require a new geometry fingerprint, reinspection and fresh binding; it is
outside this initial contract.

## Knee position inference

### Required search evidence

The AI supplies left/right hip and ankle anchor estimates plus leg surface
selections, or leg seeds for unconfirmed region proposals. Anchors carry their
provenance. Hip-to-ankle height fractions can bound a search when needed, but do
not establish the knee. Uncertain hip or ankle positions propagate to the result.
Region proposals use mesh adjacency and spatial evidence around those anchors;
connected or touching legs must expose segmentation ambiguity. Assessment requires
the exact selected region fingerprint, including for an AI-selected proposed mask.

A source A-pose label describes the arms; it cannot determine knee position.
The leg geometry and bend evidence must determine each knee independently.
The internal pivot and visible kneecap are separate returned features.

### `queryRigSections`

Input includes a selected leg, hip/ankle anchors, a normalized search interval
along their segment, sample count and local plane normals. Intersect selected
triangles with each plane; do not depend on finding vertices at an exact Y.
For a visibly bent leg, accept a caller-supplied piecewise centerline and sample
planes along it. Ambiguous straight-line searches remain inconclusive.

Return section polylines, closed/open status, contributing triangle/edge IDs,
intersection edge parameters, center estimates, area, width, front/back extent,
shape change and connected-component identity. A closed polygon supports area
and interior-center calculations; open or multiple contours expose those limits
and are never silently merged. Garment contours must not be treated as body
sections unless the caller explicitly chooses that assumption.

### `proposeRigKnees`

Combine inspectable evidence rather than returning a single height guess:

1. Scan section size/shape changes and centerline direction around possible knees.
2. Inspect topology around those sections for usable bend loops; topology supplies
   support, not proof that the artist placed a joint there.
3. Estimate front/back placement from the selected leg surface and any visible
   patellar feature. Estimate an internal pivot within the supported section.
4. Use hip/ankle distances and bilateral similarity as soft plausibility terms.
   Mirror a missing side only with explicit symmetry permission and mirrored provenance.
5. Rank several distinct candidates, retaining uncertainty in height and depth.

Return `RigKneeProposal` with per-side candidate IDs, positions, supporting section,
vertex and triangle IDs, scoring terms, uncertainty bounds, provenance
(`geometrySupported`, `mirrored` or `referencePrior`), diagnostics and status.
Scores must expose weights and assumptions. Deterministic inputs and algorithm
versions produce identical ordering; tied scores use stable candidate IDs.

Covered legs, boots, touching limbs, sparse topology or uncertain anchors can
leave the whole search inconclusive. Do not call a reference prior detected anatomy.

### Proposed request and response examples

The following JSON is a proposed contract illustration. Placeholder IDs must be
replaced by real inspection/selection receipts after implementation.

```json
{
  "operation": "proposeRigKnees",
  "rigId": "target",
  "expectedRevision": 0,
  "expectedGeometryFingerprint": "<inspection fingerprint>",
  "request": {
    "legs": [
      {
        "side": "left",
        "selection": {"triangleIndices": [201, 202, 203]},
        "hip": {"position": [0.088, 0.975, 0.015], "provenance": "aiEstimate"},
        "ankle": {"position": [0.063, 0.105, -0.023], "provenance": "aiEstimate"}
      }
    ],
    "searchInterval": [0.2, 0.8],
    "sectionSamples": 48,
    "maximumCandidatesPerSide": 5,
    "allowMirroredFallback": false
  }
}
```

Triangle indices above illustrate the integer type; replace them with the entire
selected leg surface from the inspected mesh. The example anchors describe S104
only. `searchInterval` is a fraction of the
hip-to-ankle segment, starting at the hip, not a universal mesh Y interval.

```json
{
  "proposalId": "<immutable proposal ID>",
  "proposalFingerprint": "<proposal fingerprint>",
  "geometryFingerprint": "<inspection fingerprint>",
  "algorithmVersion": "knee-sections-v1",
  "status": "inconclusive",
  "candidates": [
    {
      "id": "left-knee-0",
      "side": "left",
      "position": [0.075, 0.555, 0.012],
      "provenance": "geometrySupported",
      "supportVertexIds": [101, 102, 103],
      "supportSectionIds": ["<actual section ID>"],
      "requiresBendAssessment": true
    }
  ],
  "diagnostics": ["Illustration only; no inference has been executed"]
}
```

Vertex IDs above are illustrative integers, not measured S104 evidence. The
implemented response must also carry numeric scoring terms, uncertainty bounds
and region/parameter fingerprints. This abbreviated example does not represent
a successful detection or a measured probability.

### `evaluateRigKnees`

Inputs are an exact knee proposal, selected candidate IDs, a complete base fit,
binding options, bend-plane hints and action tests. Hip and ankle anchors remain
fixed during a comparison unless the request explicitly supplies alternatives.
When joint centers and weights both change, report separate trials with existing
weights and recomputed weights so the source of improvement is inspectable.

For each candidate, create a temporary rig, then evaluate a configurable flexion
sweep such as 0, 30, 60 and 90 degrees with the production evaluator. The 0-degree
sample alone is not moving evidence. Also run actual sprint and seated actions
against both CC0 references at identical phases. A knee flexion plane follows
canonical forward and the hip/knee/ankle chain; ambiguous roll remains inconclusive.

Return per-candidate local/global transforms and geometry metrics:

- distance from pivot to the supported cross-section interior;
- coincidence of the visible bend region with supported knee loops;
- thigh/shin influence coverage and unwanted pelvis influence;
- severe-edge counts, stretch, collapse and remaining volume limitations;
- endpoint trajectories and per-bone direction errors for each action sample.

Report metrics separately; a weighted aggregate score cannot hide a hard failure.
An AI selects a supported candidate or revises the anchors/region and repeats.
Read-only assessment does not mutate a committed binding or accept a candidate.

## Verification and acceptance

`verifyPosedRig` composes existing structure/bind/direction/deformation checks
with `poseFitStatus`, `landmarkFitStatus` and `weightCoverageStatus`. An authoring
receipt records confirmed selections, unresolved roll/facing, frame/section
evidence, algorithm parameters and the exact action test suite.

Pose labels alone, high proposal scores and manually supplied coordinates cannot
override failed skin tests. Missing evidence or ambiguous anatomy stays
inconclusive. Required moving Action samples retain the current verification
minimum; comparative knee sweeps provide additional targeted evidence.
Thresholds for coverage and geometric fit are explicit, scale-aware and recorded
per target. S104's height bands are regression fixtures, not global defaults.

`commitPosedRig` requires all gates to pass for the exact posed candidate,
geometry, selections, proposal choices and test-suite fingerprints. Changing a
landmark, frame, region, pose constraint or geometry invalidates that receipt.
Ground/seat contact, cloth, collision and anatomical volume retain separate
limitations. A candidate can improve visibly while remaining an unaccepted draft.

For bounded deterministic work, section queries accept 8-256 samples and knee
proposals accept 1-8 candidates per side. Assessment evaluates at most eight
selected candidates per side in one call; larger searches use separate immutable
proposals. Reject invalid limits and degenerate/non-finite anchors before processing.
Reports record tolerances, normalization and every sampled angle/action phase.

## Required regression evidence for implementation

- Equivalent A-pose and T-pose meshes reproduce their own raw bind geometry and
  retarget the same actions without outward arm drift.
- A/T pose labels with conflicting measured geometry are reported explicitly.
- Left/right asymmetry, swapped facing and unresolved palm roll cannot silently pass.
- Translating/scaling equivalent geometry yields corresponding proposals in
  inspected mesh space; stale geometry and topology receipts are rejected.
- Several leg proportions and cross-section layouts produce independently supported
  candidate sets instead of the same normalized knee height.
- S104's low hip/knee and pelvis-dominated thigh regressions fail targeted coverage
  and fit checks even if hierarchy or rotational-delta checks pass.
- Garments, touching legs, open sections and insufficient topology yield explicit
  uncertainty rather than a claimed detected knee.
- A visually promising knee candidate with wrong weights or reversed bend frames
  fails assessment; proposal IDs and scores cannot bypass acceptance.
- Generated rendering DSL roundtrips without new parser tags or attributes, and
  legacy authoring requests keep their existing behavior.
