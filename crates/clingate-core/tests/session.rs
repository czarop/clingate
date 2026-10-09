//! A workspace opened with no window, and asked about by name.
//!
//! The folder holds what an Omiq user's would: two FCS files, a metadata
//! export, a scaling export and a gating file (the checked-in fixture, whose
//! file ids the metadata names). Everything a tool would do goes through
//! [`Session`].

mod common;

use clingate_core::session::{Refusal, Session};
use common::*;
use rand::SeedableRng;
use rand_distr::{Distribution, Normal, Uniform};

const FLUORESCENCE: [&str; 8] = [
    "BUV661-A",
    "BV785-A",
    "Alexa Fluor 700-A",
    "BUV737-A",
    "BUV805-A",
    "BUV563-A",
    "Alexa Fluor 647-A",
    "Vio Bright 423-A",
];

/// Scatter spread evenly, each fluorescence channel a negative and a
/// positive population.
fn events(seed: u64, n: usize) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let scatter = Uniform::new(1_000.0f32, 4_000_000.0).unwrap();
    let negative = Normal::new(0.0f32, 300.0).unwrap();
    let positive = Normal::new(40_000.0f32, 8_000.0).unwrap();
    (0..n)
        .map(|i| {
            let mut row = vec![scatter.sample(&mut rng), scatter.sample(&mut rng)];
            for c in 0..FLUORESCENCE.len() {
                row.push(if (i + c) % 3 == 0 {
                    positive.sample(&mut rng)
                } else {
                    negative.sample(&mut rng)
                });
            }
            row
        })
        .collect()
}

fn workspace(name: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    write_fcs(&dir.join("sample1_FMX.fcs"), &channels, &events(1, 3_000));
    write_fcs(&dir.join("sample2_FS.fcs"), &channels, &events(2, 3_000));
    write_metadata(
        &dir.join("metadata.csv"),
        &["test", "Type", "SampleType"],
        &[
            ("sample1", "sample1_FMX.fcs", &["one", "one", "FMX"]),
            ("sample2", "sample2_FS.fcs", &["two", "two", "FS"]),
        ],
    );
    let mut scaling = vec![
        ("FSC-A", "", "None (linear)", 0, 0, 4_194_304),
        ("SSC-A", "", "None (linear)", 0, 0, 4_194_304),
    ];
    for channel in FLUORESCENCE {
        scaling.push((channel, channel, "Arcsinh", 6000, -2000, 200_000));
    }
    write_scaling(&dir.join("scaling.csv"), &scaling);
    std::fs::copy(
        fixture("quadrant_with_boolean_child.omiqgt"),
        dir.join("gating.omiqgt"),
    )
    .unwrap();
    dir
}

fn clarification(refusal: Refusal) -> clingate_core::session::lookup::Clarification {
    match refusal {
        Refusal::NeedsClarification(c) => c,
        other => panic!("expected a question, got {other:?}"),
    }
}

#[test]
fn a_folder_opens_whole_and_says_what_it_holds() {
    let session = Session::open(&workspace("session-open")).unwrap();
    let overview = session.overview();
    let json = serde_json::to_value(&overview).unwrap();
    for part in ["metadata", "scaling", "gating"] {
        assert_eq!(json["parts"][part]["state"], "loaded", "{part}: {json}");
    }
    assert_eq!(json["parts"]["rules"]["state"], "missing");
    assert_eq!(overview.samples, 2);
    assert!(overview.samples_without_metadata.is_empty());
    assert!(overview.populations > 5);
    let kinds = overview
        .metadata_columns
        .iter()
        .find(|c| c.column == "SampleType")
        .unwrap();
    assert_eq!(kinds.values, ["FMX", "FS"]);
}

#[test]
fn samples_are_found_by_name_or_metadata_and_a_near_miss_is_asked_about() {
    let session = Session::open(&workspace("session-samples")).unwrap();
    let found = session.find_samples("fmx").unwrap();
    assert_eq!(found.samples.len(), 1);
    assert_eq!(found.samples[0].name, "sample1_FMX.fcs");
    assert_eq!(
        session.find_samples("two").unwrap().samples[0].name,
        "sample2_FS.fcs"
    );

    let asked = clarification(session.find_samples("fm").unwrap_err());
    assert!(
        asked.suggestions.iter().any(|s| s.starts_with("fmx")),
        "{asked:?}"
    );
}

#[test]
fn every_population_can_be_asked_for_by_the_name_it_is_listed_under() {
    let session = Session::open(&workspace("session-names")).unwrap();
    let populations = session.populations(None).unwrap();
    assert!(!populations.is_empty());
    for p in populations.iter().filter(|p| !p.name.contains("(id:")) {
        let stats = session.population_stats(&p.name, "fmx");
        match stats {
            Ok(stats) => assert_eq!(stats.population, p.name),
            // A population with nothing above it to count against.
            Err(Refusal::Failed { .. }) => {}
            Err(other) => panic!("{} did not name itself: {other:?}", p.name),
        }
    }
}

#[test]
fn a_population_is_counted_in_every_sample_named() {
    let session = Session::open(&workspace("session-stats")).unwrap();
    let stats = session.population_stats("Tmem", "all").unwrap();
    assert_eq!(stats.rows.len(), 2);
    for row in &stats.rows {
        assert!(row.problem.is_none(), "{row:?}");
        let (total, parent, events) = (
            row.total_events.unwrap(),
            row.parent_events.unwrap(),
            row.events.unwrap(),
        );
        assert_eq!(total, 3_000);
        assert!(events <= parent && parent <= total, "{row:?}");
        let expected = 100.0 * events as f64 / parent as f64;
        assert!((row.percent_of_parent.unwrap() - expected).abs() < 1e-2);
    }
    assert_eq!(stats.summary.unwrap().samples, 2);
}

#[test]
fn a_distribution_accounts_for_every_event_and_places_the_gate() {
    let session = Session::open(&workspace("session-distribution")).unwrap();
    let tmem = session.populations(Some("Tmem")).unwrap();
    let parameter = tmem[0].parameters[0].clone();
    let d = session.distribution("Tmem", "fmx", &parameter).unwrap();
    let h = d.histogram.as_ref().unwrap();
    assert_eq!(h.counts.iter().sum::<usize>() + h.below + h.above, d.events);
    let p = d.percentiles.as_ref().unwrap();
    assert!(p.p1 <= p.p50 && p.p50 <= p.p99);
    assert!(d.gate_on_this_parameter.is_some(), "{d:?}");
}

#[test]
fn one_sample_is_needed_for_a_distribution_and_several_are_asked_about() {
    let session = Session::open(&workspace("session-one-sample")).unwrap();
    let tmem = session.populations(Some("Tmem")).unwrap();
    let parameter = tmem[0].parameters[0].clone();
    let asked = clarification(session.distribution("Tmem", "all", &parameter).unwrap_err());
    assert_eq!(asked.suggestions.len(), 2);
    let asked = clarification(session.distribution("Tmem", "fmx", "nothing").unwrap_err());
    assert_eq!(asked.about, "parameter");
}

#[test]
fn an_unknown_compensation_group_is_asked_about() {
    let mut session = Session::open(&workspace("session-comp")).unwrap();
    let asked = clarification(session.answer_omiq("no such group", None).unwrap_err());
    assert!(!asked.suggestions.is_empty());
}

#[test]
fn parameters_are_listed_with_their_scales_or_named_one_at_a_time() {
    let session = Session::open(&workspace("session-parameters")).unwrap();
    let all = session.parameters(None).unwrap();
    assert_eq!(all.len(), 2 + FLUORESCENCE.len());
    let one = session.parameters(Some("BUV661-A")).unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].channel, "BUV661-A");
    assert!(one[0].scale.contains("arcsinh"), "{:?}", one[0]);
    assert!(one[0].axis_from < one[0].axis_to);
    let asked = clarification(session.parameters(Some("nothing")).unwrap_err());
    assert_eq!(asked.about, "parameter");
}

#[test]
fn a_gate_is_described_as_drawn_and_as_positioned_for_a_sample() {
    let session = Session::open(&workspace("session-gate")).unwrap();
    let drawn = session.gate("Tmem", None).unwrap();
    assert_eq!(drawn.parameters.len(), 2);
    assert!(drawn.position.starts_with("as drawn"), "{drawn:?}");
    assert!(drawn.geometry.is_some());
    assert!(
        drawn
            .extent
            .iter()
            .any(|e| e.lower.is_some() || e.upper.is_some()),
        "{drawn:?}"
    );
    let for_sample = session.gate("Tmem", Some("fmx")).unwrap();
    assert_eq!(for_sample.sample.as_deref(), Some("sample1_FMX.fcs"));
    // Several samples where one is needed is a question.
    clarification(session.gate("Tmem", Some("all")).unwrap_err());
}

#[test]
fn samples_are_compared_on_a_parameter_against_the_plate() {
    let session = Session::open(&workspace("session-compare")).unwrap();
    let parameter = session.gate("Tmem", None).unwrap().parameters[0].clone();
    let compared = session.compare_samples("Tmem", &parameter, "all").unwrap();
    assert_eq!(compared.rows.len(), 2);
    assert!(compared.plate_median.is_some() && compared.typical_iqr.is_some());
    for row in &compared.rows {
        assert!(row.problem.is_none(), "{row:?}");
        let (p5, median, p95) = (row.p5.unwrap(), row.median.unwrap(), row.p95.unwrap());
        assert!(p5 <= median && median <= p95, "{row:?}");
        assert!(row.shift_in_iqrs.is_some() && row.spread_ratio.is_some());
        assert!(row.gate.is_some(), "{row:?}");
    }
}

/// The workspace with one rule: Tmem keeps the top 1-2% of its parent on its
/// first parameter, measured on each sample itself.
fn with_rules(name: &str) -> std::path::PathBuf {
    with_rule(
        name,
        clingate_core::gate_rules::rule::Rule::TailFraction(
            clingate_core::gate_rules::rule::TailFractionRule::new((0.01, 0.02)),
        ),
    )
}

/// The same, with another rule.
fn with_rule(name: &str, rule: clingate_core::gate_rules::rule::Rule) -> std::path::PathBuf {
    rule_in(workspace(name), rule)
}

/// `dir` with one rule for Tmem, on its first parameter, read on each sample
/// itself.
fn rule_in(
    dir: std::path::PathBuf,
    rule: clingate_core::gate_rules::rule::Rule,
) -> std::path::PathBuf {
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let parameter = Session::open(&dir)
        .unwrap()
        .gate("Tmem", None)
        .unwrap()
        .parameters[0]
        .clone();
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named("Tmem"),
        GateRule {
            parameter: parameter.into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule,
        },
    );
    store
        .save(&clingate_core::workspace::rules_file(&dir))
        .unwrap();
    dir
}

/// Where Tmem's gate edge is on the sample `fmx` - what a rules run moves.
fn tmem_edge(session: &Session) -> f64 {
    session.gate("Tmem", Some("fmx")).unwrap().extent[0]
        .lower
        .unwrap()
}

#[test]
fn rules_are_listed_and_a_workspace_without_them_says_so() {
    let session = Session::open(&workspace("session-no-rules")).unwrap();
    assert!(matches!(
        session.rules_view().unwrap_err(),
        Refusal::Failed { .. }
    ));

    let session = Session::open(&with_rules("session-rules-view")).unwrap();
    let view = session.rules_view().unwrap();
    assert_eq!(view.rules.len(), 1);
    assert_eq!(view.rules[0].population, "Tmem");
    assert_eq!(view.rules[0].keeps, "above the line");
    assert_eq!(view.specimen_column, "test");
}

#[test]
fn a_preview_moves_nothing_until_applied_and_applies_once() {
    let mut session = Session::open(&with_rules("session-rules-apply")).unwrap();
    let before = session.population_stats("Tmem", "all").unwrap();

    // Nothing to apply before a preview.
    assert!(session.apply_previewed_rules().is_err());

    let preview = session.preview_rules().unwrap();
    assert_eq!(preview.would_move.len(), 2, "{preview:?}");
    for m in &preview.would_move {
        assert!(m.from != m.to, "{m:?}");
    }
    let unmoved = session.population_stats("Tmem", "all").unwrap();
    for (a, b) in before.rows.iter().zip(&unmoved.rows) {
        assert_eq!(a.events, b.events, "a preview moved a gate");
    }

    session.apply_previewed_rules().unwrap();
    // Each specimen's gate edge is where the preview said it would go.
    for (query, specimen) in [("fmx", "one"), ("fs", "two")] {
        let proposed = preview
            .would_move
            .iter()
            .find(|m| m.specimen == specimen)
            .unwrap();
        let placed = session.gate("Tmem", Some(query)).unwrap();
        assert!(!placed.position.starts_with("as drawn"), "{placed:?}");
        let lower = placed.extent[0].lower.unwrap();
        assert!(
            (lower - proposed.to).abs() < 1e-3,
            "{placed:?} {proposed:?}"
        );
    }
    // The preview is used up.
    assert!(session.apply_previewed_rules().is_err());
}

#[test]
fn the_saved_copy_is_exported_by_name_into_the_folder_and_never_over_a_file_unasked() {
    let folder = workspace("session-export");
    let session = Session::open(&folder).unwrap();
    for bad in [
        "",
        "../escape",
        "sub/dir.omiqgt",
        ".hidden",
        "clingate_gating",
    ] {
        assert!(session.export(bad, false).is_err(), "{bad:?}");
    }
    let exported = session.export("claude", false).unwrap();
    assert_eq!(exported.file, folder.join("claude.omiqgt"));
    assert!(!exported.unsaved_changes_left_out);
    assert!(session.export("claude.omiqgt", false).is_err());
    session.export("claude.omiqgt", true).unwrap();
    // What was written reads back as a gating file.
    let text = std::fs::read_to_string(&exported.file).unwrap();
    let _: serde_json::Value = serde_json::from_str(&text).unwrap();
}

#[test]
fn a_rules_run_is_one_step_undone_redone_saved_and_opened_again() {
    let folder = with_rules("session-working-copy");
    let mut session = Session::open(&folder).unwrap();
    let drawn = tmem_edge(&session);
    assert!(!session.edit_state().unsaved_changes);
    assert!(session.undo().is_err(), "nothing to undo yet");

    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let placed = tmem_edge(&session);
    assert_ne!(placed, drawn);
    let state = session.edit_state();
    assert!(state.unsaved_changes && state.undo_steps == 1);
    assert!(
        folder.join(".clingate_recovery.omiqgt").is_file(),
        "kept for a crash"
    );

    session.undo().unwrap();
    assert_eq!(tmem_edge(&session), drawn);
    assert!(
        !session.edit_state().unsaved_changes,
        "back to the saved copy"
    );
    assert!(!folder.join(".clingate_recovery.omiqgt").exists());
    session.redo().unwrap();
    assert_eq!(tmem_edge(&session), placed);

    // An export is the saved copy, not the working one.
    let exported = session.export("before-save", false).unwrap();
    assert!(exported.unsaved_changes_left_out);

    let saved = session.save().unwrap();
    assert_eq!(saved.gating_file, folder.join("clingate_gating.omiqgt"));
    assert!(!saved.edits.unsaved_changes);
    assert!(session.revert().is_err(), "nothing unsaved to revert");

    // Opened again, the folder opens on the save.
    let reopened = Session::open(&folder).unwrap();
    assert_eq!(tmem_edge(&reopened), placed);
    assert!(!reopened.edit_state().unsaved_changes);
}

#[test]
fn unsaved_changes_left_behind_are_offered_back_and_revert_is_undoable() {
    let folder = with_rules("session-recovery");
    {
        let mut session = Session::open(&folder).unwrap();
        session.preview_rules().unwrap();
        session.apply_previewed_rules().unwrap();
        // Dropped without saving - a close, or a crash.
    }
    let mut session = Session::open(&folder).unwrap();
    assert!(session.edit_state().earlier_unsaved_changes);
    let drawn = tmem_edge(&session);
    session.restore_unsaved_changes().unwrap();
    let state = session.edit_state();
    assert!(state.unsaved_changes && !state.earlier_unsaved_changes);
    let restored = tmem_edge(&session);
    assert_ne!(restored, drawn);

    session.revert().unwrap();
    assert_eq!(tmem_edge(&session), drawn);
    session.undo().unwrap();
    assert_eq!(tmem_edge(&session), restored, "the revert itself is undone");
}

#[test]
fn a_workspace_the_app_saved_in_the_folder_opens_as_it_was_left() {
    use clingate_core::gate_rules::rule_store::SamplePairing;
    use clingate_core::workspace::Remembered;
    let dir = with_rules("session-saved-workspace");
    // Only the first sample was in the workspace; a stray file arrived since.
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    write_fcs(&dir.join("stray.fcs"), &channels, &events(3, 100));
    Remembered {
        folder: Some(dir.clone()),
        fcs: vec![dir.join("sample1_FMX.fcs")],
        metadata: Some(dir.join("metadata.csv")),
        scaling: Some(dir.join("scaling.csv")),
        gating: Some(dir.join("gating.omiqgt")),
        compensation: None,
        pairing: Some(SamplePairing {
            sample_id_column: "Type".into(),
            ..SamplePairing::default()
        }),
    }
    .save_into_folder()
    .unwrap();

    let session = Session::open(&dir).unwrap();
    let overview = serde_json::to_value(session.overview()).unwrap();
    assert_eq!(
        overview["parts"]["workspace"]["state"], "loaded",
        "{overview}"
    );
    assert_eq!(overview["samples"], 1, "{overview}");
    assert_eq!(overview["parts"]["gating"]["state"], "loaded");
    // The grouping as the app left it, over the rules file's own.
    assert_eq!(session.rules_view().unwrap().specimen_column, "Type");
}

