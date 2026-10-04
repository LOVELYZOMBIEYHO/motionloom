<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/HUMANOID_BINDING.md -->

# Mesh-only humanoid binding guide for AI callers

Read this guide before generating a humanoid rig or diagnosing an Action Library
deformation. It records the S104/S105 binding experience and a repeatable inspection,
fitting and verification workflow. Positions from that example are not defaults
for another mesh.

## Capability and ownership

MotionLoom supplies Scene evaluation, Action playback, rig diagnostics, DSL
parsing and mesh-only 65-node authoring. Import
`motionloom::api::character_authoring::rig`; see [Rig Authoring](RIG_AUTHORING.md)
for the typed workflow. The caller owns the session and supplies mesh bytes;
input GLB skeletons never guide anatomy fitting.

`CharacterService::execute` supplies a headless JSON adapter. Native path
operations retain source-change checks; portable callers use
`RigAuthoringSession::from_glb_bytes` or `CharacterService::inspect_rig_bytes`
and byte-returning exports. Read `schema` for platform capabilities. Current
operations are:

| Operation | Use |
| --- | --- |
| `rigStandard` | Read the fixed core, reference frames and provenance |
| `inspectRigMesh` | Inspect static GLB geometry while excluding its input rig |
| `rigMeshData` | Read positions, normals, triangles and part ranges |
| `queryRigVertices` | Query nearby vertices; this does not detect a joint center |
| `buildRig` | Propose skeleton, skin, calibration and existing DSL |
| `verifyRig` | Evaluate hierarchy, bind reconstruction, directions and deformation |
| `inspectRigWeights` | Diagnose anatomical region conflicts and adjacent weight jumps |
| `proposeRigWeightRegions` | Propose regions with explicit pending vertices |
| `suggestRigWeightRefinement` | Suggest corrections from the candidate's verification receipt |
| `refineRigWeights` | Rebuild weights in a new candidate, then require fresh verification |
| `proposeRigHeadConstraints` | Propose head selections and persistent constraints for caller review |
| `commitRig` | Accept an exactly verified immutable candidate |
| `discardRig` | Discard a candidate |
| `exportRig` | Export a review candidate or an accepted binding |

[Pose and knee API design](HUMANOID_POSE_API.md) describes proposed additions.
Those additions are not callable yet. Use the actual host's `schema` operation
to discover available capabilities before sending a request.

## What remains fixed, and what must be fitted

`humanoid65_v1` fixes core names, parents and one-to-one mappings to the known
CC0 Character1/Character2 references. It includes one root, 22 body nodes,
30 finger articulation nodes and 12 terminal nodes. Root and terminals are
not skin influences. Optional extensions preserve core parents.

Every target needs its own joint positions, bone lengths, joint frames, inverse
bind matrices and skin weights. Reference positions provide an initial prior.
They do not identify the target's shoulder, hip or knee. An A-pose mesh can have
the same hierarchy as a T-pose mesh while having different bind transforms.

There are three distinct poses to track:

- **Mesh bind pose:** the supplied static geometry and fitted joint transforms
  used to construct inverse bind matrices.
- **Calibrated neutral pose:** the baseline used by MotionLoom's retargeting and
  direction comparison. This may differ from the mesh bind pose.
- **Animated pose:** the evaluated Action, including any enabled Scene solving.

Never silently rotate the mesh to T-pose and keep its old inverse bind matrices.
Never assume a neutral-pose render must equal the raw bind-pose geometry.

## Seven-step API workflow

Use `motionloom::api::character_authoring::rig` for these steps. This is a
headless API workflow; the caller reviews evidence and the generated DSL remains
the playback source of truth.

1. **Inspect the mesh.** Start with `RigAuthoringSession::from_glb_path()` or
   `from_glb_bytes()`, then read `session.mesh()`. Ignore the source GLB's skeleton,
   joint names, weights and animations as fitting evidence.
2. **Locate and calibrate joints.** Review mesh surfaces and cross-sections for
   shoulders, elbows, wrists, hips, knees, ankles and finger roots. Supply
   `RigBuildRequest.landmarks`, `rotations` and `RigBindingOptions.axis_map` for
   the actual A/T-pose and motion directions.
