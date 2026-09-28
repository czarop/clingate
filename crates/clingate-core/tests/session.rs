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
    use clingate_core::gate_rules::rule::{Rule, TailFractionRule};
    use clingate_core::gate_rules::rule_store::{
        Bound, GateRule, MeasuredOn, RuleStore, RuleTarget, SamplePairing,
    };
    let dir = workspace(name);
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
            rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
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