#[test]
fn an_applied_run_is_kept_for_review_and_knows_when_a_gate_has_moved_since() {
    use clingate_core::review::run_record::PlacementStatus;
    use clingate_core::review::{RunRecord, placement_status};
    let folder = with_rules("session-run-record");
    let mut session = Session::open(&folder).unwrap();
    assert!(RunRecord::load(&folder).unwrap().is_none());

    // A preview keeps nothing: only an applied run is a run.
    session.preview_rules().unwrap();
    assert!(RunRecord::load(&folder).unwrap().is_none());
    session.apply_previewed_rules().unwrap();

    let record = RunRecord::load(&folder).unwrap().expect("kept on apply");
    assert!(folder.join("reviews").join("rules_run.json").is_file());
    assert_eq!(record.placed.len(), 2, "{record:#?}");
    assert_eq!(record.rules, *session.rules().unwrap());
    for placed in &record.placed {
        assert_eq!(placed.gate, "Tmem");
        // Every measure the confidence came from, not only the weakest.
        assert!(placed.components.len() > 1, "{placed:#?}");
        assert!(placed.weakest.is_some());
        // Named and typed from the metadata.
        assert!(placed.sample.name.is_some());
        assert!(placed.sample.sample_type.is_some());
        assert_eq!(placed.specimen_column, "test");
        assert_eq!(
            placement_status(placed, session.gates(), session.metadata().metadata()),
            PlacementStatus::AsPlaced
        );
    }

    // Undone, the gates are no longer where the run put them.
    session.undo().unwrap();
    for placed in &record.placed {
        assert_eq!(
            placement_status(placed, session.gates(), session.metadata().metadata()),
            PlacementStatus::Moved
        );
    }
}

#[test]
fn a_bad_placement_is_reported_its_fix_recorded_on_save_and_the_run_reviewed() {
    use clingate_core::review::report::{Decision, reports_in};
    let folder = with_rules("session-report");
    let library = scratch("session-report-library");
    let mut session = Session::open(&folder).unwrap();
    session.set_review_library(Some(library.clone()));
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();

    // The problem is one of the listed ones.
    assert!(
        session
            .report_placement("Tmem", "fmx", "wonky", "")
            .is_err()
    );
    let reported = session
        .report_placement("Tmem", "fmx", "too high", "misses the dim positives")
        .unwrap();
    assert!(
        reported.rule_did.starts_with("the rule moved it"),
        "{reported:?}"
    );
    assert!(reported.events_kept > 0);

    let reports = reports_in(&folder);
    assert_eq!(reports.len(), 1);
    let report = &reports[0].1;
    assert_eq!(report.gate, "Tmem");
    assert_eq!(report.note, "misses the dim positives");
    assert!(matches!(report.decision, Decision::Placed(_)));
    assert!(report.rule.is_some(), "the rule as it stood");
    assert_eq!(report.data.histograms.0.counts.len(), 64);
    assert_eq!(
        report.data.histograms.0.counts.iter().sum::<u32>() as usize,
        report.data.events,
        "every event is in the histogram"
    );
    assert!(report.data.events_subsample.len() <= 5_000);
    let kept = report
        .data
        .gate
        .as_ref()
        .expect("the gate's outline is kept");
    assert_eq!(&*kept.name, "Tmem");
    for at in &report.data.gate_at {
        let (lower, upper) =
            clingate_core::gate_rules::autogate::extent_on(&kept.geometry, &at.parameter).unwrap();
        assert_eq!(
            (Some(f64::from(lower)), Some(f64::from(upper))),
            (at.lower, at.upper),
            "{}",
            at.parameter
        );
    }
    assert!(report.correction.is_none(), "not fixed yet");

    // Reviewed: the reported one, and the other accepted as placed.
    let reviewed = session.mark_run_reviewed().unwrap();
    assert_eq!(
        (reviewed.reported, reviewed.accepted),
        (1, 1),
        "{reviewed:?}"
    );
    let copy = reviewed.library_copy.expect("copied into the library");
    assert!(copy.join("review.json").is_file());
    // With the events behind every placement, the accepted one too.
    let run = clingate_core::review::RunRecord::load(&folder)
        .unwrap()
        .unwrap();
    let here = clingate_core::review::events::load(&folder, &run.applied_at)
        .unwrap()
        .expect("kept on apply")
        .samples;
    assert!(
        here.len() >= run.placed.len(),
        "{} for {}",
        here.len(),
        run.placed.len()
    );
    for placed in &run.placed {
        assert!(
            here.iter()
                .any(|e| e.gate_id == placed.gate_id && e.file == placed.sample.id),
            "no events for {} on {}",
            placed.gate,
            placed.sample.id
        );
    }
    // In the library as a list of populations in its pool - read back, the
    // very events the workspace kept.
    assert!(copy.join("run_events.json").is_file());
    assert!(!copy.join("run_events.bin").exists());
    let pool = clingate_core::review::events::Pool::in_library(&library);
    assert_eq!(pool.len(), here.len());
    let from_library = clingate_core::review::events::load_from_library(&copy)
        .unwrap()
        .expect("in the library");
    assert_eq!(
        Some(from_library),
        clingate_core::review::events::load(&folder, &run.applied_at).unwrap()
    );
    assert_eq!(std::fs::read_dir(copy.join("reports")).unwrap().count(), 1);

    // The reviewer's fix - here, taking the run back - is the correction once
    // saved; and the other gate, moved with no report, is counted as such.
    session.undo().unwrap();
    session.save().unwrap();
    let report = &reports_in(&folder)[0].1;
    let fixed = report.correction.as_ref().expect("recorded on save");
    assert_ne!(fixed.gate_at, report.data.gate_at);
    let reviewed = session.mark_run_reviewed().unwrap();
    assert_eq!(
        (
            reviewed.reported,
            reviewed.accepted,
            reviewed.moved_without_a_report
        ),
        (1, 0, 1)
    );
}

#[test]
fn a_kept_run_is_assessed_and_a_sample_compared_with_its_peers() {
    let folder = with_rules("session-assess");
    let mut session = Session::open(&folder).unwrap();
    // Nothing to assess before a run is applied.
    assert!(session.assess_run().is_err());
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();

    let assessed = session.assess_run().unwrap();
    assert_eq!(assessed.placements, 2);
    assert_eq!(assessed.gates.len(), 1);
    assert_eq!(assessed.gates[0].gate, "Tmem");
    // Two samples, one of each kind: nothing to compare with, so only what
    // the rule itself said can raise a flag.
    for flag in &assessed.flags {
        assert!(
            flag.reasons
                .iter()
                .all(|r| r.measure == "low_confidence" || r.measure == "outside_band"),
            "{flag:?}"
        );
    }

    // Every placement is on the board, in one pile or another.
    let on_board: usize = assessed.piles.iter().map(|(_, n)| n).sum();
    assert_eq!(on_board, assessed.placements);

    // Looks right needs a placement the run made.
    assert!(session.mark_looks_right("Tmem", "fmx", true).is_ok());
    assert!(session.mark_looks_right("Tmem", "fmx", false).is_ok());

    let compared = session.compare_to_peers("Tmem", "fmx").unwrap();
    assert!(compared.peers.is_empty(), "no other FMX: {compared:?}");
    assert!(compared.line.is_some());
    assert_eq!(compared.percentiles.len(), 9);
    assert!(
        compared
            .percentiles
            .iter()
            .all(|(_, here, _)| here.is_some())
    );
}

#[test]
fn a_report_before_any_run_says_so_and_bad_requests_are_refused() {
    let folder = with_rules("session-report-before");
    let session = Session::open(&folder).unwrap();
    let reported = session
        .report_placement("Tmem", "fmx", "too_loose", "")
        .unwrap();
    assert!(reported.rule_did.contains("no rules run"), "{reported:?}");
    assert_eq!(reported.problem, "too loose");
    assert_eq!(reported.reference_events_kept, None);
    assert!(reported.events_kept > 0 && reported.events_kept <= 3_000);
    assert!(
        reported
            .file
            .starts_with(folder.join("reviews").join("reports"))
    );
    assert!(reported.file.is_file());
    // The histogram runs along the axis the scaling gives the parameter.
    let report = &clingate_core::review::report::reports_in(&folder)[0].1;
    let h = &report.data.histograms.0;
    let parameter = session.gate("Tmem", None).unwrap().parameters[0].clone();
    assert_eq!(h.parameter, parameter);
    // In the axis's display units: arcsinh with cofactor 6000.
    let shown = |v: f64| (v / 6000.0).asinh();
    assert!(
        (h.lower - shown(-2000.0)).abs() < 1e-5,
        "{} {parameter}",
        h.lower
    );
    assert!(
        (h.upper - shown(200_000.0)).abs() < 1e-5,
        "{} {parameter}",
        h.upper
    );

    // No such population, no such sample, no such problem.
    assert!(
        session
            .report_placement("CD999+", "fmx", "too_high", "")
            .is_err()
    );
    assert!(
        session
            .report_placement("Tmem", "sample9", "too_high", "")
            .is_err()
    );
    assert!(session.report_placement("Tmem", "fmx", "", "").is_err());
    assert_eq!(
        clingate_core::review::report::reports_in(&folder).len(),
        1,
        "nothing more kept"
    );
}

/// Apply the rules, then rewrite the kept run's confidences - which placements
/// the rule was unsure of is the test's to choose.
fn applied_with_confidence(
    name: &str,
    confidence: impl Fn(&str) -> f64,
) -> (std::path::PathBuf, Session) {
    use clingate_core::review::RunRecord;
    let folder = with_rules(name);
    let mut session = Session::open(&folder).unwrap();
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let mut record = RunRecord::load(&folder).unwrap().unwrap();
    for p in &mut record.placed {
        p.confidence = confidence(p.sample.name.as_deref().unwrap_or(""));
    }
    record.save(&folder).unwrap();
    (folder, session)
}

fn pile(
    assessed: &clingate_core::session::RunAssessment,
    pile: clingate_core::review::board::Pile,
) -> usize {
    assessed.piles.iter().find(|(p, _)| *p == pile).unwrap().1
}

#[test]
fn looks_right_moves_a_flagged_placement_to_passed_and_back() {
    use clingate_core::review::board::Pile;
    let (_, session) = applied_with_confidence("session-looks-right", |name| {
        if name.contains("FMX") { 0.05 } else { 0.9 }
    });
    let before = session.assess_run().unwrap();
    let is_fmx = |f: &clingate_core::review::assess::Flag| {
        f.sample.name.as_deref().is_some_and(|n| n.contains("FMX"))
    };
    let fmx = before
        .flags
        .iter()
        .find(|f| is_fmx(f))
        .expect("the unsure FMX is flagged");
    assert_eq!(fmx.reasons[0].measure, "low_confidence", "{fmx:?}");
    let needing = pile(&before, Pile::NeedsALook);

    session.mark_looks_right("Tmem", "fmx", true).unwrap();
    let after = session.assess_run().unwrap();
    assert_eq!(pile(&after, Pile::NeedsALook), needing - 1);
    assert_eq!(pile(&after, Pile::Passed), pile(&before, Pile::Passed) + 1);
    assert!(!after.flags.iter().any(|f| is_fmx(f)), "{after:?}");

    session.mark_looks_right("Tmem", "fmx", false).unwrap();
    let back = session.assess_run().unwrap();
    assert_eq!(pile(&back, Pile::NeedsALook), needing);
    assert!(back.flags.iter().any(|f| is_fmx(f)));
}

/// Five samples with their gate 1 IQR above their median, and one with its
/// gate 3.5 IQRs above: 2.5 IQRs from its peers, past the limit of 2. Read on
/// an FMX of 600 events and in its band, it is trusted for its low
/// confidence - but not for where its gate sits.
#[test]
fn a_gate_far_from_its_peers_is_flagged_even_when_read_on_a_trusted_fmx() {
    use clingate_core::review::RunRecord;
    let (folder, session) = applied_with_confidence("session-far-from-peers", |_| 0.9);
    let mut record = RunRecord::load(&folder).unwrap().unwrap();
    let model = record
        .placed
        .iter()
        .find(|p| p.shape.is_some())
        .expect("a placement with its population kept")
        .clone();
    let shape = model.shape.clone().unwrap();
    let at = |iqrs: f64| shape.median() + iqrs * shape.iqr();
    let named = |name: &str, line: f64| {
        let mut p = model.clone();
        p.sample.id = name.into();
        p.sample.name = Some(name.into());
        p.to = Some(line);
        p
    };
    // The run's own sample, so the board finds its gate as placed.
    let mut far = model.clone();
    far.to = Some(at(3.5));
    (far.confidence, far.read_on_control, far.reference_events, far.in_band) =
        (0.1, true, 600, true);
    record.placed = (0..5)
        .map(|n| named(&format!("peer{n}"), at(1.0)))
        .chain([far])
        .collect();
    record.save(&folder).unwrap();

    let assessed = session.assess_run().unwrap();
    assert_eq!(assessed.flags.len(), 1, "{:#?}", assessed.flags);
    let flag = &assessed.flags[0];
    assert_eq!(flag.sample.id, model.sample.id);
    let measures: Vec<&str> = flag.reasons.iter().map(|r| r.measure).collect();
    assert_eq!(measures, ["gate_position"]);
    assert!(
        flag.reasons[0].says.contains("+2.50 IQRs from them"),
        "{}",
        flag.reasons[0].says
    );
}

#[test]
fn looks_right_and_peer_comparison_need_a_placement_the_run_made() {
    use clingate_core::review::RunRecord;
    let (folder, session) = applied_with_confidence("session-not-in-run", |_| 0.9);
    let mut record = RunRecord::load(&folder).unwrap().unwrap();
    record
        .placed
        .retain(|p| !p.sample.name.as_deref().unwrap().contains("FMX"));
    record.save(&folder).unwrap();
    let e = format!(
        "{:?}",
        session.mark_looks_right("Tmem", "fmx", true).unwrap_err()
    );
    assert!(e.contains("did not place or keep"), "{e}");
    assert!(!folder.join("reviews").join("looks_right.json").exists());
    let e = format!("{:?}", session.compare_to_peers("Tmem", "fmx").unwrap_err());
    assert!(e.contains("did not place or keep"), "{e}");
    assert!(session.compare_to_peers("CD999+", "fs").is_err());
    // The one it did make is still compared.
    assert!(session.compare_to_peers("Tmem", "fs").is_ok());
}

#[test]
fn the_tools_are_handed_at_most_forty_flags_and_the_piles_count_them_all() {
    use clingate_core::review::RunRecord;
    use clingate_core::review::board::Pile;
    use clingate_core::review::run_record::extent_of;
    let (folder, session) = applied_with_confidence("session-forty", |_| 0.9);
    // The fixture's own placements miss their band, so are flagged already.
    let already = pile(&session.assess_run().unwrap(), Pile::NeedsALook);
    let mut record = RunRecord::load(&folder).unwrap().unwrap();
    // 45 placements on samples no metadata row names, each where the gate's
    // own position is, each unsure.
    let template = record.placed[0].clone();
    let gate_id: std::sync::Arc<str> = std::sync::Arc::from(template.gate_id.as_str());
    let at = extent_of(session.gates().registered_gate(&gate_id).unwrap().as_ref());
    for i in 0..45 {
        let mut p = template.clone();
        p.sample.id = format!("unnamed-{i}");
        p.placed_at = at.clone();
        p.confidence = 0.01;
        record.placed.push(p);
    }
    record.save(&folder).unwrap();
    let assessed = session.assess_run().unwrap();
    assert_eq!(
        pile(&assessed, Pile::NeedsALook),
        already + 45,
        "{:?}",
        assessed.piles
    );
    assert_eq!(assessed.flags.len(), clingate_core::session::FLAGS_SHOWN);
    assert_eq!(assessed.placements, 47);
}

#[test]
fn a_run_reviewed_with_no_library_is_kept_in_the_workspace_only() {
    let (folder, mut session) = applied_with_confidence("session-no-library", |_| 0.9);
    session.set_review_library(None);
    let reviewed = session.mark_run_reviewed().unwrap();
    assert!(reviewed.library_copy.is_none());
    assert_eq!(
        reviewed.review_file,
        folder.join("reviews").join("review.json")
    );
    assert!(reviewed.review_file.is_file());
    assert_eq!(
        (reviewed.accepted, reviewed.reported, reviewed.left_alone),
        (2, 0, 0)
    );
}

/// A rules run applied, the gate on `fmx` reported too high and taken back,
/// saved and marked reviewed into a library: the run a replay reads. Returns
/// the session, where Tmem's edge sat before the run, and the library.
///
/// The rule puts the line half a unit below the median of the parent - the
/// slanted gate's boundary then runs left of the negatives, which are two
/// thirds of it, and takes them in; the gate as drawn sits in the gap
/// between the negatives and the positives, holding the positives alone,
/// and that is where the reviewer puts it back.
fn reviewed_run(name: &str) -> (Session, f64, std::path::PathBuf) {
    reviewed_run_in(rule_in(tmem_workspace(name), percentile(-0.5)), name)
}

/// [`reviewed_run`], of the rules in `folder`.
fn reviewed_run_in(folder: std::path::PathBuf, name: &str) -> (Session, f64, std::path::PathBuf) {
    let library = scratch(&format!("{name}-library"));
    let mut session = Session::open(&folder).unwrap();
    session.set_review_library(Some(library.clone()));
    let before = tmem_edge(&session);
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    session
        .report_placement("Tmem", "fmx", "too_high", "the old gate was right")
        .unwrap();
    session.undo().unwrap();
    session.save().unwrap();
    session.mark_run_reviewed().unwrap();
    (session, before, library)
}

/// The workspace, with events Tmem's gate reaches. The gate is a slanted
/// polygon on BUV805-A and BUV563-A, well up BUV563-A; here every event sits
/// there on BUV563-A, and on BUV805-A a third are positive - inside the gate
/// as drawn - and the rest negative, to the left of it.
fn tmem_workspace(name: &str) -> std::path::PathBuf {
    let dir = tmem_workspace_beside_teff_naive(name);
    out_of_tmems_way(&dir.join("gating.omiqgt"));
    dir
}

/// [`tmem_workspace`] with teff_naive where it is drawn: beside Tmem on its
/// plot, to the left of it on BUV805-A.
fn tmem_workspace_beside_teff_naive(name: &str) -> std::path::PathBuf {
    tmem_workspace_positive_one_in(name, [(20_000, 3), (20_000, 3)])
}

/// [`tmem_workspace_beside_teff_naive`], with sample1 and sample2 each
/// `(events, one_in)`: BUV805-A positive on one event in `one_in`, a multiple
/// of 3, so those events are all positive on BUV661-A and negative on
/// BV785-A too.
fn tmem_workspace_positive_one_in(name: &str, files: [(usize, usize); 2]) -> std::path::PathBuf {
    let [(first, one_in_first), (second, one_in_second)] = files;
    tmem_workspace_of(
        name,
        [
            (first, one_in_first, 40_000.0, (0.0, 300.0)),
            (second, one_in_second, 40_000.0, (0.0, 300.0)),
        ],
    )
}

