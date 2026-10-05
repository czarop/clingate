//! A band counted on the whole run: every FMX file analysed together - the
//! workspace - read at once, and one line for every specimen.
//!
//! "Placed on a per-run basis in the first instance, as all samples in a run
//! will have been stained from the same cocktail preparation." Four donors,
//! each with an FMX of 600 events and a full stain.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rand::SeedableRng;
use rand_distr::{Distribution, Normal};
use rustc_hash::FxBuildHasher;

use crate::file_load_tests::{scratch, write_fcs_rows};
use crate::gate_rules::autogate::{apply_placements, extent_on};
use crate::gate_rules::rule::{BandAim, Pool, Rule, TailFractionRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, RunOutcome, run_rules};
use crate::gates::GateState;
use crate::gates::gate_store::GateSource;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";
const BIG: f32 = 1e16;

/// (file, donor, type, where the donor's negative sits)
const FILES: [(&str, &str, &str, &str); 8] = [
    ("d1_fmx", "D1", "FMX", "300"),
    ("d1_fs", "D1", "FS", "300"),
    ("d2_fmx", "D2", "FMX", "300"),
    ("d2_fs", "D2", "FS", "300"),
    ("d3_fmx", "D3", "FMX", "340"),
    ("d3_fs", "D3", "FS", "340"),
    ("d4_fmx", "D4", "FMX", "340"),
    ("d4_fs", "D4", "FS", "340"),
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
    gates_with_lymph_from(-BIG)
}

