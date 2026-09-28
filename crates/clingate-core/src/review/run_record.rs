//! A rules run, kept once it is applied.
//!
//! What a run decided exists only while the run's result is in hand; once the
//! placements are written into the gates, the reasons for them - where each
//! gate moved from, how sure the rule was and why - are gone. The record keeps
//! them, in the workspace's `reviews` folder, so that:
//!
//! - a review of the run reads what the rules did without running them again;
//! - a report of a badly placed gate can say where the *rule* put it, even
//!   after a person has moved it;
//! - whether a gate is still where the rule put it can be told
//!   ([`placement_status`]), so a hand-moved gate is never mistaken for the
//!   rule's answer.
//!
//! One record per workspace: the last run applied. The app and the tools for
//! Claude both write it, through [`RunRecord::from_run`] and
//! [`RunRecord::save`], when a run's placements are applied.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxBuildHasher;
use serde::{Deserialize, Serialize};

use crate::gate_rules::autogate::{Placement, Report};
use crate::gate_rules::rule_store::RuleStore;
use crate::gates::GateState;
use crate::gates::gate_store::FileId;
use crate::gates::gate_traits::DrawableGate;
use crate::omiq::metadata::MetaDataFileMap;

/// The file a workspace's last applied run is kept in, in
/// [`super::REVIEWS_DIR`].
pub const RUN_RECORD_FILE: &str = "rules_run.json";

/// Bumped when the shape of the file changes in a way an older reader would
/// misread.
pub const FORMAT: u32 = 1;

/// A number the file can hold: JSON has no NaN or infinity, so a measurement
/// that could not be taken is written as nothing rather than as a lie.
fn finite(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub format: u32,
    /// When the run was applied, in UTC.
    pub applied_at: String,
    /// The rules as they stood for this run - rules are edited between runs,
    /// and a placement means nothing without the rule that made it.
    pub rules: RuleStore,
    /// Gates the rules moved.
    pub placed: Vec<PlacedRecord>,
    /// Gates that already met their rule, and the references the rules
    /// calibrated from - left where they were.
    pub kept: Vec<KeptRecord>,
    /// Gates the rules could not place, and why.
    pub skipped: Vec<SkippedRecord>,
}

/// The sample a record is about: its gating id, and what a person calls it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleRef {
    pub id: String,
    /// Its name in the program, where the metadata gives one.
    pub name: Option<String>,
    /// What kind of sample it is, from the pairing's sample-type column: what
    /// it is compared with when a run is reviewed.
    pub sample_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub name: String,
    pub score: f64,
    pub detail: String,
}

