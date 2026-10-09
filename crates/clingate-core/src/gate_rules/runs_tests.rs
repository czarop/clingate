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
use crate::gate_rules::fit::{FitSettings, Fit, MOST_CANDIDATES, RankBy, fit_rule};
use crate::gate_rules::rule::{BandAim, Pool, Rule, TailFractionRule, ValleyOrSmearRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::{RunInputs, RunOutcome, run_rules};
use crate::gate_rules::score::{least_agreeing_first, summarise};
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
    gates_with(lymph_from, 0.0)
}

/// The gates, with Lymph's left edge at `lymph_from` and CD69+'s at
/// `cd69_from`.
fn gates_with(lymph_from: f32, cd69_from: f32) -> GateState {
    let mut state = GateState::default();
    for (id, name, x0, parent) in [
        ("lymph", "Lymph", lymph_from, None),
        ("cd69", "CD69+", cd69_from, Some("lymph")),
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
    // Counted against 100,000 events rather than 10,000, 600 scores
    // ln(600 / 100) / ln(100,000 / 100) = 0.26, under the review floor.
    let mut rule = band(Pool::Specimen, fmx());
    let Rule::TailFraction(tail) = &mut rule.rule else {
        unreachable!("band() makes a tail-fraction rule")
    };
    tail.confidence.limits.events_full = 100_000.0;
    let outcome = run(&gates(), &written, &FILES, store(rule));
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
    score_files(state, written, &FILES, rules)
}

fn score_files(
    state: &GateState,
    written: &[(Arc<str>, PathBuf)],
    files: &[(&str, &str, &str, &str)],
    rules: RuleStore,
) -> crate::gate_rules::score::Score {
    crate::gate_rules::score::score_rules(
        state,
        &inputs(written, files, rules),
        |_| true,
        crate::gate_rules::score::ScoreSettings::default(),
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

/// Linear between the order statistics either side of rank q(n - 1).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let rank = q * (sorted.len() - 1) as f64;
    let (lo, hi) = (sorted[rank.floor() as usize], sorted[rank.ceil() as usize]);
    lo + (hi - lo) * rank.fract()
}

fn middle(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    (sorted[(n - 1) / 2] + sorted[n / 2]) / 2.0
}

/// Each sample read on itself. The drawn gate holds every event from 0 up;
/// the rule's line is where a run puts it, so its gate holds the events from
/// there up, all of them inside the drawn one. Every number is counted here
/// from the events written.
#[test]
fn each_sample_is_scored_by_the_events_both_gates_hold() {
    let written = write("score-each", &FILES);
    let rules = store(band(Pool::Specimen, MeasuredOn::Itself));
    let placed = applied(&gates(), &run(&gates(), &written, &FILES, rules.clone()), &FILES);
    let scored = score(&gates(), &written, rules);

    assert_eq!(scored.gates.len(), 1);
    // The full stains: the FMX files are what a rule reads, not what it gates.
    assert_eq!(scored.rows.len(), 4, "{:#?}", scored.rows);
    for row in &scored.rows {
        let file = row.file.trim_end_matches(".fcs");
        let mut values: Vec<f64> = values_of(file).into_iter().map(f64::from).collect();
        values.sort_by(f64::total_cmp);
        let rule_line = line(&placed, file, &FILES) as f64;
        let held: Vec<f64> = values.iter().copied().filter(|v| *v >= rule_line).collect();
        let (hand, rule) = (values.len(), held.len());

        assert_eq!(row.what, "moved", "{row:?}");
        let events = row.events.unwrap();
        assert_eq!((events.hand, events.rule, events.both), (hand, rule, rule), "{row:?}");
        assert_eq!(row.caught, Some(rule as f64 / hand as f64));
        assert_eq!(row.extra, Some(0.0));
        let total = (hand + rule) as f64;
        assert_eq!(row.agreement, Some(2.0 * rule as f64 / total), "{row:?}");
        assert_eq!(row.off_line, Some(0.8 - 1.0 / total.sqrt()), "{row:?}");

        // Every event is kept, so the shift is read on all of them; the
        // second axis is one value throughout, with no spread to read it in.
        let iqr = quantile(&values, 0.75) - quantile(&values, 0.25);
        let shift = (middle(&held) - middle(&values)) / iqr;
        assert_eq!(row.shift_iqrs.len(), 1, "{row:?}");
        assert_eq!(row.shift_iqrs[0].0, X);
        assert!((row.shift_iqrs[0].1 - shift).abs() < 1e-3 * shift, "{row:?} {shift}");

        assert_eq!(row.hand_edge, Some(0.0));
        assert!((row.rule_edge.unwrap() - rule_line).abs() < 1e-3, "{row:?}");
        let off = row.edge_off_iqrs.unwrap();
        // The same interpolation both ways round, so only rounding differs.
        assert!((off - rule_line / iqr).abs() < 1e-6 * off, "{file}: {off}");
        assert_eq!(row.hand_holds, Some(1.0), "the drawn gate holds everything");
        assert!((row.holds_difference.unwrap() - (rule as f64 / hand as f64 - 1.0) * 100.0).abs() < 1e-3);
    }
    let gate = &scored.gates[0];
    assert_eq!((gate.scored, gate.not_placed), (4, 0));
    let lowest = scored.rows.iter().filter_map(|r| r.agreement).fold(1.0, f64::min);
    assert_eq!(gate.lowest_agreement, Some(lowest));
    assert_eq!(gate.off, 4, "the rule holds a few % of a gate drawn round everything");
    assert_eq!(gate.off_samples[0], scored.rows[0].file, "least agreeing first");
    assert_eq!(gate.median_extra, Some(0.0));
}

/// A band the drawn gate already sits in: the rule would leave it, so it
/// holds exactly what the hand gate holds.
#[test]
fn a_rule_the_hand_gating_already_meets_agrees_entirely() {
    let written = write("score-kept", &FILES);
    let mut rule = band(Pool::Specimen, MeasuredOn::Itself);
    rule.rule = Rule::TailFraction(TailFractionRule::new((0.5, 1.0)));
    let scored = score(&gates(), &written, store(rule));
    for row in &scored.rows {
        assert_eq!(row.what, "kept", "{row:?}");
        assert_eq!((row.agreement, row.caught, row.extra), (Some(1.0), Some(1.0), Some(0.0)));
        assert_eq!((row.edge_off_iqrs, row.holds_difference), (Some(0.0), Some(0.0)));
    }
    let gate = &scored.gates[0];
    assert_eq!((gate.typical_agreement, gate.off), (Some(1.0), 0));
    assert!(gate.off_samples.is_empty());
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
            .any(|r| (r.rule_edge.unwrap() - child_line(&r.file)).abs() > 1.0),
        "the parent rule moved nothing under it, so this proves nothing"
    );
}

/// A sample the rule cannot place is listed with why, and scored as a gate
/// holding nothing: it would be gated by hand.
#[test]
fn a_sample_the_rule_cannot_place_is_scored_as_holding_nothing() {
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
        let hand = values_of(row.file.trim_end_matches(".fcs")).len();
        let events = row.events.unwrap();
        assert_eq!((events.hand, events.rule, events.both), (hand, 0, 0), "{row:?}");
        assert_eq!((row.agreement, row.caught, row.extra), (Some(0.0), Some(0.0), None));
        assert_eq!(row.off_line, Some(0.8 - 1.0 / (hand as f64).sqrt()));
        assert_eq!(row.holds_difference, None);
    }
    let gate = &scored.gates[0];
    assert_eq!((gate.scored, gate.not_placed), (0, 4), "one for each of the four specimens");
    assert_eq!((gate.typical_agreement, gate.lowest_agreement), (Some(0.0), Some(0.0)));
    assert_eq!((gate.off, gate.off_samples.len()), (4, 4));
}

