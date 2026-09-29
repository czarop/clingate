//! Replaying reviewed runs for the tools for Claude: what the rules - as they
//! were, or changed - do on the samples reviewed runs kept, judged against
//! where the reviews say each gate belongs. See [`crate::review::replay`].
//!
//! Replays change nothing. [`Session::update_rule`] is the one step that
//! does, and only on the user's word.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use super::{Refusal, Session, failed};
use crate::gate_rules::autogate::describe;
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore};
use crate::review::replay::{
    Case, ReviewedRun, RuleChange, RunSummary, Truth, Verdict, library_runs,
};
use crate::review::report::Histogram;
use crate::review::shape::Shape;

/// Which reviewed runs a replay reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayScope {
    /// The workspace's last run - as marked reviewed, or as the gates stand.
    Workspace,
    /// Every run in the review library.
    Library,
    Both,
}

impl ReplayScope {
    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().as_str() {
            "workspace" => Some(Self::Workspace),
            "library" => Some(Self::Library),
            "both" | "" => Some(Self::Both),
            _ => None,
        }
    }
}

/// Cases a replay lists by default, and at most.
pub const CASES_SHOWN: usize = 60;
pub const CASES_MAX: usize = 500;

/// The order cases are listed in: what a change did first, then what the
/// run got wrong, then the rest.
const LISTED_FIRST: [Verdict; 8] = [
    Verdict::Broken,
    Verdict::Fixed,
    Verdict::StillWrong,
    Verdict::Changed,
    Verdict::NotReproduced,
    Verdict::NotReplayed,
    Verdict::StillRight,
    Verdict::Unchanged,
];

/// One case in a line.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseLine {
    /// What replay_case takes to show this case in full.
    pub case: String,
    pub run: String,
    pub gate: String,
    pub sample: String,
    pub sample_type: Option<String>,
    pub truth: Truth,
    pub verdict: Verdict,
    /// The fraction of the sample's kept events the gate holds: where the
    /// review says it belongs, where the run put it, the replay with the
    /// run's rules, and the replay with the changes.
    pub right_holds: Option<f64>,
    pub run_holds: Option<f64>,
    pub baseline_holds: Option<f64>,
    pub replay_holds: Option<f64>,
    /// Where those lines sit on the rule's parameter.
    pub right_at: Option<f64>,
    pub run_at: Option<f64>,
    pub replay_at: Option<f64>,
    pub replay_confidence: Option<f64>,
    pub replay_weakest: Option<String>,
    pub why: Option<String>,
    /// Earlier runs that made this same placement from the same input, and
    /// what their reviews said - see replay_case.
    pub earlier_reviews: usize,
    pub notes: Vec<String>,
}

/// A replay, for the tools.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplayAnswer {
    /// Each change tried, against the rule it replaces.
    pub changes_tried: Vec<String>,
    pub runs: Vec<RunSummary>,
    /// Runs in the library that could not be read, and why.
    pub unreadable: Vec<String>,
    /// Placements an earlier run made from exactly the same input as a
    /// later one, counted once with the later.
    pub repeats_counted_once: usize,
    pub totals: BTreeMap<Verdict, usize>,
    pub by_gate: BTreeMap<String, BTreeMap<Verdict, usize>>,
    pub cases_total: usize,
    /// The cases, most telling first - see `LISTED_FIRST`.
    pub cases: Vec<CaseLine>,
    pub next: &'static str,
}

/// One population in a case, as a histogram on the rule's parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CasePopulation {
    pub file: String,
    pub metadata: BTreeMap<String, String>,
    /// How many events the population held, and how many the run kept.
    pub events: usize,
    pub events_kept: usize,
    pub shape: Option<Shape>,
    pub histogram: Histogram,
}

/// One case in full.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaseDetail {
    pub case: Case,
    /// The rule the run used for this gate, and the one replayed.
    pub rule_in_run: Option<String>,
    pub rule_replayed: Option<String>,
    /// Where the gate's line sat before the run.
    pub started_at: Option<f64>,
    /// The sample being gated, and - when the rule read another - the file
    /// it read, on one histogram range so the lines can be read across both.
    pub sample: Option<CasePopulation>,
    pub read: Option<CasePopulation>,
    pub how_to_read: &'static str,
}

/// A rule changed in the workspace's rules file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleUpdated {
    pub file: PathBuf,
    pub population: String,
    pub was: Option<String>,
    pub now: String,
    pub next: &'static str,
}