/// Where a gate sits on one of its parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtentRecord {
    pub parameter: String,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NegativeRecord {
    pub centre: f64,
    pub spread: f64,
    pub flank_events: usize,
    pub widths: f64,
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValleyRecord {
    pub peak: f64,
    pub bottom: f64,
    pub depth: f64,
    pub offset: f64,
    pub at: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhenotypeRecord {
    pub markers: Vec<String>,
    pub matched: usize,
    pub parent: usize,
    pub reference_matched: usize,
    pub reference_parent: usize,
    pub purity: f64,
    pub caught: f64,
    pub pieces: usize,
}

/// One gate the rules moved, for one specimen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedRecord {
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    /// The specimen the gate was placed for: the metadata column and value
    /// that group its files.
    pub specimen_column: String,
    pub specimen: String,
    /// The sample the placement was worked out for.
    pub sample: SampleRef,
    /// The sample the rule read to decide where the line goes.
    pub measured_on: SampleRef,
    /// Where the line was and where it went, on the rule's parameter. For a
    /// phenotype rule, the fraction of the parent the gate held before and
    /// after.
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub confidence: f64,
    pub weakest: Option<String>,
    pub components: Vec<ComponentRecord>,
    /// What the placed gate admits from the population it was judged on.
    pub achieved: Option<f64>,
    /// What a bare threshold at the line would admit, the gate's other sides
    /// ignored.
    pub above_the_line: Option<f64>,
    pub reference_events: usize,
    pub in_band: bool,
    /// For a rule placed against the negative: on the reference, then here.
    pub negative: Option<(NegativeRecord, NegativeRecord)>,
    /// For a rule placed in the valley: on the reference, then here.
    pub valley: Option<(ValleyRecord, ValleyRecord)>,
    pub phenotype: Option<PhenotypeRecord>,
    /// Where the placed gate sits on each of its parameters - what tells
    /// whether it is still where the rule put it.
    pub placed_at: Vec<ExtentRecord>,
    /// The parent population the rule read on this sample, on the rule's
    /// parameter - what the run is assessed on. `None` for a phenotype rule.
    #[serde(default)]
    pub shape: Option<crate::review::shape::Shape>,
    /// Which side of the line the gate keeps.
    #[serde(default)]
    pub bound: Option<crate::gate_rules::rule_store::Bound>,
}

/// A gate left where it was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeptRecord {
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    pub specimen: String,
    pub sample: SampleRef,
    /// Already met its rule, rather than being a reference the rule
    /// calibrated from.
    pub met_rule: bool,
    pub achieved: Option<f64>,
    pub above_the_line: Option<f64>,
    /// Where its line sits, the parent population on the rule's parameter,
    /// and which side the gate keeps.
    #[serde(default)]
    pub line: Option<f64>,
    #[serde(default)]
    pub shape: Option<crate::review::shape::Shape>,
    #[serde(default)]
    pub bound: Option<crate::gate_rules::rule_store::Bound>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkippedRecord {
    pub gate: String,
    pub parent_gate: Option<String>,
    /// Empty for a problem with a whole file rather than one gate.
    pub sample: SampleRef,
    pub reason: String,
}

/// Where a gate sits on its two parameters.
pub fn extent_of(gate: &dyn DrawableGate) -> Vec<ExtentRecord> {
    let (x, y) = gate.get_params();
    let geometry = gate.get_gate_ref(None).map(|g| g.geometry.clone());
    [x, y]
        .iter()
        .map(|p| {
            let found = geometry
                .as_ref()
                .and_then(|g| crate::gate_rules::autogate::extent_on(g, p));
            ExtentRecord {
                parameter: p.to_string(),
                lower: found.and_then(|(lo, _)| finite(lo as f64)),
                upper: found.and_then(|(_, hi)| finite(hi as f64)),
            }
        })
        .collect()
}

/// What is known about each sample, to name it in a record.
pub struct Samples<'a> {
    names: HashMap<&'a str, &'a str>,
    metadata: &'a MetaDataFileMap,
    pairing: &'a crate::gate_rules::rule_store::SamplePairing,
}

impl<'a> Samples<'a> {
    pub fn new(
        file_name_to_gating_id: &'a HashMap<Arc<str>, FileId, FxBuildHasher>,
        metadata: &'a MetaDataFileMap,
        pairing: &'a crate::gate_rules::rule_store::SamplePairing,
    ) -> Self {
        Self {
            names: file_name_to_gating_id
                .iter()
                .map(|(name, id)| (id.as_ref(), name.as_ref()))
                .collect(),
            metadata,
            pairing,
        }
    }

    pub fn sample(&self, id: &str) -> SampleRef {
        SampleRef {
            id: id.to_string(),
            name: self.names.get(id).map(|n| n.to_string()),
            // As the Gate Rules tab reads it: the type column, or where that
            // is empty, derived from whichever column the pairing names.
            sample_type: self
                .metadata
                .get(id)
                .and_then(|m| self.pairing.sample_type_of(m))
                .map(|v| v.to_string()),
        }
    }
}

