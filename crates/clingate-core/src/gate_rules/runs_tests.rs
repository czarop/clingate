//! Rules that go by run: a band read on all of a run's FMX files together,
//! and a reference for each run.
//!
//! "Placed on a per-run basis in the first instance, as all samples in a run
//! will have been stained from the same cocktail preparation." Four donors on
//! two plates, each with an FMX and a full stain; the second plate's
//! negative sits higher, so its line has to.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use rustc_hash::FxBuildHasher;

use crate::file_load_tests::{scratch, write_fcs_rows};
use crate::gate_rules::autogate::{apply_placements, extent_on};
use crate::gate_rules::rule::{BandAim, Pool, Rule, TailFractionRule};
use crate::gate_rules::rule_store::{
    Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, RunReference, SamplePairing,
};
use crate::gate_rules::run::{RunInputs, RunOutcome, run_rules};
use crate::gates::GateState;
use crate::gates::gate_store::GateSource;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";
const BIG: f32 = 1e16;

/// (file, donor, type, plate)
const FILES: [(&str, &str, &str, &str); 8] = [
    ("d1_fmx", "D1", "FMX", "P1"),
    ("d1_fs", "D1", "FS", "P1"),
    ("d2_fmx", "D2", "FMX", "P1"),
    ("d2_fs", "D2", "FS", "P1"),
    ("d3_fmx", "D3", "FMX", "P2"),
    ("d3_fs", "D3", "FS", "P2"),
    ("d4_fmx", "D4", "FMX", "P2"),
    ("d4_fs", "D4", "FS", "P2"),
];

fn rect(id: &str, name: &str, x0: f32) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(x0, -BIG), (BIG, -BIG), (BIG, BIG), (x0, BIG)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from(id),
                name: name.into(),
                geometry,
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(X), Arc::from(Y)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    )
}

fn gates() -> GateState {
    let mut state = GateState::default();
    for (id, name, x0, parent) in [
        ("lymph", "Lymph", -BIG, None),
        ("cd69", "CD69+", 0.0, Some("lymph")),
    ] {
        let gate = rect(id, name, x0);
        state.place_gate(&[Arc::from(id)], &gate, &GateSource::Global);
        state
            .place_new_gate(parent.map(Arc::from), Arc::from(id))
            .unwrap();
    }
    state
}

/// A negative centred on `centre`: 600 events for an FMX - a MAIT-sized
/// parent - and more with a positive tail for a full stain.
fn events(centre: f32, seed: u64, full_stain: bool) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let neg = Normal::new(centre, 50.0).unwrap();
    let pos = Normal::new(centre + 400.0, 50.0).unwrap();
    let mut rows: Vec<Vec<f32>> = (0..600)
        .map(|_| vec![neg.sample(&mut rng), 100.0])
        .collect();
    if full_stain {
        rows.extend((0..200).map(|_| vec![pos.sample(&mut rng), 100.0]));
    }
    rows
}

fn centre_of(plate: &str) -> f32 {
    if plate == "P1" { 300.0 } else { 450.0 }
}

fn metadata(files: &[(&str, &str, &str, &str)]) -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, donor, kind, plate) in files {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
        columns.insert(Arc::from("SampleID"), Arc::from(*donor));
        columns.insert(Arc::from("SampleType"), Arc::from(*kind));
        columns.insert(Arc::from("Plate"), Arc::from(*plate));
        map.insert(Arc::from(*file) as Arc<str>, columns);
    }
    map
}

/// The files written, each with events of its own.
fn write(name: &str, files: &[(&str, &str, &str, &str)]) -> Vec<(Arc<str>, PathBuf)> {
    let dir = scratch(name);
    files
        .iter()
        .enumerate()
        .map(|(seed, (file, _, kind, plate))| {
            let path = dir.join(format!("{file}.fcs"));
            write_fcs_rows(
                &path,
                &[(X, None), (Y, None)],
                &events(centre_of(plate), seed as u64 + 1, *kind == "FS"),
                &[],
            );
            (Arc::from(format!("{file}.fcs").as_str()), path)
        })
        .collect()
}

fn store(rule: GateRule, run_column: Option<&str>) -> RuleStore {
    let mut store = RuleStore::with_pairing(SamplePairing {
        run_column: run_column.map(Arc::from),
        ..SamplePairing::default()
    });
    store.insert(RuleTarget::under("CD69+", "Lymph"), rule);
    store
}

fn band(pool: Pool, measured_on: MeasuredOn) -> GateRule {
    GateRule {
        parameter: Arc::from(X),
        bound: Bound::Above,
        measured_on,
        rule: Rule::TailFraction(TailFractionRule {
            pool,
            ..TailFractionRule::aimed((0.02, 0.03), BandAim::Middle)
        }),
    }
}

fn fmx() -> MeasuredOn {
    MeasuredOn::Partner(Arc::from("FMX"))
}