/// [`tmem_workspace_positive_one_in`], with each file's BUV805-A positives
/// centred where its third number says and its negative centred and as wide
/// as its fourth.
fn tmem_workspace_of(
    name: &str,
    files: [(usize, usize, f32, (f32, f32)); 2],
) -> std::path::PathBuf {
    let dir = workspace(name);
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    let at = |channel: &str| 2 + FLUORESCENCE.iter().position(|c| *c == channel).unwrap();
    for ((seed, file), (count, every, positive, negative)) in
        [(11, "sample1_FMX.fcs"), (12, "sample2_FS.fcs")]
            .into_iter()
            .zip(files)
    {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let high = Normal::new(400_000.0f32, 40_000.0).unwrap();
        let rows: Vec<Vec<f32>> = events(seed, count)
            .into_iter()
            .enumerate()
            .map(|(i, mut row)| {
                row[at("BUV563-A")] = high.sample(&mut rng);
                row[at("BUV805-A")] = if i % every == 0 {
                    Normal::new(positive, positive / 5.0)
                        .unwrap()
                        .sample(&mut rng)
                } else {
                    Normal::new(negative.0, negative.1)
                        .unwrap()
                        .sample(&mut rng)
                };
                row
            })
            .collect();
        write_fcs(&dir.join(file), &channels, &rows);
    }
    dir
}

/// teff_naive moved down BUV563-A, below Tmem, so nothing beside Tmem on its
/// plot holds it back wherever a rule slides it along BUV805-A.
fn out_of_tmems_way(gating: &std::path::Path) {
    fn lower(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, field) in fields.iter_mut() {
                    match (key.as_str(), field.as_f64()) {
                        ("f2Val", Some(v)) => *field = serde_json::json!(v - 7.0),
                        _ => lower(field),
                    }
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(lower),
            _ => {}
        }
    }
    fn find(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields)
                if fields.get("name") == Some(&"teff_naive".into()) =>
            {
                lower(value)
            }
            serde_json::Value::Object(fields) => fields.values_mut().for_each(find),
            serde_json::Value::Array(items) => items.iter_mut().for_each(find),
            _ => {}
        }
    }
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(gating).unwrap()).unwrap();
    find(&mut json);
    std::fs::write(gating, serde_json::to_vec(&json).unwrap()).unwrap();
}

/// The line `offset` above the median of the parent.
fn percentile(offset: f64) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::PercentileOffset(
        clingate_core::gate_rules::rule::PercentileOffsetRule::new(50.0, offset),
    )
}

fn band(band: (f64, f64)) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::TailFraction(
        clingate_core::gate_rules::rule::TailFractionRule::new(band),
    )
}

/// The Tmem rule, done another way.
fn tmem_rule(
    session: &Session,
    how: clingate_core::gate_rules::rule::Rule,
) -> clingate_core::review::replay::RuleChange {
    use clingate_core::gate_rules::rule_store::RuleTarget;
    let target = RuleTarget::named("Tmem");
    let mut rule = session.rules().unwrap().get(&target).unwrap().clone();
    rule.rule = how;
    clingate_core::review::replay::RuleChange { target, rule }
}

#[test]
fn a_reviewed_run_is_replayed_with_its_rules_and_with_a_change_that_fixes_it() {
    use clingate_core::review::replay::{Truth, Verdict, close_enough};
    use clingate_core::session::ReplayScope;
    let (session, before, _) = reviewed_run("session-replay");

    // With the rules it ran with: the run is reproduced, and wrong on both
    // samples - the reviewer took both gates back to where they were.
    let as_run = session
        .replay_rules(&[], ReplayScope::Both, None, None)
        .unwrap();
    assert_eq!(as_run.runs.len(), 1, "the library copy is the same run");
    assert!(!as_run.runs[0].provisional);
    assert!(as_run.unreadable.is_empty(), "{:?}", as_run.unreadable);
    assert!(as_run.changes_tried.is_empty());
    assert_eq!(as_run.cases_total, 2);
    assert_eq!(
        as_run.totals.get(&Verdict::StillWrong),
        Some(&2),
        "{as_run:?}"
    );
    let fmx = as_run
        .cases
        .iter()
        .find(|c| c.sample.contains("FMX"))
        .unwrap();
    assert!(
        matches!(&fmx.truth, Truth::Corrected { note, .. } if note == "the old gate was right"),
        "{fmx:?}"
    );
    let fs = as_run
        .cases
        .iter()
        .find(|c| c.sample.contains("FS"))
        .unwrap();
    assert_eq!(fs.truth, Truth::MovedByHand);
    for case in &as_run.cases {
        assert!((case.right_at.unwrap() - before).abs() < 1e-3, "{case:?}");
        assert_eq!(
            case.baseline_holds, case.replay_holds,
            "no change, same replay"
        );
        let kept = 5_000;
        assert!(
            close_enough(case.baseline_holds.unwrap(), case.run_holds.unwrap(), kept),
            "reproduced: {case:?}"
        );
        // The run put the line left of the negatives, so the gate took them in.
        assert!(case.run_at.unwrap() < case.right_at.unwrap(), "{case:?}");
        assert!(
            case.run_holds.unwrap() > case.right_holds.unwrap() + 0.05,
            "{case:?}"
        );
        assert!(!close_enough(
            case.run_holds.unwrap(),
            case.right_holds.unwrap(),
            kept
        ));
        assert!(case.notes.is_empty(), "{case:?}");
        assert_eq!(case.earlier_reviews, 0);
    }
    // By gate, named with its parent.
    assert_eq!(as_run.by_gate.len(), 1, "{:?}", as_run.by_gate);
    let (named, counts) = as_run.by_gate.iter().next().unwrap();
    assert!(named.starts_with("Tmem of "), "{named}");
    assert_eq!(counts.get(&Verdict::StillWrong), Some(&2));

    // A step up off the median puts the line in the gap - where the reviewer
    // put it - on both; and the change says what it replaced.
    let change = tmem_rule(&session, percentile(1.0));
    let changed = session
        .replay_rules(std::slice::from_ref(&change), ReplayScope::Both, None, None)
        .unwrap();
    assert_eq!(changed.changes_tried.len(), 1);
    assert!(
        changed.changes_tried[0].starts_with("Tmem: ")
            && changed.changes_tried[0].contains("50th percentile of the negative by 1 ")
            && changed.changes_tried[0]
                .contains("below the 50th percentile of the negative by 0.5 [Percentile offset])"),
        "{:?}",
        changed.changes_tried
    );
    assert_eq!(changed.totals.get(&Verdict::Fixed), Some(&2), "{changed:?}");
    for case in &changed.cases {
        assert!(
            close_enough(case.replay_holds.unwrap(), case.right_holds.unwrap(), 5_000),
            "{case:?}"
        );
    }
}

#[test]
fn a_replay_is_narrowed_to_a_gate_and_a_number_of_cases_and_read_from_the_library_alone() {
    use clingate_core::review::replay::Verdict;
    use clingate_core::session::ReplayScope;
    let (session, _, _) = reviewed_run("session-replay-narrow");

    let one = session
        .replay_rules(&[], ReplayScope::Both, Some("tmem"), Some(1))
        .unwrap();
    assert_eq!((one.cases.len(), one.cases_total), (1, 2));
    // Totals count every case, not only those listed.
    assert_eq!(one.totals.values().sum::<usize>(), 2);

    let refused = format!(
        "{:?}",
        session
            .replay_rules(&[], ReplayScope::Both, Some("CD8+"), None)
            .unwrap_err()
    );
    assert!(refused.contains("the gates are: Tmem"), "{refused}");

    // The library holds the reviewed copy, under its own folder's name.
    let library = session
        .replay_rules(&[], ReplayScope::Library, None, None)
        .unwrap();
    assert_eq!(library.runs.len(), 1);
    let folder_name = session
        .folder()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_ne!(library.runs[0].run, folder_name);
    assert_eq!(library.totals.get(&Verdict::StillWrong), Some(&2));
    let workspace = session
        .replay_rules(&[], ReplayScope::Workspace, None, None)
        .unwrap();
    assert_eq!(workspace.runs[0].run, folder_name);
    assert!(
        workspace.cases[0]
            .case
            .starts_with(&format!("{folder_name}|")),
        "{:?}",
        workspace.cases[0]
    );
}

#[test]
fn one_case_is_shown_in_full_with_the_population_and_every_line() {
    use clingate_core::session::ReplayScope;
    let (session, before, _) = reviewed_run("session-replay-case");
    let change = tmem_rule(&session, band((0.05, 0.06)));
    let listed = session
        .replay_rules(
            std::slice::from_ref(&change),
            ReplayScope::Workspace,
            None,
            None,
        )
        .unwrap();
    let line = listed
        .cases
        .iter()
        .find(|c| c.sample.contains("FMX"))
        .unwrap();

    let detail = session
        .replay_case(
            std::slice::from_ref(&change),
            ReplayScope::Workspace,
            &line.case,
        )
        .unwrap();
    assert_eq!(detail.case.verdict, line.verdict);
    assert_eq!(
        detail
            .case
            .replay_decided
            .as_ref()
            .unwrap()
            .line
            .holds
            .map(|b| (b * 1e6).round() / 1e6),
        line.replay_holds
    );
    // The run's line below the negatives: nearly all the population past it
    // on the parameter alone; the gate's slanted far side holds some back.
    let past = detail.case.run_decided.line.beyond.unwrap();
    assert!(past > 0.9, "{past}");
    assert!(detail.case.run_decided.line.holds.unwrap() < past);
    assert!(
        detail
            .rule_in_run
            .as_deref()
            .unwrap()
            .contains("50th percentile"),
        "{:?}",
        detail.rule_in_run
    );
    assert!(
        detail
            .rule_replayed
            .as_deref()
            .unwrap()
            .contains("capture 5.000% to 6.000%"),
        "{:?}",
        detail.rule_replayed
    );
    assert!(
        detail
            .rule_in_run
            .as_deref()
            .unwrap()
            .contains("read on the sample itself"),
        "{:?}",
        detail.rule_in_run
    );
    assert!((detail.started_at.unwrap() - before).abs() < 1e-3);
    // The rule read the sample itself, so there is no second population.
    assert!(detail.read.is_none());
    let sample = detail.sample.as_ref().unwrap();
    // Tmem's parent, every event of it kept.
    assert!(
        sample.events > 500 && sample.events < 20_000,
        "{}",
        sample.events
    );
    assert_eq!(sample.events_kept, sample.events);
    assert_eq!(
        sample.metadata.get("SampleType").map(String::as_str),
        Some("FMX")
    );
    assert_eq!(
        sample.histogram.counts.iter().sum::<u32>() as usize,
        sample.events_kept
    );
    assert!(sample.histogram.lower < before && before < sample.histogram.upper);
    assert!(sample.shape.is_some());

    // A case is named exactly as listed.
    for bad in [
        "nonsense",
        "no-such-run|x|y",
        &format!("{}|no-gate|x", listed.runs[0].run),
    ] {
        assert!(
            session
                .replay_case(&[], ReplayScope::Workspace, bad)
                .is_err(),
            "{bad}"
        );
    }
}

#[test]
fn a_replay_needs_a_run_and_a_library_when_asked_for_one() {
    use clingate_core::session::ReplayScope;
    let session = Session::open(&with_rules("session-replay-none")).unwrap();
    for scope in [ReplayScope::Workspace, ReplayScope::Both] {
        let refused = format!(
            "{:?}",
            session.replay_rules(&[], scope, None, None).unwrap_err()
        );
        assert!(refused.contains("no rules run"), "{refused}");
    }
    let mut session = session;
    session.set_review_library(None);
    let refused = format!(
        "{:?}",
        session
            .replay_rules(&[], ReplayScope::Library, None, None)
            .unwrap_err()
    );
    assert!(refused.contains("no review library"), "{refused}");
    assert_eq!(ReplayScope::from_key("Library"), Some(ReplayScope::Library));
    assert_eq!(ReplayScope::from_key(""), Some(ReplayScope::Both));
    assert_eq!(ReplayScope::from_key("everywhere"), None);
}

#[test]
fn a_rule_is_written_to_the_rules_file_and_a_preview_made_before_it_is_dropped() {
    let folder = with_rules("session-update-rule");
    let mut session = Session::open(&folder).unwrap();
    session.preview_rules().unwrap();
    let change = tmem_rule(&session, band((0.05, 0.06)));
    let updated = session.update_rule(change.clone()).unwrap();
    assert_eq!(updated.population, "Tmem");
    assert!(
        updated
            .was
            .as_deref()
            .unwrap()
            .contains("capture 1.000% to 2.000%")
    );
    assert!(updated.now.contains("capture 5.000% to 6.000%"));
    assert_eq!(updated.file, clingate_core::workspace::rules_file(&folder));
    // The preview was made under the old rule.
    assert!(session.apply_previewed_rules().is_err());
    assert!(
        session.rules_view().unwrap().rules[0]
            .rule
            .contains("5.000% to 6.000%")
    );
    // Written: a session opened afresh reads it.
    let again = Session::open(&folder).unwrap();
    assert_eq!(
        again.rules().unwrap().get(&change.target),
        Some(&change.rule)
    );

    // A rule for a new target is added beside it.
    let mut other = change.clone();
    other.target = clingate_core::gate_rules::rule_store::RuleTarget::under("Tmem", "1");
    let added = session.update_rule(other).unwrap();
    assert_eq!(added.population, "Tmem of 1");
    assert_eq!(added.was, None);
    assert_eq!(
        Session::open(&folder)
            .unwrap()
            .rules_view()
            .unwrap()
            .rules
            .len(),
        2
    );
}

#[test]
fn a_rule_is_not_written_where_there_is_no_rules_file_or_one_that_did_not_read() {
    let folder = workspace("session-update-no-rules");
    let mut session = Session::open(&folder).unwrap();
    let change = clingate_core::review::replay::RuleChange {
        target: clingate_core::gate_rules::rule_store::RuleTarget::named("Tmem"),
        rule: clingate_core::gate_rules::rule_store::GateRule {
            parameter: "BUV805-A".into(),
            bound: clingate_core::gate_rules::rule_store::Bound::Above,
            measured_on: clingate_core::gate_rules::rule_store::MeasuredOn::Itself,
            rule: clingate_core::gate_rules::rule::Rule::TailFraction(
                clingate_core::gate_rules::rule::TailFractionRule::new((0.01, 0.02)),
            ),
        },
    };
    assert!(session.update_rule(change.clone()).is_err());
    assert!(!clingate_core::workspace::rules_file(&folder).exists());

    let broken = with_rules("session-update-broken-rules");
    std::fs::write(clingate_core::workspace::rules_file(&broken), "{ not json").unwrap();
    let mut session = Session::open(&broken).unwrap();
    let refused = format!("{:?}", session.update_rule(change).unwrap_err());
    assert!(refused.contains("not written over"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(clingate_core::workspace::rules_file(&broken)).unwrap(),
        "{ not json"
    );
}

#[test]
fn a_case_whose_rule_read_the_specimen_s_fmx_shows_that_population_beside_the_sample() {
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleStore, RuleTarget};
    use clingate_core::session::ReplayScope;
    // One specimen: its FMX and its full stain.
    let folder = with_rule("session-replay-partner", percentile(0.0));
    write_metadata(
        &folder.join("metadata.csv"),
        &["test", "Type", "SampleType"],
        &[
            ("sample1", "sample1_FMX.fcs", &["one", "one", "FMX"]),
            ("sample2", "sample2_FS.fcs", &["one", "one", "FS"]),
        ],
    );
    let file = clingate_core::workspace::rules_file(&folder);
    let mut store = RuleStore::load(&file).unwrap();
    let target = RuleTarget::named("Tmem");
    let mut rule = store.get(&target).unwrap().clone();
    rule.measured_on = MeasuredOn::Partner("FMX".into());
    store.insert(target, rule);
    store.save(&file).unwrap();

    let mut session = Session::open(&folder).unwrap();
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    session.mark_run_reviewed().unwrap();

    let listed = session
        .replay_rules(&[], ReplayScope::Workspace, None, None)
        .unwrap();
    // The full stain is the file gated; the FMX is read.
    assert_eq!(listed.cases_total, 1, "{listed:?}");
    let line = &listed.cases[0];
    assert!(line.sample.contains("FS"), "{line:?}");
    let detail = session
        .replay_case(&[], ReplayScope::Workspace, &line.case)
        .unwrap();
    assert!(
        detail
            .rule_in_run
            .as_deref()
            .unwrap()
            .contains("read on the specimen's FMX"),
        "{:?}",
        detail.rule_in_run
    );
    let sample = detail.sample.as_ref().unwrap();
    let read = detail.read.as_ref().expect("the FMX the rule read");
    assert_eq!(sample.metadata["SampleType"], "FS");
    assert_eq!(read.metadata["SampleType"], "FMX");
    assert_eq!(
        read.histogram.counts.iter().sum::<u32>() as usize,
        read.events_kept
    );
    // One range, so a line reads the same on both.
    assert_eq!(
        (sample.histogram.lower, sample.histogram.upper),
        (read.histogram.lower, read.histogram.upper)
    );
    assert_eq!(
        detail.case.run_decided.measured_on.as_deref(),
        Some(read.file.as_str())
    );
}

