//! Grouping a folder of files into the pairs a person actually looks at.
//!
//! Two plots side by side are only useful if they show the same specimen: the
//! FMO on the left, where the line is set, and the full stain on the right,
//! where the positives are read off. Stepping through the folder one file at a
//! time puts unrelated samples next to each other and buries the comparison.
//!
//! The grouping comes from the metadata - the same sample id column the rules
//! use - so this follows whatever the dataset calls things rather than parsing
//! file names.

use crate::gate_editor::gates::gate_store::FileId;
use crate::gate_rules::rule_store::SamplePairing;
use crate::omiq::metadata::MetaDataFileMap;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::sync::Arc;

/// One specimen's files, in the order they should be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    /// The specimen these belong to, where the metadata says.
    pub specimen: Option<Arc<str>>,
    /// Indices into the file list, ordered by [`SamplePairing::display_order`].
    pub files: Vec<usize>,
    /// One slot per entry in [`SamplePairing::display_order`], holding that
    /// specimen's file of that type where it has one.
    ///
    /// A slot rather than a position, because a specimen missing its FMO must
    /// leave the left plot empty rather than have its full stain slide over to
    /// fill it. Ordering by rank alone did that, and it meant the same side of
    /// the screen showed the control for one specimen and the stain for the
    /// next.
    pub slots: Vec<Option<usize>>,
}

impl Pair {
    pub fn left(&self) -> Option<usize> {
        self.slots.first().copied().flatten()
    }
    pub fn right(&self) -> Option<usize> {
        self.slots.get(1).copied().flatten()
    }
}

/// Where a file's type sits in the display order. Anything unnamed sorts after
/// everything named, rather than jumping the queue.
fn rank(order: &[Arc<str>], sample_type: Option<&Arc<str>>) -> usize {
    match sample_type {
        Some(t) => order.iter().position(|o| o == t).unwrap_or(order.len()),
        None => order.len(),
    }
}

/// Group files into specimens, each ordered for display.
///
/// `keys` is the lookup name of each file, positionally - index `i` of the
/// result's `files` refers to `keys[i]`.
///
/// Files the metadata says nothing about are not forced together: each becomes
/// a pair of its own rather than being lumped into one nameless specimen, which
/// would put unrelated samples side by side and claim they were linked.
/// Specimens appear in the order their first file does, so stepping through
/// them follows the folder.
pub fn pair_files(
    keys: &[Arc<str>],
    names_to_id: &HashMap<Arc<str>, FileId, FxBuildHasher>,
    metadata: &MetaDataFileMap,
    pairing: &SamplePairing,
) -> Vec<Pair> {
    let mut pairs: Vec<Pair> = Vec::new();
    // Specimen name to its slot in `pairs`, so first appearance sets the order.
    let mut seen: HashMap<Arc<str>, usize> = HashMap::new();
    // Sample type per file index, kept to sort each specimen at the end.
    let mut types: Vec<Option<Arc<str>>> = vec![None; keys.len()];

    for (index, key) in keys.iter().enumerate() {
        let columns = names_to_id.get(key).and_then(|id| metadata.get(id));
        let specimen = columns.and_then(|c| c.get(&pairing.sample_id_column).cloned());
        types[index] = columns.and_then(|c| pairing.sample_type_of(c));

        match specimen {
            Some(name) => match seen.get(&name) {
                Some(at) => pairs[*at].files.push(index),
                None => {
                    seen.insert(name.clone(), pairs.len());
                    pairs.push(Pair {
                        specimen: Some(name),
                        files: vec![index],
                        slots: Vec::new(),
                    });
                }
            },
            None => pairs.push(Pair {
                specimen: None,
                files: vec![index],
                slots: Vec::new(),
            }),
        }
    }

    for pair in &mut pairs {
        // Stable, so files of the same type keep the folder's order.
        pair.files
            .sort_by_key(|i| rank(&pairing.display_order, types[*i].as_ref()));
        // One slot per named type, filled by the first file of that type.
        pair.slots = pairing
            .display_order
            .iter()
            .map(|wanted| {
                pair.files
                    .iter()
                    .copied()
                    .find(|i| types[*i].as_ref().is_some_and(|t| t == wanted))
            })
            .collect();
        // A specimen whose types the display order does not name at all would
        // otherwise show nothing. Falling back to its files in order keeps it
        // visible; the pairing controls say when this is happening, because a
        // misconfigured order is worth seeing rather than silently working
        // half way.
        if pair.slots.iter().all(Option::is_none) {
            pair.slots = pair.files.iter().map(|i| Some(*i)).collect();
        }
    }

    // Then the specimens themselves, by whichever metadata column the pairing
    // names. Stable, so without a column they stay in the folder's order, and
    // a specimen the column says nothing about sorts last rather than first.
    if pairing.sort_column.is_some() {
        let key_of = |pair: &Pair| -> Option<Arc<str>> {
            let index = *pair.files.first()?;
            let columns = names_to_id
                .get(&keys[index])
                .and_then(|id| metadata.get(id))?;
            pairing.sort_key(columns)
        };
        let keyed: Vec<Option<Arc<str>>> = pairs.iter().map(key_of).collect();
        let mut order: Vec<usize> = (0..pairs.len()).collect();
        order.sort_by(|a, b| match (&keyed[*a], &keyed[*b]) {
            (Some(x), Some(y)) => crate::gate_rules::rule_store::human_order(x, y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        pairs = order.into_iter().map(|i| pairs[i].clone()).collect();
    }
    pairs
}

/// Which pair holds this file, for keeping a selection made by file name.
pub fn pair_of(pairs: &[Pair], file: usize) -> Option<usize> {
    pairs.iter().position(|p| p.files.contains(&file))
}
