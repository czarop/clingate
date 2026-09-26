//! Tests for grouping a folder into the pairs a person looks at.

#![cfg(test)]

use crate::gate_editor::plots::sample_pairs::{pair_files, pair_of};
use crate::gate_rules::rule_store::SamplePairing;
use rustc_hash::{FxBuildHasher, FxHashMap};
use std::collections::HashMap;
use std::sync::Arc;

/// Files named `f0`, `f1`, ... each with the given specimen and sample type.
/// A `None` specimen stands for a file the metadata says nothing about.
fn fixture(
    files: &[(Option<&str>, Option<&str>)],
) -> (
    Vec<Arc<str>>,
    HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    crate::omiq::metadata::MetaDataFileMap,
) {
    let mut keys = Vec::new();
    let mut names: HashMap<Arc<str>, Arc<str>, FxBuildHasher> = HashMap::default();
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);

    for (i, (specimen, kind)) in files.iter().enumerate() {
        let key: Arc<str> = Arc::from(format!("f{i}").as_str());
        keys.push(key.clone());
        let Some(specimen) = specimen else { continue };
        let id: Arc<str> = Arc::from(format!("id{i}").as_str());
        names.insert(key, id.clone());
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(*specimen));
        if let Some(kind) = kind {
            columns.insert(Arc::from("SampleType"), Arc::from(*kind));
        }
        metadata.insert(id, columns);
    }
    (keys, names, metadata)
}

#[test]
fn a_specimens_files_are_shown_together() {
    // The folder interleaves two donors; the pairs must not.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FS")),
        (Some("QC-B"), Some("FS")),
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].specimen.as_deref(), Some("QC-A"));
    assert_eq!(pairs[0].files, vec![2, 0], "FMX first, then FS");
    assert_eq!(pairs[1].files, vec![3, 1]);
}

#[test]
fn the_fmo_goes_on_the_left() {
    // The line is set on the FMO and read off the full stain, so that is the
    // order a person reads them in.
    let (keys, names, metadata) =
        fixture(&[(Some("QC-A"), Some("FS")), (Some("QC-A"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].left(), Some(1));
    assert_eq!(pairs[0].right(), Some(0));
}

#[test]
fn the_display_order_is_configurable() {
    let pairing = SamplePairing {
        display_order: vec![Arc::from("Unstained"), Arc::from("FS")],
        ..SamplePairing::default()
    };
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("Unstained")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &pairing);

    assert_eq!(pairs[0].files, vec![1, 0]);
}

#[test]
fn specimens_keep_the_order_their_first_file_appears_in() {
    // Stepping through pairs should still feel like stepping through the folder.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-B"), Some("FMX")),
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].specimen.as_deref(), Some("QC-B"));
    assert_eq!(pairs[1].specimen.as_deref(), Some("QC-A"));
}

#[test]
fn a_specimen_with_only_one_file_shows_it_on_its_own_side() {
    // The FMO was never run, so the left slot stays empty and the full stain
    // keeps the right. It used to slide over to fill the gap, which meant the
    // same side of the screen showed a control for one specimen and a stain for
    // the next - and comparing those two is exactly the mistake the paired
    // layout exists to prevent.
    let (keys, names, metadata) = fixture(&[(Some("QC-A"), Some("FS"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].left(), None);
    assert_eq!(pairs[0].right(), Some(0));
    // The file is still listed - it is shown, just not on the FMO's side.
    assert_eq!(pairs[0].files, vec![0]);
}

#[test]
fn files_with_no_metadata_are_not_lumped_together() {
    // Grouping every unlabelled file into one nameless specimen would put
    // unrelated samples side by side and imply they were linked.
    let (keys, names, metadata) = fixture(&[(None, None), (None, None)]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].files, vec![0]);
    assert_eq!(pairs[1].files, vec![1]);
}

#[test]
fn a_type_the_order_does_not_name_sorts_last() {
    // A third file in the specimen should not displace the pair being compared.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("Unstained")),
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs[0].files, vec![2, 1, 0]);
    assert_eq!(pairs[0].left(), Some(2));
    assert_eq!(pairs[0].right(), Some(1));
}