#[test]
fn the_same_rules_run_again_on_the_same_gates_are_stored_once_and_counted_once() {
    use clingate_core::review::events::Pool;
    use clingate_core::review::replay::{Truth, Verdict};
    use clingate_core::session::ReplayScope;
    // Run, reviewed - the gate on fmx reported and both taken back.
    let (mut session, _, library) = reviewed_run("session-replay-twice");
    let pool = Pool::in_library(&library);
    let stored = pool.len();
    assert!(stored >= 2, "{stored}");

    // The same rules on the same files from the same gates - a second later,
    // so it is a run of its own - and this time accepted as placed.
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let reviewed = session.mark_run_reviewed().unwrap();
    assert_eq!(reviewed.accepted, 2);
    let runs_in_library = std::fs::read_dir(&library)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("review.json").is_file())
        .count();
    assert_eq!(runs_in_library, 2, "each review is kept");
    assert_eq!(pool.len(), stored, "but no population twice");

    let replayed = session
        .replay_rules(&[], ReplayScope::Both, None, None)
        .unwrap();
    assert_eq!(replayed.runs.len(), 2, "{:?}", replayed.runs);
    assert_eq!(replayed.cases_total, 2, "each placement once");
    assert_eq!(replayed.repeats_counted_once, 2);
    let earlier = replayed
        .runs
        .iter()
        .min_by(|a, b| a.run_applied_at.cmp(&b.run_applied_at))
        .unwrap();
    assert_eq!(earlier.repeated_later, 2);
    // Counted with the later review, which accepted them.
    assert_eq!(
        replayed.totals.get(&Verdict::StillRight),
        Some(&2),
        "{replayed:?}"
    );
    for case in &replayed.cases {
        assert_eq!(case.truth, Truth::AcceptedAsPlaced, "{case:?}");
        assert_eq!(case.earlier_reviews, 1);
    }
    // In full, what the earlier review said - it disagreed.
    let fmx = replayed
        .cases
        .iter()
        .find(|c| c.sample.contains("FMX"))
        .unwrap();
    let detail = session
        .replay_case(&[], ReplayScope::Both, &fmx.case)
        .unwrap();
    assert_eq!(detail.case.earlier_reviews.len(), 1);
    assert_eq!(
        detail.case.earlier_reviews[0].run_applied_at,
        earlier.run_applied_at
    );
    assert!(
        matches!(
            &detail.case.earlier_reviews[0].truth,
            Truth::Corrected { note, .. } if note == "the old gate was right"
        ),
        "{:?}",
        detail.case.earlier_reviews
    );

    // One run alone repeats nothing.
    let workspace_only = session
        .replay_rules(&[], ReplayScope::Workspace, None, None)
        .unwrap();
    assert_eq!(workspace_only.repeats_counted_once, 0);
    assert!(workspace_only.cases.iter().all(|c| c.earlier_reviews == 0));
}

#[test]
fn a_run_from_moved_gates_is_a_different_input_and_stores_only_what_changed() {
    use clingate_core::review::events::Pool;
    use clingate_core::session::ReplayScope;
    // Reviewed with the run's placements kept this time: the next run starts
    // from where the first put the gate.
    let folder = rule_in(tmem_workspace("session-replay-moved"), percentile(-0.5));
    let library = scratch("session-replay-moved-library");
    let mut session = Session::open(&folder).unwrap();
    session.set_review_library(Some(library.clone()));
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    session.mark_run_reviewed().unwrap();
    let pool = Pool::in_library(&library);
    let stored = pool.len();

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    session.mark_run_reviewed().unwrap();
    // Tmem's parent did not move, so its populations are the same ones.
    assert_eq!(pool.len(), stored);

    // A percentile rule does not read where the gate starts on the sample,
    // but read on the sample itself it is calibrated there: a different
    // input, so both runs count.
    let replayed = session
        .replay_rules(&[], ReplayScope::Both, None, None)
        .unwrap();
    assert_eq!(replayed.cases_total, 4, "{replayed:?}");
    assert_eq!(replayed.repeats_counted_once, 0);
}

#[test]
fn a_report_belongs_to_the_run_it_was_made_against() {
    use clingate_core::review::board::Pile;
    use clingate_core::review::report::{is_reviewed, reports_in, reports_of_run};
    let (mut session, _, _) = reviewed_run("session-report-run");
    let folder = session.folder().to_path_buf();
    let first = clingate_core::review::RunRecord::load(&folder)
        .unwrap()
        .unwrap()
        .applied_at;
    assert!(is_reviewed(&folder, &first));
    let fixed = reports_in(&folder)[0]
        .1
        .correction
        .clone()
        .expect("taken back and saved");

    std::thread::sleep(std::time::Duration::from_millis(1_100));
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    session.save().unwrap();
    let second = clingate_core::review::RunRecord::load(&folder)
        .unwrap()
        .unwrap()
        .applied_at;
    assert_ne!(first, second);

    // The new run's placements are not reported: the report was about the
    // first run's.
    let piles = session.assess_run().unwrap().piles;
    let count = |pile: Pile| piles.iter().find(|(p, _)| *p == pile).unwrap().1;
    assert_eq!(count(Pile::Reported), 0, "{piles:?}");
    assert!(reports_of_run(&folder, &second).is_empty());
    assert_eq!(reports_of_run(&folder, &first).len(), 1);
    // Its fix is where the reviewer left the gate, not where the new run put
    // it - saving after the new run does not rewrite history.
    assert_eq!(
        reports_in(&folder)[0]
            .1
            .correction
            .as_ref()
            .map(|c| &c.gate_at),
        Some(&fixed.gate_at)
    );
    // And the first run's review is not this run's.
    assert!(!is_reviewed(&folder, &second));
    let reviewed = session.mark_run_reviewed().unwrap();
    assert_eq!((reviewed.accepted, reviewed.reported), (2, 0));
    assert!(is_reviewed(&folder, &second));
}

/// Tmem's rule, done another way, as a whole rule.
fn tmem_candidate(
    session: &Session,
    how: clingate_core::gate_rules::rule::Rule,
) -> clingate_core::gate_rules::rule_store::GateRule {
    tmem_rule(session, how).rule
}

#[test]
fn candidate_rules_are_tried_side_by_side_and_nothing_moves() {
    let folder = rule_in(tmem_workspace("session-try"), percentile(0.0));
    let session = Session::open(&folder).unwrap();
    let before = tmem_edge(&session);
    let rules_file = std::fs::read(clingate_core::workspace::rules_file(&folder)).unwrap();

    let candidates = vec![
        // The gate as drawn holds the positives, about a third of the parent:
        // a band round that is already met, so the gate is kept.
        tmem_candidate(&session, band((0.30, 0.40))),
        // Half a unit below the median: the slanted gate takes the negatives in.
        tmem_candidate(&session, percentile(-0.5)),
        // A unit above it: in the gap, as drawn.
        tmem_candidate(&session, percentile(1.0)),
    ];
    let trial = session.try_rules("Tmem", &candidates, None).unwrap();
    assert!(trial.gate.starts_with("Tmem of "), "{}", trial.gate);
    assert_eq!(trial.files_read, 2);
    assert!(trial.problems.is_empty(), "{:?}", trial.problems);
    assert_eq!(trial.specimen_column, "test");
    assert_eq!(trial.candidates.len(), 3);
    assert!(
        trial.candidates[0]
            .rule
            .contains("capture 30.000% to 40.000%")
    );
    assert!(trial.candidates[1].rule.contains("50th percentile"));

    // One row per specimen's gated file, each with a cell per candidate.
    assert_eq!(trial.rows_total, 2);
    for row in &trial.rows {
        assert_eq!(row.candidates.len(), 3);
        let current = row.current_holds.unwrap();
        assert!((0.25..0.45).contains(&current), "{row:?}");
        let [kept, lower, gap] = &row.candidates[..] else {
            unreachable!()
        };
        assert_eq!(kept.what, "kept", "{row:?}");
        assert_eq!(kept.holds, row.current_holds);
        assert_eq!(lower.what, "moved");
        assert!(lower.holds.unwrap() > current + 0.05, "{row:?}");
        assert_eq!(gap.what, "moved");
        assert!((gap.holds.unwrap() - current).abs() < 0.02, "{row:?}");
        assert!(lower.line.unwrap() < gap.line.unwrap());
        assert!(lower.confidence.is_some() && lower.weakest.is_some());
    }

    // Summed up by sample type: the pairing's FMX and FS, one sample each.
    for summary in trial.candidates.iter().map(|c| &c.summary) {
        let types: Vec<(&str, usize)> = summary
            .by_type
            .iter()
            .map(|t| (t.sample_type.as_str(), t.samples))
            .collect();
        assert_eq!(types, vec![("FMX", 1), ("FS", 1)]);
        assert_eq!(
            summary.placed + summary.kept + summary.not_placed,
            2,
            "{summary:?}"
        );
    }
    assert_eq!(trial.candidates[0].summary.kept, 2);
    assert_eq!(trial.candidates[1].summary.placed, 2);
    let lower_fs = trial.candidates[1].summary.by_type[1]
        .holds
        .as_ref()
        .unwrap();
    let current_fs = trial.current[1].holds.as_ref().unwrap();
    assert!(lower_fs.median > current_fs.median + 0.05);

    // Nothing written, nothing moved.
    assert_eq!(tmem_edge(&session), before);
    assert_eq!(
        std::fs::read(clingate_core::workspace::rules_file(&folder)).unwrap(),
        rules_file
    );
    assert!(!folder.join("reviews").exists(), "no run was recorded");

    // Fewer rows on request, the total still said.
    let one = session.try_rules("Tmem", &candidates, Some(1)).unwrap();
    assert_eq!((one.rows.len(), one.rows_total), (1, 2));
}

#[test]
fn a_trial_needs_one_to_four_candidates_and_a_population_that_is_there() {
    let folder = rule_in(tmem_workspace("session-try-refused"), percentile(0.0));
    let session = Session::open(&folder).unwrap();
    let one = tmem_candidate(&session, percentile(0.0));
    assert!(session.try_rules("Tmem", &[], None).is_err());
    let five = vec![one.clone(); 5];
    let refused = format!("{:?}", session.try_rules("Tmem", &five, None).unwrap_err());
    assert!(refused.contains("at most 4"), "{refused}");
    assert!(matches!(
        session.try_rules("Tme", std::slice::from_ref(&one), None),
        Err(Refusal::NeedsClarification(_))
    ));
    // A rule on a marker the gate is not drawn on is refused before anything
    // is read, and says which markers it is drawn on.
    let mut elsewhere = one.clone();
    elsewhere.parameter = "BUV661-A".into();
    let refused = session
        .try_rules("Tmem", &[elsewhere], None)
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("candidate 1: Tmem of 1 is drawn on BUV805-A and BUV563-A")
            && refused.contains("not BUV661-A"),
        "{refused}"
    );
}

#[test]
fn a_gate_is_profiled_on_both_its_markers_by_sample_type() {
    use clingate_core::gate_rules::profile::ShapeClass;
    let folder = rule_in(tmem_workspace("session-profile"), percentile(0.0));
    let session = Session::open(&folder).unwrap();
    let answer = session.gate_profile(Some("Tmem"), None).unwrap();
    assert_eq!((answer.specimens_read, answer.specimens), (2, 2));
    assert!(answer.problems.is_empty(), "{:?}", answer.problems);
    assert!(answer.classes.contains("smear (no dip"));
    let profile = answer.profile.as_ref().expect("one gate, in full");
    assert!(profile.gate.starts_with("Tmem of "));
    let markers: Vec<&str> = profile
        .markers
        .iter()
        .map(|m| m.parameter.as_str())
        .collect();
    assert_eq!(markers, ["BUV805-A", "BUV563-A"]);

    // On its marker: a negative and a separate positive on both kinds of
    // sample, the gate in the gap holding the positives.
    let on_x = &profile.markers[0];
    let kinds: Vec<&str> = on_x
        .by_type
        .iter()
        .map(|t| t.sample_type.as_str())
        .collect();
    assert_eq!(kinds, ["FMX", "FS"]);
    for t in &on_x.by_type {
        assert_eq!(t.samples, 1);
        assert_eq!(t.classes.get(&ShapeClass::Separate), Some(&1), "{t:?}");
        let held = t.holds.as_ref().unwrap().median;
        assert!((0.25..0.45).contains(&held), "{held}");
        assert!(t.gate_in_right_widths.as_ref().unwrap().median > 1.0);
        assert!(t.parent_events_median > 500);
    }
    // The FMX and the full stain are different specimens here, so no full
    // stain has a control to be read against.
    assert!(on_x.signal_over_control.is_none());
    // In a line each.
    assert_eq!(answer.lines.len(), 2);
    assert!(
        answer.lines[0].contains("| BUV805-A: FMX x1 [separate 1]"),
        "{}",
        answer.lines[0]
    );

    // One specimen's FMX and full stain: the full stain read against its
    // FMX's top. The FMX here carries the same positives as the full stain,
    // so almost nothing lies above it - no signal the FMX lacks.
    write_metadata(
        &folder.join("metadata.csv"),
        &["test", "Type", "SampleType"],
        &[
            ("sample1", "sample1_FMX.fcs", &["one", "one", "FMX"]),
            ("sample2", "sample2_FS.fcs", &["one", "one", "FS"]),
        ],
    );
    let paired = Session::open(&folder)
        .unwrap()
        .gate_profile(Some("Tmem"), None)
        .unwrap();
    let on_x = &paired.profile.as_ref().unwrap().markers[0];
    let (control, signal) = on_x.signal_over_control.as_ref().unwrap();
    assert_eq!(control, "FMX");
    assert!(signal.median < 0.02, "{signal:?}");
    assert!(
        paired.lines[0].contains("above the FMX's top"),
        "{}",
        paired.lines[0]
    );

    // Fewer specimens on request, the total still said.
    let one = session.gate_profile(Some("Tmem"), Some(1)).unwrap();
    assert_eq!((one.specimens_read, one.specimens), (1, 2));
}

#[test]
fn every_gate_is_profiled_in_a_line_and_nothing_moves() {
    let folder = rule_in(tmem_workspace("session-profile-all"), percentile(0.0));
    let session = Session::open(&folder).unwrap();
    let before = tmem_edge(&session);
    let answer = session.gate_profile(None, None).unwrap();
    assert!(answer.profile.is_none(), "many gates: lines only");
    assert!(answer.lines.len() >= 2, "{:?}", answer.lines);
    assert!(answer.lines.iter().any(|l| l.starts_with("Tmem of ")));
    assert_eq!(tmem_edge(&session), before);
    assert!(!folder.join("reviews").exists());
    assert!(matches!(
        session.gate_profile(Some("Tme"), None),
        Err(Refusal::NeedsClarification(_))
    ));
}

#[test]
fn a_gate_is_pictured_on_the_samples_worth_seeing_with_its_outline_drawn_in() {
    let folder = rule_in(tmem_workspace("session-picture"), percentile(0.0));
    let session = Session::open(&folder).unwrap();
    let picture = session.gate_picture("Tmem", None, None).unwrap();
    let image = image::load_from_memory(&picture.png).unwrap().to_rgba8();
    // Both samples, each the typical one of its kind, side by side.
    assert_eq!(picture.tiles.len(), 2, "{:?}", picture.tiles);
    assert_eq!((image.width(), image.height()), (600, 300));
    assert!(
        picture.tiles.iter().any(|t| t.contains("(FS) - typical")),
        "{:?}",
        picture.tiles
    );
    assert!(
        picture.tiles.iter().any(|t| t.contains("(FMX) - typical")),
        "{:?}",
        picture.tiles
    );
    for tile in &picture.tiles {
        let held: f64 = tile
            .split("the gate holds ")
            .nth(1)
            .and_then(|s| s.split('%').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| panic!("{tile}"));
        assert!((25.0..45.0).contains(&held), "{tile}");
    }
    // The outline is drawn into the picture: black inside the plotting area,
    // where only the gate is black - the axes' text is outside it.
    let ink = (70..280)
        .flat_map(|x| (15..230).map(move |y| (x, y)))
        .filter(|(x, y)| image.get_pixel(*x, *y).0 == [0, 0, 0, 255])
        .count();
    assert!(ink > 100, "{ink} black pixels in the plot");

    // Named samples; one tile.
    let one = session.gate_picture("Tmem", Some("fmx"), None).unwrap();
    assert_eq!(one.tiles.len(), 1);
    let image = image::load_from_memory(&one.png).unwrap();
    assert_eq!((image.width(), image.height()), (300, 300));
    assert!(
        session
            .gate_picture("Tmem", Some("nothing like it"), None)
            .is_err()
    );
}

// ─── rules as Claude writes them ──────────────────────────────────────────────
//
// A rule names things the way the tools show them - a marker, a file name -
// and the rules file holds what a run reads: a channel, a metadata row. Rules
// written the first way were stored without a word and then failed on every
// file of every run as "no reference sample to measure". These are checked
// against a panel whose markers have names of their own, as a real one does.

/// The marker each channel carries in [`marked_workspace`].
const MARKERS: [&str; 8] = [
    "CD161", "CD4", "CD8", "CCR6", "CD69", "TCRgd", "IL17A", "CD3",
];

/// [`workspace`], with a scaling export that names each channel's marker, and
/// a rules file holding one rule for Tmem as the Gate Rules tab would write it.
fn marked_workspace(name: &str) -> std::path::PathBuf {
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let dir = workspace(name);
    let mut scaling = vec![
        ("FSC-A", "", "None (linear)", 0, 0, 4_194_304),
        ("SSC-A", "", "None (linear)", 0, 0, 4_194_304),
    ];
    for (channel, marker) in FLUORESCENCE.iter().zip(MARKERS) {
        scaling.push((channel, marker, "Arcsinh", 6000, -2000, 200_000));
    }
    write_scaling(&dir.join("scaling.csv"), &scaling);
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named("Tmem"),
        GateRule {
            parameter: "BUV805-A".into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::Itself,
            rule: band((0.01, 0.02)),
        },
    );
    store
        .save(&clingate_core::workspace::rules_file(&dir))
        .unwrap();
    dir
}

fn change(
    gate: &str,
    parent: Option<&str>,
    parameter: &str,
    measured_on: clingate_core::gate_rules::rule_store::MeasuredOn,
    rule: clingate_core::gate_rules::rule::Rule,
) -> clingate_core::review::replay::RuleChange {
    use clingate_core::gate_rules::rule_store::{Bound, GateRule, RuleTarget};
    clingate_core::review::replay::RuleChange {
        target: match parent {
            Some(p) => RuleTarget::under(gate, p),
            None => RuleTarget::named(gate),
        },
        rule: GateRule {
            parameter: parameter.into(),
            bound: Bound::Above,
            measured_on,
            rule,
        },
    }
}

fn file(named: &str) -> clingate_core::gate_rules::rule_store::MeasuredOn {
    clingate_core::gate_rules::rule_store::MeasuredOn::File(named.into())
}

fn refusal_text(refused: Refusal) -> String {
    refused.to_string()
}

