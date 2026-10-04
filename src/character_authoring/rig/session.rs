// =========================================
// =========================================
// src/character_authoring/rig/session.rs

use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RigCandidate {
    pub id: String,
    pub base_revision: u64,
    pub binding: HumanoidBinding,
}

/// An independent rig session leaves existing character geometry editing unchanged.
pub struct RigAuthoringSession {
    mesh: RigMesh,
    #[cfg(not(target_arch = "wasm32"))]
    source_path: Option<PathBuf>,
    revision: u64,
    next_candidate: u64,
    candidates: BTreeMap<String, RigCandidate>,
    verifications: BTreeMap<String, HumanoidBindingVerification>,
    committed: Option<HumanoidBinding>,
}

impl RigAuthoringSession {
    pub fn from_glb_bytes(bytes: &[u8], options: &MeshInspectionOptions) -> Result<Self, RigError> {
        Ok(Self {
            mesh: inspect_rig_mesh(bytes, options)?,
            #[cfg(not(target_arch = "wasm32"))]
            source_path: None,
            revision: 0,
            next_candidate: 1,
            candidates: BTreeMap::new(),
            verifications: BTreeMap::new(),
            committed: None,
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_glb_path(
        path: impl AsRef<Path>,
        options: &MeshInspectionOptions,
    ) -> Result<Self, RigError> {
        let path = path.as_ref();
        let mut session =
            Self::from_glb_bytes(&super::super::native::read_rig_file(path)?, options)?;
        session.source_path = Some(path.to_path_buf());
        Ok(session)
    }
    pub fn mesh(&self) -> &RigMesh {
        &self.mesh
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn committed(&self) -> Option<&HumanoidBinding> {
        self.committed.as_ref()
    }
    fn check(&self, expected: u64) -> Result<(), RigError> {
        if self.revision != expected {
            return Err(RigError::Stale {
                expected,
                current: self.revision,
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = &self.source_path
            && super::mesh::fingerprint(&super::super::native::read_rig_file(path)?)
                != self.mesh.inspection.source_fingerprint
        {
            return Err(RigError::SourceChanged);
        }
        Ok(())
    }
    pub fn candidate(&self, id: &str) -> Result<&RigCandidate, RigError> {
        self.candidates
            .get(id)
            .ok_or_else(|| RigError::Missing(id.into()))
    }
    /// A proposal never overwrites the current binding, and carries its own geometry fingerprint.
    pub fn propose(
        &mut self,
        expected_revision: u64,
        request: &RigBuildRequest,
        options: &RigBindingOptions,
    ) -> Result<RigCandidate, RigError> {
        self.check(expected_revision)?;
        let skeleton = build_humanoid_skeleton(&self.mesh, request)?;
        let binding = bind_humanoid_skin(&self.mesh, skeleton, options)?;
        let id = format!("rig-candidate-{}", self.next_candidate);
        self.next_candidate += 1;
        let candidate = RigCandidate {
            id: id.clone(),
            base_revision: self.revision,
            binding,
        };
        self.candidates.insert(id, candidate.clone());
        Ok(candidate)
    }
    pub fn verify(
        &mut self,
        id: &str,
        options: &RigVerificationOptions,
    ) -> Result<HumanoidBindingVerification, RigError> {
        self.check(self.revision)?;
        let candidate = self.candidate(id)?;
        let report = verify_humanoid_binding(&self.mesh, &candidate.binding, options)?;
        self.verifications.insert(id.into(), report.clone());
        Ok(report)
    }
    /// Explain candidate weights before running a motion suite.
    pub fn inspect_weights(
        &self,
        id: &str,
        options: &RigWeightCheckOptions,
    ) -> Result<RigWeightDiagnostics, RigError> {
        self.check(self.revision)?;
        inspect_rig_weights(&self.mesh, &self.candidate(id)?.binding, options)
    }
    /// Uncertain geometry remains pending instead of falling back to the torso.
    pub fn propose_weight_regions(
        &self,
        id: &str,
        options: &RigWeightCheckOptions,
    ) -> Result<RigBindingOptions, RigError> {
        self.check(self.revision)?;
        propose_rig_weight_regions(&self.mesh, &self.candidate(id)?.binding.skeleton, options)
    }
    /// Head suggestions keep unreviewed geometry pending; reviewed annotations become persistent constraints.
    pub fn propose_head_constraints(
        &self,
        id: &str,
        options: &RigHeadConstraintOptions,
    ) -> Result<RigHeadConstraintProposal, RigError> {
        self.check(self.revision)?;
        propose_rig_head_constraints(&self.mesh, &self.candidate(id)?.binding, options)
    }
    /// Suggestions require an actual verification receipt for this immutable candidate.
    pub fn suggest_weight_refinement(
        &self,
        id: &str,
        options: &RigWeightCheckOptions,
    ) -> Result<RigWeightRefinement, RigError> {
        self.check(self.revision)?;
        let report = self.verifications.get(id).ok_or(RigError::NotVerified)?;
        suggest_rig_weight_refinement(&self.mesh, &self.candidate(id)?.binding, report, options)
    }
    /// Apply reviewed corrections to a new immutable candidate at the same revision.
    pub fn refine_weights(
        &mut self,
        expected_revision: u64,
        id: &str,
        refinement: &RigWeightRefinement,
    ) -> Result<RigCandidate, RigError> {
        self.check(expected_revision)?;
        let candidate = self.candidate(id)?;
        if candidate.base_revision != self.revision {
            return Err(RigError::Stale {
                expected: candidate.base_revision,
                current: self.revision,
            });
        }
        let options =
            super::weights::refined_binding_options(&self.mesh, &candidate.binding, refinement)?;
        // A no-op proposal preserves the exact receipt, but still gets a new unverified candidate ID.
        let unchanged = candidate
            .binding
            .binding_options
            .as_ref()
            .map(|old| {
                Ok::<_, RigError>(serde_json::to_value(old)? == serde_json::to_value(&options)?)
            })
            .transpose()?
            .unwrap_or(false);
        let binding = if unchanged {
            candidate.binding.clone()
        } else {
            bind_humanoid_skin(&self.mesh, candidate.binding.skeleton.clone(), &options)?
        };
        let id = format!("rig-candidate-{}", self.next_candidate);
        self.next_candidate += 1;
        let candidate = RigCandidate {
            id: id.clone(),
            base_revision: self.revision,
            binding,
        };
        self.candidates.insert(id, candidate.clone());
        Ok(candidate)
    }
    /// Only a report for the exact candidate can authorize accepting its binding.
    pub fn commit(
        &mut self,
        expected_revision: u64,
        id: &str,
    ) -> Result<&HumanoidBinding, RigError> {
        self.check(expected_revision)?;
        let candidate = self.candidate(id)?;
        let report = self.verifications.get(id).ok_or(RigError::NotVerified)?;
        if candidate.base_revision != self.revision
            || !report.accepted
            || report.binding_fingerprint != candidate.binding.fingerprint
        {
            return Err(RigError::NotVerified);
        }
        self.committed = Some(candidate.binding.clone());
        self.revision += 1;
        self.candidates.clear();
        self.verifications.clear();
        Ok(self.committed.as_ref().unwrap())
    }
    pub fn discard(&mut self, id: &str) -> Result<(), RigError> {
        self.candidates
            .remove(id)
            .ok_or_else(|| RigError::Missing(id.into()))?;
        self.verifications.remove(id);
        Ok(())
    }
    pub fn export_candidate(&self, id: &str) -> Result<Vec<u8>, RigError> {
        self.check(self.revision)?;
        export_rig_glb(&self.mesh, &self.candidate(id)?.binding)
    }
    pub fn export_committed(&self) -> Result<Vec<u8>, RigError> {
        self.check(self.revision)?;
        export_rig_glb(
            &self.mesh,
            self.committed.as_ref().ok_or(RigError::NotVerified)?,
        )
    }
}
