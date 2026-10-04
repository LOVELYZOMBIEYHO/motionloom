// =========================================
// =========================================
// examples/rig_api_workflow.rs

//! A host example of the Rust API; no UI, ACP or existing target skeleton is involved.
use motionloom::api::character_authoring::rig::*;
use std::{collections::BTreeMap, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    if args.len() < 4 {
        return Err(
            "Expected target.glb action-library.motionloom output-directory [build-request.json]"
                .into(),
        );
    }
    let output = Path::new(&args[3]);
    fs::create_dir_all(output)?;
    let mut session = RigAuthoringSession::from_glb_path(
        &args[1],
        &MeshInspectionOptions {
            target_height: Some(1.8),
            ..Default::default()
        },
    )?;
    let request = if let Some(path) = args.get(4) {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        RigBuildRequest {
            expected_geometry_fingerprint: session.mesh().inspection.geometry_fingerprint.clone(),
            reference: "character1".into(),
            landmarks: BTreeMap::new(),
            rotations: BTreeMap::new(),
            extensions: vec![],
        }
    };
    let candidate = session.propose(0, &request, &Default::default())?;
    let library_source = fs::read_to_string(&args[2])?;
    let actions = motionloom::api::parse_action_library_document(&library_source)?;
    let options = RigVerificationOptions {
        actions: actions
            .iter()
            .map(|action| RigActionTest {
                library_source: library_source.clone(),
                action_id: action.id.clone(),
                phases: vec![0.2, 0.5, 0.8],
            })
            .collect(),
        ..Default::default()
    };
    let report = session.verify(&candidate.id, &options)?;
    fs::write(
        output.join("inspection.json"),
        serde_json::to_string_pretty(&session.mesh().inspection)?,
    )?;
    fs::write(
        output.join("candidate.json"),
        serde_json::to_string_pretty(&candidate)?,
    )?;
    fs::write(
        output.join("verification.json"),
        serde_json::to_string_pretty(&report)?,
    )?;
    // Review artifacts remain drafts when either anatomy or deformation is unverified.
    fs::write(
        output.join("candidate.glb"),
        session.export_candidate(&candidate.id)?,
    )?;
    fs::write(
        output.join("skeleton.fragment.motionloom"),
        &candidate.binding.skeleton.skeleton_dsl,
    )?;
    fs::write(
        output.join("profile.fragment.motionloom"),
        &candidate.binding.profile_dsl,
    )?;
    if report.accepted {
        session.commit(0, &candidate.id)?;
    }
    println!(
        "{}",
        serde_json::json!({"coreNodes":report.core_nodes,"samples":report.samples.len(),"structure":report.structure_status,"bindPose":report.bind_pose_status,"direction":report.direction_status,"deformation":report.deformation_status,"status":report.status,"accepted":report.accepted,"issues":report.issues.len(),"output":output})
    );
    Ok(())
}