#[test]
fn a_rule_naming_a_marker_and_a_file_is_stored_as_the_channel_and_metadata_row_a_run_reads() {
    let folder = marked_workspace("session-rule-names");
    let mut session = Session::open(&folder).unwrap();
    // Tmem is drawn on BUV805-A, whose marker is CD69; sample1_FMX.fcs is the
    // file the metadata calls sample1.
    let written = session
        .update_rule(change(
            "Tmem",
            None,
            "CD69",
            file("sample1_FMX.fcs"),
            band((0.01, 0.02)),
        ))
        .unwrap();
    assert_eq!(
        written.resolved,
        [
            "parameter CD69 is the channel BUV805-A",
            "the reference sample1_FMX.fcs is the file sample1",
        ]
    );

    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&clingate_core::gate_rules::rule_store::RuleTarget::named(
            "Tmem",
        ))
        .unwrap()
        .clone();
    assert_eq!(&*stored.parameter, "BUV805-A");
    assert_eq!(stored.measured_on, file("sample1"));

    // And it runs: the reference is found, so the other specimen is placed
    // and nothing is left for want of a reference.
    let preview = session.preview_rules().unwrap();
    assert!(
        preview.not_positioned.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    assert_eq!(preview.references.len(), 1, "sample1 is the reference");
    assert_eq!(
        preview.would_move.len() + preview.already_in_place.len(),
        1,
        "and sample2 is positioned from it"
    );
}

#[test]
fn a_rule_already_written_with_the_wrong_names_says_what_is_wrong_in_the_list_and_the_run() {
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    // What Claude wrote before these checks existed, straight into the file.
    let folder = marked_workspace("session-rule-names-already-wrong");
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    store.insert(
        RuleTarget::named("Tmem"),
        GateRule {
            parameter: "CD69".into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::File("sample1_FMX.fcs".into()),
            rule: clingate_core::gate_rules::rule::Rule::AboveTheNegative(Default::default()),
        },
    );
    store.insert(
        RuleTarget::named("teff_naive"),
        GateRule {
            parameter: "BUV805-A".into(),
            bound: Bound::Above,
            measured_on: MeasuredOn::Partner("FMO".into()),
            rule: band((0.01, 0.02)),
        },
    );
    store
        .save(&clingate_core::workspace::rules_file(&folder))
        .unwrap();
    let mut session = Session::open(&folder).unwrap();

    let listed = session.rules_view().unwrap().rules;
    let tmem = &listed[0].problems;
    assert!(
        tmem.iter()
            .any(|p| p.contains("parameter CD69 is the channel BUV805-A")),
        "{tmem:?}"
    );
    assert!(
        tmem.iter()
            .any(|p| p.contains("the reference sample1_FMX.fcs is the file sample1")),
        "{tmem:?}"
    );
    let naive = &listed[1].problems;
    assert!(
        naive
            .iter()
            .any(|p| p.contains("no file has the sample type FMO") && p.contains("FMX, FS")),
        "{naive:?}"
    );

    // The run says the same, in terms a person can act on - not "no
    // reference sample to measure".
    let preview = session.preview_rules().unwrap();
    let said: Vec<String> = preview
        .not_positioned
        .iter()
        .map(|n| n.reason.clone())
        .collect();
    assert!(
        said.iter().all(|r| r != "no reference sample to measure"),
        "{said:?}"
    );
    assert!(
        said.iter().any(|r| r
            .contains("the rule positions CD69 but this gate is drawn on BUV805-A and BUV563-A")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|r| r.contains("has the sample type FMO, so there is no FMO to measure")),
        "{said:?}"
    );
}

#[test]
fn a_phenotype_rule_s_markers_are_stored_as_channels() {
    let folder = marked_workspace("session-rule-phenotype-names");
    let mut session = Session::open(&folder).unwrap();
    let rule = clingate_core::gate_rules::rule::Rule::MatchThePhenotype(
        clingate_core::gate_rules::rule::PhenotypeRule {
            markers: vec!["CD69".into(), "cd4".into(), "BUV563-A".into()],
            ..Default::default()
        },
    );
    let written = session
        .update_rule(change("Tmem", None, "", file("sample2"), rule))
        .unwrap();
    assert_eq!(
        written.resolved,
        [
            "marker CD69 is the channel BUV805-A",
            "marker cd4 is the channel BV785-A"
        ],
        "a channel given as a channel needs no note"
    );
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&clingate_core::gate_rules::rule_store::RuleTarget::named(
            "Tmem",
        ))
        .unwrap()
        .clone();
    match stored.rule {
        clingate_core::gate_rules::rule::Rule::MatchThePhenotype(p) => assert_eq!(
            p.markers.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
            ["BUV805-A", "BV785-A", "BUV563-A"]
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pinned_marker_is_stored_as_its_channel_and_must_be_one_the_gate_is_drawn_on() {
    use clingate_core::gate_rules::rule::{PhenotypeRule, Rule};
    let folder = marked_workspace("session-rule-phenotype-pinned");
    let mut session = Session::open(&folder).unwrap();
    let rule = |markers: &[&str], pinned: &[&str]| {
        Rule::MatchThePhenotype(PhenotypeRule {
            markers: markers.iter().map(|m| (*m).into()).collect(),
            pinned: pinned.iter().map(|m| (*m).into()).collect(),
            ..Default::default()
        })
    };
    let written = session
        .update_rule(change(
            "Tmem",
            None,
            "",
            file("sample2"),
            rule(&["CD69", "cd4"], &["CD69"]),
        ))
        .unwrap();
    assert!(
        written
            .resolved
            .iter()
            .any(|r| r == "pinned marker CD69 is the channel BUV805-A"),
        "{:?}",
        written.resolved
    );
    assert!(
        written.now.contains("pinned to the negative"),
        "{}",
        written.now
    );

    let unread = session
        .update_rule(change(
            "Tmem",
            None,
            "",
            file("sample2"),
            rule(&["CD69"], &["cd4"]),
        ))
        .unwrap_err();
    assert!(
        format!("{unread:?}").contains("BV785-A is pinned but is not one of the rule's markers"),
        "{unread:?}"
    );
    let off_the_plot = session
        .update_rule(change(
            "Tmem",
            None,
            "",
            file("sample2"),
            rule(&["CD69", "cd4"], &["cd4"]),
        ))
        .unwrap_err();
    assert!(
        format!("{off_the_plot:?}").contains("so it has no edge on BV785-A to pin"),
        "{off_the_plot:?}"
    );
}

#[test]
fn a_rule_that_cannot_run_is_refused_with_what_would_and_the_file_is_left_alone() {
    let folder = marked_workspace("session-rule-refused");
    let rules_file = clingate_core::workspace::rules_file(&folder);
    let before = std::fs::read_to_string(&rules_file).unwrap();
    let mut session = Session::open(&folder).unwrap();
    let itself = clingate_core::gate_rules::rule_store::MeasuredOn::Itself;
    let other_marker = {
        let drawn = session.gate("Tmem", None).unwrap().parameters;
        FLUORESCENCE
            .iter()
            .zip(MARKERS)
            .find(|(channel, _)| !drawn.iter().any(|d| d == **channel))
            .map(|(_, marker)| marker)
            .unwrap()
    };

    let cases: Vec<(clingate_core::review::replay::RuleChange, &str)> = vec![
        (
            change("Tmem2", None, "CD69", itself.clone(), band((0.01, 0.02))),
            "there is no gate called Tmem2",
        ),
        (
            change(
                "Tmem",
                Some("CD4+"),
                "CD69",
                itself.clone(),
                band((0.01, 0.02)),
            ),
            "no Tmem gate is drawn under CD4+ - its parents are: 1",
        ),
        (
            change(
                "Tmem",
                None,
                other_marker,
                itself.clone(),
                band((0.01, 0.02)),
            ),
            "is drawn on BUV805-A and",
        ),
        (
            change(
                "Tmem",
                None,
                "CD69",
                clingate_core::gate_rules::rule_store::MeasuredOn::Partner("FMO".into()),
                band((0.01, 0.02)),
            ),
            "the types in this workspace are: FMX, FS",
        ),
        (
            change(
                "Tmem",
                None,
                "",
                itself.clone(),
                clingate_core::gate_rules::rule::Rule::MatchThePhenotype(Default::default()),
            ),
            "measured on one named file",
        ),
        (
            change("Tmem", None, "CD69", file("sample9"), band((0.01, 0.02))),
            "sample9",
        ),
    ];
    for (asked, expected) in cases {
        let said = refusal_text(session.update_rule(asked.clone()).unwrap_err());
        assert!(said.contains(expected), "{asked:?}\nsaid: {said}");
    }
    // A marker nobody has is a question, not a guess.
    let unknown = session
        .update_rule(change("Tmem", None, "CD999", itself, band((0.01, 0.02))))
        .unwrap_err();
    assert_eq!(clarification(unknown).about, "parameter");

    assert_eq!(
        std::fs::read_to_string(&rules_file).unwrap(),
        before,
        "nothing refused was written"
    );
}

#[test]
fn a_rule_edited_by_claude_keeps_its_place_in_the_file() {
    let folder = marked_workspace("session-rule-order");
    let mut session = Session::open(&folder).unwrap();
    let itself = clingate_core::gate_rules::rule_store::MeasuredOn::Itself;
    let naive_parameter = session.gate("teff_naive", None).unwrap().parameters[0].clone();
    session
        .update_rule(change(
            "teff_naive",
            None,
            &naive_parameter,
            itself.clone(),
            band((0.01, 0.02)),
        ))
        .unwrap();
    session
        .update_rule(change("Tmem", None, "CD69", itself, band((0.05, 0.06))))
        .unwrap();
    let listed: Vec<String> = Session::open(&folder)
        .unwrap()
        .rules_view()
        .unwrap()
        .rules
        .iter()
        .map(|r| format!("{}: {}", r.population, r.rule))
        .collect();
    assert_eq!(listed.len(), 2);
    assert!(
        listed[0].starts_with("Tmem: capture 5.000% to 6.000%"),
        "{listed:?}"
    );
    assert!(listed[1].starts_with("teff_naive"), "{listed:?}");
}

#[test]
fn candidates_are_tried_under_the_names_a_run_reads_and_a_bad_one_is_named_by_its_number() {
    use clingate_core::gate_rules::rule_store::{Bound, GateRule, MeasuredOn};
    let session = Session::open(&marked_workspace("session-try-names")).unwrap();
    let candidate = |parameter: &str, measured_on: MeasuredOn| GateRule {
        parameter: parameter.into(),
        bound: Bound::Above,
        measured_on,
        rule: band((0.01, 0.02)),
    };
    let tried = session.try_rules(
        "Tmem",
        &[
            candidate("CD69", MeasuredOn::Itself),
            candidate("BUV805-A", MeasuredOn::Itself),
        ],
        None,
    );
    assert!(tried.is_ok(), "{:?}", tried.err());

    let refused = session
        .try_rules(
            "Tmem",
            &[
                candidate("CD69", MeasuredOn::Itself),
                candidate("CD69", MeasuredOn::Partner("FMO".into())),
            ],
            None,
        )
        .unwrap_err()
        .to_string();
    assert!(refused.starts_with("candidate 2:"), "{refused}");
    assert!(refused.contains("FMO"), "{refused}");
}

// ─── a linked gate is set from one place ──────────────────────────────────────

fn omiq_rectangle(id: &str, name: &str) -> String {
    format!(
        r#"{{
            "containerType": "AtomicFilterContainer",
            "id": "{id}",
            "name": "{name}",
            "defaultFilter": {{
                "type": "RectangleGate",
                "f1": "FSC-A",
                "f2": "SSC-A",
                "min": {{ "f1Val": 0.0, "f2Val": 0.0 }},
                "max": {{ "f1Val": 4194304.0, "f2Val": 4194304.0 }}
            }}
        }}"#
    )
}

/// [`workspace`], gated as Omiq links a gate: "Shared" is one gate drawn
/// under both Branch A and Branch B. An empty rules file is ready to write to.
fn linked_workspace(name: &str) -> std::path::PathBuf {
    use clingate_core::gate_rules::rule_store::{RuleStore, SamplePairing};
    let dir = workspace(name);
    let gating = format!(
        r#"{{
            "tree": {{
                "nodes": {{
                    "na": {{ "id": "na", "parentId": "",   "filterContainerId": "a", "ord": 0, "collapsed": false }},
                    "nb": {{ "id": "nb", "parentId": "",   "filterContainerId": "b", "ord": 1, "collapsed": false }},
                    "nc": {{ "id": "nc", "parentId": "na", "filterContainerId": "shared", "ord": 2, "collapsed": false }},
                    "nd": {{ "id": "nd", "parentId": "nb", "filterContainerId": "shared", "ord": 3, "collapsed": false }}
                }},
                "filterContainers": {{ "a": {}, "b": {}, "shared": {} }}
            }}
        }}"#,
        omiq_rectangle("a", "Branch A"),
        omiq_rectangle("b", "Branch B"),
        omiq_rectangle("shared", "Shared"),
    );
    std::fs::write(dir.join("gating.omiqgt"), gating).unwrap();
    RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    })
    .save(&clingate_core::workspace::rules_file(&dir))
    .unwrap();
    dir
}

#[test]
fn a_second_rule_for_a_linked_gate_is_refused_and_the_first_kept() {
    let folder = linked_workspace("session-linked-two-rules");
    let rules_file = clingate_core::workspace::rules_file(&folder);
    let mut session = Session::open(&folder).unwrap();
    let itself = clingate_core::gate_rules::rule_store::MeasuredOn::Itself;

    session
        .update_rule(change(
            "Shared",
            Some("Branch A"),
            "SSC-A",
            itself.clone(),
            band((0.1, 0.2)),
        ))
        .expect("one rule, at one place, is what linking wants");
    let after_first = std::fs::read_to_string(&rules_file).unwrap();

    for (parent, rule) in [
        (Some("Branch B"), band((0.3, 0.4))),
        // A rule naming no parent reaches the copy under B, where no more
        // specific rule applies - a second rule for the same gate.
        (None, band((0.3, 0.4))),
    ] {
        let said = session
            .update_rule(change("Shared", parent, "SSC-A", itself.clone(), rule))
            .unwrap_err()
            .to_string();
        assert!(
            said.contains("Shared is one gate linked under Branch A and Branch B")
                && said.contains("would fight over its one position"),
            "{parent:?}: {said}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&rules_file).unwrap(),
        after_first,
        "nothing refused was written"
    );
}

#[test]
fn rules_already_fighting_over_a_linked_gate_are_flagged_in_the_list_and_left_alone_by_a_run() {
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let folder = linked_workspace("session-linked-listed");
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    for (parent, low) in [("Branch A", 0.1), ("Branch B", 0.3)] {
        store.insert(
            RuleTarget::under("Shared", parent),
            GateRule {
                parameter: "SSC-A".into(),
                bound: Bound::Above,
                measured_on: MeasuredOn::Itself,
                rule: band((low, low + 0.1)),
            },
        );
    }
    store
        .save(&clingate_core::workspace::rules_file(&folder))
        .unwrap();
    let mut session = Session::open(&folder).unwrap();

    for row in session.rules_view().unwrap().rules {
        assert!(
            row.problems
                .iter()
                .any(|p| p.contains("2 rules set it - Shared of Branch A and Shared of Branch B")),
            "{}: {:?}",
            row.population,
            row.problems
        );
    }
    let preview = session.preview_rules().unwrap();
    assert!(preview.would_move.is_empty() && preview.already_in_place.is_empty());
    assert_eq!(
        preview.not_positioned.len(),
        1,
        "{:?}",
        preview.not_positioned
    );
    assert!(preview.not_positioned[0].reason.contains("would fight"));
}

#[test]
fn a_linked_gate_is_listed_with_the_other_places_it_is_drawn() {
    let session = Session::open(&linked_workspace("session-linked-listing")).unwrap();
    let rows = session.populations(None).unwrap();
    let shared: Vec<_> = rows
        .iter()
        .filter(|r| r.rule_target.starts_with("Shared"))
        .collect();
    assert_eq!(shared.len(), 2, "{rows:?}");
    let pairs: Vec<(String, Vec<String>)> = shared
        .iter()
        .map(|r| (r.rule_target.clone(), r.linked_with.clone()))
        .collect();
    assert!(pairs.contains(&(
        "Shared of Branch A".to_string(),
        vec!["Shared of Branch B".to_string()]
    )));
    assert!(pairs.contains(&(
        "Shared of Branch B".to_string(),
        vec!["Shared of Branch A".to_string()]
    )));
    let branch = rows
        .iter()
        .find(|r| r.rule_target.starts_with("Branch A"))
        .unwrap();
    assert!(branch.linked_with.is_empty(), "not linked: {branch:?}");

    let details = session.gate("Branch B > Shared", None).unwrap();
    assert_eq!(details.rule_target, "Shared of Branch B");
    assert_eq!(details.linked_with, ["Shared of Branch A"]);
}

#[test]
fn every_rule_target_the_listing_gives_is_one_update_rule_takes() {
    // What Claude is shown is what it can write - checked for every gate in
    // the fixture, not one example.
    let folder = marked_workspace("session-rule-targets");
    let mut session = Session::open(&folder).unwrap();
    let rows = session.populations(None).unwrap();
    let mut tried = 0;
    for row in rows.iter().filter(|r| r.parameters.len() == 2) {
        let (gate, parent) = match row.rule_target.split_once(" of ") {
            Some((gate, parent)) => (gate.to_string(), Some(parent.to_string())),
            None => (row.rule_target.clone(), None),
        };
        let written = session.update_rule(change(
            &gate,
            parent.as_deref(),
            &row.parameters[0],
            clingate_core::gate_rules::rule_store::MeasuredOn::Itself,
            band((0.01, 0.02)),
        ));
        assert!(written.is_ok(), "{}: {:?}", row.rule_target, written.err());
        tried += 1;
    }
    assert!(
        tried >= 5,
        "the fixture has gates enough to mean something: {tried}"
    );
}

// ─── a rule from another gate, as Claude writes one ───────────────────────────

fn from_gate(
    same_shape_as: Option<clingate_core::gate_rules::rule_store::RuleTarget>,
    edges: Vec<clingate_core::gate_rules::rule::EdgeFrom>,
) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::FromAnotherGate(
        clingate_core::gate_rules::rule::FromGateRule {
            same_shape_as,
            edges,
        },
    )
}

fn edge_from(
    anchor: &str,
    parameter: &str,
    side: clingate_core::gate_rules::rule::Side,
    anchor_side: clingate_core::gate_rules::rule::Side,
) -> clingate_core::gate_rules::rule::EdgeFrom {
    clingate_core::gate_rules::rule::EdgeFrom {
        anchor: clingate_core::gate_rules::rule_store::RuleTarget::named(anchor),
        parameter: parameter.into(),
        side,
        anchor_side,
        gap: 0.0,
    }
}

