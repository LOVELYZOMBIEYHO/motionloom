// =========================================
// =========================================
// tests/character_rig_authoring.rs

use motionloom::api::character_authoring::{CharacterCommand, CharacterService, rig::*};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[test]
fn geodesic_weights_support_explicit_extension_attachments() {
    let m = mesh();
    let mut r = request(&m, true);
    r.extensions.push(RigExtension {
        id: "hair_root".into(),
        parent: "head".into(),
        position: r.landmarks["head"],
        rotation: [0., 0., 0., 1.],
    });
    let mut options = rigid_regions(&m);
    let region = options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["head"])
        .unwrap();
    region.influences = vec!["hair_root".into()];
    options.surface_regions = vec![RigSurfaceRegion {
        id: "hair".into(),
        vertices: region.vertices.clone(),
        purpose: RigSurfacePurpose::Attachment {
            anchors: vec!["hair_root".into()],
        },
    }];
    options.weight_method = RigWeightMethod::SurfaceGeodesic;
    let binding =
        bind_humanoid_skin(&m, build_humanoid_skeleton(&m, &r).unwrap(), &options).unwrap();
    let diagnostics = inspect_rig_weights(&m, &binding, &Default::default()).unwrap();
    assert_eq!(diagnostics.anatomical_conflict_count, 0);
    for &v in &options.surface_regions[0].vertices {
        assert_eq!(
            binding.skeleton.joints[binding.joints[v][0] as usize].id,
            "hair_root"
        );
        assert_eq!(binding.weights[v], [1., 0., 0., 0.]);
    }
}

#[test]
fn reviewed_painting_is_exact_persistent_and_requires_fresh_verification() {
    let mut session = RigAuthoringSession::from_glb_bytes(
        &fixture(false),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let mut options = rigid_regions(session.mesh());
    options.regions[0].influences = vec!["hips".into(), "spine".into()];
    options.smoothing_iterations = 12;
    options.weight_method = RigWeightMethod::SurfaceGeodesic;
    let original = session
        .propose(0, &request(session.mesh(), true), &options)
        .unwrap();
    let correction = RigWeightRefinement {
        expected_binding_fingerprint: original.binding.fingerprint.clone(),
        constraints: vec![],
        regions: vec![],
        surface_regions: vec![],
        pending_vertices: vec![],
        vertex_weights: (0..3)
            .map(|vertex| RigVertexWeight {
                vertex,
                influences: vec![
                    RigNamedInfluence {
                        bone: "hips".into(),
                        weight: 0.73,
                    },
                    RigNamedInfluence {
                        bone: "spine".into(),
                        weight: 0.27,
                    },
                ],
            })
            .collect(),
    };
    let painted = session
        .refine_weights(0, &original.id, &correction)
        .unwrap();
    for vertex in 0..3 {
        assert_eq!(painted.binding.weights[vertex], [0.73, 0.27, 0., 0.]);
    }
    assert_eq!(
        painted
            .binding
            .binding_options
            .as_ref()
            .unwrap()
            .regions
            .len(),
        options.regions.len()
    );
    assert!(
        session
            .candidate(&original.id)
            .unwrap()
            .binding
            .binding_options
            .as_ref()
            .unwrap()
            .vertex_weights
            .is_empty()
    );
    let check = session
        .inspect_weights(&painted.id, &Default::default())
        .unwrap();
    assert_eq!(check.constraint_status, BindingStatus::Pass);
    assert!(matches!(
        session.commit(0, &painted.id),
        Err(RigError::NotVerified)
    ));
    let next = session
        .refine_weights(
            0,
            &painted.id,
            &RigWeightRefinement {
                expected_binding_fingerprint: painted.binding.fingerprint.clone(),
                vertex_weights: vec![],
                ..correction.clone()
            },
        )
        .unwrap();
    assert_eq!(next.binding.fingerprint, painted.binding.fingerprint);
    assert!(matches!(
        session.refine_weights(0, &next.id, &correction),
        Err(RigError::Invalid(_))
    ));
    // A valid hash does not make painted metadata true: independent checks detect changed rows.
    let mut corrupt = next.binding;
    corrupt.weights[0] = [0.8, 0.2, 0., 0.];
    use sha2::{Digest, Sha256};
    let payload = (
        &corrupt.skeleton,
        &corrupt.joints,
        &corrupt.weights,
        &corrupt.profile,
        &corrupt.profile_dsl,
    );
    corrupt.fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(payload, corrupt.binding_options.as_ref().unwrap())).unwrap()
        )
    );
    let check = inspect_rig_weights(session.mesh(), &corrupt, &Default::default()).unwrap();
    assert_eq!(check.constraint_status, BindingStatus::Fail);
    assert!(
        check
            .constraint_violations
            .iter()
            .any(|v| v.cause == "PAINTED_WEIGHT_MISMATCH" && v.evidence.vertex == 0)
    );
}

#[test]
fn painted_weights_reject_invalid_rows_and_cannot_override_rigid_or_pending_vertices() {
    let m = mesh();
    let skeleton = build_humanoid_skeleton(&m, &request(&m, true)).unwrap();
    for row in [
        json!({"vertex":0,"influences":[{"bone":"hips","weight":0.5}]}),
        json!({"vertex":0,"influences":[{"bone":"hips","weight":0.5},{"bone":"hips","weight":0.5}]}),
        json!({"vertex":0,"influences":[{"bone":"root","weight":1.0}]}),
        json!({"vertex":0,"influences":[{"bone":"index_end_l","weight":1.0}]}),
        json!({"vertex":999999,"influences":[{"bone":"hips","weight":1.0}]}),
        json!({"vertex":0,"influences":[{"bone":"hips","weight":-1.0},{"bone":"spine","weight":2.0}]}),
    ] {
        let options: RigBindingOptions =
            serde_json::from_value(json!({"vertexWeights":[row]})).unwrap();
        assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    }
    let valid = RigVertexWeight {
        vertex: 0,
        influences: vec![RigNamedInfluence {
            bone: "spine".into(),
            weight: 1.,
        }],
    };
    for options in [
        RigBindingOptions {
            vertex_weights: vec![valid.clone(), valid.clone()],
            ..Default::default()
        },
        RigBindingOptions {
            vertex_weights: vec![valid.clone()],
            pending_vertices: vec![0],
            ..Default::default()
        },
        RigBindingOptions {
            vertex_weights: vec![valid.clone()],
            constraints: vec![RigWeightConstraint::RigidJoint {
                id: "pelvis".into(),
                vertices: vec![0],
                joint: "hips".into(),
            }],
            ..Default::default()
        },
        RigBindingOptions {
            vertex_weights: vec![valid],
            regions: vec![RigWeightRegion {
                vertices: vec![0],
                influences: vec!["hips".into()],
            }],
            ..Default::default()
        },
    ] {
        assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    }
}

// A shared primitive reproduces facial skin following chest while rigid attachments follow head.
fn head_vertices(m: &RigMesh) -> Vec<usize> {
    rigid_regions(m)
        .regions
        .into_iter()
        .find(|r| r.influences == ["head"])
        .unwrap()
        .vertices
}

