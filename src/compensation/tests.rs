//! Tests for compensation: reading matrices, matching them to files,
//! compensating, and grouping files by matrix.

use super::groups::{Compensation, Saved, SavedSource, Source};
use super::{Spillover, compensate, compensate_fcs};
use crate::file_load::FcsSampleStub;
use crate::file_load_tests::{scratch, write_fcs_rows, write_fcs_with};
use polars::prelude::*;
use rand::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn matrix(channels: &[&str], values: &[f64]) -> Spillover {
    Spillover::new(
        channels.iter().map(|&c| Arc::from(c)).collect(),
        values.to_vec(),
    )
    .unwrap()
}

/// A three-channel matrix that is not symmetric, so a matrix applied the
/// wrong way round gives the wrong answer.
fn three() -> Spillover {
    matrix(
        &["FITC-A", "PE-A", "APC-A"],
        &[1.0, 0.25, 0.02, 0.05, 1.0, 0.10, 0.0, 0.30, 1.0],
    )
}

fn close(a: f32, b: f64) -> bool {
    (f64::from(a) - b).abs() <= 1e-3 * b.abs().max(1.0)
}

// ─── Reading an Omiq CSV ─────────────────────────────────────────────────────

/// Omiq's layout: names across the top, then the rows in the same order with
/// no names, in percent. Read as fractions, the right way round.
#[test]
fn an_omiq_csv_is_read_as_fractions() {
    let csv = "FITC-A,PE-A,APC-A\n100,25,2\n5,100,10\n0,30,100\n";
    let read = Spillover::from_omiq_csv(csv).unwrap();
    assert!(read.same_as(&three()));
    assert_eq!(read.value(0, 1), 0.25, "row FITC, column PE");
    assert_eq!(read.value(1, 0), 0.05);
}

/// What a spreadsheet adds on the way - a byte-order mark, quotes, spaces,
/// Windows line endings, blank lines at the end - changes nothing.
#[test]
fn an_omiq_csv_survives_a_spreadsheet() {
    let csv = "\u{feff}\"FITC-A\", \"PE-A\" ,APC-A\r\n100, 25,2\r\n5,100,10\r\n0,30,100\r\n\r\n";
    assert!(Spillover::from_omiq_csv(csv).unwrap().same_as(&three()));
}

/// Fractions, with a diagonal of 1, are read as they are.
#[test]
fn a_csv_in_fractions_is_read_too() {
    let csv = "FITC-A,PE-A,APC-A\n1,0.25,0.02\n0.05,1,0.1\n0,0.3,1\n";
    assert!(Spillover::from_omiq_csv(csv).unwrap().same_as(&three()));
}

/// Channels named down the first column as well are read, and must be in
/// the order the top row gives them.
#[test]
fn a_csv_with_row_names_is_read_if_they_agree() {
    let csv = ",FITC-A,PE-A,APC-A\nFITC-A,100,25,2\nPE-A,5,100,10\nAPC-A,0,30,100\n";
    assert!(Spillover::from_omiq_csv(csv).unwrap().same_as(&three()));
    let swapped = ",FITC-A,PE-A,APC-A\nPE-A,100,25,2\nFITC-A,5,100,10\nAPC-A,0,30,100\n";
    let refused = Spillover::from_omiq_csv(swapped).unwrap_err().to_string();
    assert!(refused.contains("row 1 is named PE-A"), "{refused}");
}

/// Anything that is not a spillover matrix is refused, saying why.
#[test]
fn a_csv_that_is_not_a_spillover_matrix_is_refused() {
    let cases = [
        ("", "empty"),
        ("A,B\n100,5\n", "2 channels, and there are 1 rows"),
        ("A,B\n100,5\n5,100,3\n", "the row for B has 3 values"),
        ("A,B\n100,x\n5,100\n", "\"x\", is not a number"),
        ("A,B\n100,5\n5,90\n", "B's is 90"),
        ("A,A\n100,5\n5,100\n", "named twice"),
        ("A,\n100,5\n5,100\n", "no name"),
    ];
    for (csv, why) in cases {
        let refused = Spillover::from_omiq_csv(csv).unwrap_err().to_string();
        assert!(refused.contains(why), "{csv:?}: {refused}");
    }
}

