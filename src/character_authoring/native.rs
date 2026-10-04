// =========================================
// =========================================
// src/character_authoring/native.rs

//! Native storage and blocking conveniences delegate to the portable authoring core.
use super::*;
use serde_json::{Value, json};
use std::path::Path;

pub fn bounds(snapshot: &CharacterSnapshot) -> Result<[[f32; 3]; 2], CharacterError> {
    pollster::block_on(bounds_async(snapshot))
}

pub fn review_report(
    snapshot: &CharacterSnapshot,
    revision: u64,
    compare: Option<&CharacterSnapshot>,
) -> Result<ReviewReport, CharacterError> {
    pollster::block_on(review_report_async(snapshot, revision, compare))
}

/// Store the same pixels returned by asynchronous in-memory review.
pub async fn render_review(
    snapshot: &CharacterSnapshot,
    revision: u64,
    compare: Option<&CharacterSnapshot>,
    output: &Path,
    size: u32,
    gray: bool,
    gpu: bool,
) -> Result<ReviewReport, CharacterError> {
    let rendered = render_review_images(snapshot, revision, compare, size, gray, gpu).await?;
    write_review(rendered, output)
}

pub fn write_review(
    rendered: RenderedReview,
    output: &Path,
) -> Result<ReviewReport, CharacterError> {
    std::fs::create_dir_all(output)?;
    let mut report = rendered.report;
    for image in rendered.images {
        image.pixels.save(output.join(image.name))?;
    }
    report.images = report
        .images
        .iter()
        .map(|name| output.join(name).display().to_string())
        .collect();
    std::fs::write(
        output.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    Ok(report)
}

pub fn save_document(doc: &CharacterDocument, path: &Path) -> Result<(), CharacterError> {
    std::fs::write(path, doc.to_json_bytes()?)?;
    Ok(())
}

pub fn load_document(path: &Path) -> Result<CharacterDocument, CharacterError> {
    CharacterDocument::from_json_bytes(&std::fs::read(path)?)
}

pub(crate) fn read_rig_file(path: &Path) -> Result<Vec<u8>, rig::RigError> {
    Ok(std::fs::read(path)?)
}

// Legacy JSON path operations retain their response shape and native source checks.
pub(crate) fn execute_native(
    service: &mut CharacterService,
    command: CharacterCommand,
) -> Result<Value, CharacterError> {
    match command {
        CharacterCommand::InspectRigMesh {
            rig_id,
            path,
            options,
        } => {
            if rig_id.trim().is_empty() || service.rigs.contains_key(&rig_id) {
                return Err(CharacterError::Invalid(
                    "Choose a new, nonempty rig ID".into(),
                ));
            }
            let session = rig::RigAuthoringSession::from_glb_path(path, &options)?;
            let result = json!({"rigId":rig_id,"revision":session.revision(),"inspection":session.mesh().inspection});
            service.rigs.insert(rig_id, session);
            Ok(result)
        }
        CharacterCommand::ExportRig {
            rig_id,
            candidate_id,
            output_path,
        } => {
            let session = service.rig_session(&rig_id)?;
            let bytes = if let Some(id) = &candidate_id {
                session.export_candidate(id)?
            } else {
                session.export_committed()?
            };
            std::fs::write(&output_path, &bytes)?;
            Ok(
                json!({"path":output_path,"bytes":bytes.len(),"accepted":candidate_id.is_none(),"candidateId":candidate_id}),
            )
        }
        CharacterCommand::Export {
            character_id,
            output_path,
        } => {
            let doc = service.document(&character_id)?;
            doc.check_revision(doc.revision)?;
            let bytes = pollster::block_on(export_character_glb(&doc.current))?;
            std::fs::write(&output_path, &bytes)?;
            Ok(
                json!({"path":output_path,"bytes":bytes.len(),"revision":doc.revision,"static":true,
                "skeletalAnimation":false,"portableCelShader":false}),
            )
        }
        CharacterCommand::Save { character_id, path } => {
            let doc = service.document(&character_id)?;
            save_document(doc, Path::new(&path))?;
            Ok(json!({"path":path,"revision":doc.revision}))
        }
        CharacterCommand::Load { path } => service.load_document_bytes(&std::fs::read(path)?),
        _ => Err(CharacterError::Invalid(
            "Expected a native path command".into(),
        )),
    }
}

// The legacy blocking review command delegates to the same async service path.
pub(crate) fn execute_review(
    service: &mut CharacterService,
    command: CharacterCommand,
) -> Result<Value, CharacterError> {
    pollster::block_on(service.execute_async(command))
}
