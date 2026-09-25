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
    /// Each file's sample type, in the same order as `files`; `None` where
    /// the metadata gives it none.
    pub kinds: Vec<Option<Arc<str>>>,
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
                        kinds: Vec::new(),
                        slots: Vec::new(),
                    });
                }
            },
            None => pairs.push(Pair {
                specimen: None,
                files: vec![index],
                kinds: Vec::new(),
                slots: Vec::new(),
            }),
        }
    }

    for pair in &mut pairs {
        // Stable, so files of the same type keep the folder's order.
        pair.files
            .sort_by_key(|i| rank(&pairing.display_order, types[*i].as_ref()));
        pair.kinds = pair.files.iter().map(|i| types[*i].clone()).collect();
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

/// The file to select after stepping `steps` specimens on from the one holding
/// `file`: the first file the specimen arrived at shows, left plot first.
/// Wraps at either end. `None` only if there is nothing to step through.
///
/// The first *shown* file rather than the left one: a specimen with no FMO
/// keeps its left plot empty, and landing on that empty side selected nothing,
/// so the next press stepped from the same place to the same specimen and the
/// buttons could never get past it.
///
/// A file no pair holds steps from the first specimen.
pub fn step_from(pairs: &[Pair], file: usize, steps: isize) -> Option<usize> {
    if pairs.is_empty() {
        return None;
    }
    let at = pair_of(pairs, file).unwrap_or(0) as isize;
    let count = pairs.len() as isize;
    let next = &pairs[(at + steps).rem_euclid(count) as usize];
    next.slots
        .iter()
        .flatten()
        .next()
        .or_else(|| next.files.first())
        .copied()
}

// ── what the editor shows, and what the gallery shows ─────────────────────

/// Which of a specimen's other files the editor's second plot shows,
/// described so it carries from one specimen to the next: the file's sample
/// type, and which file of that type (0 for the first).
///
/// By type rather than by file, because the point of remembering it is the
/// next specimen, whose files are different ones. Someone comparing each
/// FMO with its unstained control wants the unstained control of every
/// specimen they step to, not the full stain the second plot would
/// otherwise fall back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecondChoice {
    pub kind: Option<Arc<str>>,
    pub nth: usize,
}

impl Pair {
    /// `file`'s type and its place among the specimen's files of that type.
    pub fn choice_of(&self, file: usize) -> Option<SecondChoice> {
        let at = self.files.iter().position(|f| *f == file)?;
        let kind = self.kinds.get(at).cloned().flatten();
        let nth = self.files[..at]
            .iter()
            .zip(&self.kinds)
            .filter(|(_, k)| **k == kind)
            .count();
        Some(SecondChoice { kind, nth })
    }

    /// The file `choice` names in this specimen, or the nearest to it: the
    /// same type's first file when this specimen has fewer of that type.
    fn file_for(&self, choice: &SecondChoice, among: &[usize]) -> Option<usize> {
        let of_kind: Vec<usize> = self
            .files
            .iter()
            .zip(&self.kinds)
            .filter(|(f, k)| **k == choice.kind && among.contains(f))
            .map(|(f, _)| *f)
            .collect();
        of_kind.get(choice.nth).or_else(|| of_kind.first()).copied()
    }
}

/// What the editor's two plots show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The first plot: the specimen's left-hand file - its FMO - or nothing,
    /// if it has none. It keeps that side whatever is selected, so the same
    /// side of the screen shows the same kind of file for every specimen.
    pub left: Option<usize>,
    /// The second plot.
    pub right: Option<usize>,
    /// Every file the second plot could show: the specimen's files other
    /// than the first plot's. More than one is what the selector above the
    /// second plot is for.
    pub choices: Vec<usize>,
}