// ─── A file's own matrix ─────────────────────────────────────────────────────

fn file_with(dir: &Path, name: &str, keywords: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(name);
    write_fcs_with(
        &path,
        4,
        &[
            ("FITC-A", Some("CD3")),
            ("PE-A", Some("CD4")),
            ("APC-A", None),
        ],
        keywords,
    );
    path
}

const SPILLOVER: &str = "3,FITC-A,PE-A,APC-A,1,0.25,0.02,0.05,1,0.1,0,0.3,1";

fn own(path: &Path) -> anyhow::Result<Option<Spillover>> {
    Spillover::from_keywords(&FcsSampleStub::open(&path.to_string_lossy())?.metadata)
}

/// `$SPILLOVER`, and BD's `$SPILL`, are read the same way round as a CSV;
/// a file with neither has no matrix.
#[test]
fn a_files_own_matrix_is_read_from_its_keywords() {
    let dir = scratch("own-matrix");
    for keyword in ["$SPILLOVER", "SPILL"] {
        let path = file_with(
            &dir,
            &format!("{}.fcs", keyword.trim_start_matches('$')),
            &[(keyword, SPILLOVER)],
        );
        assert!(own(&path).unwrap().unwrap().same_as(&three()), "{keyword}");
    }
    assert_eq!(own(&file_with(&dir, "none.fcs", &[])).unwrap(), None);
}

/// A `$SPILLOVER` that is not a spillover matrix is an error, not a file
/// treated as having none.
#[test]
fn a_files_bad_matrix_is_an_error() {
    let dir = scratch("bad-own-matrix");
    let path = file_with(
        &dir,
        "bad.fcs",
        &[("$SPILLOVER", "2,FITC-A,PE-A,1,0.1,0.2,0.5")],
    );
    let refused = own(&path).unwrap_err().to_string();
    assert!(
        refused.contains("$SPILLOVER is not a spillover matrix"),
        "{refused}"
    );
}

// ─── Matching channels ───────────────────────────────────────────────────────

/// A name is found as a `$PnN`, then ignoring case, then with `-A` after it,
/// then as a marker label.
#[test]
fn a_matrix_channel_is_found_by_name_then_label() {
    let channels = [
        ("FITC-A", Some("CD3")),
        ("pe-a", Some("CD4")),
        ("APC-A", None),
        ("BV421-H", Some("CD8")),
    ];
    let by = |names: &[&str]| {
        let m = matrix(names, &identity(names.len()));
        m.resolve(&channels)
    };
    assert_eq!(by(&["FITC-A", "PE-A", "APC", "CD8"]).unwrap(), [0, 1, 2, 3]);
}

/// A name that finds nothing, or several, is an error naming it; two names
/// that find the same channel are too.
#[test]
fn a_matrix_that_does_not_fit_the_file_is_refused() {
    let channels = [("FITC-A", None), ("FITC-H", None), ("PE-A", Some("PE-A"))];
    let refused = |names: &[&str]| {
        matrix(names, &identity(names.len()))
            .resolve(&channels)
            .unwrap_err()
            .to_string()
    };
    assert!(refused(&["FITC-A", "APC-A"]).contains("the file has no channel APC-A"));
    let two = [("FITC-A", Some("CD3")), ("PE-A", Some("CD3"))];
    let ambiguous = matrix(&["CD3", "X"], &identity(2))
        .resolve(&two)
        .unwrap_err()
        .to_string();
    assert!(
        ambiguous.contains("CD3 could be any of FITC-A, PE-A"),
        "{ambiguous}"
    );
    assert!(refused(&["PE-A", "pe-a"]).contains("both channel PE-A"));
}

fn identity(n: usize) -> Vec<f64> {
    (0..n * n)
        .map(|k| if k / n == k % n { 1.0 } else { 0.0 })
        .collect()
}