impl RunRecord {
    /// The record of a run whose `placements` are about to be applied, from
    /// its `report`. The two come from one run: [`Report::positioned`] and
    /// the placements are made together, one for one, in the same order.
    pub fn from_run(
        report: &Report,
        placements: &[Placement],
        rules: &RuleStore,
        samples: &Samples,
    ) -> Self {
        debug_assert_eq!(report.positioned.len(), placements.len());
        let placed = report
            .positioned
            .iter()
            .zip(placements)
            .map(|(p, placement)| PlacedRecord {
                gate_id: placement.gate_id.to_string(),
                gate: p.gate.to_string(),
                parent_gate: p.parent_gate.as_ref().map(|g| g.to_string()),
                specimen_column: placement.specimen.parameter.to_string(),
                specimen: placement.specimen.group.to_string(),
                sample: samples.sample(&p.file),
                measured_on: samples.sample(&p.measured_on),
                from: finite(p.from),
                to: finite(p.to),
                confidence: p.confidence,
                weakest: p.weakest.map(str::to_string),
                components: p
                    .components
                    .iter()
                    .map(|c| ComponentRecord {
                        name: c.name.to_string(),
                        score: c.score,
                        detail: c.detail.clone(),
                    })
                    .collect(),
                achieved: finite(p.achieved),
                above_the_line: finite(p.above_the_line),
                reference_events: p.reference_events,
                in_band: p.in_band,
                negative: p.negative.as_ref().map(|(reference, here)| {
                    let read = |r: &crate::gate_rules::rule::NegativeRead| NegativeRecord {
                        centre: r.centre,
                        spread: r.spread,
                        flank_events: r.flank_events,
                        widths: r.widths,
                        at: r.at,
                    };
                    (read(reference), read(here))
                }),
                valley: p.valley.as_ref().map(|(reference, here)| {
                    let read = |r: &crate::gate_rules::rule::ValleyRead| ValleyRecord {
                        peak: r.peak,
                        bottom: r.bottom,
                        depth: r.depth,
                        offset: r.offset,
                        at: r.at,
                    };
                    (read(reference), read(here))
                }),
                phenotype: p.phenotype.as_ref().map(|r| PhenotypeRecord {
                    markers: r.markers.iter().map(|m| m.to_string()).collect(),
                    matched: r.matched,
                    parent: r.parent,
                    reference_matched: r.reference_matched,
                    reference_parent: r.reference_parent,
                    purity: r.purity,
                    caught: r.caught,
                    pieces: r.pieces,
                }),
                placed_at: extent_of(placement.gate.as_ref()),
                shape: p.shape.clone(),
                bound: p.bound,
            })
            .collect();

        let kept_one = |u: &crate::gate_rules::autogate::Unchanged, met_rule: bool| KeptRecord {
            gate_id: u.gate_id.to_string(),
            gate: u.gate.to_string(),
            parent_gate: u.parent_gate.as_ref().map(|g| g.to_string()),
            specimen: u.specimen.to_string(),
            sample: samples.sample(&u.file),
            met_rule,
            achieved: finite(u.achieved),
            above_the_line: finite(u.above_the_line),
            line: u.line.and_then(finite),
            shape: u.shape.clone(),
            bound: u.bound,
        };
        let kept = report
            .unchanged
            .iter()
            .map(|u| kept_one(u, true))
            .chain(report.reference.iter().map(|u| kept_one(u, false)))
            .collect();

        let skipped = report
            .skipped
            .iter()
            .map(|s| SkippedRecord {
                gate: s.gate.to_string(),
                parent_gate: s.parent_gate.as_ref().map(|g| g.to_string()),
                sample: samples.sample(&s.file),
                reason: s.reason.clone(),
            })
            .collect();

        Self {
            format: FORMAT,
            applied_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            rules: rules.clone(),
            placed,
            kept,
            skipped,
        }
    }