/// What the editor shows for the specimen holding `selected`.
///
/// The selected file is always shown: in the first plot if it is the
/// specimen's left-hand file, in the second otherwise. That is the whole of
/// B-PAIR-1's fix for the editor - a specimen's second file of one type (a
/// re-acquired tube), a third type in the plot order, or a third file of an
/// untyped specimen used to be listed but never drawn, because the plots
/// showed the specimen's slots whatever was picked.
///
/// When the first plot's file is the one selected - which is where Previous
/// and Next land - the second plot shows `remembered`, the choice last made
/// for it, as found in this specimen; failing that, the specimen's own
/// right-hand file, then whichever other file it has.
///
/// A file no pair holds is shown on its own, in the first plot.
pub fn shown(pairs: &[Pair], selected: usize, remembered: Option<&SecondChoice>) -> Shown {
    let Some(pair) = pair_of(pairs, selected).map(|at| &pairs[at]) else {
        return Shown {
            left: Some(selected),
            right: None,
            choices: Vec::new(),
        };
    };
    let left = pair.left();
    let choices: Vec<usize> = pair
        .files
        .iter()
        .copied()
        .filter(|f| Some(*f) != left)
        .collect();
    let right = if Some(selected) != left {
        Some(selected)
    } else {
        remembered
            .and_then(|choice| pair.file_for(choice, &choices))
            .or_else(|| pair.right())
            .or_else(|| choices.first().copied())
    };
    Shown {
        left,
        right,
        choices,
    }
}

/// The file to select after stepping `steps` specimens from the one holding
/// `file`, with the second plot's remembered choice: the specimen's left-hand
/// file, so the second plot is free to show the remembered one; or, for a
/// specimen with no left-hand file, the remembered one itself, which is then
/// what the second plot shows. See [`step_from`] for the stepping.
pub fn landing(
    pairs: &[Pair],
    file: usize,
    steps: isize,
    remembered: Option<&SecondChoice>,
) -> Option<usize> {
    let first = step_from(pairs, file, steps)?;
    let pair = &pairs[pair_of(pairs, first)?];
    if pair.left().is_some() {
        return Some(first);
    }
    Some(
        remembered
            .and_then(|choice| pair.file_for(choice, &pair.files))
            .unwrap_or(first),
    )
}

/// A specimen's rows in the gallery, and in the PDF made from it: two plots a
/// row, the first row its left- and right-hand files as the editor shows them.
///
/// Every other file gets a place in a further row, in the column of the first
/// row's file of the same type if there is one - a re-acquired full stain
/// under the full stain - and otherwise the first column free. The gallery
/// only ever showed the first row, so those files were never drawn at all.
pub fn gallery_rows(pair: &Pair) -> Vec<Vec<Option<usize>>> {
    let first: Vec<Option<usize>> = pair.slots.iter().take(2).copied().collect();
    let width = first.len().max(1);
    let kind_of = |file: usize| -> Option<Arc<str>> {
        pair.files
            .iter()
            .position(|f| *f == file)
            .and_then(|at| pair.kinds.get(at).cloned().flatten())
    };
    let column_of = |file: usize| -> Option<usize> {
        let kind = kind_of(file)?;
        first
            .iter()
            .position(|slot| slot.is_some_and(|f| kind_of(f).as_ref() == Some(&kind)))
    };

    let mut rows = vec![first.clone()];
    for file in pair.files.iter().copied() {
        if first.contains(&Some(file)) {
            continue;
        }
        let wanted = column_of(file);
        // The last row, if it has room where this file goes; a new one if not.
        let last = rows.len() - 1;
        let fits = |row: &Vec<Option<usize>>| -> Option<usize> {
            match wanted {
                Some(column) => row[column].is_none().then_some(column),
                None => row.iter().position(Option::is_none),
            }
        };
        let (row, column) = match (last > 0).then(|| fits(&rows[last])).flatten() {
            Some(column) => (last, column),
            None => {
                rows.push(vec![None; width]);
                let column = fits(&rows[last + 1]).expect("a fresh row has room");
                (last + 1, column)
            }
        };
        rows[row][column] = Some(file);
    }
    // A first row with nothing in it - a specimen with neither of the first
    // two types, whose files are all in the rows below - says nothing. Only
    // the first can be empty: a further row is only made to hold a file.
    if rows[0].iter().all(Option::is_none) {
        rows.remove(0);
    }
    rows
}
