// src/world/render/transparent_batches.rs

use std::ops::Range;

/// Adjacent ordered draws may share attachments only while their optical
/// underlay stays immutable. A new slab snapshot always begins a new pass.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TransparentBatch {
    pub draws: Range<usize>,
    pub copy_snapshot: bool,
    pub pane_has_snapshot: bool,
}

pub(super) fn plan<'a, K: Eq + 'a>(
    panes: impl IntoIterator<Item = Option<(&'a K, bool)>>,
    layer_limit: u32,
) -> Vec<TransparentBatch> {
    let mut batches = Vec::new();
    let mut current_pane = None;
    let mut start = 0;
    let mut count = 0;
    let mut layers = 0;
    let mut copy_snapshot = false;
    let mut pane_has_snapshot = false;
    for (index, pane) in panes.into_iter().enumerate() {
        count = index + 1;
        if let Some((_, solid)) = pane
            && pane != current_pane
        {
            let next_has_snapshot = solid || layers < layer_limit;
            let next_copy = next_has_snapshot && !solid;
            if index > start && (next_copy || next_has_snapshot != pane_has_snapshot) {
                batches.push(TransparentBatch {
                    draws: start..index,
                    copy_snapshot,
                    pane_has_snapshot,
                });
                start = index;
                copy_snapshot = false;
            }
            copy_snapshot |= next_copy;
            pane_has_snapshot = next_has_snapshot;
            layers += u32::from(next_copy);
            current_pane = pane;
        }
    }
    if count > start {
        batches.push(TransparentBatch {
            draws: start..count,
            copy_snapshot,
            pane_has_snapshot,
        });
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::{TransparentBatch, plan};

    #[test]
    fn keeps_alpha_order_and_same_pane_chunks_in_one_pass() {
        let a = "a";
        let b = "b";
        let panes = [
            None,
            None,
            Some((&a, false)),
            Some((&a, false)),
            None,
            Some((&b, false)),
            None,
        ];
        assert_eq!(
            plan(panes, 2),
            vec![
                TransparentBatch {
                    draws: 0..2,
                    copy_snapshot: false,
                    pane_has_snapshot: false
                },
                TransparentBatch {
                    draws: 2..5,
                    copy_snapshot: true,
                    pane_has_snapshot: true
                },
                TransparentBatch {
                    draws: 5..7,
                    copy_snapshot: true,
                    pane_has_snapshot: true
                },
            ]
        );
    }

    #[test]
    fn exhausted_layers_keep_original_fallback_without_extra_snapshots() {
        let a = "a";
        let b = "b";
        let c = "c";
        let panes = [
            Some((&a, false)),
            None,
            Some((&b, false)),
            None,
            Some((&c, false)),
            Some((&a, false)),
        ];
        assert_eq!(
            plan(panes, 1),
            vec![
                TransparentBatch {
                    draws: 0..2,
                    copy_snapshot: true,
                    pane_has_snapshot: true
                },
                TransparentBatch {
                    draws: 2..6,
                    copy_snapshot: false,
                    pane_has_snapshot: false
                },
            ]
        );
    }

    #[test]
    fn ordinary_alpha_does_not_change_snapshot_ownership() {
        let a = "a";
        assert_eq!(
            plan([Some((&a, false)), None, Some((&a, false))], 2),
            vec![TransparentBatch {
                draws: 0..3,
                copy_snapshot: true,
                pane_has_snapshot: true
            },]
        );
        assert!(plan::<&str>([], 2).is_empty());
    }
}
