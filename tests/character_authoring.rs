// =========================================
// =========================================
// tests/character_authoring.rs

use motionloom::api::character_authoring::*;

fn template() -> CharacterDocument {
    build_template(
        "test".into(),
        CharacterParameters::preset(ProportionPreset::Anime),
    )
    .unwrap()
}
fn narrow(revision: u64) -> EditRequest {
    EditRequest {
        expected_revision: revision,
        operations: vec![EditOperation::Scale {
            target: "eyes.sclera".into(),
            factors: [0.735, 1., 1.],
        }],
        locks: vec!["eyes.iris".into()],
        paired: true,
        reason: "Narrow eye whites".into(),
    }
}

#[test]
fn templates_are_deterministic_and_valid() {
    for preset in [
        ProportionPreset::Anime,
        ProportionPreset::Chibi,
        ProportionPreset::Neutral,
    ] {
        let params = CharacterParameters::preset(preset);
        let first = build_template("a".into(), params.clone()).unwrap();
        let second = build_template("b".into(), params).unwrap();
        assert_eq!(first.current.source, second.current.source);
        for cage in cages(&first.current.source).unwrap().values() {
            let report =
                motionloom::api::mesh_reference::validate_mesh_topology(cage, &Default::default());
            assert!(report.valid, "{report:?}");
        }
    }
}

#[test]
fn sclera_edit_preserves_iris_and_is_atomic_undoable() {
    let mut doc = template();
    let before = doc.current.source.clone();
    let original = cages(&before).unwrap();
    let candidate = doc.propose(narrow(0)).unwrap();
    assert_eq!(doc.current.source, before);
    let after = cages(&candidate.snapshot.source).unwrap();
    for name in ["iris_l", "iris_r"] {
        assert_eq!(
            serde_json::to_value(&original[name]).unwrap(),
            serde_json::to_value(&after[name]).unwrap()
        );
    }
    assert_ne!(
        original["eye_white_l"].positions,
        after["eye_white_l"].positions
    );
    doc.commit(0, &candidate.id).unwrap();
    let accepted = doc.current.source.clone();
    assert!(doc.propose(narrow(0)).is_err());
    doc.undo(1).unwrap();
    assert_eq!(doc.current.source, before);
    doc.redo(2).unwrap();
    assert_eq!(doc.current.source, accepted);
}

#[test]
fn locked_or_late_failure_does_not_mutate_source() {
    let mut doc = template();
    let before = doc.current.source.clone();
    let mut edit = narrow(0);
    edit.operations.push(EditOperation::Scale {
        target: "eyes.iris".into(),
        factors: [0.9, 1., 1.],
    });
    assert!(matches!(doc.propose(edit), Err(CharacterError::Locked(_))));
    assert_eq!(doc.current.source, before);
    assert!(doc.candidates.is_empty());
    let mut edit = narrow(0);
    edit.operations.push(EditOperation::Translate {
        target: "head".into(),
        delta: [1000., 0., 0.],
    });
    assert!(doc.propose(edit).is_err());
    assert_eq!(doc.current.source, before);
    assert!(doc.candidates.is_empty());
}

#[test]
fn depth_preserves_x_y_and_review_uses_union_bounds() {
    let mut doc = template();
    let before = cages(&doc.current.source).unwrap();
    let candidate = doc
        .propose(EditRequest {
            expected_revision: 0,
            operations: vec![EditOperation::Depth {
                target: "body.ribcage".into(),
                factor: 1.1,
                center_z: 0.,
                falloff: 0.12,
            }],
            locks: vec![],
            paired: false,
            reason: "Increase rib cage depth".into(),
        })
        .unwrap();
    let after = cages(&candidate.snapshot.source).unwrap();
    for (a, b) in before["torso"]
        .positions
        .iter()
        .zip(&after["torso"].positions)
    {
        assert_eq!(&a[..2], &b[..2]);
    }
    let r = review_report(&candidate.snapshot, 0, Some(&doc.current)).unwrap();
    assert!(
        r.views[..4]
            .iter()
            .all(|v| v.vertical_scale == r.views[0].vertical_scale)
    );
    assert_eq!(r.views.len(), 6);
}

