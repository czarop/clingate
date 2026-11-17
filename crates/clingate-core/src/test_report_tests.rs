use std::sync::atomic::AtomicBool;

use crate::gate_rules::pick::PickSettings;
use crate::gate_rules::run::RunInputs;
use crate::gates::GateState;
use crate::test_report::{FOLDER, Report, TestJob, keep, test_report};

fn empty(workspace: &str) -> TestJob {
    TestJob {
        workspace: workspace.into(),
        made: "today".into(),
        gates: GateState::default(),
        inputs: RunInputs::default(),
        settings: PickSettings::default(),
    }
}

#[test]
fn a_step_that_panics_is_a_problem_in_the_report_not_a_crash() {
    let mut report = Report::default();

    let answer: Result<(), String> = report.timed("Exploding", || panic!("boom"));

    assert_eq!(answer.unwrap_err(), "it panicked: boom");
    assert!(report.finish().contains("| Exploding |"));
}

#[test]
fn a_report_with_nothing_to_pick_says_so_and_names_its_workspace_safely() {
    let report = test_report(&empty("plate|one"), &AtomicBool::new(false), |_| {}).unwrap();

    assert!(report.contains("| Workspace | plate\\|one |"), "{report}");
    assert!(
        report.contains("Nothing was picked, so nothing is scored."),
        "{report}"
    );
    assert!(report.contains("## Problems"), "{report}");
}

#[test]
fn a_report_asked_to_stop_is_not_made() {
    let stopped = test_report(&empty("w"), &AtomicBool::new(true), |_| {});

    assert_eq!(stopped.unwrap_err(), "stopped");
}

#[test]
fn a_report_is_kept_in_the_workspace_named_by_when_it_was_made() {
    let folder = crate::file_load_tests::scratch("test-report-keep");

    let path = keep(&folder, "# report", "2026-10-14_103000").unwrap();

    assert_eq!(
        path,
        folder.join(FOLDER).join("test-report-2026-10-14_103000.md")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# report");
}

/// Each population with rules, against each sibling: how many rules a copy
/// would write and what it would leave out - read off the fixture: 2's Tmem
/// is drawn as 1's, its teff_naive on BV785-A, not the rule's BUV805-A.
#[test]
fn the_report_says_what_copying_rules_to_siblings_would_do() {
    use crate::session::Session;
    let folder = crate::test_workspace::two_samples_with_siblings("test-report-copies");
    let session = Session::open(&folder).unwrap();
    let job = session.test_job("now".into()).unwrap();

    let section = crate::test_report::copies(&job);

    assert!(
        section.contains("## Copying rules to siblings"),
        "{section}"
    );
    assert!(
        section.contains(
            "| 1 | 2 | 1 | teff_naive: the gate is drawn on BV785-A and BUV563-A, not BUV805-A |"
        ),
        "{section}"
    );
}

/// The fixture's spill and autofluorescence, found in the first population
/// at the top of the gating - see `test_workspace::spilling_events`.
#[test]
fn the_report_tells_the_spill_from_the_autofluorescence() {
    use crate::session::Session;
    use crate::test_workspace::{GLOWS_IN, SPILLS_FROM, SPILLS_INTO};
    let folder = crate::test_workspace::spilling_samples("test-report-spillover");
    let job = Session::open(&folder)
        .unwrap()
        .test_job("now".into())
        .unwrap();
    let mut report = Report::default();

    let steps = std::sync::Mutex::new(Vec::new());

    let section =
        crate::test_report::spillover(&job, &AtomicBool::new(false), &mut report, &|step| {
            steps.lock().unwrap().push(step)
        });

    assert!(
        section.contains("Looked in 1 on 4 of 4 specimens: 4 stained and 2 unstained files."),
        "{section}"
    );
    assert!(
        section.contains(&format!(
            "- {SPILLS_INTO} rises with {SPILLS_FROM} alike on the 4 samples read"
        )),
        "{section}"
    );
    assert!(
        section.contains(&format!("look in {GLOWS_IN}, where they stand out most")),
        "{section}"
    );
    // Before the first file, then as each of the six is read, in whatever
    // order the reads finish.
    let mut read: Vec<usize> = steps
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|step| match step {
            crate::test_report::TestStep::CheckingSpillover { done, total: 6 } => done,
            other => panic!("{other:?}"),
        })
        .collect();
    read.sort();
    assert_eq!(read, [0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(
        crate::test_report::TestStep::CheckingSpillover { done: 6, total: 6 }.fraction(),
        0.1
    );
}

#[test]
fn the_report_says_nothing_was_checked_without_gating() {
    let section = crate::test_report::spillover(
        &empty("w"),
        &AtomicBool::new(false),
        &mut Report::default(),
        &|_| {},
    );

    assert!(
        section.contains("No gating is loaded, so nothing was checked."),
        "{section}"
    );
}
