<!-- ========================================= -->
<!-- ========================================= -->
<!-- docs/IMAGE_TO_MESHASSET.md -->

# Image to MeshAsset workflow

Use this workflow when the requested authoring representation is an explicit
MeshAsset cage fitted to images. API contracts and request examples live in
[Mesh authoring](MESH_AUTHORING.md) and
[Mesh reference fitting](MESH_REFERENCE_FITTING.md); read both before
calling their APIs. This guide contains execution and review guidance; the
linked API guides maintain request contracts and JSON examples.

## Scope and representation

Use the [general MotionLoom skill](../skills/motionloom/SKILL.md) for routing when the
representation is undecided. Semantic Head parameter fitting, character design
edits and imported GLB binding are separate workflows. Do not force every task
with an image through this fitting loop.

Keep editable geometry in `GeometryAsset/Mesh/Vertex` and `Face`, with a
material-bound `MeshAsset` referencing that geometry. Use `mesh_asset_element`
to emit the canonical declarations; MeshAsset does not own inline vertices.
Preserve this editable source alongside any requested GLB export.

One view establishes a `camera_match`; two consistent orthogonal views support
`partial_multiview_fit`; consistent front/side/back views support
`multiview_fit`. Prefer shallow geometry for an unconstrained single-view match.
If the user requests a complete 3D asset, author volume using explicit shape
assumptions and identify unseen surfaces as inferred. The evidence label does
not turn a requested 3D model into a 2D deliverable or prove full reconstruction.

## Measure before fitting

Resolve original image bytes in the host and retain each analysis's `imageId`
and `contentHash`. Crop a turnaround sheet into separate views in the host;
keep those bytes and coordinate systems fixed. Generated design views are not
ground truth.

Analyze each reference and inspect its mask, internal edges and overlay. Correct
segmentation, side labels and feature hints before deforming geometry. Text,
shadows, hair and neighboring subjects must not become accidental anatomical
constraints. Treat unknown depth as uncertainty.

For a new cage, query the authoring schema and execute a GeometryRecipe through
`api::mesh_authoring`; use its general operations rather than a one-off external
model generator. For an existing explicit cage, start a session from its actual
source without rebuilding it merely to satisfy a recipe step. Procedural or
derived geometry needs the explicit conversion documented in Mesh authoring
before vertex editing.

Calibrate one fixed camera/frame and object transform per reference. Establish
the evaluation baseline with unchanged images, dimensions, metric weights and
quality gates. Use neutral review conditions and keep subdivision off while
fitting authored control vertices. Rust and WASM adapters run the same core
operations; select the adapter available in the host.

## Iterate and decide

1. Evaluate the accepted revision against every fitted view. Inspect silhouette,
   boundary, landmark, feature and depth residuals together with overlays.
2. Propose a small coherent edit based on that evidence. For position edits,
   supply exact `before` values and all six current evaluation fingerprints.
   Use `apply_position_proposal_to_session` in Rust, or the documented atomic
   position adapter with host-owned revision tracking. Do not patch vertices
   manually inside this measured loop.
3. When positions cannot express the observed geometry, use a topology proposal
   with current handles, semantic regions and fingerprints. Inspect its topology
   report, correspondence and invalidated features. Follow
   [feature rebinding](MESH_AUTHORING.md#feature-rebinding-and-new-baselines)
   before comparing or accepting the candidate; changed bindings require a new
   comparison setup, not reuse of stale reports.
4. Re-evaluate the candidate. Reject it if important features or any fitted view
   regress beyond the recorded tolerance, or visual review reveals an artifact.
   Use the session decision API for its numerical aggregate/per-view rule; the
   host performs the additional semantic and visual checks.
5. Continue from accepted evidence only. Stop at the configured gates, negligible
   progress, conflicting views, exhausted movement budget or unsupported
   topology. Use `stop_reason` to record the applicable cause.

No-op reconstruction does not require an artificial edit. Never accept a
candidate merely because apply succeeded: apply validates a mutation, while
evaluation and review establish whether it helped. Keep the last accepted source
and rejected evidence available throughout fitting.

## Deliver

Return accepted DSL, the recipe when used, revision/proposal history, reference
analyses, baseline/final evaluations and useful overlays. `artifact_manifest`
provides optional host output paths; it does not write files. Render and review
requested subdivision separately because fitting measures the control cage.
State the achieved gates, evidence class and unresolved likeness/depth issues.
Retain the original input and update the user's active source when authorized
by the task; this workflow adds no extra approval requirement.