/// A rule on a parameter the files do not hold measures nothing: each
/// specimen is still listed, with why, and counted off.
#[test]
fn a_sample_the_rule_cannot_measure_is_said_too() {
    let written = write("score-unmeasured", &FILES);
    let mut rule = band(Pool::Specimen, MeasuredOn::Itself);
    rule.parameter = Arc::from("CD3");
    let scored = score(&gates(), &written, store(rule));
    assert_eq!(scored.gates.len(), 1, "{scored:#?}");
    assert_eq!(scored.gates[0].not_placed, 4);
    assert_eq!(scored.gates[0].off, 4, "each would be gated by hand");
    for row in &scored.rows {
        assert_eq!(row.what, "not placed");
        assert_eq!((row.hand_edge, row.hand_holds), (None, None));
        assert!(row.why_not.is_some(), "{row:?}");
    }
}

/// The sample a rule calibrates from is listed as the reference and not
/// scored.
#[test]
fn the_reference_is_listed_and_not_scored() {
    let written = write("score-reference", &FILES);
    let scored = score(
        &gates(),
        &written,
        store(band(Pool::Specimen, MeasuredOn::File(Arc::from("d1_fs")))),
    );
    let references: Vec<_> = scored.rows.iter().filter(|r| r.what == "reference").collect();
    assert!(!references.is_empty(), "{:#?}", scored.rows);
    assert!(references.iter().all(|r| r.agreement.is_none() && r.rule_edge.is_none()));
    let gate = &scored.gates[0];
    assert_eq!(gate.references, references.len());
    assert_eq!(gate.scored + gate.not_placed + gate.references, scored.rows.len());
}

