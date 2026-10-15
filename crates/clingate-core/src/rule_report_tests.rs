use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

use crate::gate_rules::rule::{Rule, TailFractionRule};
use crate::gate_rules::rule_store::RuleTarget;
use crate::review::RunRecord;
use crate::rule_report::{Picture, RuleReport, RuleReportJob, keep, rule_report};
use crate::session::Session;
use crate::test_report::FOLDER;

/// The two-sample workspace with its Tmem rule run and applied, the FMX's
/// placement unsure and the full stain's sure, both in band: only the FMX is
/// flagged, for its confidence.
fn applied(name: &str) -> (Session, RuleReportJob) {
    let folder = crate::test_workspace::two_samples_with_a_rule(name);
    let mut session = Session::open(&folder).unwrap();
    session.preview_rules().unwrap();
    session.apply_previewed_rules().unwrap();
    let mut record = RunRecord::load(&folder).unwrap().unwrap();
    for p in &mut record.placed {
        let fmx = p.sample.name.as_deref().unwrap().contains("FMX");
        p.confidence = if fmx { 0.05 } else { 0.9 };
        p.in_band = true;
    }
    record.save(&folder).unwrap();
    let job = session.rule_job("Tmem", "then".into()).unwrap();
    (session, job)
}

fn made(job: &RuleReportJob) -> RuleReport {
    rule_report(job, &AtomicBool::new(false), |_, _| {}).unwrap()
}

fn tile_of(picture: &Picture) -> image::RgbaImage {
    image::load_from_memory(&picture.png).unwrap().to_rgba8()
}

#[test]
fn the_flagged_come_first_numbered_as_their_plots_and_pictured_apart() {
    let (_, job) = applied("rule-report-order");

    let report = made(&job);

    let md = &report.markdown;
    assert_eq!(
        report.gate, "Tmem of 1",
        "the fixture's Tmem is under a gate named 1"
    );
    assert!(
        md.starts_with("# clingate rule report: Tmem of 1\n"),
        "{md}"
    );
    assert!(md.contains("| 1 | sample1_FMX.fcs | FMX | yes |"), "{md}");
    assert!(md.contains("| 2 | sample2_FS.fcs | FS |  |"), "{md}");
    assert!(
        md.contains("the rule's own confidence is 0.05"),
        "the flag's reason: {md}"
    );
    assert!(md.contains("- flagged-1.png: 1\n"), "{md}");
    assert!(md.contains("- not-flagged-1.png: 2\n"), "{md}");
    assert!(
        md.contains("- 2 placements: 2 moved by the rule, 0 already met it"),
        "{md}"
    );
    assert!(
        md.contains("- 1 flagged by the review, 0 of them judged to look right"),
        "{md}"
    );
    let names: Vec<&str> = report.pictures.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["flagged-1.png", "not-flagged-1.png"]);
    let ink = image::Rgba([0, 0, 0, 255]);
    for (picture, top_middle) in report.pictures.iter().zip([true, false]) {
        let tile = tile_of(picture);
        assert_eq!((tile.width(), tile.height()), (300, 300), "one plot each");
        // "1" is the font's middle pixel on its top row; "2" its whole row.
        assert!(*tile.get_pixel(6, 3) == ink);
        assert_eq!(*tile.get_pixel(3, 3) == ink, !top_middle);
    }
}

/// What the gate holds now is what the tools' own picture of the gate says
/// it holds on the same sample.
#[test]
fn the_gate_holds_now_what_the_tools_picture_says() {
    let (session, job) = applied("rule-report-holds");
    let picture = session.gate_picture("Tmem", Some("sample2"), None).unwrap();
    let caption = &picture.tiles[0];
    let (percent, events) = caption
        .split_once("the gate holds ")
        .unwrap()
        .1
        .split_once("% of ")
        .unwrap();
    let percent: f64 = percent.parse().unwrap();
    let events = events.trim_end_matches(" events");

    let md = made(&job).markdown;

    let row = md
        .lines()
        .find(|l| l.starts_with("| 2 |"))
        .unwrap()
        .to_string();
    assert!(
        row.contains(&format!("| {percent:.2}% of {events} |")),
        "{row} against {caption}"
    );
}

#[test]
fn a_rule_changed_since_the_run_is_said_beside_the_one_it_ran_with() {
    let (_, mut job) = applied("rule-report-changed");
    assert!(made(&job).markdown.contains("- Now: the same\n"));

    let mut rule = job
        .inputs
        .rules
        .get(&RuleTarget::named("Tmem"))
        .unwrap()
        .clone();
    rule.rule = Rule::TailFraction(TailFractionRule::new((0.05, 0.1)));
    job.inputs.rules.insert(RuleTarget::named("Tmem"), rule);
    let md = made(&job).markdown;

    let then = md.lines().find(|l| l.starts_with("- In the run:")).unwrap();
    let now = md.lines().find(|l| l.starts_with("- Now:")).unwrap();
    assert!(now.starts_with("- Now: changed to "), "{now}");
    assert_ne!(
        then["- In the run: ".len()..],
        now["- Now: changed to ".len()..]
    );
}