#[test]
fn head_proposal_requires_review_and_repairs_shared_body_vertices() {
    let mut session = RigAuthoringSession::from_glb_bytes(
        &fixture(true),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let vertices = head_vertices(session.mesh());
    let mut options = rigid_regions(session.mesh());
    options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["head"])
        .unwrap()
        .influences = vec!["chest".into()];
    options.smoothing_iterations = 12;
    let original = session
        .propose(0, &request(session.mesh(), true), &options)
        .unwrap();
    let draft = session
        .propose_head_constraints(
            &original.id,
            &RigHeadConstraintOptions {
                search_vertices: Some(vertices.clone()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(draft.requires_review);
    assert!(!draft.suggested_constraints.is_empty());
    assert_eq!(draft.refinement.pending_vertices, vertices);
    assert!(draft.refinement.constraints.is_empty());
    let proposal = session
        .propose_head_constraints(
            &original.id,
            &RigHeadConstraintOptions {
                reviewed_regions: vec![RigHeadRegion {
                    id: "head.face".into(),
                    vertices: vertices.clone(),
                    kind: RigHeadRegionKind::RigidHead,
                }],
                search_vertices: Some(vertices.clone()),
            },
        )
        .unwrap();
    assert!(!proposal.requires_review);
    assert!(proposal.evidence[0].reviewed);
    let next = session
        .refine_weights(0, &original.id, &proposal.refinement)
        .unwrap();
    let check = session
        .inspect_weights(&next.id, &Default::default())
        .unwrap();
    assert_eq!(check.constraint_status, BindingStatus::Pass);
    assert_eq!(check.constraint_violation_count, 0);
    for v in &vertices {
        assert_eq!(
            next.binding.skeleton.joints[next.binding.joints[*v][0] as usize].id,
            "head"
        );
        assert_eq!(next.binding.weights[*v], [1., 0., 0., 0.]);
    }
    assert!(
        session
            .candidate(&original.id)
            .unwrap()
            .binding
            .binding_options
            .as_ref()
            .unwrap()
            .constraints
            .is_empty()
    );
    assert!(matches!(
        session.commit(0, &next.id),
        Err(RigError::NotVerified)
    ));
    let report = session
        .verify(
            &next.id,
            &RigVerificationOptions {
                actions: vec![action()],
                head_checks: RigHeadCheckOptions {
                    required: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        report.head_rigidity_status,
        BindingStatus::Pass,
        "{:?}",
        report.issues
    );
    assert!(report.distinct_head_moving_samples >= 2);
    assert!(
        report
            .samples
            .iter()
            .filter_map(|s| s.head_rigidity.as_ref())
            .all(|h| h.maximum_rigid_deviation < 1e-6)
    );
    assert!(
        session
            .refine_weights(0, &next.id, &proposal.refinement)
            .is_err()
    );
    let persisted = session
        .refine_weights(
            0,
            &next.id,
            &RigWeightRefinement {
                expected_binding_fingerprint: next.binding.fingerprint.clone(),
                regions: vec![],
                pending_vertices: vec![],
                constraints: vec![],
                surface_regions: vec![],
                vertex_weights: vec![],
            },
        )
        .unwrap();
    assert_eq!(
        persisted
            .binding
            .binding_options
            .as_ref()
            .unwrap()
            .constraints
            .len(),
        1
    );
    for v in vertices {
        assert_eq!(persisted.binding.weights[v], [1., 0., 0., 0.]);
    }
}

#[test]
fn head_constraint_failures_report_named_weights_and_motion_displacement() {
    use sha2::{Digest, Sha256};
    let m = mesh();
    let vertices = head_vertices(&m);
    let mut options = rigid_regions(&m);
    options.constraints = vec![RigWeightConstraint::RigidJoint {
        id: "head.face".into(),
        vertices: vertices.clone(),
        joint: "head".into(),
    }];
    let mut binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let chest = binding
        .skeleton
        .joints
        .iter()
        .position(|j| j.id == "chest")
        .unwrap() as u16;
    // A valid receipt for deliberately bad weights isolates invariant checks from the fingerprint gate.
    for &v in &vertices {
        binding.joints[v][1] = chest;
        binding.weights[v] = [0.01, 0.99, 0., 0.];
    }
    let payload = (
        &binding.skeleton,
        &binding.joints,
        &binding.weights,
        &binding.profile,
        &binding.profile_dsl,
    );
    binding.fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(payload, binding.binding_options.as_ref().unwrap())).unwrap()
        )
    );
    let check = inspect_rig_weights(&m, &binding, &Default::default()).unwrap();
    assert_eq!(check.anatomy_status, BindingStatus::Fail);
    assert_eq!(check.constraint_status, BindingStatus::Fail);
    assert_eq!(check.constraint_violation_count, vertices.len());
    let evidence = &check.constraint_violations[0];
    assert_eq!(evidence.constraint_id, "head.face");
    assert!((evidence.actual_weight - 0.01).abs() < 1e-6);
    assert!(
        evidence
            .evidence
            .influences
            .iter()
            .any(|w| w.bone == "chest")
    );
    let report = verify_humanoid_binding(
        &m,
        &binding,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!report.accepted);
    assert_eq!(report.head_rigidity_status, BindingStatus::Fail);
    assert!(
        report
            .samples
            .iter()
            .filter_map(|s| s.head_rigidity.as_ref())
            .flat_map(|h| &h.failures)
            .any(|f| f.constraint_id == "head.face" && f.deviation > m.inspection.height * 1e-5)
    );
}

#[test]
fn reviewed_rigid_attachment_keeps_proximity_evidence_without_becoming_a_body_limb() {
    let m = mesh();
    let mut options = rigid_regions(&m);
    let surface = options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["hand_l"])
        .unwrap();
    // A separate rigid hair/prop surface can extend beside a hand while following the head.
    let vertices = surface.vertices.clone();
    surface.influences = vec!["head".into()];
    options.constraints.push(RigWeightConstraint::RigidJoint {
        id: "attachment.hair".into(),
        vertices: vertices.clone(),
        joint: "head".into(),
    });
    let binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let check = inspect_rig_weights(&m, &binding, &Default::default()).unwrap();
    assert_eq!(check.constraint_status, BindingStatus::Pass);
    assert_eq!(check.anatomy_status, BindingStatus::Pass);
    // Removing the explicit reviewed annotation must restore the wrong-body-region diagnosis.
    options.constraints.clear();
    let unreviewed = bind_humanoid_skin(&m, binding.skeleton, &options).unwrap();
    let check = inspect_rig_weights(&m, &unreviewed, &Default::default()).unwrap();
    assert_eq!(check.anatomy_status, BindingStatus::Fail);
    assert!(
        check
            .anatomical_conflicts
            .iter()
            .any(|c| vertices.contains(&c.evidence.vertex)
                && c.evidence.geometric_family.as_deref() == Some("arm_l")
                && c.evidence.anatomical_basis == "geometricProximity")
    );
}

#[test]
fn a_short_neck_edge_enters_a_rigid_face_without_tearing_during_motion() {
    let original = fixture(false);
    let text_len = u32::from_le_bytes(original[12..16].try_into().unwrap()) as usize;
    let mut doc: Value = serde_json::from_slice(&original[20..20 + text_len]).unwrap();
    let mut binary = original[28 + text_len..].to_vec();
    let start = doc["accessors"][0]["count"].as_u64().unwrap() as usize;
    let head = humanoid_rig_standard().references[0]
        .joints
        .iter()
        .find(|j| j.id == "head")
        .unwrap()
        .position;
    // A one-millimeter edge straddles the reviewed face boundary on one shared body primitive.
    for offset in [
        [0.045, -0.003, 0.015],
        [0.045, -0.004, 0.015],
        [0.045, -0.06, 0.015],
    ] {
        for k in 0..3 {
            binary.extend((head[k] * 1.8 + offset[k]).to_le_bytes());
        }
    }
    doc["accessors"][0]["count"] = json!(start + 3);
    doc["buffers"][0]["byteLength"] = json!(binary.len());
    doc["bufferViews"][0]["byteLength"] = json!(binary.len());
    let m = inspect_rig_mesh(
        &glb(doc, binary),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let mut options = rigid_regions(&m);
    options.smoothing_iterations = 12;
    options.regions.push(RigWeightRegion {
        vertices: vec![start, start + 1, start + 2],
        influences: vec!["head".into(), "neck".into(), "upper_chest".into()],
    });
    options.constraints.push(RigWeightConstraint::RigidJoint {
        id: "head.jaw".into(),
        vertices: vec![start],
        joint: "head".into(),
    });
    let binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let head_index = binding
        .skeleton
        .joints
        .iter()
        .position(|j| j.id == "head")
        .unwrap() as u16;
    let head_weight = |v: usize| {
        binding.joints[v]
            .iter()
            .zip(binding.weights[v])
            .filter(|(j, _)| **j == head_index)
            .map(|(_, w)| w)
            .sum::<f32>()
    };
    assert_eq!(head_weight(start), 1.);
    assert!(
        head_weight(start + 1) > 0.995,
        "The neighboring neck vertex must approach the rigid jaw continuously"
    );
    let report = verify_humanoid_binding(
        &m,
        &binding,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        report.deformation_status,
        BindingStatus::Pass,
        "{:?}",
        report.issues
    );
    assert_eq!(report.constraint_status, BindingStatus::Pass);
}

#[test]
fn anatomical_checks_allow_the_hip_joint_transition_but_reject_remote_chest_weights() {
    use sha2::{Digest, Sha256};
    let m = mesh();
    let mut binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let leg = binding
        .skeleton
        .joints
        .iter()
        .position(|j| j.id == "upper_leg_l")
        .unwrap() as u16;
    let hips = binding
        .skeleton
        .joints
        .iter()
        .position(|j| j.id == "hips")
        .unwrap() as u16;
    let chest = binding
        .skeleton
        .joints
        .iter()
        .position(|j| j.id == "chest")
        .unwrap() as u16;
    let v = binding.joints.iter().position(|j| j[0] == leg).unwrap();
    let checksum = |b: &mut HumanoidBinding| {
        let payload = (
            &b.skeleton,
            &b.joints,
            &b.weights,
            &b.profile,
            &b.profile_dsl,
        );
        b.fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(payload, b.binding_options.as_ref().unwrap())).unwrap()
            )
        );
    };
    // More than half pelvis weight is valid at the upper-leg articulation.
    binding.joints[v] = [hips, leg, 0, 0];
    binding.weights[v] = [0.6, 0.4, 0., 0.];
    checksum(&mut binding);
    let valid = inspect_rig_weights(&m, &binding, &Default::default()).unwrap();
    assert_eq!(valid.anatomy_status, BindingStatus::Pass);
    binding.joints[v][0] = chest;
    checksum(&mut binding);
    let invalid = inspect_rig_weights(&m, &binding, &Default::default()).unwrap();
    assert_eq!(invalid.anatomy_status, BindingStatus::Fail);
    assert!(
        invalid
            .anatomical_conflicts
            .iter()
            .any(|c| c.evidence.vertex == v)
    );
}