fn run(
    state: &GateState,
    written: &[(Arc<str>, PathBuf)],
    files: &[(&str, &str, &str, &str)],
    rules: RuleStore,
) -> RunOutcome {
    run_rules(
        state,
        &RunInputs {
            files: written.to_vec(),
            compensation: crate::compensation::groups::Compensation::default(),
            names: files
                .iter()
                .map(|(f, ..)| (Arc::from(format!("{f}.fcs").as_str()), Arc::from(*f)))
                .collect::<std::collections::HashMap<_, _, FxBuildHasher>>(),
            cofactors: Vec::new(),
            metadata: metadata(files),
            rules,
        },
        |_| {},
        &AtomicBool::new(false),
    )
}

fn line(state: &GateState, file: &str, files: &[(&str, &str, &str, &str)]) -> f32 {
    let g = state
        .gate_for_file(&Arc::from("cd69"), &Arc::from(file), &metadata(files))
        .unwrap();
    extent_on(&g.get_gate_ref(None).unwrap().geometry, X)
        .unwrap()
        .0
}

fn applied(state: &GateState, outcome: &RunOutcome) -> GateState {
    let mut after = state.clone();
    apply_placements(&mut after, &outcome.placements);
    after
}

fn reasons(outcome: &RunOutcome) -> Vec<String> {
    outcome
        .report
        .skipped
        .iter()
        .map(|s| format!("{} | {}", s.file, s.reason))
        .collect()
}

// ─── a band read across the run ───────────────────────────────────────────────

#[test]
fn a_band_read_across_the_run_gives_every_specimen_in_it_the_same_line() {
    let written = write("runs-pooled", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Run, fmx()), Some("Plate")),
    );
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&gates(), &outcome);
    let p1 = line(&after, "d1_fs", &FILES);
    let p2 = line(&after, "d3_fs", &FILES);
    assert_eq!(line(&after, "d2_fs", &FILES), p1, "one line for plate 1");
    assert_eq!(line(&after, "d4_fs", &FILES), p2, "one line for plate 2");
    assert!(
        p2 - p1 > 100.0,
        "plate 2's negative is higher, and so is its line: {p1} {p2}"
    );
    // The FMX files of a specimen share its line - they are one specimen.
    assert_eq!(line(&after, "d1_fmx", &FILES), p1);
}

#[test]
fn the_run_s_line_is_the_one_its_fmx_files_pooled_into_one_would_give() {
    // The oracle: one specimen whose FMX holds every event of plate 1's two
    // FMX files, gated per specimen. Pooling is exactly that, and no more.
    let written = write("runs-pooled-oracle", &FILES);
    let pooled = applied(
        &gates(),
        &run(
            &gates(),
            &written,
            &FILES,
            store(band(Pool::Run, fmx()), Some("Plate")),
        ),
    );

    let one: [(&str, &str, &str, &str); 2] =
        [("u_fmx", "U", "FMX", "P1"), ("u_fs", "U", "FS", "P1")];
    let dir = scratch("runs-pooled-union");
    let mut union = events(centre_of("P1"), 1, false); // d1_fmx's seed
    union.extend(events(centre_of("P1"), 3, false)); // d2_fmx's seed
    write_fcs_rows(&dir.join("u_fmx.fcs"), &[(X, None), (Y, None)], &union, &[]);
    write_fcs_rows(
        &dir.join("u_fs.fcs"),
        &[(X, None), (Y, None)],
        &events(centre_of("P1"), 2, true),
        &[],
    );
    let files_u = vec![
        (Arc::from("u_fmx.fcs"), dir.join("u_fmx.fcs")),
        (Arc::from("u_fs.fcs"), dir.join("u_fs.fcs")),
    ];
    let alone = applied(
        &gates(),
        &run(
            &gates(),
            &files_u,
            &one,
            store(band(Pool::Specimen, fmx()), None),
        ),
    );
    assert_eq!(line(&pooled, "d1_fs", &FILES), line(&alone, "u_fs", &one));
}

#[test]
fn what_the_run_s_line_holds_is_counted_over_all_its_fmx_files() {
    let written = write("runs-pooled-count", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Run, fmx()), Some("Plate")),
    );
    let at = line(&applied(&gates(), &outcome), "d1_fs", &FILES);
    // By hand: events above the line in plate 1's FMX files, over all of them.
    let fmx: Vec<f32> = events(centre_of("P1"), 1, false)
        .into_iter()
        .chain(events(centre_of("P1"), 3, false))
        .map(|row| row[0])
        .collect();
    let by_hand = fmx.iter().filter(|x| **x >= at).count() as f64 / fmx.len() as f64;
    for p in outcome
        .report
        .positioned
        .iter()
        .filter(|p| &*p.specimen == "D1" || &*p.specimen == "D2")
    {
        // The same events: the plot's percentage is a 32-bit float, so the
        // two are compared as counts.
        assert_eq!(
            (p.achieved * 1200.0).round(),
            (by_hand * 1200.0).round(),
            "{} against {by_hand}",
            p.achieved
        );
        assert!(p.in_band);
        assert_eq!(p.reference_events, 1200, "both FMX files' events");
        assert!(
            p.components
                .iter()
                .any(|c| c.name == "pooled" && c.detail.contains("its 2 FMX files")),
            "the report says it was pooled: {:?}",
            p.components
        );
    }
}

