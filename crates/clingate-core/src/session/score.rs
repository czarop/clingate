//! The rules scored against the gating as drawn by hand.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::{Refusal, Session, failed, round};
use crate::gate_rules::run::RunInputs;
use crate::gate_rules::rule_store::{RuleStore, RuleTarget};
use crate::gate_rules::score::{GateScore, ScoreRow, ScoreSettings, least_agreeing_first};

/// Rows shown when no number is asked for, and the most ever shown.
pub const SCORE_ROWS: usize = 40;
pub const SCORE_ROWS_MAX: usize = 400;

/// Every ruled gate, or one, against the hand gating.
#[derive(Debug, Clone, Serialize)]
pub struct ScoreAnswer {
    pub files_read: usize,
    pub problems: Vec<String>,
    /// Each gate in a line, the most samples off first.
    pub gates: Vec<GateScore>,
    pub rows_total: usize,
    /// The samples least like the hand gating, across every gate.
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
    /// under its parent as drawn, and say how closely each agrees with the
    /// gate drawn by hand, judged by `settings`. Moves nothing.
    pub fn score_rules(
        &self,
        population: Option<&str>,
        max_rows: Option<usize>,
        settings: ScoreSettings,
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
            settings,
            &AtomicBool::new(false),
        )
        .map_err(failed)?;

        let name = |file: &str| self.sample_name(&Arc::from(file));
        let rounded = |value: Option<f64>, places| value.map(|v| round(v, places));
        let gates = score
            .gates
            .into_iter()
            .map(|g| GateScore {
                typical_agreement: rounded(g.typical_agreement, 3),
                lowest_agreement: rounded(g.lowest_agreement, 3),
                off_samples: g.off_samples.iter().map(|f| name(f)).collect(),
                median_caught: rounded(g.median_caught, 3),
                median_extra: rounded(g.median_extra, 3),
                median_holds_difference: rounded(g.median_holds_difference, 3),
                median_edge_off_iqrs: rounded(g.median_edge_off_iqrs, 3),
                ..g
            })
            .collect();
        let rows_total = score.rows.len();
        let mut rows = score.rows;
        rows.sort_by(least_agreeing_first);
        let shown = max_rows.unwrap_or(SCORE_ROWS).clamp(1, SCORE_ROWS_MAX);
        let rows = rows
            .into_iter()
            .take(shown)
            .map(|row| ScoreRow {
                file: name(&row.file),
                agreement: rounded(row.agreement, 3),
                caught: rounded(row.caught, 3),
                extra: rounded(row.extra, 3),
                off_line: rounded(row.off_line, 3),
                shift_iqrs: row
                    .shift_iqrs
                    .iter()
                    .map(|(axis, off)| (axis.clone(), round(*off, 3)))
                    .collect(),
                hand_edge: rounded(row.hand_edge, 4),
                rule_edge: rounded(row.rule_edge, 4),
                edge_off_iqrs: rounded(row.edge_off_iqrs, 3),
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
            next: "agreement is 1 only when the rule's gate holds exactly the hand gate's \
                   events, falling to 0 when they share none; caught is how much of the hand \
                   gate's events the rule's gate holds, extra how much of the rule's gate is \
                   beyond the hand gate - which way it is off. A sample is off below its \
                   off_line, which a sample of few events has lower. A gate with a high \
                   typical and a low lowest agreement has a few samples far off \
                   (off_samples); a low typical is every sample a little off. \
                   shift_iqrs and edge_off_iqrs say where the rule's gate sits against the \
                   hand gate. Each gate was read under its parent as drawn; nothing has \
                   moved. Show the user the gates and samples furthest off; score_rules \
                   again after update_rule shows whether a change brought the rules closer",
        })
    }
}