#[test]
fn deforming_attachment_annotations_persist_and_cannot_waive_body_weight_failures() {
    let mut session = RigAuthoringSession::from_glb_bytes(
        &fixture(false),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let mut options = rigid_regions(session.mesh());
    let part = options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["hand_l"])
        .unwrap();
    let vertices = part.vertices.clone();
    part.influences = vec!["chest".into(), "upper_chest".into()];
    options.surface_regions = vec![RigSurfaceRegion {
        id: "attachment.scarf".into(),
        vertices: vertices.clone(),
        purpose: RigSurfacePurpose::Attachment {
            anchors: part.influences.clone(),
        },
    }];
    let draft = session
        .propose(0, &request(session.mesh(), true), &options)
        .unwrap();
    let check = session
        .inspect_weights(&draft.id, &Default::default())
        .unwrap();
    assert_eq!(check.anatomy_status, BindingStatus::Pass);
    let persisted = session
        .refine_weights(
            0,
            &draft.id,
            &RigWeightRefinement {
                expected_binding_fingerprint: draft.binding.fingerprint.clone(),
                constraints: vec![],
                regions: vec![],
                surface_regions: vec![],
                vertex_weights: vec![],
                pending_vertices: vec![],
            },
        )
        .unwrap();
    assert_eq!(persisted.binding.fingerprint, draft.binding.fingerprint);
    assert!(matches!(
        session.commit(0, &persisted.id),
        Err(RigError::NotVerified)
    ));
    // The same vertices described as a body hand must reject those distant torso influences.
    options.surface_regions[0].purpose = RigSurfacePurpose::Body {
        family: RigAnatomicalFamily::ArmLeft,
    };
    let wrong = session
        .propose(0, &request(session.mesh(), true), &options)
        .unwrap();
    assert_ne!(wrong.binding.fingerprint, draft.binding.fingerprint);
    let check = session
        .inspect_weights(&wrong.id, &Default::default())
        .unwrap();
    assert_eq!(check.anatomy_status, BindingStatus::Fail);
    assert!(
        check
            .anatomical_conflicts
            .iter()
            .any(|c| vertices.contains(&c.evidence.vertex)
                && c.evidence.surface_region_id.as_deref() == Some("attachment.scarf")
                && c.evidence.anatomical_basis == "reviewedBodySurface")
    );
    options.pending_vertices = vec![vertices[0]];
    assert!(
        session
            .propose(0, &request(session.mesh(), true), &options)
            .is_err()
    );
}

#[test]
fn surface_annotations_reject_overlap_unknown_anchors_and_contradictory_regions() {
    let m = mesh();
    let skeleton = build_humanoid_skeleton(&m, &request(&m, true)).unwrap();
    let mut options = rigid_regions(&m);
    options.surface_regions = vec![RigSurfaceRegion {
        id: "cloth".into(),
        vertices: vec![0],
        purpose: RigSurfacePurpose::Attachment {
            anchors: vec!["missing".into()],
        },
    }];
    assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    options.surface_regions[0].purpose = RigSurfacePurpose::Attachment {
        anchors: vec!["head".into()],
    };
    assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    options.surface_regions[0].purpose = RigSurfacePurpose::Body {
        family: RigAnatomicalFamily::Torso,
    };
    let mut duplicate = options.surface_regions[0].clone();
    duplicate.id = "overlap".into();
    options.surface_regions.push(duplicate);
    assert!(bind_humanoid_skin(&m, skeleton, &options).is_err());
}

