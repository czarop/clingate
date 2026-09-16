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
    /// One named file, whichever sample is being gated - a QC or template run
    /// that every sample is positioned against.
    File(FileId),
}

/// A reference file chosen by hand, overriding what the pairing would find.
///
/// The pairing covers the ordinary case, but it cannot cover every one: a
/// specimen's FMO is not always run, a metadata column can be wrong, and a
/// sample can need a stand-in from another specimen. Rather than let the rule
/// fail or quietly measure the wrong thing, the choice is settable - this is
/// what the UI writes when a person picks a reference by hand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceOverride {
    /// The file being gated.
    pub gated: FileId,
    /// The sample type its rule asks for.
    pub sample_type: Arc<str>,
    /// The file to measure instead.
    pub reference: FileId,
}

/// One marker to look for in a column, and what finding it means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SampleTypeMarker {
    /// Matched as a substring, case-sensitively.
    pub contains: Arc<str>,
    pub sample_type: Arc<str>,
}

/// Reading a file's sample type out of some other column when it has no column
/// of its own.
///
/// An export that predates a `SampleType` column still carries the distinction
/// somewhere - usually in the file name, which is itself a metadata column. The
/// markers are configuration rather than a built-in guess about `_FMX_`,
/// because naming conventions are exactly what differs between datasets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedSampleType {
    /// The column to read.
    pub column: Arc<str>,
    /// Tried in order; the first whose `contains` appears in the column wins.
    /// Order matters when one marker is a substring of another.
    pub markers: Vec<SampleTypeMarker>,
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
    /// Consulted only when `sample_type_column` holds nothing for a file.
    #[serde(default)]
    pub derive_type: Option<DerivedSampleType>,
    /// The order a specimen's files are shown in, by sample type. The FMO
    /// belongs on the left, where the line is set, and the full stain on the
    /// right, where the positives are read off - but which names those are is
    /// the dataset's business, not this crate's.
    #[serde(default = "default_display_order")]
    pub display_order: Vec<Arc<str>>,
}

fn default_display_order() -> Vec<Arc<str>> {
    vec![Arc::from("FMX"), Arc::from("FS")]
}

impl Default for SamplePairing {
    fn default() -> Self {
        Self {
            sample_id_column: Arc::from("Sample ID"),
            sample_type_column: Arc::from("SampleType"),
            derive_type: None,
            display_order: default_display_order(),
        }
    }
}

impl SamplePairing {
    /// A file's sample type: its own column where there is one, otherwise
    /// derived from whichever column carries the distinction.
    pub fn sample_type_of(&self, columns: &FxHashMap<Arc<str>, Arc<str>>) -> Option<Arc<str>> {
        if let Some(explicit) = columns.get(&self.sample_type_column) {
            return Some(explicit.clone());
        }
        let derive = self.derive_type.as_ref()?;
        let text = columns.get(&derive.column)?;
        derive
            .markers
            .iter()
            .find(|m| text.contains(&*m.contains))
            .map(|m| m.sample_type.clone())
    }
}

/// Which gates a rule applies to.
///
/// Named, not identified. A gate called "CD279+" occupied twenty-five separate
/// containers in one real export, and writing a rule per container is both
/// miserable to author and wrong in principle: the rule is a statement about a
/// population, and the population is "CD279+ of CD4+", not container `1TBQ`.
///
/// `parent` is what distinguishes the same marker gated on different
/// populations, which is how gates are spoken about - "Ki67+ of CD4+". Leaving
/// it out applies the rule to every gate of that name, which is the common
/// case; a target that names a parent wins over one that does not.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleTarget {
    /// The gate's name, as Omiq stores it on the container.
    pub gate: Arc<str>,
    /// The name of the gate above it, when the rule is specific to one
    /// population.
    #[serde(default)]
    pub parent: Option<Arc<str>>,
}

impl RuleTarget {
    /// Every gate of this name, wherever it appears.
    pub fn named(gate: impl Into<Arc<str>>) -> Self {
        Self {
            gate: gate.into(),
            parent: None,
        }
    }

    /// Gates of this name under a parent of that name.
    pub fn under(gate: impl Into<Arc<str>>, parent: impl Into<Arc<str>>) -> Self {
        Self {
            gate: gate.into(),
            parent: Some(parent.into()),
        }
    }

    /// How it reads in a list: "Ki67+ of CD4+", or just "Ki67+".
    pub fn describe(&self) -> String {
        match &self.parent {
            Some(parent) => format!("{} of {}", self.gate, parent),
            None => self.gate.to_string(),
        }
    }
}