/// Asked for one gate, only that gate's rule is run.
#[test]
fn only_the_rules_asked_for_are_scored() {
    let written = write("score-which", &FILES);
    let mut both = store(band(Pool::Specimen, MeasuredOn::Itself));
    both.insert(
        RuleTarget::named("Lymph"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::TailFraction(TailFractionRule::new((0.45, 0.55))),
        },
    );
    let drawn = gates_with_lymph_from(-1_000.0);
    let child = RuleTarget::under("CD69+", "Lymph");
    let scored = crate::gate_rules::score::score_rules(
        &drawn,
        &inputs(&written, &FILES, both.clone()),
        |target| *target == child,
        crate::gate_rules::score::ScoreSettings::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(scored.gates.len(), 1);
    assert!(scored.rows.iter().all(|r| r.gate_id == "cd69"));
    assert_eq!(score(&drawn, &written, both).gates.len(), 2);
}

fn fit(
    written: &[(Arc<str>, PathBuf)],
    files: &[(&str, &str, &str, &str)],
    rules: RuleStore,
    candidates: &[GateRule],
    settings: FitSettings,
) -> Result<Fit, String> {
    fit_rule(
        &gates(),
        &inputs(written, files, rules),
        &RuleTarget::under("CD69+", "Lymph"),
        candidates,
        crate::gate_rules::score::ScoreSettings::default(),
        settings,
        &AtomicBool::new(false),
    )
}

/// Every candidate, and the rule as it stands, scored on one reading of the
/// files exactly as the scorer scores it alone - those measuring alike on one
/// measurement, one measured apart - and the closest to the hand gating
/// first.
#[test]
fn each_candidate_is_scored_as_the_scorer_scores_it_alone() {
    let written = write("fit-alone", &FILES);
    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let kept = GateRule {
        rule: Rule::TailFraction(TailFractionRule::new((0.5, 1.0))),
        ..current.clone()
    };
    let valley = GateRule {
        rule: Rule::ValleyOrSmear(ValleyOrSmearRule::default()),
        ..current.clone()
    };
    let refused = band(Pool::Specimen, MeasuredOn::Partner(Arc::from("FMO")));
    // The drawn gate's right edge is unbounded: nothing to measure.
    let unbounded = GateRule {
        bound: Bound::Below,
        ..current.clone()
    };
    let candidates = [unbounded.clone(), refused.clone(), valley.clone(), kept.clone()];
    let found = fit(&written, &FILES, store(current.clone()), &candidates, FitSettings::default()).unwrap();

    assert_eq!(found.candidates.len(), 5, "the rule as it stands is tried too");
    assert_eq!(found.fit_on, ["D1", "D2", "D3", "D4"]);
    assert!(found.checked_on.is_empty(), "four specimens are too few to split");
    for candidate in &found.candidates {
        let alone = score(&gates(), &written, store(candidate.rule.clone()));
        assert_eq!(candidate.fit.as_ref(), alone.gates.first(), "{}", candidate.said);
        assert_eq!((&candidate.check, candidate.place_on_check), (&None, None));
        assert_eq!(candidate.current, candidate.rule == current);
    }
    // The hand gate already meets the wide band, and sits where the valley
    // rule reads it on the reference: both agree on every sample, a tie.
    let ranked: Vec<&GateRule> = found.candidates.iter().map(|c| &c.rule).collect();
    assert_eq!(ranked, [&valley, &kept, &current, &refused, &unbounded]);
    for (candidate, place) in found.candidates.iter().zip(1..) {
        assert_eq!((candidate.place_by_typical, candidate.among_best), (place, place <= 2));
    }
    assert_eq!(found.candidates[1].fit.as_ref().unwrap().typical_agreement, Some(1.0));

    let by_off = fit(
        &written,
        &FILES,
        store(current),
        &candidates,
        FitSettings {
            rank_by: RankBy::Off,
            ..FitSettings::default()
        },
    )
    .unwrap();
    let places: Vec<usize> = by_off.candidates.iter().map(|c| c.place_by_off).collect();
    assert_eq!(places, [1, 2, 3, 4, 5]);
}

/// Eight donors: (file, donor, type, where the donor's negative sits).
const EIGHT: [(&str, &str, &str, &str); 16] = [
    ("e1_fmx", "D1", "FMX", "300"),
    ("e1_fs", "D1", "FS", "300"),
    ("e2_fmx", "D2", "FMX", "320"),
    ("e2_fs", "D2", "FS", "320"),
    ("e3_fmx", "D3", "FMX", "340"),
    ("e3_fs", "D3", "FS", "340"),
    ("e4_fmx", "D4", "FMX", "360"),
    ("e4_fs", "D4", "FS", "360"),
    ("e5_fmx", "D5", "FMX", "300"),
    ("e5_fs", "D5", "FS", "300"),
    ("e6_fmx", "D6", "FMX", "320"),
    ("e6_fs", "D6", "FS", "320"),
    ("e7_fmx", "D7", "FMX", "340"),
    ("e7_fs", "D7", "FS", "340"),
    ("e8_fmx", "D8", "FMX", "360"),
    ("e8_fs", "D8", "FS", "360"),
];

/// Enough specimens to split: ranked on the odd donors, checked on the even,
/// each half summed up from its own samples alone.
#[test]
fn candidates_are_ranked_on_half_the_specimens_and_checked_on_the_other() {
    let written = write("fit-halves", &EIGHT);
    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let found = fit(&written, &EIGHT, store(current.clone()), &[], FitSettings::default()).unwrap();
    assert_eq!(found.fit_on, ["D1", "D3", "D5", "D7"]);
    assert_eq!(found.checked_on, ["D2", "D4", "D6", "D8"]);

    let all = score_files(&gates(), &written, &EIGHT, store(current));
    let only_of = |donors: [&str; 4]| {
        let mut rows: Vec<_> = all
            .rows
            .iter()
            .filter(|row| donors.contains(&row.specimen.as_deref().unwrap()))
            .cloned()
            .collect();
        rows.sort_by(least_agreeing_first);
        summarise("cd69".into(), &rows)
    };
    let candidate = &found.candidates[0];
    let (fitted, checked) = (candidate.fit.as_ref().unwrap(), candidate.check.as_ref().unwrap());
    assert_eq!(fitted, &only_of(["D1", "D3", "D5", "D7"]));
    assert_eq!(checked, &only_of(["D2", "D4", "D6", "D8"]));
    assert_eq!((fitted.scored, checked.scored), (4, 4), "a full stain per donor");
    assert!(fitted.off_samples.iter().all(|f| ["e1_fs", "e3_fs", "e5_fs", "e7_fs"].iter().any(|d| f.starts_with(d))));
    assert_eq!(candidate.place_on_check, Some(1));
}

/// Refused before a file is read: nothing to try, too much to try, or a tie
/// wider than any agreement.
#[test]
fn a_search_with_nothing_or_too_much_to_try_is_refused() {
    let nothing = fit(&[], &FILES, RuleStore::default(), &[], FitSettings::default());
    assert_eq!(nothing.unwrap_err(), "no candidate rule to try");

    let many: Vec<GateRule> = (1..=MOST_CANDIDATES + 1)
        .map(|at| GateRule {
            rule: Rule::TailFraction(TailFractionRule::new((0.0, at as f64 / 100.0))),
            ..band(Pool::Specimen, MeasuredOn::Itself)
        })
        .collect();
    let too_many = fit(&[], &FILES, RuleStore::default(), &many, FitSettings::default());
    assert!(too_many.unwrap_err().starts_with(&format!("at most {MOST_CANDIDATES}")));

    let wide = FitSettings {
        tie_within: 2.0,
        ..FitSettings::default()
    };
    let one = [band(Pool::Specimen, MeasuredOn::Itself)];
    assert!(fit(&[], &FILES, RuleStore::default(), &one, wide).unwrap_err().contains("tie_within"));
}

/// The best, those tied with it and the rule as it stands keep where they
/// put the gate on each sample they move it on - where a run puts it - and
/// no other candidate does.
#[test]
fn the_closest_candidates_and_the_rule_as_it_stands_keep_their_gates() {
    let written = write("fit-placed", &FILES);
    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let kept = GateRule {
        rule: Rule::TailFraction(TailFractionRule::new((0.5, 1.0))),
        ..current.clone()
    };
    let other = GateRule {
        rule: Rule::TailFraction(TailFractionRule::aimed((0.05, 0.06), BandAim::Middle)),
        ..current.clone()
    };
    let found = fit(
        &written,
        &FILES,
        store(current.clone()),
        &[kept.clone(), other.clone()],
        FitSettings::default(),
    )
    .unwrap();
    let of = |rule: &GateRule| found.candidates.iter().find(|c| &c.rule == rule).unwrap();
    assert!(of(&kept).among_best, "{found:#?}");
    assert!(of(&kept).placed.is_empty(), "it leaves every gate as drawn");
    assert!(!of(&other).among_best && of(&other).placed.is_empty());

    let ran = applied(&gates(), &run(&gates(), &written, &FILES, store(current.clone())), &FILES);
    let placed = &of(&current).placed;
    assert_eq!(placed.len(), 4, "a full stain per donor");
    for gate in placed {
        assert_eq!(gate.gate_id, "cd69");
        let left = extent_on(&gate.gate.geometry, X).unwrap().0;
        assert_eq!(left, line(&ran, &gate.file, &FILES), "{}", gate.file);
    }
}

/// Rules searched together, on one reading of the files, come out exactly as
/// each searched alone; a search that cannot be made says why in its place;
/// and the reading and the trying are reported as they go.
#[test]
fn rules_searched_together_come_out_as_each_searched_alone() {
    use crate::gate_rules::fit::{Ask, default_candidates, fit_rules};
    use crate::gate_rules::run::Progress;
    use crate::gate_rules::score::ScoreSettings;

    let written = write("fit-together", &FILES);
    let drawn = gates_with_lymph_from(-1_000.0);
    let mut rules = store(band(Pool::Specimen, MeasuredOn::Itself));
    rules.insert(
        RuleTarget::named("Lymph"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::TailFraction(TailFractionRule::new((0.45, 0.55))),
        },
    );
    let inputs = inputs(&written, &FILES, rules.clone());
    let mut asks: Vec<Ask> = rules
        .entries()
        .iter()
        .map(|entry| Ask {
            target: entry.target.clone(),
            candidates: default_candidates(&entry.rule),
        })
        .collect();
    asks.push(Ask {
        target: RuleTarget::named("Nowhere"),
        candidates: Vec::new(),
    });
    let heard = std::sync::Mutex::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let together = fit_rules(
        &drawn,
        &inputs,
        &asks,
        ScoreSettings::default(),
        FitSettings::default(),
        &cancel,
        |step| heard.lock().unwrap().push(step),
    )
    .unwrap();

    assert_eq!(together.len(), 3);
    for ((target, found), ask) in together.iter().zip(&asks).take(2) {
        assert_eq!(target, &ask.target);
        let alone = fit_rule(
            &drawn,
            &inputs,
            target,
            &ask.candidates,
            ScoreSettings::default(),
            FitSettings::default(),
            &cancel,
        )
        .unwrap();
        assert_eq!(found.as_ref().unwrap(), &alone, "{}", target.describe());
    }
    assert_eq!(together[2].1, Err("no candidate rule to try".to_string()));

    // Each band tried at five widths, aimed two ways; the rule as it stands is one.
    let tried = 2 * 10;
    let heard = heard.into_inner().unwrap();
    let files = FILES.len();
    assert!(heard.contains(&Progress::Measuring { done: files, total: files }), "{heard:?}");
    assert!(heard.contains(&Progress::Solving { done: tried, total: tried }), "{heard:?}");
}

/// The share of `file`'s events above `line`, counted from what was written.
fn share_above(file: &str, line: f32) -> f64 {
    let values = values_of(file);
    values.iter().filter(|v| **v > line).count() as f64 / values.len() as f64
}

/// Between two numbers, as a median of two is.
fn midway(mut shares: Vec<f64>) -> f64 {
    shares.sort_by(f64::total_cmp);
    (shares[1] + shares[2]) / 2.0
}

/// What the hand-drawn gates hold is read on the files a run gates - each
/// donor's full stain - and on what the rule reads for each: the full stain
/// itself, or its FMX.
#[test]
fn what_the_hand_gates_hold_is_read_where_the_rule_reads() {
    use crate::gate_rules::pick::held_by_hand;
    use crate::gate_rules::run::measure_many;

    let written = write("pick-held", &FILES);
    let drawn = gates_with(-BIG, 500.0);
    let read = |measured_on: MeasuredOn| {
        let rule = band(Pool::Specimen, measured_on);
        let inputs = inputs(&written, &FILES, store(rule.clone()));
        let (measured, _) = measure_many(
            &drawn,
            &inputs.files,
            &inputs.compensation,
            &inputs.names,
            &inputs.cofactors,
            &inputs.metadata,
            std::slice::from_ref(&inputs.rules),
            None,
            &AtomicBool::new(false),
            |_, _| {},
        );
        held_by_hand(&rule, &measured[0].0, &inputs.rules, &inputs.metadata).unwrap()
    };
    let full_stains = ["d1_fs", "d2_fs", "d3_fs", "d4_fs"];
    let fmxs = ["d1_fmx", "d2_fmx", "d3_fmx", "d4_fmx"];
    let on_itself = midway(full_stains.iter().map(|f| share_above(f, 500.0)).collect());
    let on_fmx = midway(fmxs.iter().map(|f| share_above(f, 500.0)).collect());
    assert!((read(MeasuredOn::Itself) - on_itself).abs() < 1e-12, "{on_itself}");
    assert!((read(fmx()) - on_fmx).abs() < 1e-12, "{on_fmx}");
    assert!(on_itself > 0.2, "the positives are a quarter of a full stain");
}

/// Pick for CD69+, its rule `current` and its gates as `drawn`, as
/// `settings` asks; and every step of the pick heard.
fn pick_cd69(
    name: &str,
    drawn: &GateState,
    current: GateRule,
    settings: crate::gate_rules::pick::PickSettings,
) -> (
    Vec<(Arc<str>, PathBuf)>,
    crate::gate_rules::pick::Picked,
    Vec<crate::gate_rules::pick::Picking>,
) {
    use crate::gate_rules::pick::pick_rules;

    let written = write(name, &FILES);
    let heard = std::sync::Mutex::new(Vec::new());
    let mut picked = pick_rules(
        drawn,
        &inputs(&written, &FILES, store(current)),
        &[RuleTarget::under("CD69+", "Lymph")],
        settings,
        FitSettings::default(),
        &AtomicBool::new(false),
        |step| heard.lock().unwrap().push(step),
    )
    .unwrap();
    let (_, picked) = picked.remove(0);
    (written, picked.unwrap(), heard.into_inner().unwrap())
}

fn kinds_tried(
    picked: &crate::gate_rules::pick::Picked,
) -> Vec<(crate::gate_rules::pick::Kind, bool, bool)> {
    picked
        .kinds
        .iter()
        .map(|k| (k.kind, k.passed, k.searched))
        .collect()
}

/// A band on the FMX at the range given that keeps every gate where it was
/// drawn - no FMX holds a hundredth beyond 500 - passes, and is taken as it
/// is: no other kind's settings are searched, and the rule as it stands
/// comes after it.
#[test]
fn the_band_on_the_fmx_is_taken_as_given_when_it_passes() {
    use crate::gate_rules::pick::{Kind, PickSettings};

    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let settings = PickSettings {
        fmx_band: Some((0.0, 0.01)),
        ..PickSettings::default()
    };
    let (written, picked, _) = pick_cd69(
        "pick-fmx-band",
        &gates_with(-BIG, 500.0),
        current.clone(),
        settings,
    );
    assert!(picked.passed, "{picked:#?}");
    let on_fmx = GateRule {
        measured_on: fmx(),
        rule: Rule::TailFraction(TailFractionRule::new((0.0, 0.01))),
        ..current.clone()
    };
    let rules: Vec<&GateRule> = picked.fit.candidates.iter().map(|c| &c.rule).collect();
    assert_eq!(rules, [&on_fmx, &current]);
    use Kind::*;
    assert_eq!(
        kinds_tried(&picked)[0],
        (FmxBand, true, false),
        "passed, and not searched"
    );
    assert!(picked.kinds.iter().all(|k| !k.searched), "no kind searched");
    assert_eq!(
        picked.tried, 5,
        "the rule as it stands, and each kind once but the phenotype, the last resort"
    );

    let alone = score(&gates_with(-BIG, 500.0), &written, store(on_fmx));
    assert_eq!(picked.fit.candidates[0].fit.as_ref(), alone.gates.first());
    assert_eq!(
        alone.gates[0].typical_agreement,
        Some(1.0),
        "every gate kept"
    );
}

/// A band on the FMX that would put the line in the negative fails; above
/// the negative, next in order, passes as the hand gating starts it, and its
/// settings are searched - each scored exactly as the scorer scores it
/// alone. The steps are reported as they go.
#[test]
fn the_next_kind_in_order_is_searched_when_the_fmx_band_fails() {
    use crate::gate_rules::pick::{Kind, PickSettings, Picking};

    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let settings = PickSettings {
        fmx_band: Some((0.3, 0.4)),
        ..PickSettings::default()
    };
    let (written, picked, heard) = pick_cd69(
        "pick-next-kind",
        &gates_with(-BIG, 500.0),
        current.clone(),
        settings,
    );
    assert!(picked.passed, "{picked:#?}");
    use Kind::*;
    let tried = kinds_tried(&picked);
    assert_eq!(tried[0], (FmxBand, false, false));
    assert_eq!(tried[1], (AboveNegative, true, true));
    assert!(
        tried[2..].iter().all(|(_, _, searched)| !searched),
        "{tried:?}"
    );
    let best = &picked.fit.candidates[0];
    assert!(
        matches!(best.rule.rule, Rule::AboveTheNegative(_)),
        "{}",
        best.said
    );
    assert!(
        picked
            .fit
            .candidates
            .iter()
            .all(|c| matches!(c.rule.rule, Rule::AboveTheNegative(_)) || c.current),
        "only above the negative contends"
    );
    assert!(
        picked.tried > 10,
        "the negative's settings searched: {}",
        picked.tried
    );

    let alone = score(&gates_with(-BIG, 500.0), &written, store(best.rule.clone()));
    assert_eq!(best.fit.as_ref(), alone.gates.first(), "{}", best.said);

    let last = |of: fn(&Picking) -> Option<(usize, usize)>| heard.iter().filter_map(of).last();
    let read = last(|p| match p {
        Picking::Reading { done, total } => Some((*done, *total)),
        _ => None,
    });
    let first = last(|p| match p {
        Picking::Kinds { done, total } => Some((*done, *total)),
        _ => None,
    });
    let searched = last(|p| match p {
        Picking::Settings { done, total } => Some((*done, *total)),
        _ => None,
    });
    assert_eq!(read, Some((FILES.len(), FILES.len())));
    let (first_done, first_total) = first.unwrap();
    let (searched_done, searched_total) = searched.unwrap();
    assert_eq!((first_done, searched_done), (first_total, searched_total));
    assert_eq!(first_total + searched_total, picked.tried);
}

/// CD69+ drawn by hand where no one rule puts it - in one donor's negative,
/// through the middle of another's positives, on both its files - so no kind
/// passes: every kind is tried and searched in order, the phenotype last
/// with the files read again for it, and the closest of everything tried is
/// shown, flagged.
#[test]
fn with_no_kind_passing_every_kind_is_searched_and_the_closest_shown_flagged() {
    use crate::gate_rules::pick::{Kind, PickSettings, Picking};

    let mut drawn = gates_with(-BIG, 500.0);
    for (donor, x0) in [("d1", 330.0), ("d2", 700.0), ("d3", 450.0), ("d4", 760.0)] {
        let cd69: Arc<str> = Arc::from("cd69");
        for kind in ["fmx", "fs"] {
            drawn.place_gate(
                std::slice::from_ref(&cd69),
                &rect("cd69", "CD69+", x0),
                &GateSource::Sample((cd69.clone(), Arc::from(format!("{donor}_{kind}").as_str()))),
            );
        }
    }
    let current = band(Pool::Specimen, MeasuredOn::Itself);
    let settings = PickSettings {
        fmx_band: Some((0.0, 0.01)),
        ..PickSettings::default()
    };
    let (_, picked, heard) = pick_cd69("pick-flagged", &drawn, current, settings);
    assert!(!picked.passed, "{picked:#?}");
    use Kind::*;
    assert_eq!(
        kinds_tried(&picked),
        [
            (FmxBand, false, false),
            (AboveNegative, false, true),
            (ValleyOrSmear, false, true),
            (Band, false, true),
            (Phenotype, false, true),
        ]
    );
    assert_eq!(
        picked.fit.candidates.len(),
        picked.tried,
        "everything tried"
    );
    let typical = |c: &crate::gate_rules::fit::Candidate| {
        c.fit
            .as_ref()
            .and_then(|f| f.typical_agreement)
            .unwrap_or(f64::NEG_INFINITY)
    };
    let best = typical(&picked.fit.candidates[0]);
    assert!(picked.fit.candidates.iter().all(|c| typical(c) <= best));
    let read_through = heard
        .iter()
        .filter(|p| matches!(p, Picking::Reading { done, total } if done == total && *total > 0))
        .count();
    assert_eq!(
        read_through, 2,
        "once, and again for the phenotype: {heard:?}"
    );
}

/// A gate placed from another gate, or with no rule, is not picked for, and
/// says why; the others are picked as if alone.
#[test]
fn a_gate_that_cannot_be_picked_for_says_why() {
    use crate::gate_rules::pick::pick_rules;
    use crate::gate_rules::rule::FromGateRule;

    let written = write("pick-refused", &FILES);
    let mut rules = store(band(Pool::Specimen, MeasuredOn::Itself));
    let lymph = RuleTarget::named("Lymph");
    rules.insert(
        lymph.clone(),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: Rule::FromAnotherGate(FromGateRule {
                same_shape_as: Some(RuleTarget::named("CD69+")),
                edges: Vec::new(),
            }),
        },
    );
    let nowhere = RuleTarget::named("Nowhere");
    let cd69 = RuleTarget::under("CD69+", "Lymph");
    let picked = pick_rules(
        &gates_with(-BIG, 500.0),
        &inputs(&written, &FILES, rules),
        &[lymph.clone(), cd69.clone(), nowhere.clone()],
        Default::default(),
        FitSettings::default(),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let targets: Vec<&RuleTarget> = picked.iter().map(|(t, _)| t).collect();
    assert_eq!(targets, [&lymph, &cd69, &nowhere]);
    assert!(picked[0].1.as_ref().unwrap_err().contains("from another gate"));
    assert!(picked[1].1.is_ok());
    assert!(picked[2].1.as_ref().unwrap_err().contains("no rule"));
}

/// With no sample of its own to calibrate on, a pick calibrates on the
/// specimen whose hand gate holds the middle share of its parent - read on
/// its full stain - counted from what was written.
#[test]
fn the_reference_calibrated_on_is_the_typical_hand_gated_sample() {
    use crate::gate_rules::pick::typical_gated;
    use crate::gate_rules::run::measure_many;

    let written = write("pick-typical", &FILES);
    let mut drawn = gates_with(-BIG, 500.0);
    let drawn_at = [("d1", 330.0), ("d2", 700.0), ("d3", 450.0), ("d4", 760.0)];
    for (donor, x0) in drawn_at {
        let cd69: Arc<str> = Arc::from("cd69");
        for kind in ["fmx", "fs"] {
            drawn.place_gate(
                std::slice::from_ref(&cd69),
                &rect("cd69", "CD69+", x0),
                &GateSource::Sample((cd69.clone(), Arc::from(format!("{donor}_{kind}").as_str()))),
            );
        }
    }
    let inputs = inputs(
        &written,
        &FILES,
        store(band(Pool::Specimen, MeasuredOn::Itself)),
    );
    let (measured, _) = measure_many(
        &drawn,
        &inputs.files,
        &inputs.compensation,
        &inputs.names,
        &inputs.cofactors,
        &inputs.metadata,
        std::slice::from_ref(&inputs.rules),
        None,
        &AtomicBool::new(false),
        |_, _| {},
    );
    let mut held: Vec<(f64, String)> = drawn_at
        .iter()
        .map(|(donor, x0)| {
            let file = format!("{donor}_fs");
            (share_above(&file, *x0), file)
        })
        .collect();
    held.sort_by(|a, b| a.0.total_cmp(&b.0));
    let typical = typical_gated(&measured[0].0, &drawn, &inputs.rules, &inputs.metadata);
    assert_eq!(typical.as_deref(), Some(held[1].1.as_str()), "{held:?}");
}
