// =========================================
// =========================================
// src/character_authoring/service.rs

use super::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CharacterCommand {
    Schema,
    RigStandard,
    InspectRigMesh {
        rig_id: String,
        path: String,
        #[serde(default)]
        options: rig::MeshInspectionOptions,
    },
    RigMeshData {
        rig_id: String,
    },
    QueryRigVertices {
        rig_id: String,
        position: [f32; 3],
        count: usize,
    },
    BuildRig {
        rig_id: String,
        expected_revision: u64,
        request: rig::RigBuildRequest,
        #[serde(default)]
        binding: rig::RigBindingOptions,
    },
    VerifyRig {
        rig_id: String,
        candidate_id: String,
        #[serde(default)]
        options: rig::RigVerificationOptions,
    },
    InspectRigWeights {
        rig_id: String,
        candidate_id: String,
        #[serde(default)]
        options: rig::RigWeightCheckOptions,
    },
    ProposeRigWeightRegions {
        rig_id: String,
        candidate_id: String,
        #[serde(default)]
        options: rig::RigWeightCheckOptions,
    },
    ProposeRigHeadConstraints {
        rig_id: String,
        candidate_id: String,
        #[serde(default)]
        options: rig::RigHeadConstraintOptions,
    },
    SuggestRigWeightRefinement {
        rig_id: String,
        candidate_id: String,
        #[serde(default)]
        options: rig::RigWeightCheckOptions,
    },
    RefineRigWeights {
        rig_id: String,
        expected_revision: u64,
        candidate_id: String,
        refinement: rig::RigWeightRefinement,
    },
    CommitRig {
        rig_id: String,
        expected_revision: u64,
        candidate_id: String,
    },
    DiscardRig {
        rig_id: String,
        candidate_id: String,
    },
    ExportRig {
        rig_id: String,
        candidate_id: Option<String>,
        output_path: String,
    },
    List,
    Import {
        character_id: String,
        source: String,
    },
    BuildTemplate {
        character_id: String,
        parameters: CharacterParameters,
    },
    Inspect {
        character_id: String,
    },
    RegisterSelection {
        character_id: String,
        expected_revision: u64,
        selection_id: String,
        asset_id: String,
        vertices: Option<Vec<usize>>,
        mirror_selection: Option<String>,
    },
    ProposeEdit {
        character_id: String,
        request: EditRequest,
    },
    ProposeParameters {
        character_id: String,
        expected_revision: u64,
        parameters: CharacterParameters,
        locks: Vec<String>,
    },
    CommitEdit {
        character_id: String,
        expected_revision: u64,
        candidate_id: String,
    },
    DiscardEdit {
        character_id: String,
        candidate_id: String,
    },
    Undo {
        character_id: String,
        expected_revision: u64,
    },
    Redo {
        character_id: String,
        expected_revision: u64,
    },
    BindAttachment {
        character_id: String,
        expected_revision: u64,
        parent_asset: String,
        child_asset: String,
        minimum_clearance: Option<f32>,
    },
    Review {
        character_id: String,
        candidate_id: Option<String>,
        output_directory: Option<String>,
        size: Option<u32>,
        gray: bool,
        gpu: bool,
    },
    Export {
        character_id: String,
        output_path: String,
    },
    Save {
        character_id: String,
        path: String,
    },
    Load {
        path: String,
    },
}

#[derive(Default)]
pub struct CharacterService {
    pub documents: BTreeMap<String, CharacterDocument>,
    pub rigs: BTreeMap<String, rig::RigAuthoringSession>,
}