/// The gates, with Lymph's left edge at `lymph_from`.
fn gates_with_lymph_from(lymph_from: f32) -> GateState {
    let mut state = GateState::default();
    for (id, name, x0, parent) in [
        ("lymph", "Lymph", lymph_from, None),
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

fn centre_of(at: &str) -> f32 {
    at.parse().unwrap()
}

fn metadata(files: &[(&str, &str, &str, &str)]) -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, donor, kind, _) in files {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
        columns.insert(Arc::from("SampleID"), Arc::from(*donor));
        columns.insert(Arc::from("SampleType"), Arc::from(*kind));
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
        .map(|(seed, (file, _, kind, at))| {
            let path = dir.join(format!("{file}.fcs"));
            write_fcs_rows(
                &path,
                &[(X, None), (Y, None)],
                &events(centre_of(at), seed as u64 + 1, *kind == "FS"),
                &[],
            );
            (Arc::from(format!("{file}.fcs").as_str()), path)
        })
        .collect()
}

fn store(rule: GateRule) -> RuleStore {
    let mut store = RuleStore::default();
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

fn inputs(
    written: &[(Arc<str>, PathBuf)],
    files: &[(&str, &str, &str, &str)],
    rules: RuleStore,
) -> RunInputs {
    RunInputs {
        files: written.to_vec(),
        compensation: crate::compensation::groups::Compensation::default(),
        names: files
            .iter()
            .map(|(f, ..)| (Arc::from(format!("{f}.fcs").as_str()), Arc::from(*f)))
            .collect::<std::collections::HashMap<_, _, FxBuildHasher>>(),
        cofactors: Vec::new(),
        metadata: metadata(files),
        rules,
    }
}

fn run(
    state: &GateState,
    written: &[(Arc<str>, PathBuf)],
    files: &[(&str, &str, &str, &str)],
    rules: RuleStore,
) -> RunOutcome {
    run_rules(
        state,
        &inputs(written, files, rules),
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

fn applied(
    state: &GateState,
    outcome: &RunOutcome,
    files: &[(&str, &str, &str, &str)],
) -> GateState {
    let mut after = state.clone();
    apply_placements(&mut after, &outcome.placements, &metadata(files));
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

// ─── one line for the run ─────────────────────────────────────────────────────

fn fmx_of_all() -> Vec<f32> {
    FILES
        .iter()
        .enumerate()
        .filter(|(_, (_, _, kind, _))| *kind == "FMX")
        .flat_map(|(seed, (_, _, _, at))| events(centre_of(at), seed as u64 + 1, false))
        .map(|row| row[0])
        .collect()
}

#[test]
fn counted_on_the_run_every_specimen_gets_the_same_line() {
    let written = write("runs-pooled", &FILES);
    let outcome = run(&gates(), &written, &FILES, store(band(Pool::Run, fmx())));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&gates(), &outcome, &FILES);
    let one = line(&after, "d1_fs", &FILES);
    for file in ["d1_fmx", "d2_fs", "d3_fs", "d4_fs", "d4_fmx"] {
        assert_eq!(line(&after, file, &FILES), one, "{file}");
    }

    // Per specimen, the donors whose negative sits higher get a higher line -
    // which is what counting on the run gives up.
    let alone = applied(
        &gates(),
        &run(
            &gates(),
            &written,
            &FILES,
            store(band(Pool::Specimen, fmx())),
        ),
        &FILES,
    );
    assert!(
        line(&alone, "d3_fs", &FILES) - line(&alone, "d1_fs", &FILES) > 20.0,
        "the test has to tell the two apart"
    );
}

#[test]
fn the_run_s_line_is_the_one_its_fmx_files_pooled_into_one_would_give() {
    // The oracle: one specimen whose FMX holds every event of the run's four
    // FMX files, gated per specimen. Pooling is exactly that, and no more.
    let written = write("runs-pooled-oracle", &FILES);
    let pooled = applied(
        &gates(),
        &run(&gates(), &written, &FILES, store(band(Pool::Run, fmx()))),
        &FILES,
    );

    let one: [(&str, &str, &str, &str); 2] =
        [("u_fmx", "U", "FMX", "300"), ("u_fs", "U", "FS", "300")];
    let dir = scratch("runs-pooled-union");
    let union: Vec<Vec<f32>> = fmx_of_all().into_iter().map(|x| vec![x, 100.0]).collect();
    write_fcs_rows(&dir.join("u_fmx.fcs"), &[(X, None), (Y, None)], &union, &[]);
    write_fcs_rows(
        &dir.join("u_fs.fcs"),
        &[(X, None), (Y, None)],
        &events(300.0, 2, true),
        &[],
    );
    let files_u = vec![
        (Arc::from("u_fmx.fcs"), dir.join("u_fmx.fcs")),
        (Arc::from("u_fs.fcs"), dir.join("u_fs.fcs")),
    ];
    let alone = applied(
        &gates(),
        &run(&gates(), &files_u, &one, store(band(Pool::Specimen, fmx()))),
        &one,
    );
    assert_eq!(line(&pooled, "d1_fs", &FILES), line(&alone, "u_fs", &one));
}

#[test]
fn what_the_run_s_line_holds_is_counted_over_all_its_fmx_files() {
    let written = write("runs-pooled-count", &FILES);
    let outcome = run(&gates(), &written, &FILES, store(band(Pool::Run, fmx())));
    let at = line(&applied(&gates(), &outcome, &FILES), "d1_fs", &FILES);
    let fmx = fmx_of_all();
    let by_hand = fmx.iter().filter(|x| **x >= at).count() as f64 / fmx.len() as f64;
    assert_eq!(outcome.report.positioned.len(), 4);
    for p in &outcome.report.positioned {
        // The same events: the plot's percentage is a 32-bit float, so the
        // two are compared as counts.
        assert_eq!(
            (p.achieved * 2400.0).round(),
            (by_hand * 2400.0).round(),
            "{} against {by_hand}",
            p.achieved
        );
        assert!(p.in_band);
        assert_eq!(p.reference_events, 2400, "all four FMX files' events");
        assert!(
            p.components
                .iter()
                .any(|c| c.name == "pooled" && c.detail.contains("its 4 FMX files")),
            "the report says it was pooled: {:?}",
            p.components
        );
    }
}

#[test]
fn a_specimen_without_its_own_fmx_still_takes_the_run_s_line() {
    let files: Vec<(&str, &str, &str, &str)> = FILES
        .iter()
        .copied()
        .filter(|(f, ..)| *f != "d2_fmx")
        .collect();
    let written = write("runs-pooled-missing", &files);
    let outcome = run(&gates(), &written, &files, store(band(Pool::Run, fmx())));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&gates(), &outcome, &FILES);
    assert_eq!(line(&after, "d2_fs", &files), line(&after, "d1_fs", &files));

    // Per specimen, the same donor has nothing to read.
    let alone = run(
        &gates(),
        &written,
        &files,
        store(band(Pool::Specimen, fmx())),
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
    let rules = store(band(Pool::Run, fmx()));
    let once = applied(
        &gates(),
        &run(&gates(), &written, &FILES, rules.clone()),
        &FILES,
    );
    let again = run(&once, &written, &FILES, rules);
    assert!(again.placements.is_empty(), "nothing moves the second time");
    assert_eq!(again.report.unchanged.len(), 4, "every specimen, in place");
}

#[test]
fn a_run_with_none_of_the_kind_it_reads_says_so() {
    let written = write("runs-pooled-none", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Run, MeasuredOn::Partner(Arc::from("FMO")))),
    );
    assert!(outcome.placements.is_empty());
    let said = reasons(&outcome);
    assert_eq!(said.len(), 1, "one line, not one per file: {said:?}");
    assert!(
        said[0].contains("no FMO file in the run was measured for this gate")
            && said[0].ends_with("and the same on 3 other specimens"),
        "{said:?}"
    );
}

/// CD69+ gated on both Lymph and Mono, neither with an FMO to read: a line
/// for each, the two told apart by their parents.
#[test]
fn a_run_failing_one_marker_on_two_parents_says_so_for_each() {
    let written = write("runs-pooled-two-parents", &FILES);
    let mut state = gates();
    for (id, name, parent) in [("mono", "Mono", None), ("mono_cd69", "CD69+", Some("mono"))] {
        state.place_gate(&[Arc::from(id)], &rect(id, name, 0.0), &GateSource::Global);
        state
            .place_new_gate(parent.map(Arc::from), Arc::from(id))
            .unwrap();
    }
    let mut rules = store(band(Pool::Run, MeasuredOn::Partner(Arc::from("FMO"))));
    rules.insert(
        RuleTarget::under("CD69+", "Mono"),
        band(Pool::Run, MeasuredOn::Partner(Arc::from("FMO"))),
    );
    let outcome = run(&state, &written, &FILES, rules);
    let mut said: Vec<(String, String)> = outcome
        .report
        .skipped
        .iter()
        .map(|s| {
            (
                format!("{} on {:?}", s.gate, s.parent_gate),
                s.reason.clone(),
            )
        })
        .collect();
    said.sort();
    assert_eq!(
        said.iter()
            .map(|(gate, _)| gate.as_str())
            .collect::<Vec<_>>(),
        ["CD69+ on Some(\"Lymph\")", "CD69+ on Some(\"Mono\")"],
        "{said:?}"
    );
    assert!(
        said.iter()
            .all(|(_, reason)| reason.ends_with("and the same on 3 other specimens")),
        "{said:?}"
    );
}

/// Each specimen's line read on its own FMX of 600 events: in band, but with
/// a confidence held down by the count. 600 is over the 300 a control needs
/// to be trusted, so nothing is flagged for review - nor would the run pause
/// on it.
#[test]
fn a_line_read_on_a_big_enough_fmx_and_in_band_is_not_flagged() {
    let written = write("runs-fmx-trusted", &FILES);
    let outcome = run(
        &gates(),
        &written,
        &FILES,
        store(band(Pool::Specimen, fmx())),
    );
    let placed = &outcome.report.positioned;
    assert_eq!(placed.len(), 4, "{:?}", reasons(&outcome));
    for p in placed {
        assert!(
            p.read_on_control && p.in_band && p.reference_events == 600,
            "{} {} {}",
            p.read_on_control,
            p.in_band,
            p.reference_events
        );
    }
    let floor = crate::review::assess::REVIEW_FLOOR;
    assert!(
        placed.iter().all(|p| p.confidence < floor),
        "only a weak placement shows the exemption: {:?}",
        placed.iter().map(|p| p.confidence).collect::<Vec<_>>()
    );
    assert_eq!(outcome.report.needs_review(floor).count(), 0);
}

// ─── scored against the gating drawn by hand ──────────────────────────────────

fn score(
    state: &GateState,
    written: &[(Arc<str>, PathBuf)],
    rules: RuleStore,
) -> crate::gate_rules::score::Score {
    crate::gate_rules::score::score_rules(
        state,
        &inputs(written, &FILES, rules),
        |_| true,
        &AtomicBool::new(false),
    )
    .unwrap()
}

/// The X values of `file`, as written.
fn values_of(file: &str) -> Vec<f32> {
    let (seed, (_, _, kind, at)) = FILES
        .iter()
        .enumerate()
        .find(|(_, (f, ..))| *f == file)
        .unwrap();
    events(centre_of(at), seed as u64 + 1, *kind == "FS")
        .into_iter()
        .map(|row| row[0])
        .collect()
}

/// Each sample read on itself: the rule's line is where a run puts it, the
/// hand line is the drawn 0, and the distance is in that sample's own
/// interquartile range, counted here from the events written.
#[test]
fn each_sample_is_scored_by_how_far_the_rule_s_line_is_from_the_hand_line() {
    let written = write("score-each", &FILES);
    let rules = store(band(Pool::Specimen, MeasuredOn::Itself));
    let placed = applied(&gates(), &run(&gates(), &written, &FILES, rules.clone()), &FILES);
    let scored = score(&gates(), &written, rules);

    assert_eq!(scored.gates.len(), 1);
    // The full stains: the FMX files are what a rule reads, not what it gates.
    assert_eq!(scored.rows.len(), 4, "{:#?}", scored.rows);
    for row in &scored.rows {
        let file = row.file.trim_end_matches(".fcs");
        let mut values = values_of(file);
        values.sort_by(f32::total_cmp);
        // Linear between the order statistics either side of rank q(n - 1).
        let quartile = |q: f64| {
            let rank = q * (values.len() - 1) as f64;
            let (lo, hi) = (values[rank.floor() as usize], values[rank.ceil() as usize]);
            lo as f64 + (hi - lo) as f64 * rank.fract()
        };
        let iqr = quartile(0.75) - quartile(0.25);
        let rule_line = line(&placed, file, &FILES) as f64;
        let above = values.iter().filter(|v| **v as f64 > rule_line).count();

        assert_eq!(row.what, "moved", "{row:?}");
        assert_eq!(row.hand_line, Some(0.0));
        assert!((row.rule_line.unwrap() - rule_line).abs() < 1e-3, "{row:?}");
        let off = row.off_iqrs.unwrap();
        assert!(
            (off - rule_line / iqr).abs() < 0.02 * off.abs(),
            "{file}: {off} against {}",
            rule_line / iqr
        );
        assert_eq!(row.hand_holds, Some(1.0), "the drawn gate holds everything");
        let holds = above as f64 / values.len() as f64;
        assert!((row.rule_holds.unwrap() - holds).abs() < 0.002, "{row:?}");
        assert!((row.holds_difference.unwrap() - (holds - 1.0) * 100.0).abs() < 0.2);
    }
    let gate = &scored.gates[0];
    assert_eq!((gate.scored, gate.not_placed), (4, 0));
    let worst = scored.rows.iter().map(|r| r.off_iqrs.unwrap().abs()).fold(0.0, f64::max);
    assert_eq!(gate.worst_off_iqrs, Some(worst));
    assert_eq!(gate.worst[0], scored.rows[0].file, "worst first");
}

/// A band the drawn gate already sits in: the rule would leave it, so it is
/// exactly where the hand put it.
#[test]
fn a_rule_the_hand_gating_already_meets_scores_nothing_off() {
    let written = write("score-kept", &FILES);
    let mut rule = band(Pool::Specimen, MeasuredOn::Itself);
    rule.rule = Rule::TailFraction(TailFractionRule::new((0.5, 1.0)));
    let scored = score(&gates(), &written, store(rule));
    for row in &scored.rows {
        assert_eq!(row.what, "kept", "{row:?}");
        assert_eq!(row.off_iqrs, Some(0.0));
        assert_eq!(row.holds_difference, Some(0.0));
    }
    assert_eq!(scored.gates[0].worst_off_iqrs, Some(0.0));
}

/// A gate is scored under its parent as drawn, whatever the rule above it
/// would do: the same answer with and without a rule that moves the parent.
#[test]
fn a_gate_is_scored_under_its_parent_as_drawn() {
    let written = write("score-parent", &FILES);
    // An edge a rule can move, left of every event.
    let drawn = gates_with_lymph_from(-1_000.0);
    let child = band(Pool::Specimen, MeasuredOn::Itself);
    let alone = score(&drawn, &written, store(child.clone()));

    let mut both = store(child);
    both.insert(
        RuleTarget::named("Lymph"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::TailFraction(TailFractionRule::new((0.45, 0.55))),
        },
    );
    let with_parent = score(&drawn, &written, both.clone());
    assert_eq!(
        with_parent.gates.len(),
        2,
        "{:?}",
        reasons(&run(&drawn, &written, &FILES, both.clone()))
    );
    let child_rows = |s: &crate::gate_rules::score::Score| {
        s.rows
            .iter()
            .filter(|r| r.gate_id == "cd69")
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(child_rows(&with_parent), child_rows(&alone));

    // A run moves the parent first and reads the child under it, so its
    // child lines differ: the parent rule really does change the child.
    let run_both = run(&drawn, &written, &FILES, both);
    let child_line = |file: &str| {
        run_both
            .report
            .positioned
            .iter()
            .find(|p| &*p.gate_id == "cd69" && &*p.file == file)
            .map(|p| p.to)
            .unwrap()
    };
    assert!(
        alone
            .rows
            .iter()
            .any(|r| (r.rule_line.unwrap() - child_line(&r.file)).abs() > 1.0),
        "the parent rule moved nothing under it, so this proves nothing"
    );
}


/// A sample the rule cannot place is listed with why, and scored on nothing.
#[test]
fn a_sample_the_rule_cannot_place_is_said_and_not_scored() {
    let written = write("score-unplaced", &FILES);
    let scored = score(
        &gates(),
        &written,
        store(band(Pool::Specimen, MeasuredOn::Partner(Arc::from("FMO")))),
    );
    assert!(!scored.rows.is_empty());
    for row in &scored.rows {
        assert_eq!(row.what, "not placed", "{row:?}");
        assert!(row.why_not.is_some());
        assert_eq!((row.off_iqrs, row.holds_difference), (None, None));
    }
    let gate = &scored.gates[0];
    assert_eq!(gate.scored, 0);
    assert_eq!(gate.not_placed, scored.rows.len());
    assert_eq!((gate.worst_off_iqrs, gate.worst.len()), (None, 0));
}