#[test]
fn a_specimen_without_its_own_fmx_still_takes_its_run_s_line() {
    let files: Vec<(&str, &str, &str, &str)> = FILES
        .iter()
        .copied()
        .filter(|(f, ..)| *f != "d2_fmx")
        .collect();
    let written = write("runs-pooled-missing", &files);
    let outcome = run(
        &gates(),
        &written,
        &files,
        store(band(Pool::Run, fmx()), Some("Plate")),
    );
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&gates(), &outcome);
    assert_eq!(line(&after, "d2_fs", &files), line(&after, "d1_fs", &files));

    // Per specimen, the same donor has nothing to read.
    let alone = run(
        &gates(),
        &written,
        &files,
        store(band(Pool::Specimen, fmx()), Some("Plate")),
    );
    assert!(
        reasons(&alone)
            .iter()
            .any(|r| r.contains("no file with SampleID D2 has the sample type FMX")),
        "{:?}",
        reasons(&alone)
    );
}

#[test]
fn a_run_already_on_its_line_is_left_where_it_is() {
    let written = write("runs-pooled-kept", &FILES);
    let rules = store(band(Pool::Run, fmx()), Some("Plate"));
    let once = applied(&gates(), &run(&gates(), &written, &FILES, rules.clone()));
    let again = run(&once, &written, &FILES, rules);
    assert!(again.placements.is_empty(), "nothing moves the second time");
    assert_eq!(again.report.unchanged.len(), 4, "every specimen, in place");
}

#[test]
fn a_band_read_across_runs_needs_the_run_column_and_says_so() {
    let written = write("runs-no-column", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Run, fmx()), None),
    );
    assert!(outcome.placements.is_empty());
    let said = reasons(&outcome);
    assert!(
        said.iter().all(|r| r.contains("no run column is set")),
        "{said:?}"
    );

    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Run, fmx()), Some("Batch")),
    );
    assert!(
        reasons(&outcome)
            .iter()
            .all(|r| r.contains("this file has no Batch")),
        "{:?}",
        reasons(&outcome)
    );
}

// ─── a reference for each run ─────────────────────────────────────────────────

fn per_run(references: &[(&str, &str)]) -> MeasuredOn {
    MeasuredOn::FilePerRun(
        references
            .iter()
            .map(|(run, file)| RunReference {
                run: Arc::from(*run),
                file: Arc::from(*file),
            })
            .collect(),
    )
}

#[test]
fn each_run_reads_the_reference_named_for_it() {
    let written = write("runs-references", &FILES);
    let both = run(
        &gates(),
        &written,
        &FILES,
        store(
            band(
                Pool::Specimen,
                per_run(&[("P1", "d1_fmx"), ("P2", "d3_fmx")]),
            ),
            Some("Plate"),
        ),
    );
    assert!(both.report.skipped.is_empty(), "{:?}", reasons(&both));
    let after = applied(&gates(), &both);

    // The oracle: the plate's reference named outright, one plate at a time.
    let named = |file: &str| {
        applied(
            &gates(),
            &run(
                &gates(),
                &written,
                &FILES,
                store(
                    band(Pool::Specimen, MeasuredOn::File(Arc::from(file))),
                    Some("Plate"),
                ),
            ),
        )
    };
    let p1 = named("d1_fmx");
    let p2 = named("d3_fmx");
    assert_eq!(line(&after, "d2_fs", &FILES), line(&p1, "d2_fs", &FILES));
    assert_eq!(line(&after, "d4_fs", &FILES), line(&p2, "d4_fs", &FILES));
    assert_ne!(line(&after, "d2_fs", &FILES), line(&after, "d4_fs", &FILES));

    // Each reference is left where a person put it.
    let references: Vec<&str> = both.report.reference.iter().map(|u| &*u.specimen).collect();
    assert_eq!(references.len(), 2, "{references:?}");
    assert!(references.contains(&"D1") && references.contains(&"D3"));
}

#[test]
fn a_run_with_no_reference_named_says_which_run() {
    let written = write("runs-reference-missing", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(
            band(Pool::Specimen, per_run(&[("P1", "d1_fmx")])),
            Some("Plate"),
        ),
    );
    let said = reasons(&outcome);
    assert!(
        said.iter()
            .any(|r| r.contains("no reference is named for Plate P2 - the rule names one for P1")),
        "{said:?}"
    );
    // Plate 1 is placed all the same.
    assert!(
        outcome
            .report
            .positioned
            .iter()
            .any(|p| &*p.specimen == "D2")
    );

    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Specimen, per_run(&[("P1", "d1_fmx")])), None),
    );
    assert!(
        reasons(&outcome)
            .iter()
            .all(|r| r.contains("no run column is set")),
        "{:?}",
        reasons(&outcome)
    );
}