#[test]
fn a_sample_that_cannot_be_drawn_keeps_its_place_and_is_a_problem() {
    let (_, mut job) = applied("rule-report-undrawn");
    job.inputs.files.retain(|(name, _)| !name.contains("FMX"));

    let report = made(&job);

    assert!(
        report.markdown.contains(
            "- Sample 1 (sample1_FMX.fcs) could not be drawn: its file is not in the workspace"
        ),
        "{}",
        report.markdown
    );
    let flagged = tile_of(&report.pictures[0]);
    assert_eq!(
        *flagged.get_pixel(150, 150),
        image::Rgba([200, 200, 200, 255])
    );
    assert_eq!(
        *flagged.get_pixel(6, 3),
        image::Rgba([0, 0, 0, 255]),
        "still numbered"
    );
    assert!(
        report
            .markdown
            .contains("| 1 | sample1_FMX.fcs | FMX | yes |")
    );
    assert_eq!(report.pictures.len(), 2, "the other is drawn");
}

#[test]
fn ten_plots_take_two_pictures_of_nine_and_one() {
    let (_, job) = applied("rule-report-ten");
    let mut record = RunRecord::load(&job.folder).unwrap().unwrap();
    let template = record
        .placed
        .iter()
        .find(|p| p.sample.name.as_deref().unwrap().contains("FS"))
        .unwrap()
        .clone();
    for n in 0..9 {
        let mut p = template.clone();
        p.sample.id = format!("extra{n}");
        p.sample.name = Some(format!("extra{n}.fcs"));
        record.placed.push(p);
    }
    record.save(&job.folder).unwrap();

    let report = made(&job);

    let names: Vec<&str> = report.pictures.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["flagged-1.png", "not-flagged-1.png", "not-flagged-2.png"]
    );
    let first = tile_of(&report.pictures[1]);
    assert_eq!((first.width(), first.height()), (900, 900));
    let second = tile_of(&report.pictures[2]);
    assert_eq!((second.width(), second.height()), (300, 300));
    assert!(
        report.markdown.contains("- not-flagged-2.png: 11\n"),
        "{}",
        report.markdown
    );
}

#[test]
fn progress_hears_each_plot_drawn() {
    let (_, job) = applied("rule-report-progress");
    let heard = Mutex::new(Vec::new());

    rule_report(&job, &AtomicBool::new(false), |done, total| {
        heard.lock().unwrap().push((done, total))
    })
    .unwrap();

    let mut heard = heard.into_inner().unwrap();
    heard.sort();
    assert_eq!(heard, [(0, 2), (1, 2), (2, 2)]);
}

#[test]
fn no_report_without_a_run_on_the_gate_or_when_stopped() {
    let (_, mut job) = applied("rule-report-refused");
    assert_eq!(
        rule_report(&job, &AtomicBool::new(true), |_, _| {}).unwrap_err(),
        "stopped"
    );
    job.gate_id = "not-a-gate".into();
    let e = rule_report(&job, &AtomicBool::new(false), |_, _| {}).unwrap_err();
    assert!(e.contains("did not place this gate"), "{e}");
    std::fs::remove_file(
        job.folder
            .join(crate::review::REVIEWS_DIR)
            .join(crate::review::run_record::RUN_RECORD_FILE),
    )
    .unwrap();
    let e = rule_report(&job, &AtomicBool::new(false), |_, _| {}).unwrap_err();
    assert!(e.contains("No rules run has been applied"), "{e}");
}

#[test]
fn a_report_is_kept_in_a_folder_of_its_own_named_by_gate_and_when() {
    let folder = crate::file_load_tests::scratch("rule-report-keep");
    let report = RuleReport {
        gate: "CD4+ of T/cells".into(),
        markdown: "# report".into(),
        pictures: vec![Picture {
            name: "flagged-1.png".into(),
            png: vec![1, 2, 3],
        }],
    };

    let path = keep(&folder, &report, "2026-10-14_103000").unwrap();

    let at = folder
        .join(FOLDER)
        .join("rule-report-CD4__of_T_cells-2026-10-14_103000");
    assert_eq!(path, at.join("report.md"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# report");
    assert_eq!(std::fs::read(at.join("flagged-1.png")).unwrap(), [1, 2, 3]);
}

#[test]
fn a_rule_is_said_with_the_side_it_keeps_and_without_a_parameter_it_has_not() {
    use crate::gate_rules::rule::PhenotypeRule;
    use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn};
    let band = GateRule {
        parameter: "BUV805-A".into(),
        bound: Bound::Below,
        measured_on: MeasuredOn::Itself,
        rule: Rule::TailFraction(TailFractionRule::new((0.01, 0.02))),
    };
    let phenotype = GateRule {
        parameter: "".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::File("sample1".into()),
        rule: Rule::MatchThePhenotype(PhenotypeRule::default()),
    };

    let band = crate::rule_report::rule_said(&band);
    let phenotype = crate::rule_report::rule_said(&phenotype);

    assert!(
        band.contains(" on BUV805-A, keeping below the line, measured on each sample itself"),
        "{band}"
    );
    assert!(!phenotype.contains(" on ,"), "{phenotype}");
    assert!(phenotype.ends_with(", measured on sample1"), "{phenotype}");
}