/// A rule in a line: what it reads, and how it decides.
pub fn describe_rule(rule: &GateRule) -> String {
    let side = match rule.bound {
        Bound::Above => "keeps above the line",
        Bound::Below => "keeps below the line",
    };
    let on = match &rule.measured_on {
        MeasuredOn::Partner(kind) => format!("the specimen's {kind}"),
        MeasuredOn::Itself => "the sample itself".to_string(),
        MeasuredOn::File(file) => format!("the file {file}"),
    };
    format!(
        "{} ({side}), read on {on}: {} [{}]",
        rule.parameter,
        rule.rule.describe(),
        rule.rule.kind()
    )
}

fn rounded(v: Option<f64>) -> Option<f64> {
    v.map(|v| (v * 1e6).round() / 1e6)
}

fn case_id(c: &Case) -> String {
    format!("{}|{}|{}", c.run, c.gate_id, c.sample.id)
}

fn line_of(c: &Case) -> CaseLine {
    CaseLine {
        case: case_id(c),
        run: c.run.clone(),
        gate: describe(&c.gate, c.parent_gate.as_deref()),
        sample: c.sample.name.clone().unwrap_or_else(|| c.sample.id.clone()),
        sample_type: c.sample.sample_type.clone(),
        truth: c.truth.clone(),
        verdict: c.verdict,
        right_holds: rounded(c.right.holds),
        run_holds: rounded(c.run_decided.line.holds),
        baseline_holds: rounded(c.baseline_decided.as_ref().and_then(|d| d.line.holds)),
        replay_holds: rounded(c.replay_decided.as_ref().and_then(|d| d.line.holds)),
        right_at: rounded(c.right.at),
        run_at: rounded(c.run_decided.line.at),
        replay_at: rounded(c.replay_decided.as_ref().and_then(|d| d.line.at)),
        replay_confidence: rounded(c.replay_decided.as_ref().and_then(|d| d.confidence)),
        replay_weakest: c.replay_decided.as_ref().and_then(|d| d.weakest.clone()),
        why: c.why.clone(),
        earlier_reviews: c.earlier_reviews.len(),
        notes: c.notes.clone(),
    }
}

fn names_gate(c: &Case, wanted: &str) -> bool {
    let wanted = wanted.trim().to_lowercase();
    c.gate.to_lowercase() == wanted
        || describe(&c.gate, c.parent_gate.as_deref()).to_lowercase() == wanted
}

/// Each change, against the rule it replaces in the runs replayed.
fn changes_tried(changes: &[RuleChange], runs: &[ReviewedRun]) -> Vec<String> {
    changes
        .iter()
        .map(|change| {
            let was = runs
                .iter()
                .rev()
                .find_map(|r| r.rules.get(&change.target))
                .map(describe_rule);
            let now = describe_rule(&change.rule);
            match was {
                Some(was) => format!("{}: {now} (was: {was})", change.target.describe()),
                None => format!(
                    "{}: {now} (no run replayed has a rule for exactly this target, so it \
                     applies only where no rule naming the parent is more specific, and only to \
                     gates the runs measured)",
                    change.target.describe()
                ),
            }
        })
        .collect()
}

impl Session {
    /// The reviewed runs a replay reads, and the library folders that could
    /// not be read.
    fn reviewed_runs(
        &self,
        scope: ReplayScope,
    ) -> Result<(Vec<ReviewedRun>, Vec<String>), Refusal> {
        let mut runs = Vec::new();
        let mut unreadable = Vec::new();
        if scope != ReplayScope::Library {
            match ReviewedRun::from_workspace(&self.folder, &self.gates, self.metadata.metadata()) {
                Ok(run) => runs.push(run),
                Err(e) if scope == ReplayScope::Workspace => return Err(failed(e)),
                Err(e) => unreadable.push(format!("this workspace: {e}")),
            }
        }
        if scope != ReplayScope::Workspace {
            match self.review_library.as_deref() {
                Some(library) => {
                    let (found, problems) = library_runs(library);
                    unreadable.extend(problems);
                    // The workspace's own run, reviewed and copied, is the
                    // same run: read it once, from the workspace.
                    for run in found {
                        if !runs.iter().any(|r| r.applied_at == run.applied_at) {
                            runs.push(run);
                        }
                    }
                }
                None if scope == ReplayScope::Library => {
                    return Err(failed(
                        "no review library is set: the app's Review library setting names it",
                    ));
                }
                None => {}
            }
        }
        if runs.is_empty() {
            return Err(failed(format!(
                "there is no reviewed run to replay{}",
                if unreadable.is_empty() {
                    String::new()
                } else {
                    format!(": {}", unreadable.join("; "))
                }
            )));
        }
        Ok((runs, unreadable))
    }