    /// The record of a run, naming each sample from the workspace's
    /// metadata, and its kind as the rules' pairing reads it. What
    /// the app and the tools both call when a run finishes.
    pub fn of_run(
        report: &Report,
        placements: &[Placement],
        rules: &RuleStore,
        metadata: &crate::omiq::metadata::MetaDataStore,
    ) -> Self {
        let samples = Samples::new(
            metadata.file_name_to_gating_id(),
            metadata.metadata(),
            &rules.pairing,
        );
        Self::from_run(report, placements, rules, &samples)
    }

    /// The run's placements have just been applied: stamp the time and keep
    /// it as the workspace's last applied run.
    pub fn applied(mut self, folder: &Path) -> anyhow::Result<PathBuf> {
        self.applied_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        self.save(folder)
    }

    /// Where a workspace keeps its last applied run.
    pub fn file_in(folder: &Path) -> PathBuf {
        folder.join(super::REVIEWS_DIR).join(RUN_RECORD_FILE)
    }

    /// Keep this as the workspace's last applied run, making the `reviews`
    /// folder if it is not there yet.
    pub fn save(&self, folder: &Path) -> anyhow::Result<PathBuf> {
        let path = Self::file_in(folder);
        crate::workspace::make_parent(&path)?;
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, text)?;
        Ok(path)
    }

    /// The workspace's last applied run, if one was kept.
    pub fn load(folder: &Path) -> anyhow::Result<Option<Self>> {
        let path = Self::file_in(folder);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let record: Self =
            serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        if record.format > FORMAT {
            anyhow::bail!(
                "{} was written by a newer clingate (format {}); this one reads format {FORMAT}",
                path.display(),
                record.format
            );
        }
        Ok(Some(record))
    }
}

/// Whether a placed gate is still where the rule put it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementStatus {
    /// As the rule left it.
    AsPlaced,
    /// Moved since - by hand, an undo, another run or a newer gating file.
    Moved,
    /// The gate is no longer in the document.
    Gone,
}