#[test]
fn contradictory_constraints_and_unresolved_head_vertices_are_rejected() {
    let m = mesh();
    let mut options = rigid_regions(&m);
    let vertices = head_vertices(&m);
    options.constraints = vec![RigWeightConstraint::RigidJoint {
        id: "face".into(),
        vertices: vertices.clone(),
        joint: "root".into(),
    }];
    let skeleton = build_humanoid_skeleton(&m, &request(&m, true)).unwrap();
    assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    options.constraints = vec![RigWeightConstraint::RigidJoint {
        id: "face".into(),
        vertices: vertices.clone(),
        joint: "chest".into(),
    }];
    assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    options.constraints = vec![RigWeightConstraint::RigidJoint {
        id: "face".into(),
        vertices: vertices.clone(),
        joint: "head".into(),
    }];
    options.pending_vertices = vec![vertices[0]];
    assert!(bind_humanoid_skin(&m, skeleton.clone(), &options).is_err());
    options.pending_vertices.clear();
    options.constraints.push(RigWeightConstraint::RigidJoint {
        id: "eyes".into(),
        vertices,
        joint: "head".into(),
    });
    assert!(bind_humanoid_skin(&m, skeleton, &options).is_err());
}

#[test]
fn required_head_check_without_reviewed_surface_is_inconclusive() {
    let m = mesh();
    let binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let report = verify_humanoid_binding(
        &m,
        &binding,
        &RigVerificationOptions {
            actions: vec![action()],
            head_checks: RigHeadCheckOptions {
                required: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.head_rigidity_status, BindingStatus::Inconclusive);
    assert!(!report.accepted);
}

#[test]
fn json_head_constraint_proposal_is_discoverable_and_fingerprint_guarded() {
    let mut service = CharacterService::default();
    service
        .inspect_rig_bytes(
            "head".into(),
            &fixture(true),
            &MeshInspectionOptions {
                target_height: Some(1.8),
                ..Default::default()
            },
        )
        .unwrap();
    let mesh = service.rigs["head"].mesh();
    let vertices = head_vertices(mesh);
    let candidate = service.execute(serde_json::from_value(json!({"operation":"buildRig", "rigId":"head", "expectedRevision":0, "request":request(mesh,true), "binding":rigid_regions(mesh)})).unwrap()).unwrap();
    let proposal = service.execute(serde_json::from_value(json!({"operation":"proposeRigHeadConstraints", "rigId":"head", "candidateId":candidate["id"], "options":{"searchVertices":vertices, "reviewedRegions":[{"id":"head.face","vertices":vertices,"kind":"rigidHead"}]}})).unwrap()).unwrap();
    assert_eq!(proposal["requiresReview"], false);
    assert_eq!(
        proposal["refinement"]["constraints"][0]["kind"],
        "rigidJoint"
    );
    let schema = service.execute(CharacterCommand::Schema).unwrap();
    assert!(
        schema["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|op| op == "proposeRigHeadConstraints")
    );
    assert_eq!(
        proposal["refinement"]["expectedBindingFingerprint"],
        candidate["binding"]["fingerprint"]
    );
}

// A hand/torso split reproduces S105's short-edge tearing without external GLBs.
fn wrong_hand_region(m: &RigMesh) -> (RigBindingOptions, usize) {
    let mut options = rigid_regions(m);
    let region = options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["hand_l"])
        .unwrap();
    let vertex = region.vertices.remove(0);
    options.regions.push(RigWeightRegion {
        vertices: vec![vertex],
        influences: vec!["chest".into()],
    });
    (options, vertex)
}

#[test]
fn wrong_hand_region_has_anatomical_continuity_and_motion_evidence() {
    let m = mesh();
    let (options, vertex) = wrong_hand_region(&m);
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let diagnostic = inspect_rig_weights(&m, &b, &Default::default()).unwrap();
    assert_eq!(diagnostic.anatomy_status, BindingStatus::Fail);
    assert_eq!(diagnostic.continuity_status, BindingStatus::Fail);
    let conflict = diagnostic
        .anatomical_conflicts
        .iter()
        .find(|c| c.evidence.vertex == vertex)
        .unwrap();
    assert_eq!(conflict.evidence.expected_family.as_deref(), Some("arm_l"));
    assert_eq!(conflict.evidence.influences[0].bone, "chest");
    assert_eq!(conflict.evidence.allowed_influences, ["chest"]);
    assert!(conflict.evidence.region_index.is_some());
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!report.accepted);
    assert_eq!(report.anatomy_status, BindingStatus::Fail);
    assert!(
        report
            .samples
            .iter()
            .flat_map(|s| &s.worst_deformation_edges)
            .any(|e| e.vertex_evidence.iter().any(|v| v.vertex == vertex)
                && e.likely_causes
                    .contains(&"ADJACENT_INFLUENCE_DISCONTINUITY".into()))
    );
}

#[test]
fn refinement_rebuilds_weights_without_mutating_or_accepting_the_original() {
    let mut s = RigAuthoringSession::from_glb_bytes(
        &fixture(false),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let (options, _) = wrong_hand_region(s.mesh());
    let c = s.propose(0, &request(s.mesh(), true), &options).unwrap();
    assert!(matches!(
        s.suggest_weight_refinement(&c.id, &Default::default()),
        Err(RigError::NotVerified)
    ));
    let verification = RigVerificationOptions {
        actions: vec![action()],
        ..Default::default()
    };
    s.verify(&c.id, &verification).unwrap();
    let correction = s
        .suggest_weight_refinement(&c.id, &Default::default())
        .unwrap();
    let improved = s.refine_weights(0, &c.id, &correction).unwrap();
    assert_ne!(c.binding.fingerprint, improved.binding.fingerprint);
    assert_eq!(
        improved
            .binding
            .skeleton
            .skeleton_dsl
            .matches("<BoneAxisMap>")
            .count(),
        1
    );
    assert_eq!(
        s.candidate(&c.id).unwrap().binding.fingerprint,
        c.binding.fingerprint
    );
    assert!(matches!(
        s.commit(0, &improved.id),
        Err(RigError::NotVerified)
    ));
    assert!(s.refine_weights(0, &improved.id, &correction).is_err());
    let report = s.verify(&improved.id, &verification).unwrap();
    assert_eq!(report.anatomy_status, BindingStatus::Pass);
    assert_eq!(report.weight_continuity_status, BindingStatus::Pass);
    assert!(report.accepted, "{:?}", report.issues);
}

#[test]
fn unresolved_vertices_block_acceptance_and_cannot_overlap_confirmed_regions() {
    let m = mesh();
    let mut options = rigid_regions(&m);
    let v = options
        .regions
        .iter_mut()
        .find(|r| r.influences == ["hand_l"])
        .unwrap()
        .vertices
        .remove(0);
    options.pending_vertices = vec![v];
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.anatomy_status, BindingStatus::Inconclusive);
    assert_eq!(
        report.weight_diagnostics.as_ref().unwrap().pending_vertices,
        [v]
    );
    assert!(!report.accepted);
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "UNRESOLVED_WEIGHT_VERTICES")
    );
    options.regions.push(RigWeightRegion {
        vertices: vec![v],
        influences: vec!["hand_l".into()],
    });
    assert!(bind_humanoid_skin(&m, b.skeleton.clone(), &options).is_err());
    let proposed = propose_rig_weight_regions(&m, &b.skeleton, &Default::default()).unwrap();
    assert!(!proposed.pending_vertices.is_empty());
    for v in &proposed.pending_vertices {
        assert!(proposed.regions.iter().all(|r| !r.vertices.contains(v)));
    }
}

