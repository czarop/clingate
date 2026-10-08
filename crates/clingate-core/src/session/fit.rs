//! A gate's rule settings searched for the closest to the gating drawn by
//! hand.

use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::score::ruled;
use super::{Refusal, Session, failed};
use crate::gate_rules::fit::{
    Candidate, FitSettings, MOST_CANDIDATES, RankBy, default_candidates, fit_rule,
};
use crate::gate_rules::rule_store::GateRule;
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::score::ScoreSettings;
use crate::gate_rules::searches::{self, Search};

/// Candidates shown when no number is asked for.
pub const FIT_SHOWN: usize = 10;

/// What to try for a gate.
#[derive(Debug, Clone, Default)]
pub struct FitAsk {
    /// Whole rules to try beside the rule as it stands.
    pub candidates: Vec<GateRule>,
    /// Also try the default settings around the rule as it stands - or, for
    /// a gate with no rule, around each candidate.
    pub defaults: bool,
    /// How many candidates to show, best first.
    pub shown: Option<usize>,
}

/// The candidates for one gate, best first.
#[derive(Debug, Clone, Serialize)]
pub struct FitAnswer {
    pub gate: String,
    pub files_read: usize,
    pub problems: Vec<String>,
    pub fit_on: Vec<String>,
    pub checked_on: Vec<String>,
    pub rank_by: RankBy,
    pub candidates_total: usize,
    pub candidates: Vec<Candidate>,
    pub next: &'static str,
}

impl Session {
    /// `candidate` as the tools show it: scores rounded, samples by name, and
    /// without the gates it places, which are kept for the gallery.
    pub(super) fn shown_candidate(&self, candidate: Candidate) -> Candidate {
        Candidate {
            fit: candidate.fit.map(|g| self.shown_gate(g)),
            check: candidate.check.map(|g| self.shown_gate(g)),
            placed: Vec::new(),
            ..candidate
        }
    }

    /// Try settings for `population`'s rule against the gating drawn by hand,
    /// each scored as [`Session::score_rules`] scores a rule and ranked as
    /// `fit` says - see [`crate::gate_rules::fit`] - and keep the closest for
    /// the gallery. Moves no gate.
    pub fn fit_rule(
        &self,
        population: &str,
        ask: FitAsk,
        settings: ScoreSettings,
        fit: FitSettings,
    ) -> Result<FitAnswer, Refusal> {
        let rules = self.rules.clone().unwrap_or_default();
        let gate = self.gate_target(population)?;
        let target = ruled(&rules, &gate).unwrap_or(gate);
        let current = rules.get(&target);
        let given = self.resolved_candidates(&target, &ask.candidates)?;
        if current.is_none() && given.is_empty() {
            return Err(failed(
                "no rule places that population: give candidate rules to try",
            ));
        }
        let around: Vec<&GateRule> = match current {
            Some(rule) => vec![rule],
            None => given.iter().collect(),
        };
        let mut tried = given.clone();
        if ask.defaults {
            tried.extend(around.into_iter().flat_map(default_candidates));
        }

        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let found = fit_rule(
            &self.gates,
            &inputs,
            &target,
            &tried,
            settings,
            fit,
            &AtomicBool::new(false),
        )
        .map_err(failed)?;
        let mut problems = found.problems.clone();
        if let Err(e) = searches::keep(&self.folder, Search::of(&found, &target)) {
            problems.push(format!(
                "the closest candidates could not be kept for the gallery: {e}"
            ));
        }
        let candidates_total = found.candidates.len();
        let shown = ask.shown.unwrap_or(FIT_SHOWN).clamp(1, MOST_CANDIDATES);
        let candidates = found
            .candidates
            .into_iter()
            .take(shown)
            .map(|candidate| self.shown_candidate(candidate))
            .collect();
        Ok(FitAnswer {
            gate: found.gate,
            files_read: found.files_read,
            problems,
            fit_on: found.fit_on,
            checked_on: found.checked_on,
            rank_by: found.rank_by,
            candidates_total,
            candidates,
            next: "candidates best first, ranked on the specimens in fit_on: fit is each \
                   one's score there, as score_rules scores a rule, and check its score on \
                   those in checked_on, which it was not ranked on - a candidate first on fit \
                   but far down place_on_check was tuned to the fit half. place_by_typical and \
                   place_by_off say where each stands ranked either way. among_best marks the \
                   best and those tied with it: show the user how each places the gate beside \
                   theirs before choosing, and the samples off under the best (off_samples), \
                   which may be easier gated by hand than fitted. The best, those tied with it \
                   and the rule as it stands are kept for the app: on the Gallery tab, the \
                   gate's population shows each one's gate over the user's, to step through. \
                   No gate has moved; update_rule with the chosen rule, on the user's word",
        })
    }
}