// ─── Compensating ────────────────────────────────────────────────────────────

/// Events made by spilling known true values through the matrix come back
/// as those true values - with a matrix that is not symmetric, so one
/// applied the wrong way round fails. Columns the matrix does not name, and
/// the rows, are left as they were.
#[test]
fn compensating_takes_the_spillover_back_out() {
    let s = three();
    let mut rng = StdRng::seed_from_u64(5);
    let truth: Vec<[f64; 3]> = (0..2_000)
        .map(|_| {
            [
                rng.random_range(-500.0..200_000.0),
                rng.random_range(-500.0..200_000.0),
                rng.random_range(-500.0..200_000.0),
            ]
        })
        .collect();
    // o = t · S
    let observed = |c: usize| -> Vec<f32> {
        truth
            .iter()
            .map(|t| (0..3).map(|i| t[i] * s.value(i, c)).sum::<f64>() as f32)
            .collect()
    };
    let frame = df!(
        "FSC-A" => (0..2_000).map(|i| i as f32).collect::<Vec<_>>(),
        "APC-A" => observed(2),
        "FITC-A" => observed(0),
        "PE-A" => observed(1),
    )
    .unwrap();

    let out = compensate(&frame, &s, |_| None).unwrap();
    assert_eq!(out.height(), 2_000);
    assert_eq!(out.column("FSC-A").unwrap(), frame.column("FSC-A").unwrap());
    for (c, name) in ["FITC-A", "PE-A", "APC-A"].iter().enumerate() {
        let got: Vec<f32> = out
            .column(name)
            .unwrap()
            .f32()
            .unwrap()
            .into_no_null_iter()
            .collect();
        for (e, t) in truth.iter().enumerate() {
            assert!(
                close(got[e], t[c]),
                "{name} event {e}: {} vs {}",
                got[e],
                t[c]
            );
        }
    }
}

/// A matrix that cannot be inverted is an error, and nothing is changed.
#[test]
fn a_matrix_that_cannot_be_inverted_is_refused() {
    let singular = matrix(&["A", "B"], &[1.0, 1.0, 1.0, 1.0]);
    let frame = df!("A" => [1.0f32], "B" => [2.0f32]).unwrap();
    let refused = compensate(&frame, &singular, |_| None)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("cannot be inverted"), "{refused}");
}

/// An opened file compensated in place: its own matrix, found by marker
/// label where the matrix names markers.
#[test]
fn an_opened_file_is_compensated_in_place() {
    let dir = scratch("compensate-fcs");
    let s = three();
    let truth = [
        [1000.0, 50.0, 0.0],
        [0.0, 8000.0, 300.0],
        [20.0, 0.0, 40_000.0],
    ];
    let rows: Vec<Vec<f32>> = truth
        .iter()
        .map(|t| {
            (0..3)
                .map(|c| (0..3).map(|i| t[i] * s.value(i, c)).sum::<f64>() as f32)
                .collect()
        })
        .collect();
    let path = dir.join("spilt.fcs");
    write_fcs_rows(
        &path,
        &[
            ("FITC-A", Some("CD3")),
            ("PE-A", Some("CD4")),
            ("APC-A", None),
        ],
        &rows,
        &[],
    );
    let mut fcs = flow_fcs::Fcs::open(path.to_str().unwrap()).unwrap();
    let by_marker = Spillover::new(
        vec![Arc::from("CD3"), Arc::from("CD4"), Arc::from("APC-A")],
        (0..9).map(|k| s.value(k / 3, k % 3)).collect(),
    )
    .unwrap();
    compensate_fcs(&mut fcs, &by_marker).unwrap();
    for (c, name) in ["FITC-A", "PE-A", "APC-A"].iter().enumerate() {
        let got: Vec<f32> = fcs
            .data_frame
            .column(name)
            .unwrap()
            .f32()
            .unwrap()
            .into_no_null_iter()
            .collect();
        for (e, t) in truth.iter().enumerate() {
            assert!(
                close(got[e], t[c]),
                "{name} event {e}: {} vs {}",
                got[e],
                t[c]
            );
        }
    }
}

