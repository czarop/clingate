//! A rule for each gate - its kind and its settings - picked against the
//! gating drawn by hand, the kinds tried in the order they are preferred.

use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::score::ruled;
use super::{Refusal, Session, failed, round};
use crate::gate_rules::fit::{Candidate, FitSettings};
use crate::gate_rules::pick::{KindTried, PickSettings, Picked, gain, kept_settings, pick_rules};
use crate::gate_rules::rule_store::RuleTarget;
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::searches::{self, Search};

/// One gate's pick.
#[derive(Debug, Clone, Serialize)]
pub struct PickedRule {
    pub gate: String,
    /// The best rule of the first kind, in order, with one that passes - or,
    /// when none does, the closest of everything tried.
    pub best: Candidate,
    pub as_it_stands: Option<Candidate>,
    /// How much higher the best's typical agreement is than the rule as it
    /// stands: 0 when it is the rule as it stands.
    pub gain: Option<f64>,
    /// Whether the best passes the cut-off. When not, it is only the
    /// closest, and the gate may be better gated by hand.
    pub passed: bool,
    /// Each kind of rule tried, in the order tried, and how its closest did.
    pub kinds: Vec<KindTried>,
    /// The others tied with the best, to look at side by side.
    pub also_close: Vec<Candidate>,
    /// How many rules were tried.
    pub tried: usize,
}

/// Every gate picked for.
#[derive(Debug, Clone, Serialize)]
pub struct PickAnswer {
    pub files_read: usize,
    pub problems: Vec<String>,
    /// What a rule had to meet, and the band tried first on the FMX.
    pub settings: PickSettings,
    /// Those whose best is only the closest first, then those where the best
    /// does most better than the rule as it stands.
    pub picked: Vec<PickedRule>,
    pub not_picked: Vec<String>,
    pub next: &'static str,
}

impl Session {
    /// The pick settings the user last chose in the app, or the defaults.
    pub fn pick_settings(&self) -> PickSettings {
        kept_settings(&self.folder)
    }

    /// The gates a pick is made for: `population`'s, or every gate a rule
    /// places other than from another gate.
    fn pick_targets(&self, population: Option<&str>) -> Result<Vec<RuleTarget>, Refusal> {
        let rules = self.rules_or_refuse()?;
        match population {
            Some(query) => {
                let gate = self.gate_target(query)?;
                let target = ruled(rules, &gate).ok_or_else(|| {
                    failed(
                        "no rule places that population - a pick starts from the rule a gate has",
                    )
                })?;
                Ok(vec![target])
            }
            None => Ok(rules
                .entries()
                .iter()
                .filter(|entry| !entry.rule.rule.reads_another_gate())
                .map(|entry| entry.target.clone())
                .collect()),
        }
    }

    /// One gate's pick, as the tools show it.
    fn picked(&self, picked: Picked) -> PickedRule {
        let found = picked.fit;
        let gain = gain(&found.candidates).map(|g| round(g, 3));
        let as_it_stands = found.candidates.iter().find(|c| c.current).cloned();
        let mut close = found.candidates.into_iter().filter(|c| c.among_best);
        let best = close.next().expect("the best is among the best");
        PickedRule {
            gate: found.gate,
            best: self.shown_candidate(best),
            as_it_stands: as_it_stands.map(|c| self.shown_candidate(c)),
            gain,
            passed: picked.passed,
            kinds: picked
                .kinds
                .into_iter()
                .map(|kind| KindTried {
                    typical_agreement: kind.typical_agreement.map(|t| round(t, 3)),
                    ..kind
                })
                .collect(),
            also_close: close.map(|c| self.shown_candidate(c)).collect(),
            tried: picked.tried,
        }
    }

    /// Pick a rule - its kind and its settings - for `population`'s gate, or
    /// every gate a rule places other than from another gate, against the
    /// gating drawn by hand, the kinds tried in order until one passes as
    /// `settings` asks: see [`crate::gate_rules::pick`]. The closest are kept
    /// for the gallery. Moves no gate and changes no rule.
    pub fn pick_rules(
        &self,
        population: Option<&str>,
        settings: PickSettings,
        fit: FitSettings,
    ) -> Result<PickAnswer, Refusal> {
        let targets = self.pick_targets(population)?;
        let rules = self.rules_or_refuse()?.clone();
        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let picks = pick_rules(
            &self.gates,
            &inputs,
            &targets,
            settings,
            fit,
            &AtomicBool::new(false),
            |_| {},
        )
        .map_err(failed)?;

        let mut problems: Vec<String> = Vec::new();
        let mut not_picked = Vec::new();
        let mut kept = Vec::new();
        let mut picked = Vec::new();
        for (target, found) in picks {
            match found {
                Ok(found) => {
                    for problem in &found.fit.problems {
                        if !problems.contains(problem) {
                            problems.push(problem.clone());
                        }
                    }
                    kept.push(Search::picked(&found, &target));
                    picked.push(self.picked(found));
                }
                Err(why) => not_picked.push(format!("{}: {why}", target.describe())),
            }
        }
        if let Err(e) = searches::keep_all(&self.folder, kept) {
            problems.push(format!("the picks could not be kept for the gallery: {e}"));
        }
        picked.sort_by(|a, b| {
            let gain = |p: &PickedRule| p.gain.unwrap_or(f64::NEG_INFINITY);
            a.passed.cmp(&b.passed).then(gain(b).total_cmp(&gain(a)))
        });
        Ok(PickAnswer {
            files_read: inputs.files.len(),
            problems,
            settings,
            picked,
            not_picked,
            next: "each gate's rule picked by trying the kinds of rule in the user's order of \
                   preference - the band on the FMX at the range they gave, above the \
                   negative, the valley or a smear, a band on the sample, the phenotype last - \
                   and taking the first that passes: no more than \
                   most_off of its samples agree less than off_below with the hand gate. kinds \
                   says how each kind tried did. best is the rule to suggest, beside \
                   as_it_stands. A gate that has not passed shows only the closest rule of \
                   everything tried: tell the user it is flagged, and that it may be better \
                   gated by hand. also_close are tied with the best: the user can step through \
                   them on the Gallery tab, where they are kept. No rule has changed: show the \
                   user, and update_rule with the rules they choose",
        })
    }
}
