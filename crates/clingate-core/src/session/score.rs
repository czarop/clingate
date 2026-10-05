//! The rules scored against the gating as drawn by hand.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed, round};
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::rule_store::{RuleStore, RuleTarget};
use crate::gate_rules::score::{GateScore, ScoreRow, distance, furthest_first};

/// Rows shown when no number is asked for, and the most ever shown.
pub const SCORE_ROWS: usize = 40;
pub const SCORE_ROWS_MAX: usize = 400;

/// Every ruled gate, or one, against the hand gating.
#[derive(Debug, Clone, Serialize)]
pub struct ScoreAnswer {
    pub files_read: usize,
    pub problems: Vec<String>,
    /// Each gate in a line, furthest from the hand gating first.
    pub gates: Vec<GateScore>,
    pub rows_total: usize,
    /// The samples furthest from the hand gating, across every gate.
    pub rows: Vec<ScoreRow>,
    pub next: &'static str,
}

impl Session {
    /// The target of the rule that places `query`'s gate: the one naming its
    /// parent, else one naming no parent.
    fn rule_target_for(&self, rules: &RuleStore, query: &str) -> Result<RuleTarget, Refusal> {
        let (node, _) = self.one_population(query)?;
        let (target, _, _) = self
            .target_of(&node)
            .ok_or_else(|| failed("that population has no gate"))?;
        let of_gate = |parent: Option<&str>| {
            rules
                .entries()
                .iter()
                .find(|e| e.target.gate == target.gate && e.target.parent.as_deref() == parent)
        };
        of_gate(target.parent.as_deref())
            .or_else(|| of_gate(None))
            .map(|e| e.target.clone())
            .ok_or_else(|| failed("no rule places that population"))
    }

    /// Run every rule - or `population`'s alone - on the files, each gate
    /// under its parent as drawn, and say how far each lands from the gate
    /// drawn by hand. Moves nothing.
    pub fn score_rules(
        &self,
        population: Option<&str>,
        max_rows: Option<usize>,
    ) -> Result<ScoreAnswer, Refusal> {
        let rules = self.rules_or_refuse()?.clone();
        let wanted = population
            .map(|query| self.rule_target_for(&rules, query))
            .transpose()?;
        let inputs = RunInputs::assemble(
            Some(&self.files),
            &self.compensation,
            &self.metadata,
            &self.axes.settings,
            &rules,
        );
        let score = crate::gate_rules::score::score_rules(
            &self.gates,
            &inputs,
            |target| wanted.as_ref().is_none_or(|w| w == target),
            &AtomicBool::new(false),
        )
        .map_err(failed)?;

        let name = |file: &str| self.sample_name(&Arc::from(file));
        let rounded = |value: Option<f64>, places| value.map(|v| round(v, places));
        let gates = score
            .gates
            .into_iter()
            .map(|g| GateScore {
                median_off_iqrs: rounded(g.median_off_iqrs, 3),
                worst_off_iqrs: rounded(g.worst_off_iqrs, 3),
                median_holds_difference: rounded(g.median_holds_difference, 3),
                worst_holds_difference: rounded(g.worst_holds_difference, 3),
                worst: g.worst.iter().map(|f| name(f)).collect(),
                ..g
            })
            .collect();
        let rows_total = score.rows.len();
        let mut rows = score.rows;
        rows.sort_by(|a, b| furthest_first(&distance(a), &distance(b)));
        let shown = max_rows.unwrap_or(SCORE_ROWS).clamp(1, SCORE_ROWS_MAX);
        let rows = rows
            .into_iter()
            .take(shown)
            .map(|row| ScoreRow {
                file: name(&row.file),
                hand_line: rounded(row.hand_line, 4),
                rule_line: rounded(row.rule_line, 4),
                off_iqrs: rounded(row.off_iqrs, 3),
                hand_holds: rounded(row.hand_holds, 5),
                rule_holds: rounded(row.rule_holds, 5),
                holds_difference: rounded(row.holds_difference, 3),
                confidence: rounded(row.confidence, 3),
                ..row
            })
            .collect();
        Ok(ScoreAnswer {
            files_read: score.files_read,
            problems: score.problems,
            gates,
            rows_total,
            rows,
            next: "off_iqrs is the rule's line minus the hand-drawn one, in that sample's \
                   parent's interquartile ranges; holds_difference is the rule's % of the \
                   parent minus the hand gate's, in percentage points. Each gate was read \
                   under its parent as drawn. Nothing has moved. try_rules compares other \
                   rules for one gate; score_rules again after update_rule shows whether a \
                   change brought the rules closer to the hand gating",
        })
    }
}
