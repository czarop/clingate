//! A run's placements, sorted into what a reviewer works through.
//!
//! The assessment says which placements have their gate far from their peers'
//! or were placed unsure. A reviewer
//! then works through them - looking, fixing, reporting, or deciding a flag
//! was wrong - and the review is done when nothing is left to look at. The
//! board is that list, sorted into piles:
//!
//! - **needs a look**: flagged, and nobody has done anything about it yet;
//! - **passed**: not flagged, or flagged and then judged to look right;
//! - **reported**: reported as badly placed;
//! - **changed**: no longer where the rule put it, without a report - moved
//!   by hand, most likely a fix.
//!
//! "Looks right" is the reviewer clearing a flag. It is kept per run in
//! `reviews/looks_right.json`, and marking the run reviewed records it with
//! each placement: a flag cleared this way is a flag the assessment should
//! not have raised, which is what tuning it is measured against.
//!
//! The app's Review tab and the tools for Claude both read the board, so
//! both show the same piles.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::assess::{Assessment, Flag};
use super::run_record::{PlacementStatus, RunRecord, SampleRef, placement_status};
use crate::gate_rules::rule_store::human_order;
use crate::gates::GateState;
use crate::omiq::metadata::MetaDataFileMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pile {
    NeedsALook,
    Passed,
    Reported,
    Changed,
}

impl Pile {
    pub const ALL: [Pile; 4] = [
        Pile::NeedsALook,
        Pile::Passed,
        Pile::Reported,
        Pile::Changed,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pile::NeedsALook => "Needs a look",
            Pile::Passed => "Passed",
            Pile::Reported => "Reported",
            Pile::Changed => "Changed since",
        }
    }
}

/// One placement on the board.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub gate_id: String,
    pub gate: String,
    pub parent_gate: Option<String>,
    pub sample: SampleRef,
    /// Moved by the rule, or left where it already met the rule.
    pub moved: bool,
    pub confidence: Option<f64>,
    pub weakest: Option<String>,
    pub pile: Pile,
    /// Why the assessment flagged it, if it did.
    pub flag: Option<Flag>,
    /// A reviewer cleared its flag.
    pub looks_right: bool,
    /// How many reports name it.
    pub reports: usize,
    /// Whether it is still where the rule put it. `None` for a gate the run
    /// left alone.
    pub status: Option<PlacementStatus>,
    /// The file the rule read for it - the specimen's FMO, or a reference
    /// sample every specimen is placed against - where that is not the
    /// sample itself. What the Review tab can show it beside.
    pub rule_read: Option<SampleRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Board {
    pub run_applied_at: String,
    /// Needs a look worst first; the rest by gate, then sample.
    pub entries: Vec<Entry>,
    /// What the run could not place, from the assessment.
    pub unplaced: Vec<crate::review::assess::Unplaced>,
}

impl Board {
    pub fn count(&self, pile: Pile) -> usize {
        self.entries.iter().filter(|e| e.pile == pile).count()
    }

    pub fn pile(&self, pile: Pile) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.pile == pile)
    }

    /// A pile's entries for one gate, by its id - or every gate's with
    /// `None`, as the Review tab's gate filter shows them.
    pub fn pile_for<'a>(
        &'a self,
        pile: Pile,
        gate_id: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Entry> {
        self.pile(pile)
            .filter(move |e| gate_id.is_none_or(|g| e.gate_id == g))
    }

    /// How many are in a pile for one gate, or for every gate.
    pub fn count_for(&self, pile: Pile, gate_id: Option<&str>) -> usize {
        self.pile_for(pile, gate_id).count()
    }
}

// ── looks right ─────────────────────────────────────────────────────────

/// The placements a reviewer has judged to look right despite a flag, for
/// one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LooksRight {
    pub run_applied_at: String,
    /// (gate id, sample id)
    pub placements: Vec<(String, String)>,
}

impl LooksRight {
    pub fn file_in(folder: &Path) -> PathBuf {
        folder.join(super::REVIEWS_DIR).join("looks_right.json")
    }