3. **Generate the rig.** Call `session.propose()` to create the standard 65-node
   hierarchy, fitted transforms, inverse binds, initial skin weights and DSL.
4. **Review anatomical and attachment selections.** Call
   `session.propose_head_constraints()` and review its proposed regions. Use
   `RigWeightConstraint::RigidJoint` with `joint: "head"` for reviewed rigid
   facial surfaces, and `RigSurfaceRegion` for body, garment and attachment intent.
   Leave unconfirmed vertices pending; do not rigidly constrain a whole body mesh.
5. **Refine local weights.** Call `session.refine_weights()` with the reviewed
   constraints, influence regions and, where needed,
   `RigWeightRefinement.vertex_weights`. Correct throat transitions, loose sleeve
   support and abrupt finger-root weights in a new immutable candidate.
6. **Inspect and verify repeatedly.** Call `session.inspect_weights()`, then
   `session.verify()` with `head_checks.required = true`, semantic probes and
   multiple phases of the selected actions. Use the returned vertex, bone,
   action and phase evidence to reselect regions or refit joints, regenerate
   weights and repeat verification without weakening the gates.
7. **Commit, export and review playback.** Only after the exact candidate passes,
   call `session.commit()` and `session.export_committed()`. Use that export and
   its generated ModelProfile in the DSL, inspect actual Scene poses with
   `SceneRenderer::evaluate_rig()`, and review renders and the delivered video.
   Independently compare the exported UV, material and image payloads with the source.

### Head shape and UV responsibilities

| Purpose | Actual API or check |
| --- | --- |
| Prevent facial distortion | `propose_head_constraints()` -> caller reviews selections -> `RigWeightConstraint::RigidJoint` targets `head` -> `refine_weights()` applies the constraint |
| Check head shape | `inspect_weights()` checks constraint compliance; `verify()` with `head_checks.required = true` checks head-surface rigidity in moving poses |
| Preserve UVs, materials and textures | After `commit()`, `export_committed()` calls `export_rig_glb()`, which retains the original UV/material/image payload while replacing the source rig and skin weights |

This workflow does not unwrap UVs, edit UV coordinates or regenerate textures.
Reviewed rigid facial vertices follow `head` at weight 1.0; the throat keeps a
separate blended transition. This prevents incorrect skin deformation from
stretching the apparent texture. Unchanged UV coordinates alone do not prove
that the face retains its shape.

**Export payload comparison is currently an external audit, not a public
character-authoring API.** S105 uses
`motionloom-example/showcase/s-000105/scripts/audit_head_deformation.py`:
`audit_payload()` compares source/exported `TEXCOORD_*` bytes, material assignments,
material and texture metadata, and embedded image bytes. The same script checks
head geometry using the renderer's actual final Scene matrices. Other hosts can
perform equivalent checks without depending on that showcase script. A mesh
without UVs reports the UV check as not applicable and still receives head-shape
checks. Payload preservation and head-shape verification are separate evidence.