/// Whether `placed` is still where the rule put it, in `state`.
///
/// Compared by where the gate sits on its parameters, to a hair's width of the
/// axis rather than exactly: the gate may have been saved and read back
/// through Omiq's format, which rounds.
pub fn placement_status(
    placed: &PlacedRecord,
    state: &GateState,
    metadata: &MetaDataFileMap,
) -> PlacementStatus {
    let gate_id: Arc<str> = Arc::from(placed.gate_id.as_str());
    let file: FileId = Arc::from(placed.sample.id.as_str());
    let Some(gate) = state.gate_for_file(&gate_id, &file, metadata) else {
        return PlacementStatus::Gone;
    };
    let now = extent_of(gate.as_ref());
    let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0),
        (None, None) => true,
        _ => false,
    };
    let same = now.len() == placed.placed_at.len()
        && now.iter().zip(&placed.placed_at).all(|(n, p)| {
            n.parameter == p.parameter && close(n.lower, p.lower) && close(n.upper, p.upper)
        });
    if same {
        PlacementStatus::AsPlaced
    } else {
        PlacementStatus::Moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> RunRecord {
        RunRecord {
            format: FORMAT,
            applied_at: "2026-09-28T12:00:00Z".into(),
            rules: RuleStore::default(),
            placed: vec![PlacedRecord {
                gate_id: "g1".into(),
                gate: "CD4+".into(),
                parent_gate: Some("CD3+".into()),
                specimen_column: "SampleID".into(),
                specimen: "S1".into(),
                sample: SampleRef {
                    id: "f1".into(),
                    name: Some("S1_FS.fcs".into()),
                    sample_type: Some("FS".into()),
                },
                measured_on: SampleRef {
                    id: "f2".into(),
                    name: None,
                    sample_type: Some("FMX".into()),
                },
                from: Some(1.0),
                to: Some(1.5),
                confidence: 0.4,
                weakest: Some("rule satisfied".into()),
                components: vec![ComponentRecord {
                    name: "rule satisfied".into(),
                    score: 0.4,
                    detail: "2.1% against 1-2%".into(),
                }],
                achieved: Some(0.021),
                above_the_line: None,
                reference_events: 1200,
                in_band: false,
                negative: None,
                valley: Some((
                    ValleyRecord {
                        peak: 0.1,
                        bottom: 1.0,
                        depth: 0.8,
                        offset: 0.05,
                        at: 1.05,
                    },
                    ValleyRecord {
                        peak: 0.2,
                        bottom: 1.4,
                        depth: 0.3,
                        offset: 0.05,
                        at: 1.45,
                    },
                )),
                phenotype: None,
                placed_at: vec![ExtentRecord {
                    parameter: "CD4".into(),
                    lower: Some(1.5),
                    upper: None,
                }],
                shape: None,
                bound: None,
            }],
            kept: Vec::new(),
            skipped: Vec::new(),
        }
    }

    #[test]
    fn a_sample_type_derived_from_another_column_is_the_one_recorded() {
        use crate::gate_rules::rule_store::{DerivedSampleType, SamplePairing, SampleTypeMarker};
        let pairing = SamplePairing {
            derive_type: Some(DerivedSampleType {
                column: "FileName".into(),
                markers: vec![
                    SampleTypeMarker {
                        contains: "_FMX_".into(),
                        sample_type: "FMX".into(),
                    },
                    SampleTypeMarker {
                        contains: "_FS_".into(),
                        sample_type: "FS".into(),
                    },
                ],
            }),
            ..SamplePairing::default()
        };
        let row = |name: &str| {
            let mut m = rustc_hash::FxHashMap::default();
            m.insert(Arc::<str>::from("FileName"), Arc::<str>::from(name));
            m
        };
        let mut metadata = MetaDataFileMap::default();
        metadata.insert(Arc::from("a"), row("x_FMX_1.fcs"));
        metadata.insert(Arc::from("b"), row("x_FS_1.fcs"));
        let mut explicit = row("x_FS_2.fcs");
        explicit.insert(Arc::from("SampleType"), Arc::from("Unstained"));
        metadata.insert(Arc::from("c"), explicit);
        let names = HashMap::default();
        let samples = Samples::new(&names, &metadata, &pairing);
        assert_eq!(samples.sample("a").sample_type.as_deref(), Some("FMX"));
        assert_eq!(samples.sample("b").sample_type.as_deref(), Some("FS"));
        // The type column wins where it says something.
        assert_eq!(
            samples.sample("c").sample_type.as_deref(),
            Some("Unstained")
        );
        assert_eq!(samples.sample("nobody").sample_type, None);
    }

    #[test]
    fn a_record_is_kept_in_the_reviews_folder_and_read_back_whole() {
        let folder = crate::file_load_tests::scratch("run-record-round-trip");
        let path = record().save(&folder).unwrap();
        assert_eq!(path, folder.join("reviews").join("rules_run.json"));
        assert_eq!(RunRecord::load(&folder).unwrap(), Some(record()));
    }

    #[test]
    fn a_workspace_without_a_run_has_no_record_and_a_newer_one_is_refused() {
        let folder = crate::file_load_tests::scratch("run-record-missing");
        assert_eq!(RunRecord::load(&folder).unwrap(), None);
        let mut newer = record();
        newer.format = FORMAT + 1;
        newer.save(&folder).unwrap();
        assert!(RunRecord::load(&folder).is_err());
    }

    #[test]
    fn a_measurement_that_could_not_be_taken_is_written_as_nothing() {
        assert_eq!(finite(f64::NAN), None);
        assert_eq!(finite(f64::INFINITY), None);
        assert_eq!(finite(0.5), Some(0.5));
        // And so the file is valid JSON whatever was measured.
        let mut r = record();
        r.placed[0].to = finite(f64::NAN);
        let text = serde_json::to_string(&r).unwrap();
        let back: RunRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back.placed[0].to, None);
    }
}