    /// The run's: empty if nothing has been marked, or what was marked was
    /// for another run.
    pub fn load(folder: &Path, run: &RunRecord) -> Self {
        let read = std::fs::read_to_string(Self::file_in(folder))
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok());
        match read {
            Some(held) if held.run_applied_at == run.applied_at => held,
            _ => Self {
                run_applied_at: run.applied_at.clone(),
                placements: Vec::new(),
            },
        }
    }

    pub fn contains(&self, gate_id: &str, sample: &str) -> bool {
        self.placements
            .iter()
            .any(|(g, s)| g == gate_id && s == sample)
    }

    /// Mark one placement as looking right, or take the mark back, and keep
    /// the change.
    pub fn set(
        folder: &Path,
        run: &RunRecord,
        gate_id: &str,
        sample: &str,
        looks_right: bool,
    ) -> anyhow::Result<Self> {
        let mut held = Self::load(folder, run);
        held.placements
            .retain(|(g, s)| !(g == gate_id && s == sample));
        if looks_right {
            held.placements
                .push((gate_id.to_string(), sample.to_string()));
        }
        let path = Self::file_in(folder);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&held)?)?;
        Ok(held)
    }
}

// ── the board ───────────────────────────────────────────────────────────

/// Sort the run's placements. `reported` is every (gate id, sample id) a
/// report names; `state` and `metadata` are the gates as they stand now.
pub fn board(
    run: &RunRecord,
    assessment: &Assessment,
    reported: &[(String, String)],
    looks_right: &LooksRight,
    state: &GateState,
    metadata: &MetaDataFileMap,
) -> Board {
    let flag_of = |gate_id: &str, sample: &str| {
        assessment
            .flags
            .iter()
            .find(|f| f.gate_id == gate_id && f.sample.id == sample)
            .cloned()
    };
    let reports_of = |gate_id: &str, sample: &str| {
        reported
            .iter()
            .filter(|(g, s)| g == gate_id && s == sample)
            .count()
    };
    let pile_of = |flag: &Option<Flag>, looks: bool, reports: usize, status| {
        if reports > 0 {
            Pile::Reported
        } else if matches!(status, Some(PlacementStatus::Moved | PlacementStatus::Gone)) {
            Pile::Changed
        } else if flag.is_some() && !looks {
            Pile::NeedsALook
        } else {
            Pile::Passed
        }
    };

    let mut entries = Vec::new();
    for p in &run.placed {
        let flag = flag_of(&p.gate_id, &p.sample.id);
        let looks = looks_right.contains(&p.gate_id, &p.sample.id);
        let reports = reports_of(&p.gate_id, &p.sample.id);
        let status = Some(placement_status(p, state, metadata));
        entries.push(Entry {
            pile: pile_of(&flag, looks, reports, status),
            gate_id: p.gate_id.clone(),
            gate: p.gate.clone(),
            parent_gate: p.parent_gate.clone(),
            sample: p.sample.clone(),
            moved: true,
            confidence: Some(p.confidence),
            weakest: p.weakest.clone(),
            flag,
            looks_right: looks,
            reports,
            status,
            rule_read: (p.measured_on.id != p.sample.id).then(|| p.measured_on.clone()),
        });
    }
    // A gate kept as the reference the rule calibrated from is not a
    // placement to review.
    for k in run.kept.iter().filter(|k| k.met_rule) {
        let flag = flag_of(&k.gate_id, &k.sample.id);
        let looks = looks_right.contains(&k.gate_id, &k.sample.id);
        let reports = reports_of(&k.gate_id, &k.sample.id);
        entries.push(Entry {
            pile: pile_of(&flag, looks, reports, None),
            gate_id: k.gate_id.clone(),
            gate: k.gate.clone(),
            parent_gate: k.parent_gate.clone(),
            sample: k.sample.clone(),
            moved: false,
            confidence: None,
            weakest: None,
            flag,
            looks_right: looks,
            reports,
            status: None,
            rule_read: k.measured_on.clone().filter(|read| read.id != k.sample.id),
        });
    }

    let name = |e: &Entry| e.sample.name.clone().unwrap_or_else(|| e.sample.id.clone());
    entries.sort_by(|a, b| {
        let rank = |e: &Entry| Pile::ALL.iter().position(|p| *p == e.pile);
        rank(a).cmp(&rank(b)).then_with(|| {
            if a.pile == Pile::NeedsALook {
                let severity = |e: &Entry| e.flag.as_ref().map_or(0.0, |f| f.severity);
                severity(b).total_cmp(&severity(a))
            } else {
                human_order(&a.gate, &b.gate)
                    .then_with(|| a.parent_gate.cmp(&b.parent_gate))
                    .then_with(|| human_order(&name(a), &name(b)))
            }
        })
    });
    Board {
        run_applied_at: run.applied_at.clone(),
        entries,
        unplaced: assessment.unplaced.clone(),
    }
}