#[test]
fn pending_metadata_is_fingerprinted_and_defaults_keep_old_json_readable() {
    let m = mesh();
    let mut b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    b.binding_options.as_mut().unwrap().pending_vertices.push(0);
    let report = verify_humanoid_binding(&m, &b, &Default::default()).unwrap();
    assert_eq!(report.structure_status, BindingStatus::Fail);
    let options: RigBindingOptions = serde_json::from_value(json!({"regions":[]})).unwrap();
    assert!(options.pending_vertices.is_empty());
    let checks: RigVerificationOptions = serde_json::from_value(json!({"actions":[]})).unwrap();
    assert_eq!(checks.weight_checks.maximum_examples, 64);
}

#[test]
fn welded_seams_are_checked_without_connecting_nearby_separate_surfaces() {
    for offset in [0f32, 0.0001] {
        let source = fixture(false);
        let text_len = u32::from_le_bytes(source[12..16].try_into().unwrap()) as usize;
        let mut doc: Value = serde_json::from_slice(&source[20..20 + text_len]).unwrap();
        let mut binary = source[28 + text_len..].to_vec();
        let reference = humanoid_rig_standard();
        let index = reference.references[0]
            .joints
            .iter()
            .filter(|j| !j.endpoint && j.id != "root")
            .position(|j| j.id == "hand_l")
            .unwrap();
        let mut copy = binary[index * 36..index * 36 + 36].to_vec();
        for row in copy.chunks_exact_mut(12) {
            let x = f32::from_le_bytes(row[..4].try_into().unwrap()) + offset;
            row[..4].copy_from_slice(&x.to_le_bytes());
        }
        let first = doc["accessors"][0]["count"].as_u64().unwrap() as usize;
        binary.extend(copy);
        doc["accessors"][0]["count"] = json!(first + 3);
        doc["buffers"][0]["byteLength"] = json!(binary.len());
        doc["bufferViews"][0]["byteLength"] = json!(binary.len());
        let m = inspect_rig_mesh(
            &glb(doc, binary),
            &MeshInspectionOptions {
                target_height: Some(1.8),
                ..Default::default()
            },
        )
        .unwrap();
        let mut options = rigid_regions(&m);
        options.regions.push(RigWeightRegion {
            vertices: (first..first + 3).collect(),
            influences: vec!["chest".into()],
        });
        let b = bind_humanoid_skin(
            &m,
            build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
            &options,
        )
        .unwrap();
        let d = inspect_rig_weights(&m, &b, &Default::default()).unwrap();
        assert_eq!(d.discontinuity_count > 0, offset == 0.);
        assert!(d.anatomical_conflict_count > 0);
    }
}

