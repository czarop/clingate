use std::sync::Arc;

use rustc_hash::FxBuildHasher;

use crate::feature_checks::{
    Check, CheckInputs, Checked, FEATURES, Feedback, Tried, feedback_markdown, load_feedback,
    run_check, save_feedback,
};
use crate::gate_rules::rule::{Rule, TailFractionRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::RunInputs;
use crate::session::Session;

fn checked_on_workspace(folder: &std::path::Path, check: Check) -> Checked {
    let session = Session::open(folder).unwrap();
    session
        .check_features()
        .features
        .into_iter()
        .find(|f| f.feature.check == Some(check))
        .and_then(|f| f.checked)
        .unwrap()
}

#[test]
fn every_feature_has_steps_something_to_send_back_and_its_own_id() {
    let mut ids: Vec<&str> = FEATURES.iter().map(|f| f.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), FEATURES.len());
    for feature in FEATURES {
        assert!(!feature.steps.is_empty(), "{}", feature.id);
        assert!(!feature.send_back.is_empty(), "{}", feature.id);
    }
}

#[test]
fn a_rectangle_of_the_workspace_made_a_polygon_is_saved_and_read_back_as_one() {
    let folder = crate::test_workspace::two_samples("check-polygon");

    let checked = checked_on_workspace(&folder, Check::PolygonSaved);

    assert!(checked.passed, "{checked:?}");
    assert!(
        checked.said[0].ends_with("made a polygon, saved as PolygonGate and read back a polygon"),
        "{checked:?}"
    );
}

#[test]
fn the_gating_saved_and_read_back_saves_the_same_again() {
    let folder = crate::test_workspace::two_samples("check-gating");

    let checked = checked_on_workspace(&folder, Check::GatingSaved);

    assert!(checked.passed, "{checked:?}");
}

#[test]
fn the_copies_to_siblings_are_listed_with_what_each_leaves_out() {
    let folder = crate::test_workspace::two_samples_with_siblings("check-siblings");

    let checked = checked_on_workspace(&folder, Check::SiblingCopies);

    assert_eq!(
        checked.said,
        [
            "1 to 2: 1 would be copied, 1 left out",
            "  - teff_naive of 2: the gate is drawn on BV785-A and BUV563-A, not BUV805-A",
        ]
    );
}

/// The names a rules file holds, as a run reads them: a marker its channel,
/// a file name its metadata row, and a name neither refused.
#[test]
fn the_rules_names_are_said_as_a_run_reads_them_and_one_it_cannot_fails() {
    use crate::AxisInfo;
    use crate::axis_store::Param;
    use crate::omiq::metadata::MetaDataStore;
    let dir = crate::file_load_tests::scratch("check-names");
    let csv = dir.join("metadata.csv");
    std::fs::write(&csv, "OmiqID,Filename,test\nsample1,sample1_FMX.fcs,one\n").unwrap();
    let (metadata, _) = MetaDataStore::read_omiq(csv).unwrap();
    let mut axes = im::HashMap::with_hasher(FxBuildHasher);
    axes.insert(
        Arc::from("BUV805-A"),
        AxisInfo {
            param: Param {
                marker: Arc::from("CD69"),
                fluoro: Arc::from("BUV805-A"),
            },
            ..AxisInfo::default()
        },
    );
    let band = |parameter: &str, on: MeasuredOn| GateRule {
        parameter: parameter.into(),
        bound: Bound::Above,
        measured_on: on,
        rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
    };
    let mut rules = RuleStore::default();
    rules.insert(
        RuleTarget::named("a"),
        band("cd69", MeasuredOn::File("sample1_FMX.fcs".into())),
    );
    rules.insert(RuleTarget::named("b"), band("CD999", MeasuredOn::Itself));
    rules.insert(RuleTarget::named("c"), band("BUV805-A", MeasuredOn::Itself));
    let inputs = RunInputs::assemble(None, &Default::default(), &metadata, &axes, &rules);
    let gates = crate::gates::GateState::default();

    let checked = run_check(
        Check::RuleNames,
        &CheckInputs {
            gates: &gates,
            inputs: &inputs,
            axes: &axes,
            rules: &rules,
        },
    );

    assert!(!checked.passed);
    assert_eq!(
        checked.said,
        [
            "a: parameter cd69 is read as BUV805-A",
            "a: the reference sample1_FMX.fcs is read as sample1",
            "b: parameter CD999 is not in this workspace - the run refuses it",
        ]
    );
}

