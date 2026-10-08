//! The candidates a search of a rule's settings came closest with, kept in
//! the workspace so the app can show them on the plots: the latest search of
//! each rule.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::gate_rules::fit::{Candidate, Fit};
use crate::gate_rules::rule_store::RuleTarget;

/// What a kept search is written as.
pub const FORMAT: u32 = 1;
/// The file searches are kept in, in the workspace's rules folder.
pub const SEARCHES_FILE: &str = "searches.json";
/// The most of a search's best candidates kept, beside the rule as it stood.
pub const MOST_KEPT: usize = 6;

/// One rule's search, as kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Search {
    pub format: u32,
    /// When it was made, in UTC.
    pub searched_at: String,
    pub target: RuleTarget,
    /// The gate and the population it is drawn on.
    pub gate: String,
    /// The best and those tied with it, best first, then the rule as it
    /// stood if it is not among them - each with where it puts the gate.
    pub candidates: Vec<Candidate>,
}

impl Search {
    /// What is kept of `fit`, a search of `target`'s rule.
    pub fn of(fit: &Fit, target: &RuleTarget) -> Self {
        let mut candidates: Vec<Candidate> = fit
            .candidates
            .iter()
            .filter(|c| c.among_best)
            .take(MOST_KEPT)
            .cloned()
            .collect();
        let current = fit.candidates.iter().find(|c| c.current);
        if let Some(current) = current.filter(|_| !candidates.iter().any(|c| c.current)) {
            candidates.push(current.clone());
        }
        Self {
            format: FORMAT,
            searched_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            target: target.clone(),
            gate: fit.gate.clone(),
            candidates,
        }
    }

    /// The id of the gate searched, once a sample has been scored.
    pub fn gate_id(&self) -> Option<&str> {
        self.candidates
            .iter()
            .find_map(|c| c.fit.as_ref())
            .map(|score| score.gate_id.as_str())
    }
}

/// Where `folder`'s searches are kept.
pub fn searches_file(folder: &Path) -> PathBuf {
    folder.join(crate::workspace::RULES_DIR).join(SEARCHES_FILE)
}

/// The searches kept in `folder`; none before the first.
pub fn kept(folder: &Path) -> anyhow::Result<Vec<Search>> {
    let path = searches_file(folder);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// Keep `search` in `folder`, in place of an earlier search of the same rule.
pub fn keep(folder: &Path, search: Search) -> anyhow::Result<()> {
    // A file that cannot be read is replaced: it holds nothing a search cannot
    // make again.
    let mut searches = kept(folder).unwrap_or_default();
    searches.retain(|kept| kept.target != search.target);
    searches.push(search);
    let path = searches_file(folder);
    crate::workspace::make_parent(&path)?;
    std::fs::write(path, serde_json::to_string(&searches)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::file_load_tests::scratch;
    use crate::gate_rules::fit::RankBy;
    use crate::gate_rules::rule::{Rule, TailFractionRule};
    use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn};

    fn candidate(upper: f64, among_best: bool, current: bool) -> Candidate {
        Candidate {
            rule: GateRule {
                parameter: Arc::from("CD69"),
                bound: Bound::Above,
                measured_on: MeasuredOn::Itself,
                rule: Rule::TailFraction(TailFractionRule::new((0.0, upper))),
            },
            said: format!("up to {upper}"),
            current,
            fit: None,
            check: None,
            place_by_typical: 1,
            place_by_off: 1,
            place_on_check: None,
            among_best,
            placed: Vec::new(),
        }
    }

    fn fit_of(candidates: Vec<Candidate>) -> Fit {
        Fit {
            gate: "CD69+ of Lymph".into(),
            files_read: 2,
            problems: Vec::new(),
            fit_on: Vec::new(),
            checked_on: Vec::new(),
            rank_by: RankBy::Typical,
            candidates,
        }
    }

    fn uppers(search: &Search) -> Vec<f64> {
        search
            .candidates
            .iter()
            .map(|c| match &c.rule.rule {
                Rule::TailFraction(t) => t.band.1,
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn the_best_and_those_tied_are_kept_then_the_rule_as_it_stands() {
        let mut candidates: Vec<Candidate> = (1..=8)
            .map(|at| candidate(f64::from(at) / 100.0, true, false))
            .collect();
        candidates.push(candidate(0.5, false, false));
        candidates.push(candidate(0.9, false, true));
        let search = Search::of(&fit_of(candidates), &RuleTarget::named("CD69+"));
        assert_eq!(
            uppers(&search),
            [0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.9],
            "the first {MOST_KEPT} of the best, then the rule as it stands"
        );
        let tied_current = vec![candidate(0.01, true, true), candidate(0.02, false, false)];
        let search = Search::of(&fit_of(tied_current), &RuleTarget::named("CD69+"));
        assert_eq!(uppers(&search), [0.01], "the rule as it stands, once");
    }

    #[test]
    fn the_rule_as_it_stands_is_kept_when_tied_beyond_the_most_kept() {
        let candidates: Vec<Candidate> = (1..=8)
            .map(|at| candidate(f64::from(at) / 100.0, true, at == 8))
            .collect();
        let search = Search::of(&fit_of(candidates), &RuleTarget::named("CD69+"));
        assert_eq!(uppers(&search), [0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.08]);
    }

    #[test]
    fn a_rule_searched_again_replaces_its_earlier_search_and_no_other() {
        let folder = scratch("searches-kept");
        assert!(kept(&folder).unwrap().is_empty(), "none before the first");
        let cd69 = RuleTarget::under("CD69+", "CD4+");
        let cd25 = RuleTarget::named("CD25+");
        keep(
            &folder,
            Search::of(&fit_of(vec![candidate(0.01, true, false)]), &cd69),
        )
        .unwrap();
        keep(
            &folder,
            Search::of(&fit_of(vec![candidate(0.02, true, false)]), &cd25),
        )
        .unwrap();
        keep(
            &folder,
            Search::of(&fit_of(vec![candidate(0.03, true, false)]), &cd69),
        )
        .unwrap();
        let searches = kept(&folder).unwrap();
        let targets: Vec<&RuleTarget> = searches.iter().map(|s| &s.target).collect();
        assert_eq!(targets, [&cd25, &cd69]);
        assert_eq!(uppers(&searches[1]), [0.03]);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_replaced() {
        let folder = scratch("searches-unreadable");
        let path = searches_file(&folder);
        crate::workspace::make_parent(&path).unwrap();
        std::fs::write(&path, "not a search").unwrap();
        assert!(kept(&folder).is_err());
        let target = RuleTarget::named("CD69+");
        keep(
            &folder,
            Search::of(&fit_of(vec![candidate(0.01, true, false)]), &target),
        )
        .unwrap();
        assert_eq!(kept(&folder).unwrap().len(), 1);
    }
}