#[test]
fn json_dispatch_exposes_diagnostics_and_exact_candidate_refinement() {
    let mut service = CharacterService::default();
    service
        .inspect_rig_bytes(
            "weights".into(),
            &fixture(false),
            &MeshInspectionOptions {
                target_height: Some(1.8),
                ..Default::default()
            },
        )
        .unwrap();
    let m = service.rigs["weights"].mesh();
    let (options, _) = wrong_hand_region(m);
    let command = serde_json::from_value(json!({"operation":"buildRig","rigId":"weights","expectedRevision":0,"request":request(m,true),"binding":options})).unwrap();
    let c = service.execute(command).unwrap();
    let id = c["id"].as_str().unwrap();
    let d = service.execute(serde_json::from_value(json!({"operation":"inspectRigWeights","rigId":"weights","candidateId":id,"options":{}})).unwrap()).unwrap();
    assert_eq!(d["anatomyStatus"], "fail");
    let regions = service
        .execute(
            serde_json::from_value(
                json!({"operation":"proposeRigWeightRegions","rigId":"weights","candidateId":id}),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(regions["pendingVertices"].is_array());
    let options = RigVerificationOptions {
        actions: vec![action()],
        ..Default::default()
    };
    service.execute(serde_json::from_value(json!({"operation":"verifyRig","rigId":"weights","candidateId":id,"options":options})).unwrap()).unwrap();
    let correction = service.execute(serde_json::from_value(json!({"operation":"suggestRigWeightRefinement","rigId":"weights","candidateId":id})).unwrap()).unwrap();
    let next = service.execute(serde_json::from_value(json!({"operation":"refineRigWeights","rigId":"weights","candidateId":id,"expectedRevision":0,"refinement":correction})).unwrap()).unwrap();
    assert_ne!(c["id"], next["id"]);
    let report = service.execute(serde_json::from_value(json!({"operation":"verifyRig","rigId":"weights","candidateId":next["id"],"options":options})).unwrap()).unwrap();
    assert_eq!(report["accepted"], true);
}

struct TemporaryGlb(std::path::PathBuf);
impl TemporaryGlb {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "motionloom-rig-test-{}-{id}.glb",
            std::process::id()
        ));
        std::fs::write(&path, fixture(false)).unwrap();
        Self(path)
    }
}
impl Drop for TemporaryGlb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// A deterministic triangle mesh makes skinning regressions independent of proprietary assets.
fn fixture(poison_skin: bool) -> Vec<u8> {
    let mut vertices = vec![];
    for j in &humanoid_rig_standard().references[0].joints {
        if j.endpoint || j.id == "root" {
            continue;
        }
        for d in [[0., 0., 0.], [0.001, 0., 0.], [0., 0.001, 0.]] {
            vertices.push(std::array::from_fn::<_, 3, _>(|k| {
                j.position[k] * 1.8 + d[k]
            }));
        }
    }
    let mut binary = vec![];
    for p in &vertices {
        for x in p {
            binary.extend(x.to_le_bytes());
        }
    }
    let mut doc = json!({"asset":{"version":"2.0"},"buffers":[{"byteLength":binary.len()}],"bufferViews":[{"buffer":0,"byteLength":binary.len()}],"accessors":[{"bufferView":0,"componentType":5126,"count":vertices.len(),"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],"nodes":[{"mesh":0}],"scenes":[{"nodes":[0]}],"scene":0});
    if poison_skin {
        doc["skins"] = json!([{"joints":[999999],"inverseBindMatrices":999999}]);
        doc["animations"] = json!([{"samplers":[{"input":999999,"output":999999}],"channels":[{"target":{"node":999999,"path":"rotation"},"sampler":0}]}]);
        doc["nodes"][0]["skin"] = json!(99999);
        doc["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(999999);
        doc["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(999999);
    }
    glb(doc, binary)
}
fn glb(doc: Value, mut binary: Vec<u8>) -> Vec<u8> {
    let mut text = serde_json::to_vec(&doc).unwrap();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let mut bytes = vec![];
    for n in [
        0x46546c67,
        2,
        (28 + text.len() + binary.len()) as u32,
        text.len() as u32,
        0x4e4f534a,
    ] {
        bytes.extend(n.to_le_bytes());
    }
    bytes.extend(text);
    bytes.extend((binary.len() as u32).to_le_bytes());
    bytes.extend(0x004e4942u32.to_le_bytes());
    bytes.extend(binary);
    bytes
}
fn mutate_fixture(change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let bytes = fixture(false);
    let text_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let mut doc = serde_json::from_slice(&bytes[20..20 + text_len]).unwrap();
    let binary = bytes[28 + text_len..].to_vec();
    change(&mut doc);
    glb(doc, binary)
}
fn mesh() -> RigMesh {
    inspect_rig_mesh(
        &fixture(false),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap()
}
fn request(mesh: &RigMesh, explicit: bool) -> RigBuildRequest {
    let r = &humanoid_rig_standard().references[0];
    // Inspection floors/centers and rescales the sparse fixture; its confirmed hints must follow it.
    let origins = r
        .joints
        .iter()
        .filter(|j| !j.endpoint && j.id != "root")
        .collect::<Vec<_>>();
    let scale = (mesh.positions[3][1] - mesh.positions[0][1])
        / (origins[1].position[1] - origins[0].position[1]);
    let offset =
        std::array::from_fn::<_, 3, _>(|k| mesh.positions[0][k] - origins[0].position[k] * scale);
    RigBuildRequest {
        expected_geometry_fingerprint: mesh.inspection.geometry_fingerprint.clone(),
        reference: r.id.clone(),
        landmarks: if explicit {
            r.joints
                .iter()
                .map(|j| {
                    (
                        j.id.clone(),
                        std::array::from_fn(|k| j.position[k] * scale + offset[k]),
                    )
                })
                .collect()
        } else {
            BTreeMap::new()
        },
        rotations: r
            .joints
            .iter()
            .map(|j| (j.id.clone(), j.rotation))
            .collect(),
        extensions: vec![],
    }
}
fn rigid_regions(_mesh: &RigMesh) -> RigBindingOptions {
    let names = humanoid_rig_standard().references[0]
        .joints
        .iter()
        .filter(|j| !j.endpoint && j.id != "root")
        .map(|j| j.id.clone())
        .collect::<Vec<_>>();
    RigBindingOptions {
        regions: names
            .iter()
            .enumerate()
            .map(|(i, name)| RigWeightRegion {
                vertices: vec![i * 3, i * 3 + 1, i * 3 + 2],
                influences: vec![name.clone()],
            })
            .collect(),
        smoothing_iterations: 0,
        ..Default::default()
    }
}
fn action() -> RigActionTest {
    RigActionTest{action_id:"probe_motion".into(),library_source:"<ActionLibrary>\n<Action id=\"probe_motion\" skeleton=\"humanoid_v1\" duration=\"1s\">\n<Pose t=\"0s\"><Bone id=\"upper_arm_l\" forward=\"0\" /></Pose>\n<Pose t=\"1s\"><Bone id=\"upper_arm_l\" forward=\"45\" /></Pose>\n</Action>\n</ActionLibrary>".into(),phases:vec![0.2,0.5,0.8]}
}

#[test]
fn source_skin_and_animation_are_ignored_before_loading() {
    let clean = inspect_rig_mesh(&fixture(false), &Default::default()).unwrap();
    let poison = inspect_rig_mesh(&fixture(true), &Default::default()).unwrap();
    assert_eq!(clean.positions, poison.positions);
    assert_eq!(clean.triangles, poison.triangles);
    assert_eq!(
        clean.inspection.geometry_fingerprint,
        poison.inspection.geometry_fingerprint
    );
    assert_ne!(
        clean.inspection.source_fingerprint,
        poison.inspection.source_fingerprint
    );
}

#[test]
fn declared_joint_transforms_and_unused_rig_hierarchy_do_not_guide_inspection() {
    let bytes = mutate_fixture(|doc| {
        doc["nodes"] = json!([
            {"mesh":0},
            {"name":"wrong_bone","translation":"invalid","rotation":[99],"children":[0]},
            {"children":[3]}, {"children":[2]}
        ]);
        doc["skins"] = json!([{"joints":[1,2,3],"inverseBindMatrices":999999}]);
        doc["scenes"] = json!([{"nodes":[1,2]}]);
    });
    let clean = inspect_rig_mesh(&fixture(false), &Default::default()).unwrap();
    let ignored = inspect_rig_mesh(&bytes, &Default::default()).unwrap();
    assert_eq!(clean.positions, ignored.positions);
    assert_eq!(
        clean.inspection.geometry_fingerprint,
        ignored.inspection.geometry_fingerprint
    );
}

#[test]
fn mesh_instances_and_static_object_transforms_are_baked() {
    let bytes = mutate_fixture(|doc| {
        doc["nodes"] = json!([
            {"mesh":0,"translation":[-3,0,0]},
            {"mesh":0,"translation":[3,0,0]},
            {"scale":[2,2,2],"children":[0,1]}
        ]);
        doc["scenes"] = json!([{"nodes":[2]}]);
    });
    let clean = inspect_rig_mesh(&fixture(false), &Default::default()).unwrap();
    let instanced = inspect_rig_mesh(&bytes, &Default::default()).unwrap();
    assert_eq!(instanced.positions.len(), clean.positions.len() * 2);
    assert_eq!(instanced.triangles.len(), clean.triangles.len() * 2);
    let n = clean.positions.len();
    for v in 0..n {
        assert!((instanced.positions[v + n][0] - instanced.positions[v][0] - 12.).abs() < 1e-5);
    }
    assert!((instanced.inspection.height - clean.inspection.height * 2.).abs() < 1e-5);
}

#[test]
fn embedded_reference_has_65_one_to_one_nodes_and_optional_extensions() {
    let standard = humanoid_rig_standard();
    assert_eq!(standard.license, "CC0-1.0");
    for r in &standard.references {
        assert_eq!(r.joints.len(), 65);
        assert_eq!(r.joints.iter().filter(|j| j.endpoint).count(), 12);
    }
    let m = mesh();
    let mut r = request(&m, true);
    r.extensions.push(RigExtension {
        id: "hair_root".into(),
        parent: "head".into(),
        position: [0., 1.8, 0.],
        rotation: [0., 0., 0., 1.],
    });
    let skeleton = build_humanoid_skeleton(&m, &r).unwrap();
    assert_eq!(skeleton.joints.len(), 66);
    assert_eq!(skeleton.joints.iter().filter(|j| j.core).count(), 65);
    assert!(
        skeleton
            .skeleton_dsl
            .contains("sourceRig=\"character1_reference_v1\"")
    );
    assert!(skeleton.skeleton_dsl.contains("id=\"index_end_l\""));
}

#[test]
fn derived_endpoints_follow_fitted_terminal_segment_direction_and_length() {
    let m = mesh();
    let mut r = request(&m, true);
    let previous_endpoint = r.landmarks.remove("index_end_l").unwrap();
    r.rotations.remove("index_3_l");
    let p = r.landmarks["index_2_l"];
    let old = r.landmarks["index_3_l"];
    let old_length = old
        .iter()
        .zip(p)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt();
    r.landmarks
        .insert("index_3_l".into(), [p[0], p[1] + old_length * 2., p[2]]);
    let s = build_humanoid_skeleton(&m, &r).unwrap();
    let parent = s.joints.iter().find(|j| j.id == "index_3_l").unwrap();
    let endpoint = s.joints.iter().find(|j| j.id == "index_end_l").unwrap();
    let ref_length = previous_endpoint
        .iter()
        .zip(old)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt();
    let actual_length = endpoint
        .position
        .iter()
        .zip(parent.position)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt();
    assert!((actual_length - ref_length * 2.).abs() < 1e-5);
    assert!(endpoint.position[1] > parent.position[1]);
    assert!(s.inferred_landmarks.is_empty());
}

#[test]
fn export_replaces_source_rig_and_preserves_geometry() {
    let m = mesh();
    let binding = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let bytes = export_rig_glb(&m, &binding).unwrap();
    let loaded = motionloom::experimental::load_glb_mesh_data_from_bytes(
        std::path::Path::new("candidate.glb"),
        &bytes,
    )
    .unwrap();
    assert_eq!(loaded.positions, m.positions);
    assert_eq!(
        loaded.indices,
        m.triangles.iter().flatten().copied().collect::<Vec<_>>()
    );
    assert_eq!(loaded.skin.as_ref().unwrap().joints.len(), 65);
    assert!(loaded.animations.is_empty());
    assert!(binding.skeleton.skeleton_dsl.contains("<BoneAxisMap>"));
    assert_eq!(binding.profile.retarget.as_ref().unwrap().maps.len(), 65);
}

#[test]
fn malformed_landmarks_and_regions_are_rejected() {
    let m = mesh();
    let mut r = request(&m, true);
    r.expected_geometry_fingerprint = "stale".into();
    assert!(matches!(
        build_humanoid_skeleton(&m, &r),
        Err(RigError::SourceChanged)
    ));
    r = request(&m, true);
    r.landmarks.insert("unknown".into(), [0.; 3]);
    assert!(build_humanoid_skeleton(&m, &r).is_err());
    let mut o = rigid_regions(&m);
    o.regions[0].influences = vec!["index_end_l".into()];
    assert!(
        bind_humanoid_skin(
            &m,
            build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
            &o
        )
        .is_err()
    );
}

#[test]
fn changed_typed_geometry_and_invalid_export_indices_are_rejected() {
    let mut m = mesh();
    let r = request(&m, true);
    let mut b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &r).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    b.joints[0][0] = 9999;
    assert!(matches!(export_rig_glb(&m, &b), Err(RigError::Invalid(_))));
    m.positions[0][0] += 0.1;
    assert!(matches!(
        build_humanoid_skeleton(&m, &r),
        Err(RigError::SourceChanged)
    ));
    assert!(matches!(
        verify_humanoid_binding(&m, &b, &Default::default()),
        Err(RigError::SourceChanged)
    ));
}

#[test]
fn altered_dsl_and_invalid_axis_proposals_cannot_be_verified() {
    let m = mesh();
    let mut b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    b.profile_dsl = b.profile_dsl.replace("upper_arm_l", "upper_arm_r");
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.structure_status, BindingStatus::Fail);
    assert!(!report.accepted);
    let mut options = rigid_regions(&m);
    let mut axes = b.profile.bone_axis_map.unwrap();
    axes.axes[0].forward = Some("invalid\"axis".into());
    options.axis_map = Some(axes);
    assert!(
        bind_humanoid_skin(
            &m,
            build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
            &options
        )
        .is_err()
    );
}

#[test]
fn binding_acceptance_requires_exact_verified_candidate() {
    let mut s = RigAuthoringSession::from_glb_bytes(
        &fixture(false),
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )
    .unwrap();
    let c = s
        .propose(0, &request(s.mesh(), true), &rigid_regions(s.mesh()))
        .unwrap();
    assert!(matches!(s.commit(0, &c.id), Err(RigError::NotVerified)));
    let report = s
        .verify(
            &c.id,
            &RigVerificationOptions {
                actions: vec![action()],
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(report.core_nodes, 65);
    assert!(report.samples.iter().all(|s| s.evaluated_nodes == 65));
    assert!(
        report
            .samples
            .iter()
            .all(|s| s.per_bone_rotation_error_degrees.len() == 65)
    );
    assert!(report.raw_bind_pose_maximum_error < 1e-5);
    assert!(
        report.accepted,
        "{}",
        serde_json::to_string_pretty(&report.weight_diagnostics).unwrap()
    );
    s.commit(0, &c.id).unwrap();
    assert_eq!(s.revision(), 1);
    assert!(s.export_committed().is_ok());
    assert!(matches!(
        s.propose(0, &request(s.mesh(), true), &rigid_regions(s.mesh())),
        Err(RigError::Stale { .. })
    ));
}

#[test]
fn missing_hints_and_action_evidence_cannot_pass() {
    let m = mesh();
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, false)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let r = verify_humanoid_binding(&m, &b, &Default::default()).unwrap();
    assert!(!r.accepted);
    assert!(
        r.issues
            .iter()
            .any(|i| i.code == "ACTION_EVIDENCE_INCOMPLETE")
    );
    assert!(
        r.issues
            .iter()
            .any(|i| i.code == "ANATOMICAL_HINTS_UNCONFIRMED")
    );
}

#[test]
fn reversed_finger_axis_is_detected_by_semantic_probes() {
    let m = mesh();
    let good = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let mut options = rigid_regions(&m);
    let mut axes = good.profile.bone_axis_map.unwrap();
    let a = axes
        .axes
        .iter_mut()
        .find(|a| a.bone == "index_1_l")
        .unwrap();
    let original = a.bend.as_ref().unwrap();
    let (axis, sign) = original.split_once(':').unwrap();
    a.bend = Some(format!("{axis}:{}", -sign.parse::<f32>().unwrap()));
    options.axis_map = Some(axes);
    let bad = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let r = verify_humanoid_binding(
        &m,
        &bad,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(r.direction_status, BindingStatus::Fail);
    assert!(!r.accepted);
    assert!(
        r.issues
            .iter()
            .any(|i| i.code == "REFERENCE_MOTION_DIRECTION_MISMATCH"
                && i.bone.as_deref() == Some("index_1_l"))
    );
}

#[test]
fn reversed_arm_side_axis_is_detected() {
    let m = mesh();
    let good = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let mut axes = good.profile.bone_axis_map.unwrap();
    let a = axes
        .axes
        .iter_mut()
        .find(|a| a.bone == "upper_arm_l")
        .unwrap();
    let (axis, sign) = a.side.as_ref().unwrap().split_once(':').unwrap();
    a.side = Some(format!("{axis}:{}", -sign.parse::<f32>().unwrap()));
    let options = RigBindingOptions {
        axis_map: Some(axes),
        ..rigid_regions(&m)
    };
    let bad = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let report = verify_humanoid_binding(
        &m,
        &bad,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.direction_status, BindingStatus::Fail);
    assert!(!report.accepted);
    assert!(
        report
            .issues
            .iter()
            .any(|i| i.code == "REFERENCE_MOTION_DIRECTION_MISMATCH"
                && i.bone.as_deref() == Some("upper_arm_l"))
    );
}

#[test]
fn static_actions_and_duplicate_frames_cannot_certify_a_binding() {
    let m = mesh();
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let mut duplicate = action();
    duplicate.phases = vec![0.5, 0.5, 0.50001];
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![duplicate.clone(), duplicate],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.distinct_moving_action_samples, 1);
    assert!(!report.accepted);
    let mut stationary = action();
    stationary.library_source = stationary
        .library_source
        .replace("forward=\"45\"", "forward=\"0\"");
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![stationary],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.distinct_moving_action_samples, 0);
    assert!(
        report
            .samples
            .iter()
            .filter(|s| !s.builtin_probe)
            .all(|s| s.maximum_vertex_motion < 1e-5)
    );
    assert!(!report.accepted);
}

#[test]
fn optional_extension_does_not_change_core_weights() {
    let m = mesh();
    let base = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &Default::default(),
    )
    .unwrap();
    let mut r = request(&m, true);
    r.extensions.push(RigExtension {
        id: "hair_root".into(),
        parent: "head".into(),
        position: [0.8, 1.8, 0.8],
        rotation: [0., 0., 0., 1.],
    });
    let extended = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &r).unwrap(),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(base.joints, extended.joints);
    assert_eq!(base.weights, extended.weights);
}

#[test]
fn densely_sampled_pose_actions_use_the_production_evaluator() {
    let m = mesh();
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &rigid_regions(&m),
    )
    .unwrap();
    let poses = (0..=40)
        .map(|i| {
            format!(
                "<Pose t=\"{}ms\"><Bone id=\"upper_arm_l\" forward=\"{}\" /></Pose>",
                i * 25,
                i as f32 * 1.125
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let action = RigActionTest {
        library_source: format!(
            "<ActionLibrary><Action id=\"dense\" skeleton=\"humanoid_v1\" duration=\"1s\">{poses}</Action></ActionLibrary>"
        ),
        action_id: "dense".into(),
        phases: vec![0.2, 0.5, 0.8],
    };
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![action],
            ..Default::default()
        },
    )
    .unwrap();
    assert!(report.accepted, "{:?}", report.issues);
    assert_eq!(report.distinct_moving_action_samples, 3);
}

#[test]
fn skin_tearing_reports_vertices_for_weight_refinement() {
    let m = mesh();
    let mut options = rigid_regions(&m);
    options.regions.remove(0);
    for (v, bone) in ["hips", "head", "hand_l"].iter().enumerate() {
        options.regions.push(RigWeightRegion {
            vertices: vec![v],
            influences: vec![(*bone).into()],
        });
    }
    let b = bind_humanoid_skin(
        &m,
        build_humanoid_skeleton(&m, &request(&m, true)).unwrap(),
        &options,
    )
    .unwrap();
    let report = verify_humanoid_binding(
        &m,
        &b,
        &RigVerificationOptions {
            actions: vec![action()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(report.deformation_status, BindingStatus::Fail);
    assert!(!report.accepted);
    for sample in &report.samples {
        if sample.maximum_edge_stretch > 2. {
            assert!(
                sample
                    .worst_deformation_edges
                    .iter()
                    .any(|edge| (edge.length_ratio - sample.maximum_edge_stretch).abs() < 1e-5),
                "Maximum stretching edge must remain available for refinement"
            );
        }
    }
    assert!(
        report
            .samples
            .iter()
            .flat_map(|s| &s.worst_deformation_edges)
            .any(|e| e.vertices.iter().all(|i| *i < 3) && e.length_ratio > 4.)
    );
}

#[test]
fn json_dispatch_runs_full_workflow_and_rejects_changed_source() {
    let source = TemporaryGlb::new();
    let output = TemporaryGlb::new();
    let mut service = CharacterService::default();
    let inspect: CharacterCommand = serde_json::from_value(json!({"operation":"inspectRigMesh","rigId":"test","path":source.0,"options":{"targetHeight":1.8}})).unwrap();
    let inspection = service.execute(inspect).unwrap();
    assert_eq!(inspection["revision"], 0);
    let data = service
        .execute(CharacterCommand::RigMeshData {
            rig_id: "test".into(),
        })
        .unwrap();
    assert_eq!(data["positions"].as_array().unwrap().len(), 156);
    let nearby = service
        .execute(CharacterCommand::QueryRigVertices {
            rig_id: "test".into(),
            position: [0., 1., 0.],
            count: 3,
        })
        .unwrap();
    assert_eq!(nearby["vertices"].as_array().unwrap().len(), 3);
    assert!(nearby["vertices"][0]["vertex"].is_number());
    assert!(nearby["vertices"][0]["position"].is_array());
    assert!(nearby["vertices"][0]["distance"].is_number());
    let m = service.rigs["test"].mesh();
    let build: CharacterCommand = serde_json::from_value(json!({"operation":"buildRig","rigId":"test","expectedRevision":0,"request":request(m,true),"binding":rigid_regions(m)})).unwrap();
    let candidate = service.execute(build).unwrap();
    assert_eq!(
        candidate["binding"]["skeleton"]["joints"]
            .as_array()
            .unwrap()
            .len(),
        65
    );
    let id = candidate["id"].as_str().unwrap().to_string();
    let verify: CharacterCommand = serde_json::from_value(json!({"operation":"verifyRig","rigId":"test","candidateId":id,"options":{"actions":[action()]}})).unwrap();
    let report = service.execute(verify).unwrap();
    assert_eq!(report["accepted"], true);
    let committed = service
        .execute(CharacterCommand::CommitRig {
            rig_id: "test".into(),
            expected_revision: 0,
            candidate_id: id,
        })
        .unwrap();
    assert_eq!(committed["revision"], 1);
    let export = || CharacterCommand::ExportRig {
        rig_id: "test".into(),
        candidate_id: None,
        output_path: output.0.to_string_lossy().into(),
    };
    service.execute(export()).unwrap();
    let bytes = std::fs::read(&output.0).unwrap();
    assert_eq!(
        motionloom::experimental::load_glb_mesh_data_from_bytes(&output.0, &bytes)
            .unwrap()
            .skin
            .unwrap()
            .joints
            .len(),
        65
    );
    std::fs::write(&source.0, fixture(true)).unwrap();
    assert!(matches!(
        service.execute(export()),
        Err(motionloom::api::character_authoring::CharacterError::Rig(
            RigError::SourceChanged
        ))
    ));
}

#[test]
fn json_api_discovers_rig_workflow() {
    let mut service = CharacterService::default();
    let schema = service.execute(CharacterCommand::Schema).unwrap();
    assert!(
        schema["operations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "verifyRig")
    );
    let standard = service.execute(CharacterCommand::RigStandard).unwrap();
    assert_eq!(
        standard["references"][0]["joints"]
            .as_array()
            .unwrap()
            .len(),
        65
    );
    let command:CharacterCommand=serde_json::from_value(json!({"operation":"buildRig","rigId":"a","expectedRevision":0,"request":{"expectedGeometryFingerprint":"f"}})).unwrap();
    assert!(service.execute(command).is_err());
}
