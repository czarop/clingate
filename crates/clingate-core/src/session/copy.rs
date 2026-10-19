//! Copying one population's rules to its sibling populations - see
//! [`crate::gate_rules::siblings`].

use std::path::PathBuf;

use serde::Serialize;

use super::replay::describe_rule;
use super::{Refusal, Session, failed};
use crate::gate_rules::siblings::copy_rules;

/// A rule copied, as the tools show it.
#[derive(Debug, Clone, Serialize)]
pub struct CopiedRule {
    pub population: String,
    pub rule: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// What may stop the rule placing its gate on some samples, though it
    /// is copied.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

/// A rule left out, and why.
#[derive(Debug, Clone, Serialize)]
pub struct LeftOut {
    pub population: String,
    pub reason: String,
}

/// What a copy wrote, or would write.
#[derive(Debug, Clone, Serialize)]
pub struct CopyAnswer {
    pub from: String,
    pub copied: Vec<CopiedRule>,
    pub left_out: Vec<LeftOut>,
    /// The rules file written, or none for a preview.
    pub written: Option<PathBuf>,
    pub next: &'static str,
}

impl Session {
    /// Copy every rule written for a gate directly under `from` to the gate of
    /// the same name under each of `to`, replacing a rule a gate already has
    /// only when `replace`. Written to the rules file only when `write`;
    /// otherwise what would be written.
    pub fn copy_rules(
        &mut self,
        from: &str,
        to: &[String],
        replace: bool,
        write: bool,
    ) -> Result<CopyAnswer, Refusal> {
        let file = self.rules_file_to_write()?;
        let (from_node, from_facts) = self.one_population(from)?;
        if to.is_empty() {
            return Err(failed("name the populations to copy the rules to"));
        }
        let mut rules = self.rules_or_refuse()?.clone();
        let mut copied = Vec::new();
        let mut left_out = Vec::new();
        for query in to {
            let (to_node, _) = self.one_population(query)?;
            if to_node == from_node {
                return Err(failed("a population's rules cannot be copied to itself"));
            }
            let copy = copy_rules(&self.gates, &rules, &from_node, &to_node, replace);
            rules = copy.written_into(&rules);
            copied.extend(copy.copied.into_iter().map(|c| CopiedRule {
                population: c.target.describe(),
                rule: describe_rule(&c.rule),
                replaces: c.replaces.as_ref().map(describe_rule),
                problems: c.problems,
            }));
            left_out.extend(copy.not_copied.into_iter().map(|n| LeftOut {
                population: n.target.describe(),
                reason: n.reason,
            }));
        }
        if copied.is_empty() && left_out.is_empty() {
            return Err(failed(format!(
                "no gate directly under {} has a rule of its own to copy",
                from_facts.full_path()
            )));
        }
        let written = if write && !copied.is_empty() {
            rules.save(&file).map_err(failed)?;
            self.rules = Some(rules);
            // A preview made under the old rules would apply what they no
            // longer place.
            self.pending = None;
            Some(file)
        } else {
            None
        };
        Ok(CopyAnswer {
            from: from_facts.full_path(),
            copied,
            left_out,
            next: if written.is_some() {
                "the rules are written: list_rules shows them, and preview_rules what they \
                 would move"
            } else {
                "nothing is written yet: show the user what would be copied and what is left \
                 out and why, and copy_rules with write only on their word"
            },
            written,
        })
    }
}