For callable types and request details, see
[persistent head and skin constraints](RIG_AUTHORING.md#persistent-head-and-skin-constraints).

## Detailed fitting and verification notes

### Inspect and freeze geometry

Start a separate session for the selected character. Inspection excludes input
skins, animation, morph targets, joint names, joint transforms, `JOINTS_*` and
`WEIGHTS_*` as fitting evidence. Static non-joint object transforms are baked.
The original GLB remains unchanged.

Record source and geometry fingerprints, selected mesh parts and normalization.
The existing mesh-only API centers X/Z, floors Y and optionally normalizes height.
Use right-handed +Y up, +Z forward and +X anatomical left. A caller-supplied
`facingRotation` is needed when mesh symmetry leaves facing ambiguous.

Landmarks and vertex indices refer to the inspected mesh space, across all parts.
Comparing them with unnormalized source coordinates can create false errors.
Geometry changes invalidate the previous landmarks, selections and verification.

### Estimate anatomy from the mesh

Read vertices and triangles, inspect front and side views, and query the surface
near possible joint locations. Supply hip, knee and ankle anchors as a chain;
review shoulder, elbow and wrist together in the same way.

A surface vertex is not an internal joint center. A kneecap lies on the surface;
the knee pivot lies inside the leg. Use surrounding cross-sections, topology,
front/back depth and the proposed motion to infer the center. Retain alternatives
when garments, overlapping limbs or sparse geometry obscure the evidence.

Supply explicit landmarks for all 52 non-root, non-terminal core nodes before
the current API can accept a binding. This confirms caller-supplied coordinates,
not anatomical ground truth. Empty landmarks produce an unconfirmed draft.
Unspecified terminals follow the fitted terminal segment.

### Fit joint frames and rest calibration

`RigBuildRequest.landmarks` uses inspected mesh-space positions.
`RigBuildRequest.rotations` uses global normalized XYZW quaternions. The builder
derives local transforms and inverse bind matrices. It aligns reference frames
to fitted segment directions; explicit rotations can refine roll, palms and fingers.

Check facing, anatomical side, elbow/knee bend planes and palm orientation.
An A-pose needs frames fitted to its lowered arms. Inspect the emitted
`ModelProfile` and rest calibration rather than copying T-pose arm coordinates.
The current request has no explicit A-pose/T-pose declaration or automatic
pose-classification operation.

### Bind anatomically appropriate regions

The current binder uses segment distances, side filtering, welded adjacency
smoothing and at most four normalized influences per vertex. Explicit,
non-overlapping regions restrict allowed influences throughout smoothing.

Inspect actual influence coverage before evaluating motion:

- Arms and hands need meaningful upper-arm, forearm and hand influence.
- Upper thighs need upper-leg influence instead of following the pelvis alone.
- Knee loops need a transition between upper and lower leg.
- Ankle loops need a transition between lower leg and foot.
- Nearby limbs must not attract each other's vertices through empty space.

Zero dominant forearm or hand vertices on a detailed arm mesh is a useful warning.
Dominant counts alone are not a universal pass criterion: coarse meshes and small
bones can have valid blended influences without dominating a vertex.
Local-distance weighting can require additional regions near overlapping limbs
or clothing. Region thresholds must come from the target geometry.

### Evaluate the real skin and the actual driver

Check raw bind reconstruction before any neutral calibration. Preserve indices,
UVs, material assignments and embedded images when exporting. Then run independent
semantic probes and multiple distinct moving Action Library samples.

Use the same action, phase, root-motion mode and evaluator for target and reference.
Record each sampled frame. Render front, side and quarter views; reproduce the
exact time reported by the user as well as sampled phase contact sheets.

MotionLoom's public diagnostic APIs include
`SceneRenderer::evaluate_rig_frame`, `compare_humanoid_poses` and
`propose_rig_calibration`. Read the active driver and effective axes. A baked
reference driver can bypass semantic motion axes while retaining rest calibration;
changing a bypassed bend axis will not fix that driver's movement.
See [Rig diagnostics](RIG_DIAGNOSTICS.md) for the stage and comparison contract.

Rotation agreement is only one check. A correctly rotating skeleton can deform
the wrong vertices or bend at a misplaced pivot. Inspect skin displacement,
severe edges, regional coverage and the visible bend location independently.
Ground contact, foot IK, seated root lowering, cloth, collision and joint volume
require separate Scene evidence. An in-place sitting test does not establish
seat contact or feet on a floor.

### Refine and accept the exact candidate

The Rust flow is `RigAuthoringSession::from_glb_bytes` or `from_glb_path`,
`mesh`, `propose`, `verify`, then `commit` and `export_committed` only when
the report is accepted. The JSON sequence is:

```text
inspectRigMesh -> rigMeshData -> queryRigVertices
buildRig -> proposeRigWeightRegions -> buildRig -> inspectRigWeights -> verifyRig
proposeRigHeadConstraints -> review head selections -> refineRigWeights
inspectRigWeights -> verifyRig
suggestRigWeightRefinement -> inspect corrections -> refineRigWeights -> verifyRig
commitRig -> exportRig
```

Keep candidate IDs, revisions, binding fingerprints and test-suite fingerprints
together. Rebuild after changing landmarks, roll, regions or calibration.
Never reuse an old verification receipt for a changed candidate. Rendered evidence
and exported GLB must use the same candidate and generated ModelProfile.

An explicit draft export is useful for review but does not imply acceptance.
Report `structureStatus`, `bindPoseStatus`, `directionStatus`,
`deformationStatus` and `accepted` separately. Missing evidence is inconclusive.
Do not increase tolerances merely to make a defective preview pass.

Also report `anatomyStatus`, `weightContinuityStatus` and `weightDiagnostics`.
Unclassified vertices stay in `pendingVertices`; never fall back to torso bones.
Retained deformation edges include named weights, candidate-local region indices,
likely causes and a next step. Review uncertain geometry and incorrect pivots
before applying suggestions. If bounded refinement makes no progress, retain
a draft rather than clearing pending evidence. See the executable loop in
[Rig Authoring](RIG_AUTHORING.md#weight-diagnostics-and-ai-refinement).

The final rendering behavior remains expressible in existing `Skeleton`,
`ModelProfile`, `BoneAxisMap` and Action DSL. Store authoring evidence separately;
do not invent new DSL attributes for a standard version or an inspection score.

## S104 evidence and corrections

S104 selected one of three static base meshes. The active mesh was normalized
to height 1.8. These observations apply to that geometry only:

| Visible failure | Evidence | Correction |
| --- | --- | --- |
| Arms splayed during sprint at 6 s | T-pose reference positions did not fit A-pose arms; forearm/hand had no dominant vertices and arm vertices followed torso bones | Supply actual shoulder/elbow/wrist landmarks and restrict limb influences |
| Short-looking thighs and misplaced sitting knees at 16 s | Hip pivots were below the visible crotch, knees below/behind knee loops, and the upper-thigh region followed the pelvis | Refit pelvis/hip/knee/ankle and move the pelvis-to-thigh weight transition to the hip crease |
| Apparently successful direction check with a visibly bad mesh | Rotational deltas agreed but surface attachment did not | Add regional coverage and deformation review to direction comparison |

For this mesh the hip pivots moved from Y=0.825 to 0.975, knee pivots from
0.485 to 0.555, and ankle pivots from 0.085 to 0.105. Knee Z moved from about
-0.0224 to +0.012. The pelvis moved from Y=0.875 to 1.015. These are fitted
example values, not anthropometric rules or universal height fractions.

The Y=0.80-0.86 thigh band changed from predominantly pelvis influence to about
82% combined upper-leg influence. The Y=0.525-0.585 knee band blended about
51% upper leg and 49% lower leg. Such bands are useful fixture regressions;
another mesh needs newly identified regions rather than those numeric intervals.

The corrected six- and sixteen-second renders improved the visible pose. Full
direction and deformation checks still failed, so S104 remained an unaccepted
draft. This workflow does not claim that arbitrary GLBs can already be bound
automatically without anatomical hints.

## S105 hand and attachment review

The eight-input stress test exposed adjacent palm vertices assigned to hand and
torso regions. At the same idle/sprint phase, millimeter-long surface edges
became long strips. Diagnose the affected vertices and their named influences
before changing motion axes: a valid bone rotation cannot repair incorrect
surface attachment.

Use overlapping influence candidates around shoulder, wrist, neck and hip
transitions. Disjoint family restrictions prevent the binder's welded smoothing
from crossing the boundary. For lowered arms near the waist, inspect body skin
and garments separately: Euclidean proximity can attract a coat hem to a hand.
Review attachment parts explicitly. Long hair belongs to its head attachment,
while a backpack belongs to the torso; nearest limb distance is insufficient
evidence. Part ordering can differ even between two characters in the same GLB.

Depth estimates must come from the selected anatomical surface, not a mixture
of hair, clothing and skin. In S105, Iluyee's old elbow/wrist depth samples
selected long hair. Refit those joints from each body's mesh parts, and place
the wrist above the palm rather than at the outermost finger vertices. Rebuild
the skeleton, inverse binds and skin through the public API after any refit.

Transferred CC0 finger layouts remain priors. Separate actual distal finger
components using mesh topology before restricting a vertex to one finger
branch. Do not allow adjacent fingers to exchange influences merely because
their surfaces are close. Retain uncertainty when component separation or
knuckle locations cannot be confirmed. Small finger defects failed strict checks
in early S105 iterations and required reviewed surface-fork fitting and continuous
local weight refinement before the final candidates passed.

Review the exact candidate used by the default DSL and delivered video. Compare
asset hashes with the renderer's rig provenance, sample both hands at several
action phases, and decode final movie frames to compare with reviewed renders.
A corrected candidate stored beside an unchanged default scene is not a
delivered correction. Keep geometry, source assets and independent showcase
fixtures unchanged, and retain failing certificates rather than relaxing
thresholds or describing visual improvement as acceptance.

## Rigid facial surfaces after body binding

A standard skeleton does not guarantee correct facial weights. S105 facial skin
inside a shared body primitive followed upper chest/neck while separate beard,
eye and hair primitives followed head. Native head nods exposed relative drift
and stretched texture appearance despite byte-identical source UV coordinates.

Use the persistent head constraint workflow in
[RIG_AUTHORING.md](RIG_AUTHORING.md#persistent-head-and-skin-constraints). Review
actual face/skull/eye/teeth/beard surfaces per vertex, keep only the throat seam as
a neck transition, and verify production poses against the same rigid head
transform. Do not rigidly anchor an entire body primitive. Retain pending vertices
where geometry cannot distinguish anatomy. Accept head results only for the exact
constrained candidate, with UV payload comparison and native action review; full
binding acceptance still requires all other gates to pass.

## Surface transitions and attachment review

Do not estimate a pivot's depth from only the closest surface vertices. Inspect a
body cross-section containing its front and back, exclude hair and loose clothes,
and place the pivot inside that section. A correct action axis does not repair a
spine whose pivots alternate between the front and back surfaces. Place the head
rotation pivot near the base of the skull and retain a continuous throat transition.

Select connected garment panels before assigning nearby limb influences. A skirt
or tail anchored to hips must not alternate between left and right legs; a rigid
backpack should follow its reviewed torso anchor. Keep sleeves attached to the
arm chain. Use explicit `RigidJoint` constraints only for reviewed rigid surfaces,
never to hide a deforming body part. Uncertain selections remain pending.

Metric smoothing and continuous four-influence projection reduce short-edge
tearing. Reviewed rigid selections blend through compatible surface neighbors,
but these mechanisms cannot correct an incorrect anatomical selection or pivot.
Verify the new candidate against the unchanged direction, continuity, strain and
head-rigidity thresholds, then review the production renderer and final scene.

S105 refinement also exposed several selection traps. A primitive can contain
both a coat and shoes; inspect individual surfaces before assigning torso anchors.
A neck transition based only on height can rotate the trapezius with the neck:
measure the throat width and taper its influence across the actual cervical
surface. Duplicate skin shells and material seams need consistent fields even
when their tessellation differs. Topological smoothing alone can create different
weights at nearly identical positions; inspect the reported stretching edge as
well as the worst compression examples.

For fingers, identify the shared web and the isolated shaft before fitting the
phalanges. Keep a continuous hand-to-proximal-finger transition close to the
measured fork. A broad blend from hand directly into the distal joint can stretch
the fingertip during a fist pose. Review both sides independently; preserve
separation between neighboring digit chains. Lowered A-pose arms can be isolated
from the torso through mesh connectivity below the axilla, including duplicate
material primitives, rather than through a single X-coordinate threshold.

Use reviewed `RigSurfaceRegion` intent to record these selections and
`RigWeightRefinement.vertex_weights` when an exact continuous field is necessary.
`refine_weights()` validates the named rows, preserves constraints and creates a
new immutable candidate; `inspect_weights()` and `verify()` still control
acceptance. Preserve failed iteration receipts, keep uncertain geometry pending,
and promote the exact accepted export into the default DSL and rendered video.