/// How far a loaded matrix is from a file's own, and whether it was written
/// the other way round.
#[test]
fn comparing_two_matrices_spots_a_transposed_one() {
    let s = three();
    let transposed = Spillover::new(
        s.channels().to_vec(),
        (0..9).map(|k| s.value(k % 3, k / 3)).collect(),
    )
    .unwrap();
    let same = s.compare(&s).unwrap();
    assert_eq!(same.largest_difference, 0.0);
    assert!(!same.transposed_fits_better);
    let swapped = s.compare(&transposed).unwrap();
    assert!(
        (swapped.largest_difference - 20.0).abs() < 1e-9,
        "{swapped:?}"
    );
    assert!(swapped.transposed_fits_better);
    assert_eq!(s.compare(&matrix(&["X", "Y"], &identity(2))), None);
}

// ─── Groups ──────────────────────────────────────────────────────────────────

fn other() -> Spillover {
    matrix(&["FITC-A", "PE-A"], &[1.0, 0.4, 0.0, 1.0])
}

fn p(name: &str) -> PathBuf {
    PathBuf::from(format!("/data/{name}.fcs"))
}

/// Files join the group of the matrix they carry - the same matrix listed in
/// another order is the same matrix - and files with none, or an identity,
/// share a group that compensates nothing.
#[test]
fn files_are_grouped_by_the_matrix_they_carry() {
    let reordered = Spillover::new(
        vec![Arc::from("PE-A"), Arc::from("APC-A"), Arc::from("FITC-A")],
        vec![1.0, 0.10, 0.05, 0.30, 1.0, 0.0, 0.25, 0.02, 1.0],
    )
    .unwrap();
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    c.add_file(p("b"), Ok(Some(reordered)));
    c.add_file(p("c"), Ok(Some(other())));
    c.add_file(p("d"), Ok(None));
    c.add_file(p("e"), Ok(Some(matrix(&["A", "B"], &identity(2)))));
    assert_eq!(c.groups().len(), 3);
    assert_eq!(c.group_of(&p("a")), c.group_of(&p("b")));
    assert_ne!(c.group_of(&p("a")), c.group_of(&p("c")));
    assert_eq!(c.group_of(&p("d")), c.group_of(&p("e")));

    assert!(c.matrix_for(&p("b")).unwrap().unwrap().same_as(&three()));
    assert!(c.matrix_for(&p("c")).unwrap().unwrap().same_as(&other()));
    assert_eq!(c.matrix_for(&p("d")).unwrap(), None);
    assert_eq!(c.matrix_for(&p("unknown")).unwrap(), None);
}

/// A group given a CSV compensates every file in it with that matrix; a
/// file moved to another group is compensated as that group says.
#[test]
fn a_groups_source_decides_and_a_moved_file_follows_its_new_group() {
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    c.add_file(p("b"), Ok(Some(three())));
    let group = c.group_of(&p("a")).unwrap();
    let csv = Arc::new(other());
    c.set_source(
        group,
        Source::Loaded {
            path: "/m.csv".into(),
            matrix: csv.clone(),
        },
    )
    .unwrap();
    assert_eq!(c.matrix_for(&p("a")).unwrap(), Some(csv.clone()));
    assert_eq!(c.matrix_for(&p("b")).unwrap(), Some(csv));

    let own = c.new_group("Kept as acquired");
    c.set_source(own, Source::FilesOwn).unwrap();
    c.move_file(&p("b"), own).unwrap();
    assert!(c.matrix_for(&p("b")).unwrap().unwrap().same_as(&three()));
    assert_eq!(c.files_in(own).collect::<Vec<_>>(), [p("b").as_path()]);
    assert!(c.move_file(&p("b"), 999).is_err());
}