/// teff_naive set at Tmem's lower edges, Tmem being beside it on its plot:
/// that puts it over Tmem, which the rules list says when the rule is
/// written, and the run, refusing it on every sample, says on the review.
#[test]
fn a_rule_that_puts_its_gate_over_the_one_it_follows_is_told_before_and_after_the_run() {
    use clingate_core::gate_rules::rule::Side;
    use clingate_core::gate_rules::rule_store::MeasuredOn;
    let folder = with_rules("session-follow-over");
    let mut session = Session::open(&folder).unwrap();
    let written = session
        .update_rule(change(
            "teff_naive",
            None,
            "",
            MeasuredOn::Itself,
            from_gate(
                None,
                vec![
                    edge_from("Tmem", "BUV805-A", Side::Lower, Side::Lower),
                    edge_from("Tmem", "BUV563-A", Side::Lower, Side::Lower),
                ],
            ),
        ))
        .unwrap();
    assert!(
        written
            .problems
            .iter()
            .any(|p| p.contains("it lies over Tmem on the gates as drawn")),
        "{written:?}"
    );
    let row = session
        .rules_view()
        .unwrap()
        .rules
        .into_iter()
        .find(|r| r.population.starts_with("teff_naive"))
        .unwrap();
    assert!(
        row.problems
            .iter()
            .any(|p| p.contains("it lies over Tmem on the gates as drawn")),
        "{:?}",
        row.problems
    );

    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let assessed = session.assess_run().unwrap();
    let refused = assessed
        .unplaced
        .iter()
        .find(|u| u.gate == "teff_naive")
        .expect("teff_naive was not placed");
    assert!(refused.everywhere, "{refused:?}");
    assert_eq!(refused.samples.len(), 2);
    assert!(refused.reason.contains("would overlap Tmem"), "{refused:?}");
    assert!(
        refused
            .rule_problem
            .as_deref()
            .is_some_and(|p| p.contains("set its edge facing Tmem against that gate's near edge")),
        "{refused:?}"
    );
    assert!(
        refused.says().contains("change the rule"),
        "{}",
        refused.says()
    );
}

#[test]
fn a_rule_from_another_gate_is_stored_with_what_a_run_reads_and_runs() {
    use clingate_core::gate_rules::rule::Side;
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = linked_workspace("session-follow-written");
    let mut session = Session::open(&folder).unwrap();
    // Given a parameter and a partner, as Claude might out of habit: neither
    // means anything to this rule, and neither is kept.
    let written = session
        .update_rule(change(
            "Branch B",
            None,
            "SSC-A",
            MeasuredOn::Partner("FMX".into()),
            from_gate(
                None,
                vec![edge_from("Branch A", "fsc-a", Side::Upper, Side::Upper)],
            ),
        ))
        .unwrap();
    assert_eq!(written.resolved, ["parameter fsc-a is the channel FSC-A"]);
    assert!(
        written
            .now
            .contains("upper edge on FSC-A at the upper edge of Branch A")
    );
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&RuleTarget::named("Branch B"))
        .unwrap()
        .clone();
    assert_eq!(&*stored.parameter, "");
    assert_eq!(stored.measured_on, MeasuredOn::Itself);

    // The two branches are drawn alike, beside each other on one plot, so
    // the rules list says Branch B lies over Branch A.
    let row = session
        .rules_view()
        .unwrap()
        .rules
        .into_iter()
        .find(|r| r.population.starts_with("Branch B"))
        .unwrap();
    assert!(
        row.problems
            .iter()
            .any(|p| p.contains("it lies over Branch A")),
        "{:?}",
        row.problems
    );

    // It runs: the two branches are drawn alike, so Branch B is already where
    // Branch A puts it, on both samples - and nothing is refused.
    let preview = session.preview_rules().unwrap();
    assert!(
        preview.not_positioned.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    let kept: Vec<&str> = preview
        .already_in_place
        .iter()
        .filter(|k| k.gate.starts_with("Branch B"))
        .map(|k| k.specimen.as_str())
        .collect();
    assert_eq!(kept.len(), 2, "{:?}", preview.already_in_place);
}

#[test]
fn a_rule_from_another_gate_that_could_never_place_it_is_refused() {
    use clingate_core::gate_rules::rule::Side;
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = linked_workspace("session-follow-refused");
    let rules_file = clingate_core::workspace::rules_file(&folder);
    let mut session = Session::open(&folder).unwrap();
    let before = std::fs::read_to_string(&rules_file).unwrap();
    let cases = [
        (
            from_gate(Some(RuleTarget::named("Branch B")), Vec::new()),
            "cannot follow itself",
        ),
        (
            from_gate(Some(RuleTarget::named("Branch C")), Vec::new()),
            "Branch C, is not in the gating",
        ),
        (
            from_gate(
                None,
                vec![edge_from("Branch A", "BUV661-A", Side::Lower, Side::Lower)],
            ),
            "Branch A is drawn on FSC-A and SSC-A, so it has no edge on BUV661-A",
        ),
        (from_gate(None, Vec::new()), "at least one edge"),
    ];
    for (rule, expected) in cases {
        let said = session
            .update_rule(change("Branch B", None, "", MeasuredOn::Itself, rule))
            .unwrap_err()
            .to_string();
        assert!(said.contains(expected), "{expected}\nsaid: {said}");
    }
    assert_eq!(std::fs::read_to_string(&rules_file).unwrap(), before);

    // A loop: B follows A, then A following B is refused - and B's rule kept.
    session
        .update_rule(change(
            "Branch B",
            None,
            "",
            MeasuredOn::Itself,
            from_gate(Some(RuleTarget::named("Branch A")), Vec::new()),
        ))
        .unwrap();
    let after_first = std::fs::read_to_string(&rules_file).unwrap();
    let said = session
        .update_rule(change(
            "Branch A",
            None,
            "",
            MeasuredOn::Itself,
            from_gate(Some(RuleTarget::named("Branch B")), Vec::new()),
        ))
        .unwrap_err()
        .to_string();
    assert!(said.contains("lead back to it"), "{said}");
    assert_eq!(std::fs::read_to_string(&rules_file).unwrap(), after_first);
}

#[test]
fn a_loop_already_in_the_rules_file_is_flagged_on_both_rules() {
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let folder = linked_workspace("session-follow-loop-listed");
    let mut store = RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    });
    for (gate, anchor) in [("Branch A", "Branch B"), ("Branch B", "Branch A")] {
        store.insert(
            RuleTarget::named(gate),
            GateRule {
                parameter: "".into(),
                bound: Bound::Above,
                measured_on: MeasuredOn::Itself,
                rule: from_gate(Some(RuleTarget::named(anchor)), Vec::new()),
            },
        );
    }
    store
        .save(&clingate_core::workspace::rules_file(&folder))
        .unwrap();
    let mut session = Session::open(&folder).unwrap();
    for row in session.rules_view().unwrap().rules {
        assert!(
            row.problems.iter().any(|p| p.contains("lead back to it")),
            "{}: {:?}",
            row.population,
            row.problems
        );
    }
    let preview = session.preview_rules().unwrap();
    assert!(preview.would_move.is_empty() && preview.already_in_place.is_empty());
    assert_eq!(
        preview.not_positioned.len(),
        2,
        "{:?}",
        preview.not_positioned
    );
}

#[test]
fn a_quadrant_s_corners_are_named_by_their_own_names() {
    // A quadrant from Omiq is registered under its group id, and its corners
    // used to be named by it - four populations called "IinB", and the gates
    // under one corner named as if they were under all four.
    let session = Session::open(&workspace("session-corner-names")).unwrap();
    let rows = session.populations(None).unwrap();
    let targets: Vec<&str> = rows.iter().map(|r| r.rule_target.as_str()).collect();
    for corner in [
        "Q1 IL-22- / IL-17A+ of teff_naive",
        "Q2 IL-22+ / IL-17A+ of teff_naive",
        "Q3 IL-22+ / IL-17A- of teff_naive",
        "Q4 IL-22- / IL-17A- of teff_naive",
        "IFny+ of Q4 IL-22- / IL-17A-",
    ] {
        assert!(targets.contains(&corner), "{corner}: {targets:?}");
    }
    assert!(
        !targets.iter().any(|t| t.contains("IinB")),
        "the group id names nothing: {targets:?}"
    );
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    assert!(
        paths.contains(&"1 > teff_naive > Q4 IL-22- / IL-17A- > IFny+"),
        "{paths:?}"
    );
}

// ─── one run ──────────────────────────────────────────────────────────────────

fn pooled_band(
    pool: clingate_core::gate_rules::rule::Pool,
) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::TailFraction(
        clingate_core::gate_rules::rule::TailFractionRule {
            pool,
            ..clingate_core::gate_rules::rule::TailFractionRule::new((0.01, 0.02))
        },
    )
}

#[test]
fn a_band_counted_on_the_run_reads_a_kind_of_file_and_runs_with_no_setting() {
    use clingate_core::gate_rules::rule::Pool;
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = marked_workspace("session-pooled");
    let mut session = Session::open(&folder).unwrap();
    let fmx = || MeasuredOn::Partner("FMX".into());
    session
        .update_rule(change("Tmem", None, "CD69", fmx(), pooled_band(Pool::Run)))
        .expect("the workspace is the run: nothing to set first");
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&RuleTarget::named("Tmem"))
        .unwrap()
        .clone();
    assert!(
        matches!(&stored.rule, clingate_core::gate_rules::rule::Rule::TailFraction(b) if b.pool == Pool::Run),
        "{stored:?}"
    );
    let said = session
        .update_rule(change(
            "Tmem",
            None,
            "CD69",
            file("sample1"),
            pooled_band(Pool::Run),
        ))
        .unwrap_err()
        .to_string();
    assert!(said.contains("not one named file"), "{said}");

    // It runs: both specimens take the one line their FMX file sets.
    let preview = session.preview_rules().unwrap();
    assert!(
        preview.not_positioned.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    assert_eq!(preview.would_move.len() + preview.already_in_place.len(), 2);
}

// ─── a valley rule's fallback, as Claude writes one ───────────────────────────

fn valley_falling_back_to(
    fallback: clingate_core::gate_rules::rule_store::RuleTarget,
) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::InTheValley(
        clingate_core::gate_rules::rule::ValleyRule {
            fallback: Some(fallback),
            ..Default::default()
        },
    )
}

#[test]
fn a_valley_rule_s_fallback_is_kept_and_checked_as_a_followed_gate_is() {
    use clingate_core::gate_rules::rule_store::RuleTarget;
    let folder = linked_workspace("session-valley-fallback");
    let rules_file = clingate_core::workspace::rules_file(&folder);
    let mut session = Session::open(&folder).unwrap();
    let before = std::fs::read_to_string(&rules_file).unwrap();
    for (fallback, expected) in [
        ("Branch B", "cannot follow itself"),
        ("Branch C", "Branch C, is not in the gating"),
    ] {
        let said = session
            .update_rule(change(
                "Branch B",
                None,
                "FSC-A",
                file("sample1_FMX.fcs"),
                valley_falling_back_to(RuleTarget::named(fallback)),
            ))
            .unwrap_err()
            .to_string();
        assert!(said.contains(expected), "{expected}\nsaid: {said}");
    }
    assert_eq!(std::fs::read_to_string(&rules_file).unwrap(), before);

    let written = session
        .update_rule(change(
            "Branch B",
            None,
            "FSC-A",
            file("sample1_FMX.fcs"),
            valley_falling_back_to(RuleTarget::named("Branch A")),
        ))
        .unwrap();
    assert!(
        written.now.contains("with no dip, where Branch A is"),
        "{}",
        written.now
    );
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&RuleTarget::named("Branch B"))
        .unwrap()
        .clone();
    assert_eq!(
        stored.rule,
        valley_falling_back_to(RuleTarget::named("Branch A"))
    );
}

// ─── a run and the gate's mode of positioning ─────────────────────────────────

#[test]
fn a_run_puts_a_gate_positioned_per_sample_into_positions_by_specimen() {
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleStore, SamplePairing};
    use clingate_core::gates::gate_positions::{Mode, mode};
    let folder = workspace("session-run-mode");
    RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    })
    .save(&clingate_core::workspace::rules_file(&folder))
    .unwrap();
    let mut session = Session::open(&folder).unwrap();
    let ifny: std::sync::Arc<str> = std::sync::Arc::from("2PJQ");
    assert_eq!(
        mode(session.gates(), &ifny),
        Mode::PerSample,
        "as Omiq has it"
    );

    let parameter = session.gate("IFny+", None).unwrap().parameters[0].clone();
    session
        .update_rule(change(
            "IFny+",
            None,
            &parameter,
            MeasuredOn::Itself,
            band((0.01, 0.02)),
        ))
        .unwrap();
    let preview = session.preview_rules().unwrap();
    assert!(
        !preview.would_move.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    session.apply_previewed_rules().unwrap();
    assert_eq!(
        mode(session.gates(), &ifny),
        Mode::ByColumn(std::sync::Arc::from("test"))
    );
}

// ─── a rule next to another gate, as Claude writes one ────────────────────────

fn next_to(
    anchor: clingate_core::gate_rules::rule_store::RuleTarget,
    parameter: &str,
) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::NextToGate(clingate_core::gate_rules::rule::NextToRule {
        anchor,
        parameter: parameter.into(),
        side: clingate_core::gate_rules::rule::Side::Upper,
        meet: clingate_core::gate_rules::rule::Meet::GrowSide,
        gap: 0.0,
    })
}

#[test]
fn a_rule_next_to_another_gate_is_checked_and_kept_with_its_channel() {
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = linked_workspace("session-next-to");
    let rules_file = clingate_core::workspace::rules_file(&folder);
    let mut session = Session::open(&folder).unwrap();
    let before = std::fs::read_to_string(&rules_file).unwrap();
    for (rule, expected) in [
        (
            next_to(RuleTarget::named("Branch B"), "FSC-A"),
            "cannot sit next to itself",
        ),
        (
            next_to(RuleTarget::under("Shared", "Branch A"), "FSC-A"),
            "is not on the same plot as Branch B",
        ),
        (
            next_to(RuleTarget::named("Branch A"), "BUV661-A"),
            "so it does not move along BUV661-A",
        ),
        (
            next_to(RuleTarget::named("Branch C"), "FSC-A"),
            "Branch C, is not in the gating",
        ),
        (
            {
                let mut rule = next_to(RuleTarget::named("Branch A"), "FSC-A");
                if let clingate_core::gate_rules::rule::Rule::NextToGate(next) = &mut rule {
                    next.gap = -1.0;
                }
                rule
            },
            "the gap must be a number, 0 or more",
        ),
    ] {
        let said = session
            .update_rule(change("Branch B", None, "", MeasuredOn::Itself, rule))
            .unwrap_err()
            .to_string();
        assert!(said.contains(expected), "{expected}\nsaid: {said}");
    }
    assert_eq!(std::fs::read_to_string(&rules_file).unwrap(), before);

    let written = session
        .update_rule(change(
            "Branch B",
            None,
            "SSC-A",
            MeasuredOn::Partner("FMX".into()),
            next_to(RuleTarget::named("Branch A"), "fsc-a"),
        ))
        .unwrap();
    assert_eq!(written.resolved, ["parameter fsc-a is the channel FSC-A"]);
    assert!(
        written
            .now
            .contains("next to Branch A, higher than it on FSC-A"),
        "{}",
        written.now
    );
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&RuleTarget::named("Branch B"))
        .unwrap()
        .clone();
    assert_eq!(&*stored.parameter, "");
    assert_eq!(stored.measured_on, MeasuredOn::Itself);
}

/// A run that held Tmem back off teff_naive is replayed the same: the replay
/// keeps it clear of the gate the run kept it clear of, where it stood then.
#[test]
fn a_run_held_back_off_a_gate_beside_it_is_replayed_held_back_the_same() {
    use clingate_core::session::ReplayScope;
    let folder = rule_in(
        tmem_workspace_beside_teff_naive("session-replay-held"),
        percentile(-0.5),
    );
    let (session, before, _) = reviewed_run_in(folder, "session-replay-held");

    let as_run = session
        .replay_rules(&[], ReplayScope::Both, None, None)
        .unwrap();

    assert_eq!(as_run.cases_total, 2, "{as_run:?}");
    for case in &as_run.cases {
        let (run_at, replay_at) = (case.run_at.unwrap(), case.replay_at.unwrap());
        assert!(
            run_at < before,
            "the run moved it left, towards teff_naive: {case:?}"
        );
        assert!((replay_at - run_at).abs() < 1e-6, "{case:?}");
        assert_eq!(
            case.replay_weakest.as_deref(),
            Some("held back off another gate"),
            "{case:?}"
        );
    }
}

/// A rectangle on FSC-A by SSC-A, from `x0` to `x1` on FSC-A and the whole of
/// SSC-A.
fn omiq_rectangle_across(id: &str, name: &str, (x0, x1): (f64, f64)) -> String {
    format!(
        r#"{{
            "containerType": "AtomicFilterContainer",
            "id": "{id}",
            "name": "{name}",
            "defaultFilter": {{
                "type": "RectangleGate",
                "f1": "FSC-A",
                "f2": "SSC-A",
                "min": {{ "f1Val": {x0}, "f2Val": 0.0 }},
                "max": {{ "f1Val": {x1}, "f2Val": 4194304.0 }}
            }}
        }}"#
    )
}

