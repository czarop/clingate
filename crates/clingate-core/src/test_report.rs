//! A test report on the workspace as it stands, written as Markdown to be sent
//! on whole.
//!
//! It picks a rule for every gate a rule places, as the Gate Rules tab's pick
//! does, scores the picks and the rules as they stand against the gating drawn
//! by hand, sample by sample, and says what went wrong where. Nothing in the
//! workspace changes. Each part of the report is one function below, pushed
//! from [`test_report`]: a new thing to test is a new function there, and a
//! change to what the report says raises [`SUITE_VERSION`].

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::gate_rules::fit::{Candidate, FitSettings};
use crate::gate_rules::pick::{
    KindTried, PickSettings, Picked, Picking, Picks, gain, pick_rules, pickable_targets,
};
use crate::gate_rules::rule_store::{GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::score::{GateScore, Score, ScoreRow, least_agreeing_first, score_rules};
use crate::gate_rules::trial::{sample_type_name, specimen_name};
use crate::gates::GateState;

/// What the report tests, and the version of what it says.
pub const SUITE: &str = "rule picker";
pub const SUITE_VERSION: u32 = 1;
/// The folder in a workspace reports are kept in.
pub const FOLDER: &str = "test_reports";

/// Everything a report reads: the workspace as it stands.
pub struct TestJob {
    /// The workspace folder's name.
    pub workspace: String,
    /// When the report is made, as it is printed.
    pub made: String,
    pub gates: GateState,
    pub inputs: RunInputs,
    pub settings: PickSettings,
}

/// How far a report has got.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TestStep {
    Picking(Picking),
    Scoring(&'static str),
}

impl TestStep {
    /// The step in a line.
    pub fn describe(self) -> String {
        match self {
            TestStep::Picking(picking) => format!("Picking: {}", picking.describe()),
            TestStep::Scoring(what) => format!("Scoring {what} against your gating"),
        }
    }

    /// How far through the report, 0 to 1: the pick is most of it.
    pub fn fraction(self) -> f64 {
        match self {
            TestStep::Picking(picking) => picking.fraction() * 0.8,
            TestStep::Scoring(_) => 0.9,
        }
    }
}

/// The report, or why it stopped: only when asked to.
pub fn test_report(
    job: &TestJob,
    cancel: &AtomicBool,
    progress: impl Fn(TestStep) + Sync,
) -> Result<String, String> {
    let mut report = Report::default();
    report.push(header(job));
    report.push(workspace(job));
    report.push(settings(&job.settings));

    let targets = pickable_targets(&job.inputs.rules);
    let picks = report.timed("Picking", || {
        pick_rules(
            &job.gates,
            &job.inputs,
            &targets,
            job.settings,
            FitSettings::default(),
            cancel,
            |step| progress(TestStep::Picking(step)),
        )
    });
    stopped(cancel)?;
    let picks = match picks {
        Ok(picks) => picks,
        Err(why) => {
            report.problem(format!("The pick failed: {why}"));
            Vec::new()
        }
    };
    let section = picked(&picks, &mut report);
    report.push(section);

    let chosen = chosen_rules(&picks);
    if chosen.is_empty() {
        report.push("\n## Sample by sample\n\nNothing was picked, so nothing is scored.\n".into());
        return Ok(report.finish());
    }
    let picked_targets: Vec<RuleTarget> = chosen.iter().map(|(target, _)| target.clone()).collect();
    let scored = |what: &'static str, rules: RuleStore| {
        progress(TestStep::Scoring(what));
        let inputs = RunInputs {
            rules,
            ..job.inputs.clone()
        };
        score_rules(
            &job.gates,
            &inputs,
            |target| picked_targets.contains(target),
            job.settings.score,
            cancel,
        )
    };
    let with_picks = report.timed("Scoring the picks", || {
        scored("the picks", with_rules(&job.inputs.rules, &chosen))
    });
    stopped(cancel)?;
    let as_they_stand = report.timed("Scoring the rules as they stand", || {
        scored("the rules as they stand", job.inputs.rules.clone())
    });
    stopped(cancel)?;
    let with_picks = report.scored("the picks", with_picks);
    let as_they_stand = report.scored("the rules as they stand", as_they_stand);
    report.push(sample_by_sample(
        with_picks.as_ref(),
        as_they_stand.as_ref(),
    ));

    Ok(report.finish())
}

/// Keep `markdown` in the workspace `folder`, named by `stamp`.
pub fn keep(folder: &Path, markdown: &str, stamp: &str) -> std::io::Result<PathBuf> {
    let path = folder.join(FOLDER).join(format!("test-report-{stamp}.md"));
    crate::workspace::make_parent(&path).map_err(std::io::Error::other)?;
    std::fs::write(&path, markdown)?;
    Ok(path)
}