pub fn character_schema() -> Value {
    let mut schema = json!({"schemaVersion":1,"stability":"experimentalHostApi","operations":["schema","list","import","buildTemplate","inspect","registerSelection","proposeEdit","proposeParameters","commitEdit","discardEdit","undo","redo","bindAttachment","review","export","save","load"],"presets":["anime","chibi","neutral"],"templateDefaults":{"anime":CharacterParameters::preset(ProportionPreset::Anime),"chibi":CharacterParameters::preset(ProportionPreset::Chibi),"neutral":CharacterParameters::preset(ProportionPreset::Neutral)},"coordinates":{"up":"Y","width":"X","depth":"Z","units":"sceneWorldUnits"},"editKinds":["scale","translate","depth"],"policy":"Explicit design edits validate geometry and invariants. Use MotionLoom MeshAuthoringSession for reference fitting.","meshAuthoring":serde_json::from_str::<Value>(&crate::api::mesh_authoring::mesh_authoring_schema_json()).unwrap(),"example":{"operation":"proposeEdit","characterId":"s101","request":{"expectedRevision":0,"operations":[{"kind":"scale","target":"eyes.sclera","factors":[0.735,1,1]}],"locks":["eyes.iris"],"paired":true,"reason":"Narrow sclera without changing the iris."}}});
    let operations = schema["operations"].as_array_mut().unwrap();
    operations.extend(
        [
            "rigStandard",
            "inspectRigMesh",
            "rigMeshData",
            "queryRigVertices",
            "buildRig",
            "verifyRig",
            "inspectRigWeights",
            "proposeRigWeightRegions",
            "proposeRigHeadConstraints",
            "suggestRigWeightRefinement",
            "refineRigWeights",
            "commitRig",
            "discardRig",
            "exportRig",
        ]
        .map(|name| json!(name)),
    );
    schema["rigAuthoring"] = rig::rig_authoring_schema();
    schema["engine"] = json!("motionloom");
    schema["rustModule"] = json!("motionloom::api::character_authoring");
    schema["nativeOnlyOperations"] =
        json!(["inspectRigMesh", "exportRig", "export", "save", "load"]);
    schema["nativePathIoAvailable"] = json!(!cfg!(target_arch = "wasm32"));
    schema["asyncOperations"] = json!(["review"]);
    schema["portableTypedOperations"] = json!([
        "CharacterDocument::import",
        "CharacterDocument::to_json_bytes",
        "CharacterDocument::from_json_bytes",
        "CharacterService::inspect_rig_bytes",
        "CharacterService::review_images",
        "export_character_glb",
        "RigAuthoringSession::from_glb_bytes",
        "RigAuthoringSession::export_candidate"
    ]);
    #[cfg(target_arch = "wasm32")]
    schema["operations"].as_array_mut().unwrap().retain(|op| {
        !["inspectRigMesh", "exportRig", "export", "save", "load"]
            .contains(&op.as_str().unwrap_or(""))
    });
    schema
}