#[test]
fn feedback_is_kept_in_the_workspace_and_sent_on_with_each_check() {
    let folder = crate::file_load_tests::scratch("check-feedback");
    let mut feedback = crate::feature_checks::Feedbacks::new();
    feedback.insert(
        "copy_rules".into(),
        Feedback {
            tried: Tried::Problem,
            note: "IL2 was left out".into(),
            checked: Some(Checked {
                passed: false,
                said: vec!["1 to 2: 0 would be copied, 1 left out".into()],
            }),
        },
    );

    save_feedback(&folder, &feedback).unwrap();
    let read = load_feedback(&folder);
    let markdown = feedback_markdown(&read);

    assert_eq!(read, feedback);
    assert!(
        markdown.contains(
            "## Copying rules to sibling populations\n\n- Where: Gate Rules tab - \"Copy rules to sibling populations\"; copy_rules for Claude\n- Tried: Problem\n- Said: IL2 was left out\n- Check: FAILED\n  - 1 to 2: 0 would be copied, 1 left out\n"
        ),
        "{markdown}"
    );
    assert!(markdown.contains("## Picking the best rule for every gate\n\n- Where: Gate Rules tab - \"Pick the best rule\"; pick_rule for Claude\n- Tried: Not tried\n"), "{markdown}");
}

#[test]
fn the_spillover_check_says_which_files_it_takes_as_unstained() {
    let folder = crate::test_workspace::spilling_samples("check-spillover");

    let checked = checked_on_workspace(&folder, Check::SpilloverReads);

    assert!(checked.passed, "{checked:?}");
    assert_eq!(
        checked.said[0],
        format!("8 fluorescence channels: {}", {
            let mut channels = crate::test_workspace::FLUORESCENCE.to_vec();
            channels.sort();
            channels.join(", ")
        })
    );
    assert_eq!(checked.said[1], "4 stained files");
    assert_eq!(checked.said[2], "2 unstained: ua_U.fcs, ub_U.fcs");
}

#[test]
fn the_spillover_check_says_when_no_file_is_unstained() {
    let folder = crate::test_workspace::two_samples("check-spillover-none");

    let checked = checked_on_workspace(&folder, Check::SpilloverReads);

    assert!(checked.passed, "{checked:?}");
    assert!(
        checked.said[2].starts_with("no file's sample type"),
        "{checked:?}"
    );
}

#[test]
fn the_spillover_check_fails_without_side_scatter() {
    let folder = crate::test_workspace::spilling_samples("check-spillover-no-ssc");
    let scaling = folder.join("scaling.csv");
    let without: String = std::fs::read_to_string(&scaling)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("SSC-A"))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&scaling, without).unwrap();

    let checked = checked_on_workspace(&folder, Check::SpilloverReads);

    assert!(!checked.passed, "{checked:?}");
    assert!(
        checked
            .said
            .last()
            .unwrap()
            .starts_with("the files have no SSC-A"),
        "{checked:?}"
    );
}

#[test]
fn feedback_that_cannot_be_read_is_kept_beside_the_new_not_written_over() {
    let folder = crate::file_load_tests::scratch("feedback-unreadable");
    let file = crate::feature_checks::feedback_file(&folder);
    crate::workspace::make_parent(&file).unwrap();
    std::fs::write(&file, "{ half written").unwrap();
    let mut said = crate::feature_checks::Feedbacks::new();
    said.insert("copy_rules".into(), Feedback::default());

    save_feedback(&folder, &said).unwrap();

    assert_eq!(
        std::fs::read_to_string(file.with_extension("unreadable.json")).unwrap(),
        "{ half written"
    );
    assert_eq!(load_feedback(&folder), said);
}