/// Now, as a report prints it and as its file is named.
pub fn stamped_now() -> (String, String) {
    let now = chrono::Local::now();
    (
        now.format("%Y-%m-%d %H:%M:%S %:z").to_string(),
        now.format("%Y-%m-%d_%H%M%S").to_string(),
    )
}

pub(crate) fn stopped(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("stopped".into())
    } else {
        Ok(())
    }
}

/// The report as it is written: its sections, what went wrong and how long
/// each step took.
#[derive(Default)]
pub(crate) struct Report {
    sections: Vec<String>,
    problems: Vec<String>,
    timings: Vec<(&'static str, Duration)>,
}

impl Report {
    pub(crate) fn push(&mut self, section: String) {
        self.sections.push(section);
    }

    pub(crate) fn problem(&mut self, problem: String) {
        if !self.problems.contains(&problem) {
            self.problems.push(problem);
        }
    }

    /// `step`'s answer, timed; a panic in it is an error that names it, so
    /// the rest of the report is still written.
    pub(crate) fn timed<T>(
        &mut self,
        step: &'static str,
        work: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let started = Instant::now();
        let answer = catch_unwind(AssertUnwindSafe(work))
            .unwrap_or_else(|panic| Err(format!("it panicked: {}", panic_message(&panic))));
        self.timings.push((step, started.elapsed()));
        answer
    }

    /// `score`, its problems kept; none when it failed, which is a problem.
    fn scored(&mut self, what: &str, score: Result<Score, String>) -> Option<Score> {
        match score {
            Ok(score) => {
                for problem in &score.problems {
                    self.problem(format!("Scoring {what}: {problem}"));
                }
                Some(score)
            }
            Err(why) => {
                self.problem(format!("Scoring {what} failed: {why}"));
                None
            }
        }
    }

    pub(crate) fn finish(mut self) -> String {
        let problems = problems(&self.problems);
        let timings = timings(&self.timings);
        self.sections.push(problems);
        self.sections.push(timings);
        self.sections.join("\n")
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".into())
}

fn header(job: &TestJob) -> String {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    format!(
        "# clingate test report\n\n\
         | | |\n|---|---|\n\
         | Tests | {SUITE}, version {SUITE_VERSION} |\n\
         | clingate | {} |\n\
         | Made | {} |\n\
         | Workspace | {} |\n\
         | Threads | {threads} |\n",
        env!("CARGO_PKG_VERSION"),
        job.made,
        cell(&job.workspace),
    )
}

fn workspace(job: &TestJob) -> String {
    let inputs = &job.inputs;
    let rules = &inputs.rules;
    let ids: Vec<&str> = inputs
        .files
        .iter()
        .filter_map(|(name, _)| inputs.names.get(name).map(|id| id.as_ref()))
        .collect();
    let types = counted(ids.iter().map(|id| {
        sample_type_name(&inputs.metadata, &rules.pairing, id).unwrap_or_else(|| "none".into())
    }));
    let specimens = counted(
        ids.iter()
            .filter_map(|id| specimen_name(&inputs.metadata, &rules.pairing, id)),
    )
    .len();
    let kinds = counted(
        rules
            .entries()
            .iter()
            .map(|e| e.rule.rule.kind().to_string()),
    );
    format!(
        "\n## Workspace\n\n\
         - {} FCS files, {} with a metadata row; by sample type: {}\n\
         - {specimens} specimens\n\
         - {} gates drawn; {} rules - {}; {} picked for (a gate placed from another gate is not)\n\
         - {} arcsinh channels\n",
        inputs.files.len(),
        ids.len(),
        listed(&types),
        job.gates.gate_count(),
        rules.entries().len(),
        listed(&kinds),
        pickable_targets(rules).len(),
        inputs.cofactors.len(),
    )
}

fn counted(items: impl Iterator<Item = String>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item).or_insert(0) += 1;
    }
    counts
}