impl CharacterService {
    pub fn document(&self, id: &str) -> Result<&CharacterDocument, CharacterError> {
        self.documents
            .get(id)
            .ok_or_else(|| CharacterError::Missing(id.into()))
    }
    fn document_mut(&mut self, id: &str) -> Result<&mut CharacterDocument, CharacterError> {
        self.documents
            .get_mut(id)
            .ok_or_else(|| CharacterError::Missing(id.into()))
    }
    /// Native callers retain synchronous commands; applications own service and transport lifetimes.
    pub fn execute(&mut self, command: CharacterCommand) -> Result<Value, CharacterError> {
        match command {
            CharacterCommand::Schema => Ok(character_schema()),
            CharacterCommand::RigStandard => {
                Ok(serde_json::to_value(rig::humanoid_rig_standard())?)
            }
            command @ (CharacterCommand::InspectRigMesh { .. }
            | CharacterCommand::ExportRig { .. }
            | CharacterCommand::Export { .. }
            | CharacterCommand::Save { .. }
            | CharacterCommand::Load { .. }) => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    native::execute_native(self, command)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = command;
                    Err(CharacterError::Unsupported("native path I/O"))
                }
            }
            CharacterCommand::RigMeshData { rig_id } => {
                let session = self.rig_session(&rig_id)?;
                Ok(
                    json!({"rigId":rig_id,"revision":session.revision(),"inspection":session.mesh().inspection,"positions":session.mesh().positions,"normals":session.mesh().normals,"triangles":session.mesh().triangles}),
                )
            }
            CharacterCommand::QueryRigVertices {
                rig_id,
                position,
                count,
            } => Ok(
                json!({"vertices":rig::nearest_rig_vertices(self.rig_session(&rig_id)?.mesh(),position,count)?}),
            ),
            CharacterCommand::BuildRig {
                rig_id,
                expected_revision,
                request,
                binding,
            } => Ok(serde_json::to_value(
                self.rig_session_mut(&rig_id)?
                    .propose(expected_revision, &request, &binding)?,
            )?),
            CharacterCommand::VerifyRig {
                rig_id,
                candidate_id,
                options,
            } => Ok(serde_json::to_value(
                self.rig_session_mut(&rig_id)?
                    .verify(&candidate_id, &options)?,
            )?),
            CharacterCommand::InspectRigWeights {
                rig_id,
                candidate_id,
                options,
            } => Ok(serde_json::to_value(
                self.rig_session(&rig_id)?
                    .inspect_weights(&candidate_id, &options)?,
            )?),
            CharacterCommand::ProposeRigWeightRegions {
                rig_id,
                candidate_id,
                options,
            } => Ok(serde_json::to_value(
                self.rig_session(&rig_id)?
                    .propose_weight_regions(&candidate_id, &options)?,
            )?),
            CharacterCommand::ProposeRigHeadConstraints {
                rig_id,
                candidate_id,
                options,
            } => Ok(serde_json::to_value(
                self.rig_session(&rig_id)?
                    .propose_head_constraints(&candidate_id, &options)?,
            )?),
            CharacterCommand::SuggestRigWeightRefinement {
                rig_id,
                candidate_id,
                options,
            } => Ok(serde_json::to_value(
                self.rig_session(&rig_id)?
                    .suggest_weight_refinement(&candidate_id, &options)?,
            )?),
            CharacterCommand::RefineRigWeights {
                rig_id,
                expected_revision,
                candidate_id,
                refinement,
            } => Ok(serde_json::to_value(
                self.rig_session_mut(&rig_id)?.refine_weights(
                    expected_revision,
                    &candidate_id,
                    &refinement,
                )?,
            )?),
            CharacterCommand::CommitRig {
                rig_id,
                expected_revision,
                candidate_id,
            } => {
                self.rig_session_mut(&rig_id)?
                    .commit(expected_revision, &candidate_id)?;
                Ok(
                    json!({"rigId":rig_id,"revision":self.rig_session(&rig_id)?.revision(),"accepted":true}),
                )
            }
            CharacterCommand::DiscardRig {
                rig_id,
                candidate_id,
            } => {
                self.rig_session_mut(&rig_id)?.discard(&candidate_id)?;
                Ok(json!({"rigId":rig_id,"revision":self.rig_session(&rig_id)?.revision()}))
            }
            CharacterCommand::List => Ok(
                json!({"characters":self.documents.values().map(|d|json!({"characterId":d.id,"revision":d.revision,"sourceFingerprint":d.current.source_fingerprint})).collect::<Vec<_>>()}),
            ),
            CharacterCommand::Import {
                character_id,
                source,
            } => {
                if self.documents.contains_key(&character_id) {
                    return Err(CharacterError::Invalid(
                        "Character already exists; choose another ID".into(),
                    ));
                }
                let doc = CharacterDocument::import(character_id.clone(), source)?;
                self.documents.insert(character_id.clone(), doc);
                self.inspect(&character_id)
            }
            CharacterCommand::BuildTemplate {
                character_id,
                parameters,
            } => {
                if self.documents.contains_key(&character_id) {
                    return Err(CharacterError::Invalid("Character already exists".into()));
                }
                let doc = build_template(character_id.clone(), parameters)?;
                self.documents.insert(character_id.clone(), doc);
                self.inspect(&character_id)
            }
            CharacterCommand::Inspect { character_id } => self.inspect(&character_id),
            CharacterCommand::RegisterSelection {
                character_id,
                expected_revision,
                selection_id,
                asset_id,
                vertices,
                mirror_selection,
            } => {
                self.document_mut(&character_id)?.register_selection(
                    expected_revision,
                    selection_id,
                    asset_id,
                    vertices,
                    mirror_selection,
                )?;
                self.inspect(&character_id)
            }
            CharacterCommand::ProposeEdit {
                character_id,
                request,
            } => {
                let candidate = self.document_mut(&character_id)?.propose(request)?;
                Ok(
                    json!({"candidateId":candidate.id,"baseRevision":candidate.base_revision,"changedAssets":candidate.changed_assets,"lockedAssets":candidate.locked_assets,"validationPolicy":candidate.validation_policy}),
                )
            }
            CharacterCommand::ProposeParameters {
                character_id,
                expected_revision,
                parameters,
                locks,
            } => {
                let candidate = self.document_mut(&character_id)?.propose_parameters(
                    expected_revision,
                    parameters,
                    locks,
                )?;
                Ok(
                    json!({"candidateId":candidate.id,"baseRevision":candidate.base_revision,"changedAssets":candidate.changed_assets,"lockedAssets":candidate.locked_assets}),
                )
            }
            CharacterCommand::CommitEdit {
                character_id,
                expected_revision,
                candidate_id,
            } => {
                self.document_mut(&character_id)?
                    .commit(expected_revision, &candidate_id)?;
                self.inspect(&character_id)
            }
            CharacterCommand::DiscardEdit {
                character_id,
                candidate_id,
            } => {
                self.document_mut(&character_id)?
                    .candidates
                    .remove(&candidate_id)
                    .ok_or_else(|| CharacterError::Missing(candidate_id))?;
                self.inspect(&character_id)
            }
            CharacterCommand::Undo {
                character_id,
                expected_revision,
            } => {
                self.document_mut(&character_id)?.undo(expected_revision)?;
                self.inspect(&character_id)
            }
            CharacterCommand::Redo {
                character_id,
                expected_revision,
            } => {
                self.document_mut(&character_id)?.redo(expected_revision)?;
                self.inspect(&character_id)
            }
            CharacterCommand::BindAttachment {
                character_id,
                expected_revision,
                parent_asset,
                child_asset,
                minimum_clearance,
            } => {
                let doc = self.document_mut(&character_id)?;
                doc.check_revision(expected_revision)?;
                let mut next = doc.current.clone();
                bind_attachment(&mut next, &parent_asset, &child_asset, minimum_clearance)?;
                doc.history.push(doc.current.clone());
                doc.current = next;
                doc.revision += 1;
                doc.redo.clear();
                doc.candidates.clear();
                self.inspect(&character_id)
            }
            command @ CharacterCommand::Review { .. } => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    native::execute_review(self, command)
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = command;
                    Err(CharacterError::Unsupported(
                        "synchronous review; use execute_async",
                    ))
                }
            }
        }
    }

    /// Async review is portable; native path commands retain their synchronous contract.
    pub async fn execute_async(
        &mut self,
        command: CharacterCommand,
    ) -> Result<Value, CharacterError> {
        match command {
            CharacterCommand::Review {
                character_id,
                candidate_id,
                output_directory,
                size,
                gray,
                gpu,
            } => {
                self.review_command(
                    &character_id,
                    candidate_id.as_deref(),
                    output_directory.as_deref(),
                    size.unwrap_or(400),
                    gray,
                    gpu,
                )
                .await
            }
            other => self.execute(other),
        }
    }

    /// Restore a validated document without opening a file or replacing an existing session.
    pub fn load_document_bytes(&mut self, bytes: &[u8]) -> Result<Value, CharacterError> {
        let doc = CharacterDocument::from_json_bytes(bytes)?;
        let id = doc.id.clone();
        if self.documents.contains_key(&id) {
            return Err(CharacterError::Invalid("Character already exists".into()));
        }
        self.documents.insert(id.clone(), doc);
        self.inspect(&id)
    }

    /// The host supplies GLB bytes; input skins never become anatomy hints.
    pub fn inspect_rig_bytes(
        &mut self,
        id: String,
        bytes: &[u8],
        options: &rig::MeshInspectionOptions,
    ) -> Result<Value, CharacterError> {
        if id.trim().is_empty() || self.rigs.contains_key(&id) {
            return Err(CharacterError::Invalid(
                "Choose a new, nonempty rig ID".into(),
            ));
        }
        let session = rig::RigAuthoringSession::from_glb_bytes(bytes, options)?;
        let result = json!({"rigId":id,"revision":session.revision(),"inspection":session.mesh().inspection});
        self.rigs.insert(id, session);
        Ok(result)
    }

    // Clone review inputs so in-memory pixels and metadata share a frozen revision.
    fn review_inputs(
        &self,
        id: &str,
        candidate_id: Option<&str>,
    ) -> Result<(CharacterSnapshot, u64, Option<CharacterSnapshot>), CharacterError> {
        let doc = self.document(id)?;
        doc.check_revision(doc.revision)?;
        let snapshot = if let Some(id) = candidate_id {
            let c = doc
                .candidates
                .get(id)
                .ok_or_else(|| CharacterError::Missing(id.into()))?;
            if c.base_revision != doc.revision {
                return Err(CharacterError::Stale {
                    expected: c.base_revision,
                    current: doc.revision,
                });
            }
            &c.snapshot
        } else {
            &doc.current
        };
        let compare = if snapshot.source_fingerprint != doc.current.source_fingerprint {
            Some(&doc.current)
        } else {
            doc.history.last()
        };
        Ok((snapshot.clone(), doc.revision, compare.cloned()))
    }

    pub async fn review_images(
        &self,
        id: &str,
        candidate_id: Option<&str>,
        size: u32,
        gray: bool,
        gpu: bool,
    ) -> Result<RenderedReview, CharacterError> {
        let (snapshot, revision, compare) = self.review_inputs(id, candidate_id)?;
        render_review_images(&snapshot, revision, compare.as_ref(), size, gray, gpu).await
    }

    async fn review_command(
        &self,
        id: &str,
        candidate_id: Option<&str>,
        output: Option<&str>,
        size: u32,
        gray: bool,
        gpu: bool,
    ) -> Result<Value, CharacterError> {
        let (snapshot, revision, compare) = self.review_inputs(id, candidate_id)?;
        let report = if let Some(output) = output {
            #[cfg(not(target_arch = "wasm32"))]
            {
                native::render_review(
                    &snapshot,
                    revision,
                    compare.as_ref(),
                    std::path::Path::new(output),
                    size,
                    gray,
                    gpu,
                )
                .await?
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = (output, size, gray, gpu);
                return Err(CharacterError::Unsupported("review directory writing"));
            }
        } else {
            review_report_async(&snapshot, revision, compare.as_ref()).await?
        };
        Ok(serde_json::to_value(report)?)
    }

    pub(crate) fn rig_session(
        &self,
        id: &str,
    ) -> Result<&rig::RigAuthoringSession, CharacterError> {
        self.rigs
            .get(id)
            .ok_or_else(|| CharacterError::Missing(id.into()))
    }
    pub(crate) fn rig_session_mut(
        &mut self,
        id: &str,
    ) -> Result<&mut rig::RigAuthoringSession, CharacterError> {
        self.rigs
            .get_mut(id)
            .ok_or_else(|| CharacterError::Missing(id.into()))
    }
    pub(crate) fn inspect(&self, id: &str) -> Result<Value, CharacterError> {
        let doc = self.document(id)?;
        Ok(
            json!({"characterId":id,"revision":doc.revision,"sourceFingerprint":doc.current.source_fingerprint,"selections":doc.current.selections.iter().map(|(name,items)|(name,items.iter().map(|item|json!({"assetId":item.asset_id,"vertexCount":item.vertices.len(),"topologySignature":item.topology_signature,"pair":item.pair})).collect::<Vec<_>>())).collect::<BTreeMap<_,_>>(),"attachments":doc.current.attachments.iter().map(|a|json!({"parentAsset":a.parent_asset,"childAsset":a.child_asset,"parentTopology":a.parent_topology,"childTopology":a.child_topology,"bindingCount":a.bindings.len(),"minimumClearance":a.minimum_clearance})).collect::<Vec<_>>(),"parameters":doc.current.parameters,"assumptions":doc.current.assumptions,"candidateIds":doc.candidates.keys().collect::<Vec<_>>()}),
        )
    }
}