#[test]
fn a_file_selected_by_name_finds_its_pair() {
    // Picking a full stain off the list should show its specimen, not jump.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FMX")),
        (Some("QC-B"), Some("FMX")),
        (Some("QC-B"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pair_of(&pairs, 2), Some(1));
    assert_eq!(pair_of(&pairs, 0), Some(0));
    assert_eq!(pair_of(&pairs, 99), None);
}

#[test]
fn a_file_with_a_specimen_but_no_type_still_pairs() {
    // A missing SampleType is common in older exports; the specimen is what
    // groups them, and the type only decides which side.
    let (keys, names, metadata) = fixture(&[(Some("QC-A"), None), (Some("QC-A"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());

    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].files, vec![1, 0], "the typed file takes the left");
}

#[test]
fn the_defaults_match_an_omiq_metadata_export() {
    // Real exports carry `SampleID` and `SampleType`, and three files per
    // specimen: unstained, FMO, full stain. A default that misses the column -
    // "Sample ID", with a space - fails silently: every file lands in its own
    // specimen, no partner is ever found, and the only symptom is one plot
    // where there should be two.
    let pairing = SamplePairing::default();
    assert_eq!(&*pairing.sample_id_column, "SampleID");
    assert_eq!(&*pairing.sample_type_column, "SampleType");

    let (keys, names, metadata) = fixture(&[
        (Some("QC5_N/A"), Some("U")),
        (Some("QC5_N/A"), Some("FMX")),
        (Some("QC5_N/A"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &pairing);

    assert_eq!(pairs.len(), 1, "one specimen, not three");
    assert_eq!(pairs[0].left(), Some(1), "the FMO is shown first");
    assert_eq!(pairs[0].right(), Some(2), "the full stain beside it");
}

// ─── the slots each plot is drawn into ───────────────────────────────────────

#[test]
fn the_fmo_holds_its_side_even_when_the_specimen_has_none() {
    // The full stain must not slide over to fill the empty slot: the same side
    // of the screen showing the control for one specimen and the stain for the
    // next is how a person comes to compare the wrong two plots.
    let keys: Vec<Arc<str>> = vec![Arc::from("only_fs.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    names.insert(
        Arc::from("only_fs.fcs") as Arc<str>,
        Arc::from("f1") as Arc<str>,
    );
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
    columns.insert(Arc::from("SampleID"), Arc::from("A"));
    columns.insert(Arc::from("SampleType"), Arc::from("FS"));
    metadata.insert(Arc::from("f1") as Arc<str>, columns);

    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].left(),
        None,
        "no FMO, so the left slot stays empty"
    );
    assert_eq!(pairs[0].right(), Some(0), "and the stain keeps the right");
}

#[test]
fn the_fmo_is_on_the_left_whichever_order_the_folder_holds_them() {
    // Ranking alone let two files of unnamed types keep the folder's order,
    // which is how the two plots came to swap sides between specimens.
    for (first, second) in [("FMX", "FS"), ("FS", "FMX")] {
        let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs")];
        let mut names = HashMap::with_hasher(FxBuildHasher);
        names.insert(Arc::from("a.fcs") as Arc<str>, Arc::from("f1") as Arc<str>);
        names.insert(Arc::from("b.fcs") as Arc<str>, Arc::from("f2") as Arc<str>);
        let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
        for (id, kind) in [("f1", first), ("f2", second)] {
            let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
            columns.insert(Arc::from("SampleID"), Arc::from("A"));
            columns.insert(Arc::from("SampleType"), Arc::from(kind));
            metadata.insert(Arc::from(id) as Arc<str>, columns);
        }
        let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
        let left = pairs[0].left().unwrap();
        let right = pairs[0].right().unwrap();
        assert_ne!(left, right);
        // Whichever file it is, the left one is the FMO.
        let kind_of = |i: usize| {
            let id: Arc<str> = Arc::from(if i == 0 { "f1" } else { "f2" });
            metadata
                .get(&id)
                .unwrap()
                .get(&Arc::from("SampleType"))
                .unwrap()
                .clone()
        };
        assert_eq!(&*kind_of(left), "FMX", "folder order {first} then {second}");
        assert_eq!(&*kind_of(right), "FS");
    }
}

#[test]
fn a_specimen_the_display_order_does_not_name_is_still_shown() {
    // A misconfigured order must not blank the screen. It falls back to the
    // files in order; the pairing controls are where the mismatch is reported.
    let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    names.insert(Arc::from("a.fcs") as Arc<str>, Arc::from("f1") as Arc<str>);
    names.insert(Arc::from("b.fcs") as Arc<str>, Arc::from("f2") as Arc<str>);
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for id in ["f1", "f2"] {
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from("A"));
        columns.insert(Arc::from("SampleType"), Arc::from("something else"));
        metadata.insert(Arc::from(id) as Arc<str>, columns);
    }
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs[0].left(), Some(0));
    assert_eq!(pairs[0].right(), Some(1));
}

#[test]
fn specimens_follow_the_sort_column() {
    let keys: Vec<Arc<str>> = vec![Arc::from("a.fcs"), Arc::from("b.fcs"), Arc::from("c.fcs")];
    let mut names = HashMap::with_hasher(FxBuildHasher);
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for (key, id, day) in [
        ("a.fcs", "f1", "D85"),
        ("b.fcs", "f2", "D4"),
        ("c.fcs", "f3", "D29"),
    ] {
        names.insert(Arc::from(key) as Arc<str>, Arc::from(id) as Arc<str>);
        let mut columns: FxHashMap<Arc<str>, Arc<str>> = FxHashMap::default();
        columns.insert(Arc::from("SampleID"), Arc::from(day));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        columns.insert(Arc::from("Day"), Arc::from(day));
        metadata.insert(Arc::from(id) as Arc<str>, columns);
    }
    let pairing = SamplePairing {
        sort_column: Some(Arc::from("Day")),
        ..SamplePairing::default()
    };
    let pairs = pair_files(&keys, &names, &metadata, &pairing);
    let order: Vec<&str> = pairs
        .iter()
        .map(|p| p.specimen.as_deref().unwrap())
        .collect();
    assert_eq!(order, ["D4", "D29", "D85"], "not the folder's order");
}

// ── every file reaches the screen (was B-PAIR-1) ───────────────────────────

use crate::gate_editor::plots::sample_pairs::{Pair, SecondChoice, gallery_rows, landing, shown};

/// Whether the editor draws `file` when it is the one selected.
fn drawn_when_selected(pairs: &[Pair], file: usize) -> bool {
    let on = shown(pairs, file, None);
    on.left == Some(file) || on.right == Some(file)
}

/// How many times the gallery draws `file`, across every specimen's rows.
fn times_in_gallery(pairs: &[Pair], file: usize) -> usize {
    pairs
        .iter()
        .flat_map(gallery_rows)
        .flatten()
        .filter(|slot| *slot == Some(file))
        .count()
}

/// A tube acquired twice - a re-run after a clog - is two full stains for one
/// specimen. The list offered both; choosing the re-run showed the first run
/// beside the FMO, and the gallery and its PDF never drew it at all.
#[test]
fn a_second_file_of_the_same_type_is_shown_somewhere() {
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("FMX")),
        (Some("QC-A"), Some("FS")),
        (Some("QC-A"), Some("FS")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    for file in 0..keys.len() {
        assert!(drawn_when_selected(&pairs, file), "f{file}: {pairs:?}");
        assert_eq!(times_in_gallery(&pairs, file), 1, "f{file}: {pairs:?}");
    }
    // The re-run goes under the full stain, in a row of its own.
    assert_eq!(
        gallery_rows(&pairs[0]),
        vec![vec![Some(0), Some(1)], vec![None, Some(2)]]
    );
}

#[test]
fn every_file_of_an_untyped_specimen_is_shown_somewhere() {
    // With no type the display order names, a specimen falls back to its
    // files in order - all of them in its slots, of which two are a row.
    let (keys, names, metadata) = fixture(&[
        (Some("QC-A"), Some("other")),
        (Some("QC-A"), Some("other")),
        (Some("QC-A"), Some("other")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    for file in 0..keys.len() {
        assert!(drawn_when_selected(&pairs, file), "f{file}: {pairs:?}");
        assert_eq!(times_in_gallery(&pairs, file), 1, "f{file}: {pairs:?}");
    }
}

#[test]
fn a_third_type_in_the_plot_order_is_shown() {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FMX")),
        (Some("A"), Some("FS")),
        (Some("A"), Some("US")),
    ]);
    let pairing = SamplePairing {
        display_order: vec![Arc::from("FMX"), Arc::from("FS"), Arc::from("US")],
        ..SamplePairing::default()
    };
    let pairs = pair_files(&keys, &names, &metadata, &pairing);
    assert!(drawn_when_selected(&pairs, 2));
    assert_eq!(times_in_gallery(&pairs, 2), 1);
    assert_eq!(
        shown(&pairs, 0, None).choices,
        vec![1, 2],
        "the selector offers both"
    );
}

#[test]
fn every_file_of_any_folder_is_shown_once_in_the_gallery_and_when_selected() {
    use rand::prelude::*;
    const KINDS: [Option<&str>; 5] = [Some("FMX"), Some("FS"), Some("US"), Some("other"), None];
    for seed in 0..400 {
        let mut rng = StdRng::seed_from_u64(seed);
        let specimens = ["A", "B", "C"];
        let folder: Vec<(Option<&str>, Option<&str>)> = (0..rng.random_range(1..14))
            .map(|_| {
                let specimen = rng
                    .random_bool(0.85)
                    .then(|| specimens[rng.random_range(0..specimens.len())]);
                (specimen, KINDS[rng.random_range(0..KINDS.len())])
            })
            .collect();
        let (keys, names, metadata) = fixture(&folder);
        let pairing = if rng.random_bool(0.5) {
            SamplePairing::default()
        } else {
            SamplePairing {
                display_order: vec![Arc::from("FMX"), Arc::from("FS"), Arc::from("US")],
                ..SamplePairing::default()
            }
        };
        let pairs = pair_files(&keys, &names, &metadata, &pairing);
        for file in 0..keys.len() {
            assert!(
                drawn_when_selected(&pairs, file),
                "seed {seed}, f{file}: {pairs:?}"
            );
            assert_eq!(
                times_in_gallery(&pairs, file),
                1,
                "seed {seed}, f{file}: {pairs:?}"
            );
        }
        // No row is empty, and every row is as wide as its specimen's first.
        for pair in &pairs {
            let rows = gallery_rows(pair);
            assert!(
                rows.iter().all(|r| r.iter().any(Option::is_some)),
                "seed {seed}"
            );
            assert!(rows.iter().all(|r| r.len() == rows[0].len()), "seed {seed}");
        }
    }
}

// ── the second plot's selector, remembered from specimen to specimen ─────

/// Two donors with an FMO, a full stain and an unstained control each; the
/// second also has a re-acquired full stain.
fn three_each() -> Vec<Pair> {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FMX")), // 0
        (Some("A"), Some("FS")),  // 1
        (Some("A"), Some("US")),  // 2
        (Some("B"), Some("FMX")), // 3
        (Some("B"), Some("FS")),  // 4
        (Some("B"), Some("US")),  // 5
        (Some("B"), Some("FS")),  // 6
    ]);
    pair_files(&keys, &names, &metadata, &SamplePairing::default())
}

#[test]
fn the_fmo_keeps_the_first_plot_and_the_selected_file_takes_the_second() {
    let pairs = three_each();
    assert_eq!(
        shown(&pairs, 2, None),
        crate::gate_editor::plots::sample_pairs::Shown {
            left: Some(0),
            right: Some(2),
            choices: vec![1, 2],
        }
    );
    // Selecting the FMO itself shows the usual pair.
    assert_eq!(shown(&pairs, 0, None).right, Some(1));
}

#[test]
fn the_second_plot_s_choice_carries_to_the_next_specimen() {
    let pairs = three_each();
    // Unstained chosen on A; Next lands on B's FMO, and the second plot
    // shows B's unstained, not its full stain.
    let choice = pairs[0].choice_of(2).unwrap();
    assert_eq!(
        choice,
        SecondChoice {
            kind: Some(Arc::from("US")),
            nth: 0
        }
    );
    let at = landing(&pairs, 2, 1, Some(&choice)).unwrap();
    assert_eq!(at, 3, "Next lands on B's FMO");
    assert_eq!(shown(&pairs, at, Some(&choice)).right, Some(5));
}

#[test]
fn a_choice_the_next_specimen_cannot_match_falls_back_to_its_nearest() {
    let pairs = three_each();
    // B's re-acquired full stain is its second FS; A has only one.
    let rerun = pairs[1].choice_of(6).unwrap();
    assert_eq!(rerun.nth, 1);
    assert_eq!(
        shown(&pairs, 0, Some(&rerun)).right,
        Some(1),
        "A's only full stain"
    );
    // And a type A does not have at all falls back to its full stain.
    let unknown = SecondChoice {
        kind: Some(Arc::from("CD45")),
        nth: 0,
    };
    assert_eq!(shown(&pairs, 0, Some(&unknown)).right, Some(1));
    // Coming back to B, the re-run is found again.
    assert_eq!(shown(&pairs, 3, Some(&rerun)).right, Some(6));
}

#[test]
fn a_specimen_with_no_fmo_lands_on_the_remembered_file() {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FMX")), // 0
        (Some("A"), Some("US")),  // 1
        (Some("B"), Some("FS")),  // 2
        (Some("B"), Some("US")),  // 3
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    let choice = pairs[0].choice_of(1).unwrap();
    let at = landing(&pairs, 0, 1, Some(&choice)).unwrap();
    assert_eq!(at, 3, "B's unstained, since it has no FMO to land on");
    let on = shown(&pairs, at, Some(&choice));
    assert_eq!((on.left, on.right), (None, Some(3)));
    // Without a choice it lands where it always has.
    assert_eq!(landing(&pairs, 0, 1, None), Some(2));
}

#[test]
fn a_file_no_pair_holds_is_shown_on_its_own() {
    assert_eq!(
        shown(&[], 4, None),
        crate::gate_editor::plots::sample_pairs::Shown {
            left: Some(4),
            right: None,
            choices: Vec::new(),
        }
    );
}

// ── stepping through specimens: the editor's Previous and Next ───────────

use crate::gate_editor::plots::sample_pairs::step_from;

#[test]
fn next_lands_on_the_next_specimens_fmo_and_wraps() {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FS")),
        (Some("A"), Some("FMX")),
        (Some("B"), Some("FS")),
        (Some("B"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    // From either of A's files, Next is B's FMX; from B, it wraps to A's.
    assert_eq!(step_from(&pairs, 0, 1), Some(3));
    assert_eq!(step_from(&pairs, 1, 1), Some(3));
    assert_eq!(step_from(&pairs, 2, 1), Some(1));
    // Previous wraps the other way, and a whole lap comes back.
    assert_eq!(step_from(&pairs, 0, -1), Some(3));
    assert_eq!(step_from(&pairs, 0, 2), Some(1));
}

#[test]
fn a_file_no_pair_holds_steps_from_the_first_specimen() {
    let (keys, names, metadata) = fixture(&[(Some("A"), Some("FMX")), (Some("B"), Some("FMX"))]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(step_from(&pairs, 99, 1), Some(1));
    assert_eq!(step_from(&[], 0, 1), None, "nothing to step through");
}

/// Was B-NAV-1. B has only its full stain, which keeps the right-hand side
/// (the FMO's side stays empty - see `the_fmo_holds_its_side_even_...`). Next
/// used to land on a specimen's left file, find none, and select nothing:
/// pressing it again computed the same step from the same place, so the
/// buttons could never get past B.
#[test]
fn next_steps_onto_a_specimen_with_no_fmo() {
    let (keys, names, metadata) = fixture(&[
        (Some("A"), Some("FMX")),
        (Some("A"), Some("FS")),
        (Some("B"), Some("FS")),
        (Some("C"), Some("FMX")),
    ]);
    let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
    assert_eq!(pairs[1].left(), None, "B has no FMO, the premise");
    assert_eq!(step_from(&pairs, 0, 1), Some(2), "Next from A shows B");

    assert_eq!(step_from(&pairs, 2, 1), Some(3), "and Next from B moves on");
    assert_eq!(step_from(&pairs, 3, -1), Some(2), "Previous reaches B too");
}

#[test]
fn next_and_previous_visit_every_specimen_of_any_folder() {
    // Random folders of specimens with and without an FMO, a full stain, or
    // any file the metadata knows: pressing Next as many times as there are
    // specimens visits every one, and so does Previous.
    use rand::prelude::*;
    const KINDS: [Option<&str>; 4] = [Some("FMX"), Some("FS"), Some("other"), None];
    for seed in 0..300 {
        let mut rng = StdRng::seed_from_u64(seed);
        let specimens = ["A", "B", "C", "D", "E"];
        let folder: Vec<(Option<&str>, Option<&str>)> = (0..rng.random_range(1..12))
            .map(|_| {
                let specimen = rng
                    .random_bool(0.85)
                    .then(|| specimens[rng.random_range(0..specimens.len())]);
                (specimen, KINDS[rng.random_range(0..KINDS.len())])
            })
            .collect();
        let (keys, names, metadata) = fixture(&folder);
        let pairs = pair_files(&keys, &names, &metadata, &SamplePairing::default());
        for steps in [1isize, -1] {
            let mut at = 0;
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..pairs.len() {
                at = step_from(&pairs, at, steps).expect("there is always a file to land on");
                seen.insert(pair_of(&pairs, at));
            }
            assert_eq!(
                seen.len(),
                pairs.len(),
                "seed {seed}, step {steps}: {folder:?}"
            );
        }
    }
}
