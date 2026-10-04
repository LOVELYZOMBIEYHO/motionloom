// =========================================
// =========================================
// src/character_authoring/rig/geodesic.rs

//! Surface paths prevent an adjacent resting arm from attracting torso skin through empty space.
use super::{math::*, *};
use std::{
    cmp::Reverse,
    collections::{BTreeSet, BinaryHeap},
};

pub(crate) fn distances(
    mesh: &RigMesh,
    skeleton: &HumanoidSkeleton,
    adjacency: &[BTreeSet<usize>],
    inactive: &BTreeSet<usize>,
) -> Vec<Vec<f32>> {
    let mut fields = vec![vec![f32::INFINITY; skeleton.joints.len()]; mesh.positions.len()];
    let mut visited = vec![false; mesh.positions.len()];
    let definitions = &humanoid_rig_standard().references[0].joints;
    for start in 0..mesh.positions.len() {
        if visited[start] {
            continue;
        }
        let mut component = vec![start];
        visited[start] = true;
        let mut cursor = 0;
        while cursor < component.len() {
            for &n in &adjacency[component[cursor]] {
                if !visited[n] {
                    visited[n] = true;
                    component.push(n);
                }
            }
            cursor += 1;
        }
        if component.iter().all(|v| inactive.contains(v)) {
            continue;
        }
        for (joint, bone) in skeleton.joints.iter().enumerate() {
            if bone.endpoint || bone.id == "root" {
                continue;
            }
            let child = if bone.core {
                super::build::child_id(&bone.id, definitions)
                    .and_then(|id| skeleton.joints.iter().find(|j| j.id == id))
            } else {
                skeleton
                    .joints
                    .iter()
                    .find(|j| j.parent.as_deref() == Some(&bone.id))
            };
            let end = child.map_or(bone.position, |j| j.position);
            let mut queue = BinaryHeap::new();
            // Multiple capsule samples locate both the joint and the middle of its skin surface.
            // Seed distance includes the bone's depth inside the surface, rather than pretending it is zero.
            for fraction in [0., 0.5, 1.] {
                let target = add(bone.position, scale(sub(end, bone.position), fraction));
                let mut nearest = [(usize::MAX, f32::INFINITY); 4];
                for &v in &component {
                    let d = length(sub(mesh.positions[v], target));
                    if let Some(k) = (0..4).find(|k| d < nearest[*k].1) {
                        for next in (k + 1..4).rev() {
                            nearest[next] = nearest[next - 1];
                        }
                        nearest[k] = (v, d);
                    }
                }
                for (v, d) in nearest {
                    if v != usize::MAX && d < mesh.inspection.height * 0.25 {
                        let distance = distance_segment(mesh.positions[v], bone.position, end);
                        if distance < fields[v][joint] {
                            fields[v][joint] = distance;
                            queue.push(Reverse((distance.to_bits(), v)));
                        }
                    }
                }
            }
            while let Some(Reverse((bits, v))) = queue.pop() {
                let distance = f32::from_bits(bits);
                if distance > fields[v][joint] {
                    continue;
                }
                for &n in &adjacency[v] {
                    let d = distance + length(sub(mesh.positions[v], mesh.positions[n]));
                    if d < fields[n][joint] {
                        fields[n][joint] = d;
                        queue.push(Reverse((d.to_bits(), n)));
                    }
                }
            }
        }
    }
    fields
}
