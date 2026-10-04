// =========================================
// =========================================
// src/character_authoring/document.rs

use super::{CharacterError, CharacterParameters};
use crate::api::mesh_reference::{mesh_source_fingerprint, mesh_topology_signature};
use crate::{ControlCageNode, PrimitiveGeometry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub asset_id: String,
    pub vertices: Vec<usize>,
    pub topology_signature: String,
    pub pair: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterSnapshot {
    pub source: String,
    pub source_fingerprint: String,
    pub selections: BTreeMap<String, Vec<Selection>>,
    pub attachments: Vec<super::Attachment>,
    pub parameters: Option<CharacterParameters>,
    pub assumptions: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterDocument {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub current: CharacterSnapshot,
    pub history: Vec<CharacterSnapshot>,
    pub redo: Vec<CharacterSnapshot>,
    pub candidates: BTreeMap<String, super::EditCandidate>,
    pub next_candidate: u64,
}

impl CharacterSnapshot {
    /// Validate persisted semantic and attachment indices before exposing a loaded document.
    pub fn validate(&self) -> Result<(), CharacterError> {
        if mesh_source_fingerprint(&self.source) != self.source_fingerprint {
            return Err(CharacterError::SourceChanged);
        }
        let assets = cages(&self.source)?;
        for items in self.selections.values() {
            for item in items {
                let cage = assets
                    .get(&item.asset_id)
                    .ok_or_else(|| CharacterError::Rebind(item.asset_id.clone()))?;
                if item.topology_signature != mesh_topology_signature(cage)
                    || item.vertices.is_empty()
                    || item.vertices.iter().any(|i| *i >= cage.positions.len())
                    || item
                        .pair
                        .as_ref()
                        .is_some_and(|p| !self.selections.contains_key(p))
                {
                    return Err(CharacterError::Rebind(item.asset_id.clone()));
                }
            }
        }
        let mut parents = std::collections::BTreeMap::new();
        for a in &self.attachments {
            let p = assets
                .get(&a.parent_asset)
                .ok_or_else(|| CharacterError::Rebind(a.child_asset.clone()))?;
            let c = assets
                .get(&a.child_asset)
                .ok_or_else(|| CharacterError::Rebind(a.child_asset.clone()))?;
            if a.parent_topology != mesh_topology_signature(p)
                || a.child_topology != mesh_topology_signature(c)
                || a.bindings.len() != c.positions.len()
                || parents.insert(&a.child_asset, &a.parent_asset).is_some()
                || a.minimum_clearance
                    .is_some_and(|v| !v.is_finite() || v < 0.)
            {
                return Err(CharacterError::Rebind(a.child_asset.clone()));
            }
            for b in &a.bindings {
                if b.triangle.iter().any(|i| *i >= p.positions.len())
                    || b.rest_offset.iter().any(|v| !v.is_finite())
                    || b.weights
                        .iter()
                        .any(|v| !v.is_finite() || *v < 0. || *v > 1.)
                    || (b.weights.iter().sum::<f32>() - 1.).abs() > 1e-4
                {
                    return Err(CharacterError::Rebind(a.child_asset.clone()));
                }
            }
        }
        for child in parents.keys() {
            let mut seen = std::collections::BTreeSet::new();
            let mut node = *child;
            while let Some(parent) = parents.get(node) {
                if !seen.insert(node) {
                    return Err(CharacterError::Invalid("Attachment cycle".into()));
                }
                node = parent;
            }
        }
        Ok(())
    }
}

/// Import editable cages without inferring unavailable generator parameters.
pub fn cages(source: &str) -> Result<BTreeMap<String, ControlCageNode>, CharacterError> {
    let graph = crate::api::parse_graph_script(source)
        .map_err(|e| CharacterError::Invalid(e.to_string()))?;
    let mut result = BTreeMap::new();
    for asset in &graph.assets {
        if let Some(primitive) = asset.primitive()
            && let PrimitiveGeometry::Mesh { cage } = &primitive.geometry
        {
            result.insert(asset.id.clone(), cage.clone());
        }
    }
    if result.is_empty() {
        return Err(CharacterError::Invalid(
            "No editable MeshAsset found".into(),
        ));
    }
    Ok(result)
}

fn semantic(id: &str) -> &str {
    if id.starts_with("eye_white") || id.starts_with("sclera") {
        "eyes.sclera"
    } else if id.starts_with("iris")
        || id.starts_with("pupil")
        || id.starts_with("glint")
        || id.starts_with("eye_glint")
    {
        "eyes.iris"
    } else if id.starts_with("upper_lid") || id.starts_with("lower_lid") || id.starts_with("lid") {
        "eyes.eyelids"
    } else if id == "tunic" || id == "torso" {
        "body.torso"
    } else if id.starts_with("leg") {
        "body.legs"
    } else if id.starts_with("forearm") || id.starts_with("arm") {
        "body.arms"
    } else {
        id
    }
}

impl CharacterDocument {
    /// Serialize the existing document format without imposing host storage paths.
    pub fn to_json_bytes(&self) -> Result<Vec<u8>, CharacterError> {
        self.check_revision(self.revision)?;
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Restore and validate every persisted snapshot before exposing authoring state.
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, CharacterError> {
        let doc: Self = serde_json::from_slice(bytes)?;
        doc.check_revision(doc.revision)?;
        if doc.schema_version != 1 {
            return Err(CharacterError::Invalid(
                "Unsupported character document version".into(),
            ));
        }
        doc.current.validate()?;
        for snapshot in doc
            .history
            .iter()
            .chain(&doc.redo)
            .chain(doc.candidates.values().map(|c| &c.snapshot))
        {
            snapshot.validate()?;
        }
        Ok(doc)
    }

    pub fn import(id: String, source: String) -> Result<Self, CharacterError> {
        if id.trim().is_empty() {
            return Err(CharacterError::Invalid("Character ID is empty".into()));
        }
        let assets = cages(&source)?;
        let mut selections: BTreeMap<String, Vec<Selection>> = BTreeMap::new();
        for (asset_id, cage) in assets {
            let item = Selection {
                asset_id: asset_id.clone(),
                vertices: (0..cage.positions.len()).collect(),
                topology_signature: mesh_topology_signature(&cage),
                pair: None,
            };
            selections
                .entry(asset_id.clone())
                .or_default()
                .push(item.clone());
            if semantic(&asset_id) != asset_id {
                selections
                    .entry(semantic(&asset_id).into())
                    .or_default()
                    .push(item);
            }
        }
        // Pair existing counterparts explicitly; a mirror edit must never duplicate surfaces.
        let names: Vec<_> = selections.keys().cloned().collect();
        for name in names {
            let pair = name
                .strip_suffix("_l")
                .map(|base| format!("{base}_r"))
                .or_else(|| name.strip_suffix("_r").map(|base| format!("{base}_l")));
            if let Some(pair) = pair
                && selections.contains_key(&pair)
            {
                for item in selections.get_mut(&name).unwrap() {
                    item.pair = Some(pair.clone());
                }
            }
        }
        // S101 torso subregions constrain depth edits without altering its frontal dimensions.
        if let Some(torso) = selections.get("body.torso").cloned() {
            let all = cages(&source)?;
            for (name, lo, hi) in [
                ("body.ribcage", 1.95, 2.50),
                ("body.waist", 1.65, 1.95),
                ("body.pelvis", 1.25, 1.65),
            ] {
                let items = torso
                    .iter()
                    .map(|s| {
                        let mut s = s.clone();
                        s.vertices.retain(|i| {
                            let y = all[&s.asset_id].positions[*i][1];
                            y >= lo && y <= hi
                        });
                        s
                    })
                    .collect();
                selections.insert(name.into(), items);
            }
        }
        let current = CharacterSnapshot {
            source_fingerprint: mesh_source_fingerprint(&source),
            source,
            selections,
            attachments: vec![],
            parameters: None,
            assumptions: vec!["Unobserved depth and rear surfaces are design assumptions.".into()],
        };
        Ok(Self {
            schema_version: 1,
            id,
            revision: 0,
            current,
            history: vec![],
            redo: vec![],
            candidates: BTreeMap::new(),
            next_candidate: 0,
        })
    }

    pub fn check_revision(&self, expected: u64) -> Result<(), CharacterError> {
        if expected != self.revision {
            return Err(CharacterError::Stale {
                expected,
                current: self.revision,
            });
        }
        if mesh_source_fingerprint(&self.current.source) != self.current.source_fingerprint {
            return Err(CharacterError::SourceChanged);
        }
        Ok(())
    }

    /// Explicit selections let imported assets use semantic tools without naming or proportion guesses.
    pub fn register_selection(
        &mut self,
        expected: u64,
        name: String,
        asset: String,
        vertices: Option<Vec<usize>>,
        pair: Option<String>,
    ) -> Result<(), CharacterError> {
        self.check_revision(expected)?;
        if name.trim().is_empty() {
            return Err(CharacterError::Invalid("Selection name is empty".into()));
        }
        let all = cages(&self.current.source)?;
        let cage = all
            .get(&asset)
            .ok_or_else(|| CharacterError::Missing(asset.clone()))?;
        let vertices = vertices.unwrap_or_else(|| (0..cage.positions.len()).collect());
        let unique: std::collections::BTreeSet<_> = vertices.iter().collect();
        if vertices.is_empty()
            || unique.len() != vertices.len()
            || vertices.iter().any(|i| *i >= cage.positions.len())
        {
            return Err(CharacterError::Invalid(
                "Selection vertices must be nonempty, distinct and in range".into(),
            ));
        }
        if pair
            .as_ref()
            .is_some_and(|p| p == &name || !self.current.selections.contains_key(p))
        {
            return Err(CharacterError::Invalid(
                "Mirror counterpart must reference a different existing selection".into(),
            ));
        }
        let mut next = self.current.clone();
        next.selections.insert(
            name,
            vec![Selection {
                asset_id: asset,
                vertices,
                topology_signature: mesh_topology_signature(cage),
                pair,
            }],
        );
        self.history.push(self.current.clone());
        self.current = next;
        self.revision += 1;
        self.redo.clear();
        self.candidates.clear();
        Ok(())
    }

    /// A committed snapshot replaces geometry and dependent metadata atomically.
    pub fn commit(&mut self, expected: u64, candidate: &str) -> Result<(), CharacterError> {
        self.check_revision(expected)?;
        let next = self
            .candidates
            .get(candidate)
            .ok_or_else(|| CharacterError::Missing(candidate.into()))?;
        if next.base_revision != self.revision {
            return Err(CharacterError::Stale {
                expected: next.base_revision,
                current: self.revision,
            });
        }
        if mesh_source_fingerprint(&next.snapshot.source) != next.snapshot.source_fingerprint {
            return Err(CharacterError::SourceChanged);
        }
        let assets = cages(&next.snapshot.source)?;
        let accepted = cages(&self.current.source)?;
        // Persisted candidates receive the same lock and topology checks before any state is replaced.
        for id in &next.locked_assets {
            if serde_json::to_value(accepted.get(id))? != serde_json::to_value(assets.get(id))? {
                return Err(CharacterError::Locked(id.clone()));
            }
        }
        for (id, cage) in &assets {
            if serde_json::to_value(accepted.get(id))? == serde_json::to_value(Some(cage))? {
                continue;
            }
            let report = crate::api::mesh_reference::validate_mesh_topology(
                cage,
                &crate::api::mesh_reference::MeshProposalValidationOptions {
                    allow_boundary: true,
                    allow_multiple_components: true,
                    ..Default::default()
                },
            );
            if !report.valid {
                return Err(CharacterError::Geometry(format!(
                    "Candidate asset {id} failed topology validation"
                )));
            }
        }
        for selections in next.snapshot.selections.values() {
            for selection in selections {
                let cage = assets
                    .get(&selection.asset_id)
                    .ok_or_else(|| CharacterError::Rebind(selection.asset_id.clone()))?;
                if mesh_topology_signature(cage) != selection.topology_signature
                    || selection
                        .vertices
                        .iter()
                        .any(|i| *i >= cage.positions.len())
                {
                    return Err(CharacterError::Rebind(selection.asset_id.clone()));
                }
            }
        }
        self.history.push(self.current.clone());
        self.current = next.snapshot.clone();
        self.revision += 1;
        self.redo.clear();
        self.candidates.clear();
        Ok(())
    }

    pub fn undo(&mut self, expected: u64) -> Result<(), CharacterError> {
        self.check_revision(expected)?;
        let previous = self
            .history
            .pop()
            .ok_or_else(|| CharacterError::Invalid("Nothing to undo".into()))?;
        self.redo.push(self.current.clone());
        self.current = previous;
        self.revision += 1;
        self.candidates.clear();
        Ok(())
    }
    pub fn redo(&mut self, expected: u64) -> Result<(), CharacterError> {
        self.check_revision(expected)?;
        let next = self
            .redo
            .pop()
            .ok_or_else(|| CharacterError::Invalid("Nothing to redo".into()))?;
        self.history.push(self.current.clone());
        self.current = next;
        self.revision += 1;
        self.candidates.clear();
        Ok(())
    }
}
