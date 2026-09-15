//! Attaching a rule to a gate, and finding the sample it is measured on.
//!
//! A [`Rule`] decides where a line goes given a slice of values. Everything
//! needed to get from a gate and a sample to that slice lives here: which
//! parameter the rule positions, which side of the line the gate keeps, and
//! which file supplies the population to measure.
//!
//! Rules have no representation in an Omiq gating file, so a [`RuleStore`] is
//! written beside one as its own document.

use crate::gate_editor::gates::gate_store::FileId;
use crate::gate_rules::rule::{Rule, Solved};
use crate::gate_rules::threshold::{SolveError, Threshold};
use crate::omiq::metadata::MetaDataFileMap;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// Which side of its threshold a gate keeps.
///
/// Deliberately says nothing about an axis. A rule belongs to a *parameter* -
/// the marker it positions against - and which axis that parameter is drawn on
/// is a property of the plot, to be read off the gate every time. The same
/// marker appears on both axes within a single real workflow, so an axis
/// recorded in a rule is wrong as soon as the plot is drawn the other way
/// round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bound {
    /// The gate keeps events above the threshold - a "positive" gate.
    Above,
    /// It keeps those below - a "negative" one.
    Below,
}

/// Which file a rule reads to decide where the line goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeasuredOn {
    /// A partner in the same specimen carrying this sample type - the FMO for
    /// this marker. Its position is then applied to the sample being gated.
    Partner(Arc<str>),
    /// The sample being gated.
    Itself,
}

/// The metadata columns that say which files belong together.
///
/// Configuration rather than a filename heuristic, so pointing this at another
/// dataset is a matter of naming its columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplePairing {
    /// Groups a specimen's files: the donor and timepoint, typically.
    pub sample_id_column: Arc<str>,
    /// Says what each file is - full stain, FMO, unstained.
    pub sample_type_column: Arc<str>,
}

impl Default for SamplePairing {
    fn default() -> Self {
        Self {
            sample_id_column: Arc::from("Sample ID"),
            sample_type_column: Arc::from("SampleType"),
        }
    }
}

/// One gate's rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateRule {
    /// The parameter the rule positions against. Never an axis - see [`Bound`].
    pub parameter: Arc<str>,
    pub bound: Bound,
    pub measured_on: MeasuredOn,
    pub rule: Rule,
}

impl GateRule {
    /// Solve for this gate's threshold over a parent population.
    ///
    /// A rule that keeps the bottom of a distribution is the same rule on the
    /// distribution's reflection, so `Below` is solved by negating the values
    /// and negating the answer back. That is exact for anything built on order
    /// statistics, which both current rules are, and it means a "negative" gate
    /// needs no rule of its own.
    pub fn solve(&self, values: &[f64], reference: Option<f64>) -> Result<Solved, SolveError> {
        match self.bound {
            Bound::Above => self.rule.apply(values, reference),
            Bound::Below => {
                let mirrored: Vec<f64> = values.iter().map(|v| -v).collect();
                let solved = self.rule.apply(&mirrored, reference.map(|r| -r))?;
                Ok(Solved {
                    threshold: Threshold {
                        x: -solved.threshold.x,
                        ..solved.threshold
                    },
                    confidence: solved.confidence,
                })
            }
        }
    }

    /// How many of `values` this gate admits at `x`, counted on the side the
    /// gate keeps.
    pub fn admitted(&self, values: &[f64], x: f64) -> usize {
        match self.bound {
            Bound::Above => values.iter().filter(|v| **v > x).count(),
            Bound::Below => values.iter().filter(|v| **v < x).count(),
        }
    }
}

/// Every gate's rule, plus how to find the sample each is measured on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleStore {
    #[serde(default)]
    pub pairing: SamplePairing,
    /// Keyed by gate, matching Omiq's own model: a rule belongs to the gate,
    /// not to one of its positions in the tree, so linking a gate carries its
    /// rule with it.
    rules: FxHashMap<Arc<str>, GateRule>,
}

impl RuleStore {
    /// An empty store keyed on the given columns.
    pub fn with_pairing(pairing: SamplePairing) -> Self {
        Self {
            pairing,
            ..Self::default()
        }
    }

    pub fn get(&self, gate: &str) -> Option<&GateRule> {
        self.rules.get(gate)
    }

    pub fn insert(&mut self, gate: Arc<str>, rule: GateRule) -> Option<GateRule> {
        self.rules.insert(gate, rule)
    }

    pub fn remove(&mut self, gate: &str) -> Option<GateRule> {
        self.rules.remove(gate)
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &GateRule)> {
        self.rules.iter()
    }

    /// The file a rule is measured on, given the file being gated.
    ///
    /// `Itself` is the file it was handed. `Partner` looks for the one file in
    /// the same specimen whose sample type matches. Returns `None` when there
    /// is no such partner - an FMO is not always run - which the caller should
    /// report rather than silently fall back to the sample itself, since the
    /// two give quite different answers.
    pub fn reference_file(
        &self,
        file: &FileId,
        measured_on: &MeasuredOn,
        metadata: &MetaDataFileMap,
    ) -> Option<FileId> {
        let wanted = match measured_on {
            MeasuredOn::Itself => return Some(file.clone()),
            MeasuredOn::Partner(t) => t,
        };
        let specimen = metadata.get(file)?.get(&self.pairing.sample_id_column)?;
        metadata
            .iter()
            .find(|(_, columns)| {
                columns.get(&self.pairing.sample_id_column) == Some(specimen)
                    && columns
                        .get(&self.pairing.sample_type_column)
                        .is_some_and(|t| t == wanted)
            })
            .map(|(id, _)| id.clone())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}