/// A file is never left uncompensated by accident: one with no matrix in a
/// group using the files' own, one whose own matrix is unreadable, and one
/// in a group whose CSV could not be read are errors. Only a group set to no
/// compensation compensates nothing.
#[test]
fn a_file_that_cannot_be_compensated_as_its_group_says_is_an_error() {
    let mut c = Compensation::default();
    c.add_file(p("with"), Ok(Some(three())));
    c.add_file(p("without"), Ok(None));
    c.add_file(
        p("broken"),
        Err("its $SPILLOVER is not a spillover matrix".into()),
    );
    let with = c.group_of(&p("with")).unwrap();
    c.move_file(&p("without"), with).unwrap();
    assert!(
        c.matrix_for(&p("without"))
            .unwrap_err()
            .contains("this file has none")
    );
    assert!(
        c.matrix_for(&p("broken"))
            .unwrap_err()
            .contains("not a spillover matrix")
    );
    c.set_source(
        with,
        Source::Unreadable {
            path: "/m.csv".into(),
            why: "gone".into(),
        },
    )
    .unwrap();
    assert!(
        c.matrix_for(&p("with"))
            .unwrap_err()
            .contains("could not be read: gone")
    );
    c.set_source(with, Source::None).unwrap();
    assert_eq!(c.matrix_for(&p("with")).unwrap(), None);
}

/// A group formed around files' matrices goes when its last file does; one
/// given a CSV, or made by hand, stays, and a group with files in cannot be
/// removed.
#[test]
fn empty_groups_go_unless_someone_chose_something_for_them() {
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    c.add_file(p("b"), Ok(Some(other())));
    let b = c.group_of(&p("b")).unwrap();
    c.set_source(
        b,
        Source::Loaded {
            path: "/m.csv".into(),
            matrix: Arc::new(other()),
        },
    )
    .unwrap();
    let hand = c.new_group("By hand");
    assert!(c.remove_group(b).is_err());
    c.remove_file(&p("a"));
    c.remove_file(&p("b"));
    let left: Vec<GroupIdName> = c.groups().iter().map(|g| (g.id, g.name.clone())).collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.iter().any(|(id, _)| *id == b) && left.iter().any(|(id, _)| *id == hand));
    c.remove_group(hand).unwrap();
    assert_eq!(c.groups().len(), 1);
}

type GroupIdName = (super::groups::GroupId, String);

/// Saved and restored, the groups, their sources and who is where come back;
/// a CSV that can no longer be read leaves its group unreadable rather than
/// uncompensated; a file the saved groups do not know is grouped by its
/// matrix, joining a restored group with the same one.
#[test]
fn groups_are_remembered() {
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    c.add_file(p("b"), Ok(Some(three())));
    c.add_file(p("c"), Ok(None));
    let loaded = c.new_group("Recomputed");
    c.set_source(
        loaded,
        Source::Loaded {
            path: "/m.csv".into(),
            matrix: Arc::new(other()),
        },
    )
    .unwrap();
    c.move_file(&p("b"), loaded).unwrap();
    let saved: Saved = c.saved();
    assert!(
        saved
            .groups
            .iter()
            .any(|g| g.source == SavedSource::Csv("/m.csv".into()))
    );

    let files = || {
        vec![
            (p("a"), Ok(Some(three()))),
            (p("b"), Ok(Some(three()))),
            (p("c"), Ok(None)),
            (p("new"), Ok(Some(three()))),
        ]
    };
    let back = Compensation::restore(&saved, files(), |_| Ok(other()));
    assert_eq!(back.groups().len(), 3);
    assert_eq!(
        back.matrix_for(&p("b")).unwrap().unwrap().as_ref(),
        &other()
    );
    assert!(back.matrix_for(&p("a")).unwrap().unwrap().same_as(&three()));
    assert_eq!(back.group_of(&p("new")), back.group_of(&p("a")));
    assert_eq!(back.matrix_for(&p("c")).unwrap(), None);

    let unreadable = Compensation::restore(&saved, files(), |_| Err("no such file".into()));
    assert!(
        unreadable
            .matrix_for(&p("b"))
            .unwrap_err()
            .contains("no such file")
    );
}