/// The board for the run kept in `folder`, with the gates as they stand.
/// `None` without a run.
pub fn board_in(
    folder: &Path,
    state: &GateState,
    metadata: &MetaDataFileMap,
) -> anyhow::Result<Option<Board>> {
    let Some(run) = RunRecord::load(folder)? else {
        return Ok(None);
    };
    let assessment = super::assess::assess(&run, Some((state, metadata)));
    let reported: Vec<(String, String)> = super::report::reports_of_run(folder, &run.applied_at)
        .into_iter()
        .map(|(_, r)| (r.gate_id, r.sample.id))
        .collect();
    let looks = LooksRight::load(folder, &run);
    Ok(Some(board(
        &run,
        &assessment,
        &reported,
        &looks,
        state,
        metadata,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::assess::{Reason, assess};
    use crate::review::run_record::{KeptRecord, PlacedRecord};

    fn sample(id: &str) -> SampleRef {
        SampleRef {
            id: id.into(),
            name: Some(format!("{id}.fcs")),
            sample_type: Some("FS".into()),
        }
    }

    fn placed(id: &str, gate: &str) -> PlacedRecord {
        PlacedRecord {
            gate_id: format!("g-{gate}"),
            gate: gate.into(),
            parent_gate: Some("CD4+".into()),
            specimen_column: "SampleID".into(),
            specimen: id.into(),
            sample: sample(id),
            measured_on: sample(id),
            from: None,
            to: None,
            confidence: 0.9,
            weakest: None,
            components: Vec::new(),
            achieved: None,
            above_the_line: None,
            reference_events: 0,
            in_band: true,
            read_on_control: false,
            negative: None,
            valley: None,
            phenotype: None,
            // Nothing to compare where it sits, so the status reads Gone:
            // there is no such gate in an empty document.
            placed_at: Vec::new(),
            shape: None,
            bound: None,
        }
    }

    fn flagged(gate: &str, id: &str, severity: f64) -> Flag {
        Flag {
            gate_id: format!("g-{gate}"),
            gate: gate.into(),
            parent_gate: Some("CD4+".into()),
            sample: sample(id),
            moved: true,
            confidence: Some(0.9),
            weakest: None,
            severity,
            reasons: vec![Reason {
                measure: "gate_position",
                says: "odd".into(),
                severity,
            }],
            status: None,
            peers: 5,
            confident_peers: true,
            typical_peer: None,
        }
    }

    fn run() -> RunRecord {
        RunRecord {
            format: 1,
            applied_at: "t1".into(),
            rules: Default::default(),
            placed: Vec::new(),
            kept: vec![
                KeptRecord {
                    gate_id: "g-CD279+".into(),
                    gate: "CD279+".into(),
                    parent_gate: Some("CD4+".into()),
                    specimen: "a".into(),
                    sample: sample("a"),
                    met_rule: true,
                    achieved: None,
                    above_the_line: None,
                    line: None,
                    shape: None,
                    bound: None,
                    measured_on: None,
                },
                KeptRecord {
                    gate_id: "g-CD279+".into(),
                    gate: "CD279+".into(),
                    parent_gate: Some("CD4+".into()),
                    specimen: "ref".into(),
                    sample: sample("ref"),
                    // A reference: not on the board.
                    met_rule: false,
                    achieved: None,
                    above_the_line: None,
                    line: None,
                    shape: None,
                    bound: None,
                    measured_on: None,
                },
                KeptRecord {
                    gate_id: "g-CD279+".into(),
                    gate: "CD279+".into(),
                    parent_gate: Some("CD4+".into()),
                    specimen: "b".into(),
                    sample: sample("b"),
                    met_rule: true,
                    achieved: None,
                    above_the_line: None,
                    line: None,
                    shape: None,
                    bound: None,
                    measured_on: None,
                },
                KeptRecord {
                    gate_id: "g-CD279+".into(),
                    gate: "CD279+".into(),
                    parent_gate: Some("CD4+".into()),
                    specimen: "c".into(),
                    sample: sample("c"),
                    met_rule: true,
                    achieved: None,
                    above_the_line: None,
                    line: None,
                    shape: None,
                    bound: None,
                    measured_on: None,
                },
            ],
            skipped: Vec::new(),
        }
    }

    #[test]
    fn placements_fall_into_their_piles() {
        let mut run = run();
        run.placed.push(placed("moved", "Ki67+"));
        let mut assessment = assess(&run, None);
        assessment.flags = vec![flagged("CD279+", "b", 4.0), flagged("CD279+", "c", 6.0)];
        let reported = vec![("g-CD279+".to_string(), "a".to_string())];
        let looks = LooksRight::default();
        let board = board(
            &run,
            &assessment,
            &reported,
            &looks,
            &GateState::default(),
            &MetaDataFileMap::default(),
        );
        let piles: Vec<(&str, Pile)> = board
            .entries
            .iter()
            .map(|e| (e.sample.id.as_str(), e.pile))
            .collect();
        assert_eq!(
            piles,
            vec![
                // Worst first.
                ("c", Pile::NeedsALook),
                ("b", Pile::NeedsALook),
                ("a", Pile::Reported),
                ("moved", Pile::Changed),
            ]
        );
        assert_eq!(board.count(Pile::Passed), 0);
        assert!(board.entries.iter().all(|e| e.sample.id != "ref"));

        // With one gate chosen, each pile counts that gate's placements only.
        assert_eq!(board.count_for(Pile::NeedsALook, Some("g-CD279+")), 2);
        assert_eq!(board.count_for(Pile::Reported, Some("g-CD279+")), 1);
        assert_eq!(board.count_for(Pile::Changed, Some("g-CD279+")), 0);
        assert_eq!(board.count_for(Pile::Changed, Some("g-Ki67+")), 1);
        assert_eq!(board.count_for(Pile::NeedsALook, Some("g-Ki67+")), 0);
        assert_eq!(board.count_for(Pile::NeedsALook, Some("no such gate")), 0);
        for pile in Pile::ALL {
            assert_eq!(board.count_for(pile, None), board.count(pile));
        }
        let ki67: Vec<&str> = board
            .pile_for(Pile::Changed, Some("g-Ki67+"))
            .map(|e| e.sample.id.as_str())
            .collect();
        assert_eq!(ki67, vec!["moved"]);
    }

    #[test]
    fn gates_left_alone_are_on_the_board_and_passed_are_in_gate_then_sample_order() {
        let run = run();
        let assessment = assess(&run, None);
        let board = board(
            &run,
            &assessment,
            &[],
            &LooksRight::default(),
            &GateState::default(),
            &MetaDataFileMap::default(),
        );
        // a, b and c, left alone as meeting their rule; ref, the reference, not.
        let passed: Vec<&str> = board
            .pile(Pile::Passed)
            .map(|e| e.sample.id.as_str())
            .collect();
        assert_eq!(passed, vec!["a", "b", "c"]);
        assert!(
            board
                .pile(Pile::Passed)
                .all(|e| !e.moved && e.status.is_none())
        );
        assert_eq!(board.run_applied_at, "t1");
    }

    #[test]
    fn each_entry_says_which_file_the_rule_read_unless_it_read_the_sample_itself() {
        let mut run = run();
        let mut on_fmo = placed("fs1", "CD69+");
        on_fmo.measured_on = SampleRef {
            id: "fmx1".into(),
            name: Some("fmx1.fcs".into()),
            sample_type: Some("FMX".into()),
        };
        run.placed.push(on_fmo);
        // `placed` reads the sample itself.
        run.placed.push(placed("fs2", "CD69+"));
        run.kept[0].measured_on = Some(sample("qc"));
        let board = board(
            &run,
            &assess(&run, None),
            &[],
            &LooksRight::default(),
            &GateState::default(),
            &MetaDataFileMap::default(),
        );
        let read = |id: &str| {
            board
                .entries
                .iter()
                .find(|e| e.sample.id == id)
                .unwrap()
                .rule_read
                .as_ref()
                .map(|s| s.id.clone())
        };
        assert_eq!(read("fs1").as_deref(), Some("fmx1"));
        assert_eq!(read("fs2"), None);
        assert_eq!(read("a").as_deref(), Some("qc"));
        assert_eq!(read("b"), None);
    }

    #[test]
    fn a_placement_whose_gate_is_gone_has_changed() {
        let mut run = run();
        run.placed.push(placed("x", "Gone+"));
        let assessment = assess(&run, None);
        let board = board(
            &run,
            &assessment,
            &[],
            &LooksRight::default(),
            &GateState::default(),
            &MetaDataFileMap::default(),
        );
        let x = board.entries.iter().find(|e| e.sample.id == "x").unwrap();
        assert_eq!(x.pile, Pile::Changed);
        assert_eq!(x.status, Some(PlacementStatus::Gone));
    }

    #[test]
    fn a_damaged_looks_right_file_clears_nothing() {
        let folder = crate::file_load_tests::scratch("board-looks-right-damaged");
        let path = LooksRight::file_in(&folder);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ broken").unwrap();
        let held = LooksRight::load(&folder, &run());
        assert!(held.placements.is_empty());
        assert_eq!(held.run_applied_at, "t1");
        // And marking replaces it with a good one.
        LooksRight::set(&folder, &run(), "g", "s", true).unwrap();
        assert!(LooksRight::load(&folder, &run()).contains("g", "s"));
    }

    #[test]
    fn every_pile_has_a_title() {
        let titles: std::collections::HashSet<&str> = Pile::ALL.iter().map(|p| p.title()).collect();
        assert_eq!(titles.len(), 4);
        assert_eq!(Pile::NeedsALook.title(), "Needs a look");
        assert_eq!(
            serde_json::to_value(Pile::NeedsALook).unwrap(),
            "needs_a_look"
        );
    }

    #[test]
    fn looks_right_clears_a_flag_for_its_own_run_only() {
        let folder = crate::file_load_tests::scratch("board-looks-right");
        let run = run();
        let mut assessment = assess(&run, None);
        assessment.flags = vec![flagged("CD279+", "b", 4.0)];
        let sort = |looks: &LooksRight| {
            board(
                &run,
                &assessment,
                &[],
                looks,
                &GateState::default(),
                &MetaDataFileMap::default(),
            )
        };
        assert_eq!(
            sort(&LooksRight::load(&folder, &run)).count(Pile::NeedsALook),
            1
        );

        LooksRight::set(&folder, &run, "g-CD279+", "b", true).unwrap();
        let looks = LooksRight::load(&folder, &run);
        let after = sort(&looks);
        assert_eq!(after.count(Pile::NeedsALook), 0);
        let b = after.entries.iter().find(|e| e.sample.id == "b").unwrap();
        assert!(b.looks_right && b.flag.is_some() && b.pile == Pile::Passed);

        // A newer run starts with nothing cleared.
        let mut newer = run.clone();
        newer.applied_at = "t2".into();
        assert!(LooksRight::load(&folder, &newer).placements.is_empty());

        // And the mark can be taken back.
        LooksRight::set(&folder, &run, "g-CD279+", "b", false).unwrap();
        assert!(LooksRight::load(&folder, &run).placements.is_empty());
    }
}