    /// Replay reviewed runs with the rules they ran with, and with
    /// `changes`, and say what each change fixed and broke. Changes nothing.
    pub fn replay_rules(
        &self,
        changes: &[RuleChange],
        scope: ReplayScope,
        gate: Option<&str>,
        max_cases: Option<usize>,
    ) -> Result<ReplayAnswer, Refusal> {
        let (runs, unreadable) = self.reviewed_runs(scope)?;
        let mut replay = crate::review::replay::replay(&runs, changes);
        if let Some(wanted) = gate.filter(|g| !g.trim().is_empty()) {
            let before: Vec<String> = {
                let mut names: Vec<String> = replay
                    .cases
                    .iter()
                    .map(|c| describe(&c.gate, c.parent_gate.as_deref()))
                    .collect();
                names.sort();
                names.dedup();
                names
            };
            replay.cases.retain(|c| names_gate(c, wanted));
            if replay.cases.is_empty() {
                return Err(failed(format!(
                    "no case in the runs replayed is for the gate {wanted}; the gates are: {}",
                    before.join(", ")
                )));
            }
        }
        let totals = Verdict::ALL
            .iter()
            .map(|v| (*v, replay.count(*v)))
            .filter(|(_, n)| *n > 0)
            .collect();
        let by_gate = replay.by_gate();
        let cases_total = replay.cases.len();
        let shown = max_cases.unwrap_or(CASES_SHOWN).clamp(1, CASES_MAX);
        let rank = |v: Verdict| LISTED_FIRST.iter().position(|w| *w == v).unwrap_or(99);
        let mut listed: Vec<&Case> = replay.cases.iter().collect();
        listed.sort_by_key(|c| rank(c.verdict));
        Ok(ReplayAnswer {
            changes_tried: changes_tried(changes, &runs),
            repeats_counted_once: replay.runs.iter().map(|r| r.repeated_later).sum(),
            runs: replay.runs,
            unreadable,
            totals,
            by_gate,
            cases_total,
            cases: listed.into_iter().take(shown).map(line_of).collect(),
            next: "replay_case shows one case in full - histograms of the sample and the file \
                   the rule read, with every line; explain_gate_positioning and \
                   read_positioning_code say exactly how the rule decided. Discuss changes with \
                   the user and replay them here; update_rule writes one to the rules file only \
                   when the user says to",
        })
    }