/// A rule and the gates it applies to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleEntry {
    pub target: RuleTarget,
    pub rule: GateRule,
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
    /// A list rather than a map because a target is a pair, and because the
    /// order a person adds rules in is worth keeping.
    rules: Vec<RuleEntry>,
    /// Consulted before the pairing. A list rather than a map because a JSON
    /// object cannot key on a pair, and there are few of these by nature.
    #[serde(default)]
    overrides: Vec<ReferenceOverride>,
}

impl RuleStore {
    /// An empty store keyed on the given columns.
    pub fn with_pairing(pairing: SamplePairing) -> Self {
        Self {
            pairing,
            ..Self::default()
        }
    }

    /// The rule for a gate of this name under a parent of that name.
    ///
    /// A target naming the parent wins over one that does not, so a general
    /// rule can be written once and then overridden for the population that
    /// needs different treatment.
    pub fn rule_for(&self, gate: &str, parent: Option<&str>) -> Option<&GateRule> {
        let specific = self.rules.iter().find(|e| {
            &*e.target.gate == gate
                && e.target
                    .parent
                    .as_deref()
                    .is_some_and(|p| Some(p) == parent)
        });
        specific
            .or_else(|| {
                self.rules
                    .iter()
                    .find(|e| &*e.target.gate == gate && e.target.parent.is_none())
            })
            .map(|e| &e.rule)
    }

    pub fn get(&self, target: &RuleTarget) -> Option<&GateRule> {
        self.rules
            .iter()
            .find(|e| &e.target == target)
            .map(|e| &e.rule)
    }

    /// Add a rule, replacing any that targets exactly the same gates.
    pub fn insert(&mut self, target: RuleTarget, rule: GateRule) -> Option<GateRule> {
        let previous = self.remove(&target);
        self.rules.push(RuleEntry { target, rule });
        previous
    }

    pub fn remove(&mut self, target: &RuleTarget) -> Option<GateRule> {
        let at = self.rules.iter().position(|e| &e.target == target)?;
        Some(self.rules.remove(at).rule)
    }

    pub fn entries(&self) -> &[RuleEntry] {
        &self.rules
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Name the file to measure when gating `gated` and the rule asks for
    /// `sample_type`, in place of whatever the pairing would find. Replaces any
    /// previous choice for the same pair.
    pub fn set_reference(&mut self, gated: FileId, sample_type: Arc<str>, reference: FileId) {
        self.clear_reference(&gated, &sample_type);
        self.overrides.push(ReferenceOverride {
            gated,
            sample_type,
            reference,
        });
    }

    /// Fall back to the pairing for this pair again.
    pub fn clear_reference(&mut self, gated: &str, sample_type: &str) -> bool {
        let before = self.overrides.len();
        self.overrides
            .retain(|o| !(&*o.gated == gated && &*o.sample_type == sample_type));
        self.overrides.len() != before
    }

    pub fn references(&self) -> &[ReferenceOverride] {
        &self.overrides
    }

    /// The file a rule is measured on, given the file being gated.
    ///
    /// `Itself` is the file it was handed and `File` the one it names. `Partner`
    /// takes a hand-set reference if there is one for this pair, and otherwise
    /// looks for the one file in the same specimen whose sample type matches.
    ///
    /// Returns `None` when there is no such partner - an FMO is not always run -
    /// which the caller should report rather than silently fall back to the
    /// sample itself, since the two give quite different answers.
    pub fn reference_file(
        &self,
        file: &FileId,
        measured_on: &MeasuredOn,
        metadata: &MetaDataFileMap,
    ) -> Option<FileId> {
        let wanted = match measured_on {
            MeasuredOn::Itself => return Some(file.clone()),
            MeasuredOn::File(named) => return Some(named.clone()),
            MeasuredOn::Partner(t) => t,
        };
        if let Some(chosen) = self
            .overrides
            .iter()
            .find(|o| o.gated == *file && o.sample_type == *wanted)
        {
            return Some(chosen.reference.clone());
        }
        let specimen = metadata.get(file)?.get(&self.pairing.sample_id_column)?;
        metadata
            .iter()
            .find(|(_, columns)| {
                columns.get(&self.pairing.sample_id_column) == Some(specimen)
                    && self
                        .pairing
                        .sample_type_of(columns)
                        .is_some_and(|t| t == *wanted)
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