fn listed(counts: &BTreeMap<String, usize>) -> String {
    if counts.is_empty() {
        return "none".into();
    }
    counts
        .iter()
        .map(|(name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn settings(settings: &PickSettings) -> String {
    let band = settings
        .fmx_band
        .map_or("none - not tried".into(), |(low, high)| {
            format!("{} to {}", share(low), share(high))
        });
    format!(
        "\n## Pick settings\n\n\
         - FMX band: {band}\n\
         - A sample is off below agreement {} (lower for a sample of few events: {} counting-noise widths)\n\
         - A rule passes with no more than {} of its samples off\n",
        settings.score.off_below,
        settings.score.noise_widths,
        share(settings.most_off),
    )
}

/// Each gate's pick: a line each, then how each kind tried did.
fn picked(picks: &Picks, report: &mut Report) -> String {
    let mut out = String::from(
        "\n## Picks\n\n\
         | Gate | Result | Picked | Typical | Lowest | Off | Gain on the rule as it stands | Rules tried |\n\
         |---|---|---|---|---|---|---|---|\n",
    );
    let mut each = String::new();
    for (target, pick) in picks {
        match pick {
            Ok(pick) => {
                for problem in &pick.fit.problems {
                    report.problem(format!("{}: {problem}", target.describe()));
                }
                out.push_str(&pick_line(target, pick));
                each.push_str(&pick_detail(target, pick));
            }
            Err(why) => {
                out.push_str(&format!(
                    "| {} | not picked: {} | | | | | | |\n",
                    cell(&target.describe()),
                    cell(why)
                ));
            }
        }
    }
    if picks.is_empty() {
        out.push_str("| (nothing picked) | | | | | | | |\n");
    }
    out + &each
}

fn best(pick: &Picked) -> Option<&Candidate> {
    pick.fit.candidates.iter().find(|c| c.among_best)
}

fn pick_line(target: &RuleTarget, pick: &Picked) -> String {
    let result = if pick.passed { "passed" } else { "FLAGGED" };
    let Some(best) = best(pick) else {
        return format!(
            "| {} | {result} | (no candidate) | | | | | {} |\n",
            cell(&target.describe()),
            pick.tried
        );
    };
    let fit = best.fit.as_ref();
    format!(
        "| {} | {result} | {} - {}, {} | {} | {} | {} | {} | {} |\n",
        cell(&target.describe()),
        best.rule.rule.kind(),
        cell(&best.said),
        measured_on(&best.rule.measured_on),
        agreement(fit.and_then(|f| f.typical_agreement)),
        agreement(fit.and_then(|f| f.lowest_agreement)),
        fit.map_or("-".into(), |f| format!(
            "{} of {}",
            f.off,
            f.scored + f.not_placed
        )),
        gain(&pick.fit.candidates).map_or("-".into(), |g| format!("{g:+.3}")),
        pick.tried,
    )
}

fn pick_detail(target: &RuleTarget, pick: &Picked) -> String {
    let mut out = format!(
        "\n### {} - {}\n\n| Kind | Searched | Closest | Typical | Off | Passes |\n|---|---|---|---|---|---|\n",
        target.describe(),
        if pick.passed { "passed" } else { "flagged" }
    );
    for kind in &pick.kinds {
        out.push_str(&kind_line(kind));
    }
    if let Some(current) = pick.fit.candidates.iter().find(|c| c.current) {
        out.push_str(&format!(
            "\nAs it stands: {}, {}\n",
            candidate(current),
            measured_on(&current.rule.measured_on)
        ));
    }
    let close: Vec<String> = pick
        .fit
        .candidates
        .iter()
        .filter(|c| c.among_best)
        .skip(1)
        .map(candidate)
        .collect();
    if !close.is_empty() {
        out.push_str(&format!(
            "\nTied with the pick:\n\n- {}\n",
            close.join("\n- ")
        ));
    }
    out
}

pub(crate) fn measured_on(on: &MeasuredOn) -> String {
    match on {
        MeasuredOn::Itself => "measured on each sample itself".into(),
        MeasuredOn::Partner(kind) => format!("measured on each specimen's {kind}"),
        MeasuredOn::File(file) => format!("measured on {file}"),
    }
}

fn kind_line(kind: &KindTried) -> String {
    format!(
        "| {} | {} | {} | {} | {} of {} | {} |\n",
        kind.kind.describe(),
        if kind.searched { "yes" } else { "no" },
        cell(&kind.closest),
        agreement(kind.typical_agreement),
        kind.off,
        kind.judged,
        if kind.passed { "yes" } else { "no" },
    )
}

fn candidate(candidate: &Candidate) -> String {
    let fit = candidate.fit.as_ref();
    format!(
        "{} - typical {}, lowest {}, {} off",
        candidate.said,
        agreement(fit.and_then(|f| f.typical_agreement)),
        agreement(fit.and_then(|f| f.lowest_agreement)),
        fit.map_or(0, |f| f.off),
    )
}

/// Each gate picked for, with its pick's rule.
fn chosen_rules(picks: &Picks) -> Vec<(RuleTarget, GateRule)> {
    picks
        .iter()
        .filter_map(|(target, pick)| {
            let best = best(pick.as_ref().ok()?)?;
            Some((target.clone(), best.rule.clone()))
        })
        .collect()
}

/// `rules`, with each of `chosen` in its gate's place.
fn with_rules(rules: &RuleStore, chosen: &[(RuleTarget, GateRule)]) -> RuleStore {
    let mut rules = rules.clone();
    for (target, rule) in chosen {
        rules.insert(target.clone(), rule.clone());
    }
    rules
}

/// Each gate's picked rule sample by sample, least agreeing first, beside
/// the rule as it stands.
fn sample_by_sample(with_picks: Option<&Score>, as_they_stand: Option<&Score>) -> String {
    let mut out = String::from("\n## Sample by sample\n");
    let Some(with_picks) = with_picks else {
        out.push_str("\nThe picks could not be scored - see Problems.\n");
        return out;
    };
    let standing: BTreeMap<(&str, &str), &ScoreRow> = as_they_stand
        .map(|score| {
            score
                .rows
                .iter()
                .map(|row| ((row.gate_id.as_str(), row.file.as_str()), row))
                .collect()
        })
        .unwrap_or_default();
    let summaries: BTreeMap<&str, &GateScore> = as_they_stand
        .map(|score| {
            score
                .gates
                .iter()
                .map(|g| (g.gate_id.as_str(), g))
                .collect()
        })
        .unwrap_or_default();
    for gate in &with_picks.gates {
        out.push_str(&gate_rows(
            gate,
            summaries.get(gate.gate_id.as_str()).copied(),
            with_picks,
            &standing,
        ));
    }
    out
}

fn gate_rows(
    gate: &GateScore,
    standing: Option<&GateScore>,
    with_picks: &Score,
    standing_rows: &BTreeMap<(&str, &str), &ScoreRow>,
) -> String {
    let mut out = format!(
        "\n### {}\n\nPicked: typical {}, lowest {}, {} off of {}. As it stands: {}.\n\n\
         | Sample | Specimen | Type | Picked rule | Agreement | Off below | Caught | Extra | Hand % | Rule % | Difference (points) | Edge off (IQR) | As it stands | Why not placed |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
        gate.gate,
        agreement(gate.typical_agreement),
        agreement(gate.lowest_agreement),
        gate.off,
        gate.scored + gate.not_placed,
        standing.map_or("not scored".into(), |s| format!(
            "typical {}, lowest {}, {} off",
            agreement(s.typical_agreement),
            agreement(s.lowest_agreement),
            s.off
        )),
    );
    let mut rows: Vec<&ScoreRow> = with_picks
        .rows
        .iter()
        .filter(|r| r.gate_id == gate.gate_id)
        .collect();
    rows.sort_by(|a, b| least_agreeing_first(a, b));
    for row in rows {
        let before = standing_rows.get(&(row.gate_id.as_str(), row.file.as_str()));
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            cell(&row.file),
            cell(row.specimen.as_deref().unwrap_or("-")),
            cell(row.sample_type.as_deref().unwrap_or("-")),
            row.what,
            agreement(row.agreement),
            agreement(row.off_line),
            share_or_dash(row.caught),
            share_or_dash(row.extra),
            share_or_dash(row.hand_holds),
            share_or_dash(row.rule_holds),
            row.holds_difference
                .map_or("-".into(), |d| format!("{d:+.2}")),
            row.edge_off_iqrs.map_or("-".into(), |d| format!("{d:+.2}")),
            before.map_or("-".into(), |b| format!(
                "{} {}",
                b.what,
                agreement(b.agreement)
            )),
            cell(row.why_not.as_deref().unwrap_or("")),
        ));
    }
    out
}

fn problems(problems: &[String]) -> String {
    if problems.is_empty() {
        return "\n## Problems\n\nNone.\n".into();
    }
    format!("\n## Problems\n\n- {}\n", problems.join("\n- "))
}

fn timings(timings: &[(&'static str, Duration)]) -> String {
    let lines: Vec<String> = timings
        .iter()
        .map(|(step, took)| format!("| {step} | {:.1} s |", took.as_secs_f64()))
        .collect();
    format!(
        "\n## Timings\n\n| Step | Took |\n|---|---|\n{}\n",
        lines.join("\n")
    )
}

/// `text` safe in a table cell.
pub(crate) fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

fn agreement(value: Option<f64>) -> String {
    value.map_or("-".into(), |v| format!("{v:.3}"))
}

fn share(value: f64) -> String {
    format!("{:.2}%", value * 100.0)
}

pub(crate) fn share_or_dash(value: Option<f64>) -> String {
    value.map_or("-".into(), share)
}