#[test]
fn attachments_follow_and_stale_binding_rejects() {
    let mut doc = template();
    bind_attachment(&mut doc.current, "head", "eye_white_l", None).unwrap();
    let candidate = doc
        .propose(EditRequest {
            expected_revision: 0,
            operations: vec![EditOperation::Translate {
                target: "head".into(),
                delta: [0., 0.02, 0.],
            }],
            locks: vec![],
            paired: false,
            reason: "Move head with attached sclera".into(),
        })
        .unwrap();
    assert!(candidate.changed_assets.contains(&"eye_white_l".into()));
    doc.current.attachments[0].parent_topology = "stale".into();
    assert!(matches!(
        doc.propose(narrow(0)),
        Err(CharacterError::Rebind(_))
    ));
}

#[test]
fn source_mutation_and_attachment_cycles_are_rejected() {
    let mut doc = template();
    bind_attachment(&mut doc.current, "head", "eye_white_l", None).unwrap();
    assert!(bind_attachment(&mut doc.current, "eye_white_l", "head", None).is_err());
    doc.current.source.push(' ');
    assert!(matches!(
        doc.propose(narrow(0)),
        Err(CharacterError::SourceChanged)
    ));
}

#[test]
fn typed_review_camera_is_orthographic_and_legacy_dsl_is_perspective() {
    let doc = template();
    let report = review_report(&doc.current, 0, None).unwrap();
    let graph = review_graph(&doc.current, &report.views[0], 256, true).unwrap();
    let json = serde_json::to_string(&graph).unwrap();
    assert!(json.contains("orthographic"));
    let original = motionloom::api::parse_graph_script(&doc.current.source).unwrap();
    assert!(
        !serde_json::to_string(&original)
            .unwrap()
            .contains("\"projection\":\"orthographic\"")
    );
}

#[test]
fn parameter_edits_preserve_locks_and_json_commands_share_the_contract() {
    let mut doc = template();
    let mut params = doc.current.parameters.clone().unwrap();
    params.rib_depth *= 1.1;
    let candidate = doc
        .propose_parameters(0, params, vec!["eyes.iris".into()])
        .unwrap();
    assert!(candidate.changed_assets.contains(&"torso".into()));
    let request = serde_json::json!({"operation":"proposeEdit","characterId":"test","request":{"expectedRevision":0,"operations":[{"kind":"depth","target":"body.ribcage","factor":1.1,"centerZ":0.0,"falloff":0.0}],"locks":["eyes.iris"],"reason":"Increase chest depth"}});
    let command: CharacterCommand = serde_json::from_value(request).unwrap();
    let mut service = CharacterService::default();
    service.documents.insert(doc.id.clone(), doc);
    assert!(
        service
            .execute(command)
            .unwrap()
            .get("candidateId")
            .is_some()
    );
}

#[test]
fn tampered_persisted_candidate_is_not_committed() {
    let mut doc = template();
    let candidate = doc.propose(narrow(0)).unwrap();
    doc.candidates
        .get_mut(&candidate.id)
        .unwrap()
        .snapshot
        .source
        .push(' ');
    assert!(matches!(
        doc.commit(0, &candidate.id),
        Err(CharacterError::SourceChanged)
    ));
    assert_eq!(doc.revision, 0);
}

#[test]
fn cpu_review_renders_a_finite_nonempty_orthographic_frame() {
    let doc = template();
    let report = review_report(&doc.current, 0, None).unwrap();
    let graph = review_graph(&doc.current, &report.views[0], 96, true).unwrap();
    let image = pollster::block_on(render_cpu_review(&graph, &report.views[0], true)).unwrap();
    assert_eq!(image.dimensions(), (96, 96));
    let first = image.get_pixel(0, 0).0;
    assert!(image.pixels().filter(|p| p.0 != first).count() > 100);
}