/// [`workspace`], gated with `gates` - `(id, name, parent id, FSC-A span)` -
/// and an empty rules file ready to write to.
fn workspace_of_rectangles(
    name: &str,
    gates: &[(&str, &str, &str, (f64, f64))],
) -> std::path::PathBuf {
    use clingate_core::gate_rules::rule_store::{RuleStore, SamplePairing};
    let dir = workspace(name);
    let nodes: Vec<String> = gates
        .iter()
        .enumerate()
        .map(|(ord, (id, _, parent, _))| {
            let parent = if parent.is_empty() { String::new() } else { format!("n{parent}") };
            format!(
                r#""n{id}": {{ "id": "n{id}", "parentId": "{parent}", "filterContainerId": "{id}", "ord": {ord}, "collapsed": false }}"#
            )
        })
        .collect();
    let containers: Vec<String> = gates
        .iter()
        .map(|(id, name, _, span)| format!(r#""{id}": {}"#, omiq_rectangle_across(id, name, *span)))
        .collect();
    let gating = format!(
        r#"{{ "tree": {{ "nodes": {{ {} }}, "filterContainers": {{ {} }} }} }}"#,
        nodes.join(","),
        containers.join(",")
    );
    std::fs::write(dir.join("gating.omiqgt"), gating).unwrap();
    RuleStore::with_pairing(SamplePairing {
        sample_id_column: "test".into(),
        ..SamplePairing::default()
    })
    .save(&clingate_core::workspace::rules_file(&dir))
    .unwrap();
    dir
}

fn fsc_span(session: &Session, gate: &str, sample: &str) -> (f64, f64) {
    let extent = session.gate(gate, Some(sample)).unwrap().extent;
    let fsc = extent.iter().find(|e| e.parameter == "FSC-A").unwrap();
    (fsc.lower.unwrap(), fsc.upper.unwrap())
}

/// Left, from 0 to 1,000,000 on FSC-A, grows its right side to Right's left
/// edge at 2,000,000; its left side stays where it is.
#[test]
fn a_gate_next_to_another_grows_its_facing_side_to_touch_it_in_a_run() {
    use clingate_core::gate_rules::rule::{Meet, NextToRule, Rule, Side};
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = workspace_of_rectangles(
        "session-next-to-run",
        &[
            ("l", "Left", "", (0.0, 1_000_000.0)),
            ("r", "Right", "", (2_000_000.0, 4_000_000.0)),
        ],
    );
    let mut session = Session::open(&folder).unwrap();
    session
        .update_rule(change(
            "Left",
            None,
            "",
            MeasuredOn::Itself,
            Rule::NextToGate(NextToRule {
                anchor: RuleTarget::named("Right"),
                parameter: "FSC-A".into(),
                side: Side::Lower,
                meet: Meet::GrowSide,
                gap: 0.0,
            }),
        ))
        .unwrap();

    let preview = session.preview_rules().unwrap();
    assert!(
        preview.not_positioned.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    assert_eq!(preview.would_move.len(), 2, "{:?}", preview.would_move);
    session.apply_previewed_rules().unwrap();

    for sample in ["fmx", "fs"] {
        let (lower, upper) = fsc_span(&session, "Left", sample);
        assert!(lower.abs() < 1.0, "{sample}: {lower}");
        assert!((upper - 2_000_000.0).abs() < 1.0, "{sample}: {upper}");
        assert_eq!(
            fsc_span(&session, "Right", sample),
            (2_000_000.0, 4_000_000.0)
        );
    }
}

/// Every sample's FSC-A one peak and nothing else, with no dip either side.
fn one_peak_on_fsc(dir: &std::path::Path) {
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    for (seed, file) in [(1, "sample1_FMX.fcs"), (2, "sample2_FS.fcs")] {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed + 100);
        let peak = Normal::new(2_000_000.0f32, 200_000.0).unwrap();
        let rows: Vec<Vec<f32>> = events(seed, 3_000)
            .into_iter()
            .map(|mut row| {
                row[0] = peak.sample(&mut rng);
                row
            })
            .collect();
        write_fcs(&dir.join(file), &channels, &rows);
    }
}

/// FSC-A is one peak - a smear with no dip - so Inner B's valley rule falls
/// back to Inner A, under another parent: its lower edge goes to Inner A's,
/// 1,000,000, flagged as placed from another gate.
#[test]
fn a_valley_rule_on_a_smear_places_its_gate_from_the_fallback_in_a_run() {
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = workspace_of_rectangles(
        "session-valley-fallback-run",
        &[
            ("a", "Branch A", "", (0.0, 4_194_304.0)),
            ("b", "Branch B", "", (0.0, 4_194_304.0)),
            ("ia", "Inner A", "a", (1_000_000.0, 3_000_000.0)),
            ("ib", "Inner B", "b", (200_000.0, 2_200_000.0)),
        ],
    );
    one_peak_on_fsc(&folder);
    let mut session = Session::open(&folder).unwrap();
    session
        .update_rule(change(
            "Inner B",
            None,
            "FSC-A",
            MeasuredOn::Itself,
            valley_falling_back_to(RuleTarget::named("Inner A")),
        ))
        .unwrap();

    let preview = session.preview_rules().unwrap();
    assert!(
        preview.not_positioned.is_empty(),
        "{:?}",
        preview.not_positioned
    );
    assert!(!preview.would_move.is_empty());
    for placed in &preview.would_move {
        assert_eq!(
            placed.weakest,
            Some(clingate_core::gate_rules::confidence::FALLBACK),
            "{placed:?}"
        );
    }
    session.apply_previewed_rules().unwrap();

    for sample in ["fmx", "fs"] {
        let (lower, _) = fsc_span(&session, "Inner B", sample);
        assert!((lower - 1_000_000.0).abs() < 1.0, "{sample}: {lower}");
    }
}

// ─── valley or smear ──────────────────────────────────────────────────────────

fn valley_or_smear(example: Option<&str>) -> clingate_core::gate_rules::rule::Rule {
    clingate_core::gate_rules::rule::Rule::ValleyOrSmear(
        clingate_core::gate_rules::rule::ValleyOrSmearRule {
            smear_example: example.map(Into::into),
            ..Default::default()
        },
    )
}

/// On FSC-A, sample1 15,000 negatives at 1,000,000 and 5,000 positives at
/// 3,000,000 with a dip between; sample2 the same negative with the 5,000
/// trailing off it.
fn a_dip_and_a_smear_on_fsc(dir: &std::path::Path) {
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    let negative = Normal::new(1_000_000.0f32, 150_000.0).unwrap();
    let positive = Normal::new(3_000_000.0f32, 150_000.0).unwrap();
    let tail = rand_distr::Exp::new(1.0f32 / 400_000.0).unwrap();
    for (seed, file) in [(1, "sample1_FMX.fcs"), (2, "sample2_FS.fcs")] {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed + 200);
        let rows: Vec<Vec<f32>> = events(seed, 20_000)
            .into_iter()
            .enumerate()
            .map(|(at, mut row)| {
                row[0] = match (at < 15_000, seed) {
                    (true, _) => negative.sample(&mut rng),
                    (false, 1) => positive.sample(&mut rng),
                    (false, _) => 1_000_000.0 + tail.sample(&mut rng),
                };
                row
            })
            .collect();
        write_fcs(&dir.join(file), &channels, &rows);
    }
}

#[test]
fn a_valley_or_smear_rule_is_read_on_a_file_and_names_its_example_as_a_file() {
    use clingate_core::gate_rules::rule_store::{MeasuredOn, RuleTarget};
    let folder = workspace_of_rectangles(
        "session-valley-or-smear-written",
        &[("p", "Pos", "", (2_000_000.0, 4_194_304.0))],
    );
    let mut session = Session::open(&folder).unwrap();
    let refused = session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            MeasuredOn::Itself,
            valley_or_smear(None),
        ))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("measured on one named file"), "{refused}");

    session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            file("sample1_FMX.fcs"),
            valley_or_smear(Some("sample2_FS.fcs")),
        ))
        .unwrap();
    let stored = Session::open(&folder)
        .unwrap()
        .rules()
        .unwrap()
        .get(&RuleTarget::named("Pos"))
        .unwrap()
        .clone();
    // The metadata names the files sample1 and sample2.
    assert_eq!(stored.measured_on, file("sample1"));
    assert_eq!(stored.rule, valley_or_smear(Some("sample2")));
}

/// On FSC-A, sample1 as in [`a_dip_and_a_smear_on_fsc`]; sample2 the same
/// negative, a gap to 2,000,000, 300 positives spread thin to 3,200,000 and
/// 900 more piled up at 3,600,000.
fn a_dip_and_thin_positives_on_fsc(dir: &std::path::Path) {
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    let negative = Normal::new(1_000_000.0f32, 150_000.0).unwrap();
    let positive = Normal::new(3_000_000.0f32, 150_000.0).unwrap();
    let spread = rand_distr::Uniform::new(2_000_000.0f32, 3_200_000.0).unwrap();
    let piled = Normal::new(3_600_000.0f32, 60_000.0).unwrap();
    for (seed, file) in [(1, "sample1_FMX.fcs"), (2, "sample2_FS.fcs")] {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed + 300);
        let rows: Vec<Vec<f32>> = events(seed, 16_200)
            .into_iter()
            .enumerate()
            .map(|(at, mut row)| {
                row[0] = match (at < 15_000, seed, at < 15_300) {
                    (true, _, _) => negative.sample(&mut rng),
                    (false, 1, _) => positive.sample(&mut rng),
                    (false, _, true) => spread.sample(&mut rng),
                    (false, _, false) => piled.sample(&mut rng),
                };
                row
            })
            .collect();
        write_fcs(&dir.join(file), &channels, &rows);
    }
}

/// Sample2's thin positives stand too low beside the negative to count, so
/// the rule walks past the gap below them to the dip below the pile, above
/// 3,200,000; told to take the lowest point it walked past, it reads the dip
/// in the gap, between the negative's last cells near 1,600,000 and the
/// positives' first at 2,000,000. The run keeps the dip it read.
#[test]
fn a_valley_rule_can_gate_thin_positives_at_the_lowest_point_below_them() {
    let dip_on_sample2 = |name: &str, lowest_before: bool| {
        let folder = workspace_of_rectangles(name, &[("p", "Pos", "", (2_000_000.0, 4_194_304.0))]);
        a_dip_and_thin_positives_on_fsc(&folder);
        let mut session = Session::open(&folder).unwrap();
        let rule = clingate_core::gate_rules::rule::Rule::ValleyOrSmear(
            clingate_core::gate_rules::rule::ValleyOrSmearRule {
                lowest_before,
                ..Default::default()
            },
        );
        let written = session
            .update_rule(change("Pos", None, "FSC-A", file("sample1_FMX.fcs"), rule))
            .unwrap();
        assert_eq!(
            written
                .now
                .contains("at the lowest point between the negative and that dip"),
            lowest_before,
            "{}",
            written.now
        );
        session.preview_rules().unwrap();
        session.apply_previewed_rules().unwrap();
        let record = clingate_core::review::RunRecord::load(&folder)
            .unwrap()
            .unwrap();
        let placed = record
            .placed
            .iter()
            .find(|p| {
                p.gate == "Pos"
                    && p.sample
                        .name
                        .as_deref()
                        .is_some_and(|n| n.contains("sample2"))
            })
            .unwrap_or_else(|| panic!("{record:#?}"));
        let (_, here) = placed.valley.as_ref().expect("placed in a dip");
        here.bottom
    };
    let first = dip_on_sample2("session-valley-thin-first", false);
    assert!(first > 3_200_000.0, "{first}");
    let lowest = dip_on_sample2("session-valley-thin-lowest", true);
    assert!((1_600_000.0..2_000_000.0).contains(&lowest), "{lowest}");
}

/// Pos drawn from 2,000,000, in sample1's dip. Sample2 is a smear: with no
/// example it is left unplaced, saying so; gated by hand and named the
/// example, it is a reference too, and nothing is left unplaced.
#[test]
fn a_smear_waits_for_an_example_gated_by_hand() {
    let folder = workspace_of_rectangles(
        "session-valley-or-smear-example",
        &[("p", "Pos", "", (2_000_000.0, 4_194_304.0))],
    );
    a_dip_and_a_smear_on_fsc(&folder);
    let mut session = Session::open(&folder).unwrap();
    session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            file("sample1"),
            valley_or_smear(None),
        ))
        .unwrap();
    let preview = session.preview_rules().unwrap();
    assert!(
        preview
            .not_positioned
            .iter()
            .any(|n| n.reason.contains("no smear gated by hand")),
        "{preview:?}"
    );

    session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            file("sample1"),
            valley_or_smear(Some("sample2")),
        ))
        .unwrap();
    let preview = session.preview_rules().unwrap();
    assert!(preview.not_positioned.is_empty(), "{preview:?}");
    let mut references: Vec<&str> = preview
        .references
        .iter()
        .map(|r| r.specimen.as_str())
        .collect();
    references.sort();
    assert_eq!(references, ["one", "two"]);
}

/// Asked for a dip deeper than any can be, sample1's clear dip reads as
/// none: the reference is a smear, its own example, so sample2 - a smear
/// that would wait for one gated by hand - is placed from it.
#[test]
fn a_rule_asking_for_a_deeper_dip_reads_a_shallower_one_as_a_smear() {
    let folder = workspace_of_rectangles(
        "session-valley-or-smear-smallest-dip",
        &[("p", "Pos", "", (2_000_000.0, 4_194_304.0))],
    );
    a_dip_and_a_smear_on_fsc(&folder);
    let mut session = Session::open(&folder).unwrap();
    let rule = clingate_core::gate_rules::rule::Rule::ValleyOrSmear(
        clingate_core::gate_rules::rule::ValleyOrSmearRule {
            smallest_dip: Some(1.0),
            ..Default::default()
        },
    );
    let written = session
        .update_rule(change("Pos", None, "FSC-A", file("sample1"), rule))
        .unwrap();
    assert!(
        written.now.contains("a dip under 100% deep read as none"),
        "{}",
        written.now
    );
    let preview = session.preview_rules().unwrap();
    assert!(preview.not_positioned.is_empty(), "{preview:?}");
    let two = preview
        .would_move
        .iter()
        .find(|m| m.specimen == "two")
        .unwrap_or_else(|| panic!("{preview:?}"));
    assert_eq!(two.measured_on, "sample1_FMX.fcs");
}

/// One peak on FSC-A and nothing else, 200,000 wide: at 2,000,000 on
/// sample1 and 2,200,000 on sample2.
fn one_peak_each_on_fsc(dir: &std::path::Path) {
    let mut channels = vec!["FSC-A", "SSC-A"];
    channels.extend(FLUORESCENCE);
    for (seed, file, centre) in [
        (1, "sample1_FMX.fcs", 2_000_000.0f32),
        (2, "sample2_FS.fcs", 2_200_000.0),
    ] {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed + 300);
        let peak = Normal::new(centre, 200_000.0).unwrap();
        let rows: Vec<Vec<f32>> = events(seed, 20_000)
            .into_iter()
            .map(|mut row| {
                row[0] = peak.sample(&mut rng);
                row
            })
            .collect();
        write_fcs(&dir.join(file), &channels, &rows);
    }
}

/// No dip on the reference, sample1, so it is the example: Pos sits two
/// widths above its peak, and goes two widths above sample2's, at 2,600,000 -
/// to within half a width, the peak and width being read off a smoothed
/// density, each good to a tenth or so.
#[test]
fn a_reference_that_is_a_smear_places_the_other_sample_from_itself() {
    let folder = workspace_of_rectangles(
        "session-valley-or-smear-smeary-reference",
        &[("p", "Pos", "", (2_400_000.0, 4_194_304.0))],
    );
    one_peak_each_on_fsc(&folder);
    let mut session = Session::open(&folder).unwrap();
    session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            file("sample1"),
            valley_or_smear(None),
        ))
        .unwrap();
    let preview = session.preview_rules().unwrap();
    assert!(preview.not_positioned.is_empty(), "{preview:?}");
    session.apply_previewed_rules().unwrap();
    let (lower, _) = fsc_span(&session, "Pos", "fs");
    assert!((lower - 2_600_000.0).abs() < 100_000.0, "{lower}");
    assert_eq!(
        fsc_span(&session, "Pos", "fmx").0,
        2_400_000.0,
        "the reference"
    );
}

/// A gating file with no position for any one sample - a template, every
/// gate as drawn - takes the positions a run gives each specimen into the
/// save and the export, and opens on them again.
#[test]
fn positions_a_run_gives_are_saved_and_exported_from_a_file_with_none() {
    let folder = workspace_of_rectangles(
        "session-run-positions-exported",
        &[("p", "Pos", "", (2_400_000.0, 4_194_304.0))],
    );
    one_peak_each_on_fsc(&folder);
    // sample3 is in the metadata, of sample2's specimen, but not in the
    // workspace: not part of this gating task, so never written.
    write_metadata(
        &folder.join("metadata.csv"),
        &["test", "Type", "SampleType"],
        &[
            ("sample1", "sample1_FMX.fcs", &["one", "one", "FMX"]),
            ("sample2", "sample2_FS.fcs", &["two", "two", "FS"]),
            ("sample3", "sample3_U.fcs", &["two", "two", "U"]),
        ],
    );
    let mut session = Session::open(&folder).unwrap();
    session
        .update_rule(change(
            "Pos",
            None,
            "FSC-A",
            file("sample1"),
            valley_or_smear(None),
        ))
        .unwrap();
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let placed = fsc_span(&session, "Pos", "fs").0;
    assert!(
        (placed - 2_400_000.0).abs() > 100_000.0,
        "{placed}: the run moved it"
    );

    session.save().unwrap();
    let exported = session.export("positions.omiqgt", false).unwrap();
    let document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&exported.file).unwrap()).unwrap();
    let pos = &document["tree"]["filterContainers"]["p"];
    assert_eq!(pos["defaultFilter"]["min"]["f1Val"], 2_400_000.0, "{pos}");
    let for_sample2 = pos["perFileFilters"]["sample2"]["min"]["f1Val"]
        .as_f64()
        .unwrap_or_else(|| panic!("no position for sample2: {pos}"));
    assert!(
        (for_sample2 - placed).abs() < 1.0,
        "{for_sample2} against {placed}"
    );
    assert!(pos["perFileFilters"].get("sample3").is_none(), "{pos}");

    let reopened = Session::open(&folder).unwrap();
    assert!((fsc_span(&reopened, "Pos", "fs").0 - placed).abs() < 1.0);
}

/// A phenotype rule for Tmem, read on sample1: BUV805-A and BUV661-A
/// positive, BV785-A negative.
fn phenotype_run_in(folder: std::path::PathBuf) -> clingate_core::session::RulesPreview {
    let rule = clingate_core::gate_rules::rule::Rule::MatchThePhenotype(
        clingate_core::gate_rules::rule::PhenotypeRule {
            markers: vec!["BUV805-A".into(), "BUV661-A".into(), "BV785-A".into()],
            ..Default::default()
        },
    );
    let mut session = Session::open(&rule_in(folder, rule.clone())).unwrap();
    session
        .update_rule(change("Tmem", None, "", file("sample1"), rule))
        .unwrap();
    session.preview_rules().unwrap()
}

/// Tmem's span on each axis it is bounded on, on `sample`.
fn tmem_spans(session: &Session, sample: &str) -> Vec<(String, f64)> {
    session
        .gate("Tmem", Some(sample))
        .unwrap()
        .extent
        .iter()
        .filter_map(|e| Some((e.parameter.clone(), e.upper? - e.lower?)))
        .collect()
}