    /// One case of a replay in full: the populations it read, as histograms
    /// with every line on them, and the rule before and after.
    pub fn replay_case(
        &self,
        changes: &[RuleChange],
        scope: ReplayScope,
        case: &str,
    ) -> Result<CaseDetail, Refusal> {
        let mut parts = case.trim().splitn(3, '|');
        let (Some(run_name), Some(gate_id), Some(sample)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(failed(
                "a case is named as replay_rules gives it: run|gate id|sample id",
            ));
        };
        let (runs, _) = self.reviewed_runs(scope)?;
        let run = runs.iter().find(|r| r.name == run_name).ok_or_else(|| {
            failed(format!(
                "no run named {run_name}; the runs are: {}",
                runs.iter()
                    .map(|r| r.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        // Every run, so a placement repeated from the same input is shown
        // with its earlier reviews - and one named by an earlier run is
        // shown as the later run that counts it.
        let replay = crate::review::replay::replay(&runs, changes);
        let same_placement = |c: &&Case| c.gate_id == gate_id && c.sample.id == sample;
        let found = replay
            .cases
            .iter()
            .filter(same_placement)
            .find(|c| c.run == run_name || c.earlier_reviews.iter().any(|e| e.run == run_name))
            .cloned()
            .ok_or_else(|| failed(format!("{run_name} has no case for {gate_id} on {sample}")))?;
        let run = runs.iter().find(|r| r.name == found.run).unwrap_or(run);

        let replayed_rules = crate::review::replay::with_changes(&run.rules, changes);
        let rule_of = |rules: &RuleStore| {
            rules
                .rule_for(&found.gate, found.parent_gate.as_deref())
                .map(describe_rule)
        };
        let events = run.events.as_ref();
        let kept = |file: &str| {
            events.and_then(|e| {
                e.samples
                    .iter()
                    .find(|s| s.gate_id == gate_id && s.file == file)
            })
        };
        let parameter = found.parameter.clone();
        let values_of = |s: &crate::review::events::EventSample| -> Vec<f64> {
            let on_x = parameter.as_deref() == Some(s.x.as_str());
            s.points
                .iter()
                .map(|(x, y)| if on_x { *x } else { *y } as f64)
                .collect()
        };
        let here = kept(&found.sample.id);
        let read_file = found
            .replay_decided
            .as_ref()
            .or(found.baseline_decided.as_ref())
            .and_then(|d| d.measured_on.clone())
            .or_else(|| found.run_decided.measured_on.clone())
            .filter(|f| *f != found.sample.id);
        let there = read_file.as_deref().and_then(kept);
        let all: Vec<f64> = here
            .iter()
            .chain(there.iter())
            .flat_map(|s| values_of(s))
            .filter(|v| v.is_finite())
            .collect();
        let range = (
            all.iter().copied().fold(f64::INFINITY, f64::min),
            all.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        );
        let population = |s: &crate::review::events::EventSample| {
            let values = values_of(s);
            CasePopulation {
                file: s.file.clone(),
                metadata: events
                    .and_then(|e| e.metadata.get(&s.file).cloned())
                    .unwrap_or_default(),
                events: s.events,
                events_kept: s.points.len(),
                shape: crate::review::shape::summarise(&values),
                histogram: crate::review::report::histogram(
                    parameter.as_deref().unwrap_or(""),
                    values.iter().map(|v| *v as f32),
                    range,
                ),
            }
        };
        let started_at = here.and_then(|s| {
            let gate = s.gate.as_ref()?;
            let (low, high) =
                crate::gate_rules::autogate::extent_on(&gate.geometry, parameter.as_deref()?)?;
            Some(match found.bound? {
                Bound::Above => low,
                Bound::Below => high,
            } as f64)
        });

        Ok(CaseDetail {
            rule_in_run: rule_of(&run.rules),
            rule_replayed: rule_of(&replayed_rules),
            started_at,
            sample: here.map(population),
            read: there.map(population),
            case: found,
            how_to_read: "Every line is the gate's leading side on the rule's parameter, in the \
                          plot's units. started_at is the gate before the run; case.run_decided \
                          where the run put it; case.baseline_decided where a replay with the \
                          run's own rules puts it on the events kept (reproduced says whether \
                          that matches the run); case.replay_decided where the changed rules put \
                          it; case.right where the review says it belongs. Each 'holds' is the \
                          fraction of the sample's kept events inside the gate - its real shape, \
                          both axes - with the gate there: what the verdict is judged on. Each \
                          'beyond' is the fraction past the line on the parameter alone. \
                          case.earlier_reviews are earlier runs that made this placement from \
                          exactly the same input. The histograms share one range, split into \
                          equal bins from lower to upper.",
        })
    }

    /// Replace the rule for one target in the workspace's rules file - the
    /// file the Gate Rules tab edits. Only on the user's word.
    pub fn update_rule(&mut self, change: RuleChange) -> Result<RuleUpdated, Refusal> {
        let file = match &self.parts.rules {
            super::PartState::Loaded { file } => file.clone(),
            super::PartState::Failed { file, reason } => {
                return Err(failed(format!(
                    "{} could not be read ({reason}), so it is not written over",
                    file.display()
                )));
            }
            _ => {
                return Err(failed(
                    "this workspace has no rules file: make the rules in the Gate Rules tab first",
                ));
            }
        };
        let store = self
            .rules
            .as_mut()
            .ok_or_else(|| failed("this workspace has no rules"))?;
        let mut changed = store.clone();
        let was = changed.insert(change.target.clone(), change.rule.clone());
        changed.save(&file).map_err(failed)?;
        *store = changed;
        // A preview made under the old rule would apply placements the rules
        // no longer make.
        self.pending = None;
        Ok(RuleUpdated {
            file,
            population: change.target.describe(),
            was: was.as_ref().map(describe_rule),
            now: describe_rule(&change.rule),
            next: "preview_rules shows what the changed rules would move; the app reads the \
                   rules file when the workspace is next opened",
        })
    }
}