#[test]
fn paired_translation_mirrors_only_the_explicit_counterpart() {
    let mut doc = template();
    let original = cages(&doc.current.source).unwrap();
    let candidate = doc
        .propose(EditRequest {
            expected_revision: 0,
            operations: vec![EditOperation::Translate {
                target: "eye_white_l".into(),
                delta: [-0.01, 0.01, 0.],
            }],
            locks: vec!["eyes.iris".into()],
            paired: true,
            reason: "Widen eye spacing symmetrically".into(),
        })
        .unwrap();
    let after = cages(&candidate.snapshot.source).unwrap();
    for (name, dx) in [("eye_white_l", -0.01), ("eye_white_r", 0.01)] {
        for (a, b) in original[name].positions.iter().zip(&after[name].positions) {
            assert!((b[0] - a[0] - dx).abs() < 1e-6);
            assert!((b[1] - a[1] - 0.01).abs() < 1e-6);
        }
    }
}

#[test]
fn registered_regions_are_revisioned_and_reject_invalid_indices() {
    let mut doc = template();
    assert!(
        doc.register_selection(0, "custom".into(), "torso".into(), Some(vec![999999]), None)
            .is_err()
    );
    assert_eq!(doc.revision, 0);
    doc.register_selection(
        0,
        "custom".into(),
        "torso".into(),
        Some(vec![20, 21, 22]),
        None,
    )
    .unwrap();
    assert_eq!(
        doc.current.selections["custom"][0].vertices,
        vec![20, 21, 22]
    );
    assert!(matches!(
        doc.propose(narrow(0)),
        Err(CharacterError::Stale { .. })
    ));
    doc.undo(1).unwrap();
    assert!(!doc.current.selections.contains_key("custom"));
}

#[test]
fn gray_review_neutralizes_resolved_materials_and_cel_bindings() {
    let doc = template();
    let report = review_report(&doc.current, 0, None).unwrap();
    let graph = review_graph(&doc.current, &report.views[0], 96, true).unwrap();
    for asset in &graph.assets {
        if let Some(primitive) = asset.primitive() {
            assert_eq!(primitive.color, [0.65, 0.65, 0.65, 1.]);
            assert_eq!(
                primitive.material_definition.as_ref().unwrap().base_color,
                [0.65, 0.65, 0.65, 1.]
            );
        }
    }
    assert!(
        serde_json::to_string(&graph)
            .unwrap()
            .contains("\"shading\":\"clay\"")
    );
}

#[test]
fn s101_actual_cages_preserve_all_iris_layers() {
    // The optional local showcase is outside this repository; ordinary CI must not require that checkout.
    let path = std::env::var_os("MOTIONLOOM_S101_SOURCE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../motionloom-example/showcase/s-000101/main.motionloom")
        });
    if !path.exists() {
        eprintln!(
            "S101 checkout unavailable; run the external S101 authoring driver with its explicit scene path."
        );
        return;
    }
    let source = std::fs::read_to_string(path).unwrap();
    let mut doc = CharacterDocument::import("s101".into(), source.clone()).unwrap();
    let before = cages(&source).unwrap();
    let candidate = doc.propose(narrow(0)).unwrap();
    let after = cages(&candidate.snapshot.source).unwrap();
    for (id, cage) in &before {
        if id.starts_with("iris")
            || id.starts_with("pupil")
            || id.starts_with("glint")
            || id.starts_with("eye_glint")
        {
            assert_eq!(
                serde_json::to_value(cage).unwrap(),
                serde_json::to_value(&after[id]).unwrap(),
                "{id}"
            );
        }
    }
}