/// A phenotype rule that only moves the gate: sample2's Tmem is placed on
/// the cells found there, the same size on every axis as sample1's, the
/// reference.
#[test]
fn a_phenotype_rule_that_moves_only_keeps_the_gate_s_size() {
    use clingate_core::gate_rules::rule::{PhenotypeRule, Rule, ShapeFit};
    let rule = Rule::MatchThePhenotype(PhenotypeRule {
        markers: vec!["BUV805-A".into(), "BUV661-A".into(), "BV785-A".into()],
        fit: ShapeFit::MoveOnly,
        ..Default::default()
    });
    let folder = tmem_workspace_beside_teff_naive("session-phenotype-move-only");
    let mut session = Session::open(&rule_in(folder, rule.clone())).unwrap();
    let written = session
        .update_rule(change("Tmem", None, "", file("sample1"), rule))
        .unwrap();
    assert!(written.now.contains("move it only"), "{}", written.now);
    let preview = session.preview_rules().unwrap();
    assert!(
        preview.would_move.iter().any(|m| m.specimen == "two"),
        "{preview:?}"
    );
    session.apply_previewed_rules().unwrap();
    let (reference, moved) = (tmem_spans(&session, "fmx"), tmem_spans(&session, "fs"));
    assert!(!reference.is_empty());
    for ((axis, drawn), (_, placed)) in reference.iter().zip(&moved) {
        assert!(
            (drawn - placed).abs() <= 1e-3 * drawn.abs(),
            "{axis}: {drawn} then {placed}"
        );
    }
}

/// Tmem's lower edge on BUV805-A on the reference and on sample2, whose
/// BUV805-A positives sit at 80,000 against the reference's 40,000 while its
/// negative is the same, after a keep-shape phenotype rule pinning `pinned`.
fn tmem_lower_edges(name: &str, pinned: &[&str]) -> (f64, f64) {
    use clingate_core::gate_rules::rule::{PhenotypeRule, Rule};
    let rule = Rule::MatchThePhenotype(PhenotypeRule {
        markers: vec!["BUV805-A".into(), "BUV661-A".into(), "BV785-A".into()],
        pinned: pinned.iter().map(|m| (*m).into()).collect(),
        ..Default::default()
    });
    let folder = tmem_workspace_of(
        name,
        [
            (20_000, 3, 40_000.0, (0.0, 300.0)),
            (20_000, 3, 80_000.0, (0.0, 300.0)),
        ],
    );
    let mut session = Session::open(&rule_in(folder, rule.clone())).unwrap();
    session
        .update_rule(change("Tmem", None, "", file("sample1"), rule))
        .unwrap();
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let lower = |sample| {
        session
            .gate("Tmem", Some(sample))
            .unwrap()
            .extent
            .iter()
            .find(|e| e.parameter == "BUV805-A")
            .and_then(|e| e.lower)
            .unwrap()
    };
    (lower("fmx"), lower("fs"))
}

/// The negative is the same on both samples, so a lower edge pinned to it
/// stays where it was drawn; carried in the gap below the brighter positives,
/// it rises with them.
#[test]
fn a_pinned_edge_stays_above_the_negative_wherever_the_positives_go() {
    let (drawn, pinned) = tmem_lower_edges("session-phenotype-pinned", &["BUV805-A"]);
    let (_, carried) = tmem_lower_edges("session-phenotype-not-pinned", &[]);
    assert!(
        (pinned - drawn).abs() < 0.1 * drawn.abs(),
        "{drawn} then {pinned}"
    );
    assert!(carried > drawn * 1.2, "{drawn} then {carried}");
}

/// BUV805-A's negative sits at 10,000 and is 1,500 wide - 1.29 and 0.13 on
/// the arcsinh scale - so Tmem's lower edge, drawn at 1.42, cuts its top a
/// width above its peak: the preview says so and asks for the user's word
/// before pinning it.
#[test]
fn an_edge_drawn_inside_the_negative_is_offered_for_pinning() {
    use clingate_core::gate_rules::rule::{PhenotypeRule, Rule};
    let rule = Rule::MatchThePhenotype(PhenotypeRule {
        markers: vec!["BUV805-A".into(), "BUV661-A".into(), "BV785-A".into()],
        ..Default::default()
    });
    let folder = tmem_workspace_of(
        "session-phenotype-could-pin",
        [
            (20_000, 3, 40_000.0, (10_000.0, 1_500.0)),
            (20_000, 3, 40_000.0, (10_000.0, 1_500.0)),
        ],
    );
    out_of_tmems_way(&folder.join("gating.omiqgt"));
    let mut session = Session::open(&rule_in(folder, rule.clone())).unwrap();
    session
        .update_rule(change("Tmem", None, "", file("sample1"), rule))
        .unwrap();
    let preview = session.preview_rules().unwrap();
    assert!(
        preview
            .could_pin
            .iter()
            .any(|line| line.starts_with("Tmem of 1: ")
                && line.contains("its edge on BUV805-A lies within the negative")
                && line.contains("change nothing until they say")),
        "{preview:?}"
    );
}

/// A run keeps, beside its other measures, how far the edges placed from
/// either half of a sample's events agree - for a gate whose edges are
/// carried, not one traced afresh.
#[test]
fn a_run_keeps_whether_a_phenotype_gate_s_edges_agree_between_halves() {
    use clingate_core::gate_rules::confidence::STEADY;
    use clingate_core::gate_rules::rule::{PhenotypeRule, Rule, ShapeFit};
    use clingate_core::review::RunRecord;
    for (fit, scored) in [(ShapeFit::KeepShape, true), (ShapeFit::DrawPolygon, false)] {
        let rule = Rule::MatchThePhenotype(PhenotypeRule {
            markers: vec!["BUV805-A".into(), "BUV661-A".into(), "BV785-A".into()],
            fit,
            ..Default::default()
        });
        let folder =
            tmem_workspace_beside_teff_naive(&format!("session-phenotype-halves-{}", fit.key()));
        let mut session = Session::open(&rule_in(folder.clone(), rule.clone())).unwrap();
        session
            .update_rule(change("Tmem", None, "", file("sample1"), rule))
            .unwrap();
        session.preview_rules().unwrap();
        session.apply_previewed_rules().unwrap();
        let record = RunRecord::load(&folder).unwrap().expect("kept on apply");
        let placed = record
            .placed
            .iter()
            .find(|p| p.gate == "Tmem")
            .unwrap_or_else(|| panic!("{record:#?}"));
        let agreement = placed.components.iter().find(|c| c.name == STEADY);
        assert_eq!(
            agreement.is_some(),
            scored,
            "{fit:?}: {:#?}",
            placed.components
        );
        if let Some(agreement) = agreement {
            assert!(agreement.score > 0.9, "{agreement:?}");
            assert!(agreement.detail.contains("the same cells"), "{agreement:?}");
        }
    }
}

/// Both samples a third positive: the same cells are found on sample2 and
/// its gate is moved onto them.
#[test]
fn a_phenotype_found_on_another_sample_moves_its_gate() {
    let preview = phenotype_run_in(tmem_workspace_beside_teff_naive("session-phenotype-found"));
    assert_eq!(preview.not_positioned.len(), 0, "{preview:?}");
    assert!(
        preview.would_move.iter().any(|m| m.specimen == "two"),
        "{preview:?}"
    );
}

/// Sample2 one in 30 positive against one in three on sample1: a tenth as
/// common, under the fifth a match needs, though Tmem's parent - about 5,700
/// of sample2's 100,000 events - holds some 190 of them, well over the 50 it
/// needs. Its gate is left alone and the preview says why.
#[test]
fn a_phenotype_much_rarer_than_on_the_reference_leaves_the_gate_alone() {
    let preview = phenotype_run_in(tmem_workspace_positive_one_in(
        "session-phenotype-rare",
        [(20_000, 3), (100_000, 30)],
    ));
    assert!(
        preview.would_move.iter().all(|m| m.specimen != "two"),
        "{preview:?}"
    );
    let refused = preview
        .not_positioned
        .iter()
        .find(|n| n.sample.as_deref().is_some_and(|s| s.contains("sample2")))
        .unwrap_or_else(|| panic!("{preview:?}"));
    assert!(
        refused.reason.contains("under a fifth as common")
            && refused.reason.contains("left where it is"),
        "{refused:?}"
    );
}

/// Scored against the gating as drawn, each sample's lines are the ones a
/// preview of the same rules moves the gate from and to; scoring moves
/// nothing, and one population's rule can be scored alone.
#[test]
fn the_rules_are_scored_against_the_gating_as_drawn() {
    let folder = with_rules("session-score");
    let mut session = Session::open(&folder).unwrap();
    let scored = session.score_rules(None, None, Default::default()).unwrap();
    let preview = session.preview_rules().unwrap();

    assert_eq!(scored.gates.len(), 1);
    assert!(!preview.would_move.is_empty());
    assert_eq!(scored.gates[0].gate, preview.would_move[0].gate);
    for moved in &preview.would_move {
        let row = scored
            .rows
            .iter()
            .find(|r| r.file == moved.measured_on)
            .unwrap_or_else(|| panic!("{} not scored: {scored:#?}", moved.measured_on));
        assert_eq!(row.what, "moved");
        assert_eq!(row.hand_edge, Some(moved.from), "{row:?}");
        assert_eq!(row.rule_edge, Some(moved.to), "{row:?}");
        assert_eq!(row.edge_off_iqrs.unwrap().signum(), (moved.to - moved.from).signum());
        let events = row.events.expect("both gates counted");
        assert!(events.both <= events.hand.min(events.rule), "{row:?}");
        let agreement = row.agreement.unwrap();
        assert!((0.0..=1.0).contains(&agreement), "{row:?}");
    }
    let kept: Vec<_> = scored.rows.iter().filter(|r| r.what == "kept").collect();
    assert_eq!(kept.len(), preview.already_in_place.len(), "{scored:#?}");
    assert!(kept.iter().all(|r| r.agreement == Some(1.0) && r.edge_off_iqrs == Some(0.0)));
    assert_eq!(scored.rows_total, scored.rows.len());

    // Nothing moved: a second preview proposes the same.
    let again = session.preview_rules().unwrap();
    let lines = |p: &clingate_core::session::RulesPreview| {
        p.would_move.iter().map(|m| (m.from, m.to)).collect::<Vec<_>>()
    };
    assert_eq!(lines(&again), lines(&preview));

    let first = session.score_rules(None, Some(1), Default::default()).unwrap();
    assert_eq!(first.rows.len(), 1);
    assert_eq!(first.rows_total, scored.rows_total);
    assert_eq!(first.rows[0], scored.rows[0], "the furthest off first");
    assert!(session.score_rules(Some("no such gate"), None, Default::default()).is_err());
}

/// With rules for two gates, asking for one scores that gate alone.
#[test]
fn one_population_s_rule_is_scored_alone() {
    let folder = with_rules("session-score-one");
    let mut session = Session::open(&folder).unwrap();
    let parameter = session.gate("teff_naive", None).unwrap().parameters[0].clone();
    session
        .update_rule(change(
            "teff_naive",
            None,
            &parameter,
            clingate_core::gate_rules::rule_store::MeasuredOn::Itself,
            clingate_core::gate_rules::rule::Rule::TailFraction(
                clingate_core::gate_rules::rule::TailFractionRule::new((0.05, 0.1)),
            ),
        ))
        .unwrap();
    let every = session.score_rules(None, None, Default::default()).unwrap();
    assert_eq!(every.gates.len(), 2, "{every:#?}");
    let tmem = session.score_rules(Some("Tmem"), None, Default::default()).unwrap();
    assert_eq!(tmem.gates.len(), 1);
    let gate = &tmem.gates[0];
    assert!(gate.gate.starts_with("Tmem"), "{gate:?}");
    assert!(tmem.rows.iter().all(|r| r.gate_id == gate.gate_id));
    assert_eq!(
        tmem.rows_total,
        every.rows.iter().filter(|r| r.gate_id == gate.gate_id).count()
    );
}

/// The off line and its allowance for few events are the caller's: each
/// scored sample is judged against the line asked for, every sample below it
/// is counted off, and settings out of range are refused.
#[test]
fn how_a_score_is_judged_can_be_set() {
    use clingate_core::gate_rules::score::ScoreSettings;
    let folder = with_rules("session-score-settings");
    let session = Session::open(&folder).unwrap();
    let exact = ScoreSettings {
        off_below: 1.0,
        noise_widths: 0.0,
    };
    let scored = session.score_rules(None, Some(400), exact).unwrap();
    let judged: Vec<_> = scored.rows.iter().filter(|r| r.agreement.is_some()).collect();
    assert!(!judged.is_empty(), "{scored:#?}");
    assert!(judged.iter().all(|r| r.off_line == Some(1.0)), "{scored:#?}");
    let short = judged.iter().filter(|r| r.agreement < r.off_line).count();
    assert_eq!(scored.gates[0].off, short);
    let lower = ScoreSettings {
        off_below: 0.5,
        noise_widths: 0.0,
    };
    let rows = session.score_rules(None, Some(400), lower).unwrap().rows;
    assert!(rows.iter().filter(|r| r.agreement.is_some()).all(|r| r.off_line == Some(0.5)));
    for refused in [(1.5, 0.0), (0.8, -1.0)] {
        let settings = ScoreSettings {
            off_below: refused.0,
            noise_widths: refused.1,
        };
        assert!(session.score_rules(None, None, settings).is_err(), "{refused:?}");
    }
}

#[test]
fn scoring_needs_rules() {
    let session = Session::open(&workspace("session-score-none")).unwrap();
    assert!(session.score_rules(None, None, Default::default()).is_err());
}

/// The rule's own settings searched by default: every candidate scored as
/// score_rules scores it once it is the workspace's rule, the rule as it
/// stands among them, and nothing moved.
#[test]
fn a_rule_s_settings_are_searched_against_the_gating_as_drawn() {
    use clingate_core::session::FitAsk;
    let folder = with_rules("session-fit");
    let mut session = Session::open(&folder).unwrap();
    let before = session.preview_rules().unwrap();
    let ask = FitAsk {
        defaults: true,
        shown: Some(64),
        ..FitAsk::default()
    };
    let found = session
        .fit_rule("Tmem", ask, Default::default(), Default::default())
        .unwrap();

    // Five widths of the band, each aimed two ways; the band as it stands is one.
    assert_eq!(found.candidates_total, 10, "{found:#?}");
    assert_eq!(found.candidates.iter().filter(|c| c.current).count(), 1);
    assert!(found.checked_on.is_empty(), "two specimens are too few to split");
    assert_eq!(found.fit_on, ["one", "two"]);
    let places: Vec<usize> = found.candidates.iter().map(|c| c.place_by_typical).collect();
    assert_eq!(places, (1..=10).collect::<Vec<_>>());
    let lines = |p: &clingate_core::session::RulesPreview| {
        p.would_move.iter().map(|m| (m.from, m.to)).collect::<Vec<_>>()
    };
    assert_eq!(lines(&session.preview_rules().unwrap()), lines(&before));

    for candidate in [&found.candidates[0], found.candidates.last().unwrap()] {
        let rule = candidate.rule.clone();
        session
            .update_rule(change("Tmem", None, &rule.parameter, rule.measured_on.clone(), rule.rule))
            .unwrap();
        let scored = session.score_rules(Some("Tmem"), None, Default::default()).unwrap();
        assert_eq!(candidate.fit.as_ref(), scored.gates.first(), "{}", candidate.said);
    }
}

/// Candidates given are tried beside the rule as it stands, named as the
/// tools name a parameter, without the default settings unless asked for
/// too; fewer can be shown than were tried.
#[test]
fn candidates_given_are_tried_beside_the_rule_as_it_stands() {
    use clingate_core::gate_rules::rule::{Rule, ValleyRule};
    use clingate_core::gate_rules::rule_store::{Bound, GateRule, MeasuredOn};
    use clingate_core::session::FitAsk;
    let folder = with_rules("session-fit-given");
    let session = Session::open(&folder).unwrap();
    let channel = session.gate("Tmem", None).unwrap().parameters[0].clone();
    let valley = GateRule {
        parameter: channel.trim_end_matches("-A").into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::Itself,
        rule: Rule::InTheValley(ValleyRule::default()),
    };
    let ask = |defaults, shown| FitAsk {
        candidates: vec![valley.clone()],
        defaults,
        shown,
    };
    let given = session
        .fit_rule("Tmem", ask(false, None), Default::default(), Default::default())
        .unwrap();
    assert_eq!(given.candidates_total, 2, "{given:#?}");
    let tried = given.candidates.iter().find(|c| !c.current).unwrap();
    assert_eq!(&*tried.rule.parameter, channel.as_str(), "read by its channel");

    let with_defaults = session
        .fit_rule("Tmem", ask(true, Some(3)), Default::default(), Default::default())
        .unwrap();
    assert_eq!(with_defaults.candidates_total, 11, "the valley and the band's ten");
    assert_eq!(with_defaults.candidates.len(), 3);
}

/// Ranked by samples off when asked; refused with nothing to try, or with
/// settings out of range.
#[test]
fn a_search_is_ranked_as_asked_and_refused_with_nothing_to_try() {
    use clingate_core::gate_rules::fit::{FitSettings, RankBy};
    use clingate_core::gate_rules::score::ScoreSettings;
    use clingate_core::session::FitAsk;
    let folder = with_rules("session-fit-ranked");
    let session = Session::open(&folder).unwrap();
    let defaults = || FitAsk {
        defaults: true,
        shown: Some(64),
        ..FitAsk::default()
    };
    let by_off = FitSettings {
        rank_by: RankBy::Off,
        ..FitSettings::default()
    };
    let found = session
        .fit_rule("Tmem", defaults(), Default::default(), by_off)
        .unwrap();
    let places: Vec<usize> = found.candidates.iter().map(|c| c.place_by_off).collect();
    assert_eq!(places, (1..=found.candidates_total).collect::<Vec<_>>());

    let unruled = session.fit_rule("teff_naive", defaults(), Default::default(), Default::default());
    assert!(unruled.unwrap_err().to_string().contains("give candidate rules"));
    let wide = FitSettings {
        tie_within: 2.0,
        ..FitSettings::default()
    };
    assert!(session.fit_rule("Tmem", defaults(), Default::default(), wide).is_err());
    let off_line = ScoreSettings {
        off_below: 1.5,
        noise_widths: 0.0,
    };
    assert!(session.fit_rule("Tmem", defaults(), off_line, Default::default()).is_err());
    assert!(session.fit_rule("no such gate", defaults(), Default::default(), Default::default()).is_err());
}