/// The digest moves whenever what a file is compensated with does, and only
/// then.
#[test]
fn the_digest_follows_what_a_file_is_compensated_with() {
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    c.add_file(p("b"), Ok(Some(other())));
    let g = c.group_of(&p("a")).unwrap();
    let before = c.digest(&p("a"));
    let untouched = c.digest(&p("b"));
    c.set_source(g, Source::None).unwrap();
    assert_ne!(c.digest(&p("a")), before);
    c.set_source(g, Source::FilesOwn).unwrap();
    assert_eq!(c.digest(&p("a")), before);
    assert_eq!(c.digest(&p("b")), untouched);
}

/// Kept in step with the workspace: new files are grouped, removed files
/// leave, and a file that stays keeps the group it was moved to.
#[test]
fn groups_follow_the_workspaces_files() {
    let mut c = Compensation::default();
    c.sync(vec![
        (p("a"), Ok(Some(three()))),
        (p("b"), Ok(Some(three()))),
    ]);
    let own = c.new_group("By hand");
    c.move_file(&p("b"), own).unwrap();
    c.sync(vec![
        (p("b"), Ok(Some(three()))),
        (p("c"), Ok(Some(three()))),
    ]);
    assert_eq!(c.group_of(&p("a")), None);
    assert_eq!(c.group_of(&p("b")), Some(own));
    assert!(c.group_of(&p("c")).is_some());
    assert_ne!(c.group_of(&p("c")), Some(own));
}

/// What a group is told about itself: files that cannot be compensated, a
/// loaded matrix that does not fit a file, and one that looks transposed or
/// far from the files' own.
#[test]
fn a_group_is_told_what_is_wrong_with_it() {
    let channels = |_: &Path| {
        vec![
            ("FITC-A".to_string(), None),
            ("PE-A".to_string(), None),
            ("APC-A".to_string(), None),
        ]
    };
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(Some(three())));
    let g = c.group_of(&p("a")).unwrap();
    assert!(c.check(g, channels).is_empty(), "its own matrix fits");

    let s = three();
    let transposed = Spillover::new(
        s.channels().to_vec(),
        (0..9).map(|k| s.value(k % 3, k / 3)).collect(),
    )
    .unwrap();
    let loaded = |m: Spillover| Source::Loaded {
        path: "/m.csv".into(),
        matrix: Arc::new(m),
    };
    c.set_source(g, loaded(transposed)).unwrap();
    let notes = c.check(g, channels);
    assert!(
        notes.iter().any(|n| n.contains("the other way round")),
        "{notes:?}"
    );

    let mut far = three();
    far = Spillover::new(far.channels().to_vec(), {
        let mut v: Vec<f64> = (0..9).map(|k| far.value(k / 3, k % 3)).collect();
        v[1] = 0.60;
        v
    })
    .unwrap();
    c.set_source(g, loaded(far)).unwrap();
    let notes = c.check(g, channels);
    assert!(
        notes
            .iter()
            .any(|n| n.contains("more than 10 percentage points")),
        "{notes:?}"
    );

    c.set_source(
        g,
        loaded(matrix(&["FITC-A", "BV421-A"], &[1.0, 0.1, 0.0, 1.0])),
    )
    .unwrap();
    let notes = c.check(g, channels);
    assert!(
        notes
            .iter()
            .any(|n| n
                .contains("a.fcs: the matrix does not fit it: the file has no channel BV421-A")),
        "{notes:?}"
    );

    c.set_source(
        g,
        Source::Unreadable {
            path: "/m.csv".into(),
            why: "gone".into(),
        },
    )
    .unwrap();
    assert!(
        c.check(g, channels)
            .iter()
            .any(|n| n.contains("could not be read: gone"))
    );
}

/// A file's channels are its `$PnN`s in order, with a label only where the
/// file gives one.
#[test]
fn a_files_channels_come_with_their_labels() {
    let dir = scratch("channels-of");
    let path = file_with(&dir, "labels.fcs", &[]);
    let stub = FcsSampleStub::open(&path.to_string_lossy()).unwrap();
    assert_eq!(
        super::channels_of(&stub),
        [
            ("FITC-A".to_string(), Some("CD3".to_string())),
            ("PE-A".to_string(), Some("CD4".to_string())),
            ("APC-A".to_string(), None),
        ]
    );
    let owns = super::own_matrices(&[stub]);
    assert_eq!(owns.len(), 1);
    assert_eq!(owns[0].1, Ok(None));
}