#[test]
fn memory_persistence_preserves_history_candidates_and_validates_every_snapshot() {
    let mut doc = template();
    let first = doc.propose(narrow(0)).unwrap();
    doc.commit(0, &first.id).unwrap();
    doc.undo(1).unwrap();
    let pending = doc.propose(narrow(2)).unwrap();
    let bytes = doc.to_json_bytes().unwrap();
    let mut restored = CharacterDocument::from_json_bytes(&bytes).unwrap();
    assert_eq!(restored.to_json_bytes().unwrap(), bytes);
    assert_eq!(
        restored.candidates[&pending.id].snapshot.source,
        pending.snapshot.source
    );
    restored.redo(2).unwrap();
    assert_eq!(restored.current.source, first.snapshot.source);
    assert!(restored.commit(2, &pending.id).is_err());

    let mut service = CharacterService::default();
    service.load_document_bytes(&bytes).unwrap();
    assert!(service.load_document_bytes(&bytes).is_err());
    assert_eq!(service.document("test").unwrap().revision, 2);

    for key in ["current", "redo", "candidates"] {
        let mut poisoned: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let snapshot = match key {
            "current" => &mut poisoned["current"],
            "redo" => &mut poisoned["redo"][0],
            _ => &mut poisoned["candidates"][&pending.id]["snapshot"],
        };
        let source = snapshot["source"].as_str().unwrap().to_owned() + " ";
        snapshot["source"] = source.into();
        assert!(
            CharacterDocument::from_json_bytes(&serde_json::to_vec(&poisoned).unwrap()).is_err(),
            "{key}"
        );
    }
}

#[test]
fn async_and_native_review_share_framing_pixels_and_command_responses() {
    let mut doc = template();
    let candidate = doc.propose(narrow(0)).unwrap();
    let mut service = CharacterService::default();
    service.documents.insert(doc.id.clone(), doc);
    let command = CharacterCommand::Review {
        character_id: "test".into(),
        candidate_id: Some(candidate.id.clone()),
        output_directory: None,
        size: Some(64),
        gray: true,
        gpu: false,
    };
    let native = service.execute(command.clone()).unwrap();
    let asynchronous = pollster::block_on(service.execute_async(command)).unwrap();
    assert_eq!(native, asynchronous);
    let rendered =
        pollster::block_on(service.review_images("test", Some(&candidate.id), 64, true, false))
            .unwrap();
    assert_eq!(
        serde_json::to_value(&rendered.report.views).unwrap(),
        native["views"]
    );
    assert_eq!(rendered.report.images.len(), 6);
    assert!(rendered.images.len() > 6);
    let directory =
        std::env::temp_dir().join(format!("motionloom-memory-review-{}", std::process::id()));
    let written = native::write_review(rendered.clone(), &directory).unwrap();
    assert_eq!(
        serde_json::to_value(&written.views).unwrap(),
        native["views"]
    );
    for frame in &rendered.images {
        assert_eq!(
            image::open(directory.join(&frame.name)).unwrap().to_rgba8(),
            frame.pixels
        );
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn memory_export_and_rig_inspection_require_only_the_public_engine() {
    let doc = template();
    let bytes = pollster::block_on(export_character_glb(&doc.current)).unwrap();
    assert_eq!(&bytes[..4], b"glTF");
    let mut service = CharacterService::default();
    let result = service
        .inspect_rig_bytes("rig".into(), &bytes, &Default::default())
        .unwrap();
    assert_eq!(result["revision"], 0);
    assert!(!service.rigs["rig"].mesh().positions.is_empty());
    assert!(
        service
            .inspect_rig_bytes("rig".into(), &bytes, &Default::default())
            .is_err()
    );
    service.documents.insert(doc.id.clone(), doc.clone());
    let path = std::env::temp_dir().join(format!(
        "motionloom-memory-export-{}.glb",
        std::process::id()
    ));
    let response = service
        .execute(CharacterCommand::Export {
            character_id: doc.id,
            output_path: path.display().to_string(),
        })
        .unwrap();
    assert_eq!(response["bytes"], bytes.len());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    let schema = character_schema();
    assert_eq!(schema["rustModule"], "motionloom::api::character_authoring");
    assert_eq!(schema["nativePathIoAvailable"], true);
    assert!(
        schema["nativeOnlyOperations"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("inspectRigMesh"))
    );
}
