// =========================================
// =========================================
// src/character_authoring/rig/mod.rs

//! Mesh-only humanoid rig proposals and reproducible, headless binding verification.
mod binding;
mod build;
mod constraints;
mod geodesic;
mod math;
mod mesh;
mod session;
mod standard;
mod verify;
mod weights;

pub use binding::*;
pub use build::*;
pub use constraints::*;
pub use mesh::*;
pub use session::*;
pub use standard::*;
pub use verify::*;
pub use weights::*;

/// Discover the headless API contract without requiring a CLI, UI or ACP adapter.
pub fn rig_authoring_schema() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion":1,"standardId":HUMANOID65_STANDARD_ID,
        "coreNodeCount":65,"positionSpace":"bakedMeshWorldUnits",
        "coordinates":{"up":"+Y","facing":"+Z","anatomicalLeft":"+X"},
        "rotation":"globalQuaternionXyzw","matrixLayout":"columnMajor",
        "inputSkeletonPolicy":"ignoreInputSkinsWeightsAnimationsAndJointNames",
        "workflow":["inspectRigMesh","rigMeshData","buildRig","proposeRigWeightRegions","buildRig","proposeRigHeadConstraints","refineRigWeights","inspectRigWeights","verifyRig","suggestRigWeightRefinement","refineRigWeights","verifyRig","commitRig","exportRig"],
        "refinementLoop":["verifyRig","suggestRigWeightRefinement","refineRigWeights","verifyRig"],
        "geometryQueries":["rigMeshData","queryRigVertices"],
        "buildRequest":{"expectedGeometryFingerprint":"from inspection","reference":"character1","landmarks":{"forearm_l":[0.3,1.2,0.0]},"rotations":{},"extensions":[]},
        "bindingOptions":{"weightMethod":"localDistance","falloff":0.035,"smoothingIterations":3,"regions":[],"surfaceRegions":[],"vertexWeights":[],"pendingVertices":[],"constraints":[],"axisMap":null},
        "vertexWeights":{"shape":{"vertex":"global RigMesh index","influences":[{"bone":"canonical weighted joint ID","weight":"positive finite f32"}]},"maximumInfluences":4,"sumTolerance":0.00001,"contract":"Exact reviewed weights survive smoothing and refinement, must obey regions, rigid constraints and attachment anchors, and remain subject to independent anatomy, continuity and motion checks"},
        "weightMethods":{"localDistance":"Default capsule proximity","surfaceGeodesic":"Mesh-edge distances from fitted capsule seeds; no jumps across loose or touching surfaces"},
        "surfaceRegions":{"vertexSpace":"globalRigMeshIndices","purposes":{"body":{"family":["torso","head","arm_l","arm_r","leg_l","leg_r"]},"attachment":{"anchors":"canonical weighted joint IDs"}},"provenance":"Caller-reviewed surface intent; geometricFamily remains visible. Annotations are fingerprinted and never waive continuity, constraint or motion strain gates."},
        "weightRegions":{"vertexSpace":"globalRigMeshIndices","allowedInfluences":"canonicalJointIds","extensionsRequireRegion":true},
        "weightChecking":{"checks":["anatomicalFamilyConflict","surfaceAndWeldedSeamWeightContinuity","rigidJointWeightConstraints"],"uncertainPolicy":"pendingVertices; never a confirmed torso fallback","evidence":["vertex","position","regionIndex","surfaceRegionId","allowedInfluences","namedWeights","expectedFamily","geometricFamily","anatomicalBasis","anatomicallyCompatibleInfluences","anatomicalConfidence","suggestedInfluences"],"refinement":"Exact binding fingerprint, disjoint corrected regions and pending vertices; always creates a new unverified candidate"},
        "headConstraints":{"operation":"proposeRigHeadConstraints","kind":"rigidJoint","vertexSpace":"globalRigMeshIndices","reviewedRegions":["rigidHead","neckTransition"],"unreviewedPolicy":"suggestions stay pending until caller-reviewed","fingerprintProtected":true},
        "verification":{"headChecks":{"required":false,"maximumRigidDeviationHeightFraction":0.00001,"maximumEdgeRelativeError":0.005},"headGate":"Required when headChecks.required or any head rigid constraint exists; selected surface and two distinct head-relative moving frames are necessary","builtInSemanticProbes":true,"referenceModels":["character1","character2"],"requiredDistinctMovingActionFrames":3,"minimumMotionRelativeToNeutralHeightFraction":0.01,"evaluationFps":120,"deformationDiagnostics":"Up to eight severe unique edges with named weights, region provenance, anatomical evidence, likely cause codes and next step per sample","actions":[{"librarySource":"ActionLibrary DSL text","actionId":"sprint_standard_loop","phases":[0.2,0.5,0.8]}],"statuses":["pass","fail","inconclusive"]},
        "commitPolicy":"Requires verification of the exact immutable candidate; inferred anatomical pivots or incomplete action evidence are inconclusive.",
        "scope":"Model-space binding and Pose action verification. No scene contact, collision, cloth or anatomical volume certificate."
    })
}

#[derive(Debug, thiserror::Error)]
pub enum RigError {
    #[error("Invalid rig request: {0}")]
    Invalid(String),
    #[error("Unsupported mesh input: {0}")]
    Unsupported(String),
    #[error("Rig item not found: {0}")]
    Missing(String),
    #[error("Stale rig revision: expected {expected}, current {current}")]
    Stale { expected: u64, current: u64 },
    #[error("Rig source changed; inspect the mesh again")]
    SourceChanged,
    #[error("Binding has not passed verification for this candidate and test suite")]
    NotVerified,
    #[error("MotionLoom evaluation failed: {0}")]
    Evaluation(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