// ─── A real Omiq export ──────────────────────────────────────────────────────

/// The compensation matrix Omiq exported for a 38-colour spectral panel:
/// channel names with spaces in them, percent, and - the data being unmixed
/// already - an identity. Read as it is, it changes nothing.
#[test]
fn a_real_omiq_export_is_read() {
    let text = include_str!("../../tests/fixtures/omiq_compensation_identity.csv");
    let m = Spillover::from_omiq_csv(text).unwrap();
    assert_eq!(m.channels().len(), 38);
    let names: Vec<&str> = m.channels().iter().map(|c| c.as_ref()).collect();
    assert_eq!(names[0], "BUV395-A");
    assert!(names.contains(&"LIVE DEAD Blue-A"));
    assert!(names.contains(&"PE-eFluor 610-A"));
    assert_eq!(names[37], "AF P3-A");
    assert!(m.is_identity());
    assert_eq!(m.involved(), None);

    // Applied to a file carrying only some of the panel, it is no change -
    // not a refusal for the channels the file does not have.
    let frame = df!("FSC-A" => [1.0f32, 2.0], "BUV395-A" => [5.0f32, -3.0]).unwrap();
    assert_eq!(compensate(&frame, &m, |_| None).unwrap(), frame);
}

/// Only the channels a matrix actually mixes have to be in a file: one that
/// spills into nothing and receives nothing is left as it is whether the
/// file has it or not, and compensating over the rest gives the same answer
/// as over the whole.
#[test]
fn a_channel_the_matrix_does_not_mix_need_not_be_in_the_file() {
    // FITC and PE mix; APC and BV421 take no part.
    let m = matrix(
        &["FITC-A", "PE-A", "APC-A", "BV421-A"],
        &[
            1.0, 0.25, 0.0, 0.0, //
            0.05, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ],
    );
    let involved = m.involved().unwrap();
    assert_eq!(involved.channels().len(), 2);
    assert!(involved.same_as(&matrix(&["FITC-A", "PE-A"], &[1.0, 0.25, 0.05, 1.0])));

    let whole = df!(
        "FITC-A" => [1000.0f32, 50.0],
        "PE-A" => [250.0f32, 1000.0],
        "APC-A" => [7.0f32, 8.0],
        "BV421-A" => [9.0f32, 10.0],
    )
    .unwrap();
    let partial = whole.drop("BV421-A").unwrap();
    let a = compensate(&whole, &m, |_| None).unwrap();
    let b = compensate(&partial, &m, |_| None).unwrap();
    for c in ["FITC-A", "PE-A", "APC-A"] {
        assert_eq!(a.column(c).unwrap(), b.column(c).unwrap(), "{c}");
    }
    let fitc: Vec<f32> = a
        .column("FITC-A")
        .unwrap()
        .f32()
        .unwrap()
        .into_no_null_iter()
        .collect();
    assert!(close(fitc[0], 1000.0) && close(fitc[1], 0.0), "{fitc:?}");

    // A channel that does take part is still required.
    let refused = compensate(&whole.drop("PE-A").unwrap(), &m, |_| None)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("the file has no channel PE-A"),
        "{refused}"
    );

    // And a group is not told a file lacks a channel that takes no part.
    let mut c = Compensation::default();
    c.add_file(p("a"), Ok(None));
    let g = c.group_of(&p("a")).unwrap();
    c.set_source(
        g,
        Source::Loaded {
            path: "/m.csv".into(),
            matrix: Arc::new(m),
        },
    )
    .unwrap();
    let notes = c.check(g, |_| {
        ["FITC-A", "PE-A", "APC-A"]
            .iter()
            .map(|n| (n.to_string(), None))
            .collect()
    });
    assert!(notes.is_empty(), "{notes:?}");
}
