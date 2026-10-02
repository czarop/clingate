//! A whole rules run: read every workspace file, measure the gates the rules
//! name, solve each against its reference, and hand back the placements -
//! without writing them. The Gate Rules tab calls this on a worker thread and
//! writes the placements itself once it knows the document is the one that was
//! measured; anything else driving a run does the same.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustc_hash::FxBuildHasher;

use crate::gate_rules::autogate::Report;
use crate::gate_rules::rule_store::RuleStore;
use crate::gates::GateState;

/// Everything a run reads apart from the gates.
///
/// Also what a caller compares, before writing a run's answers, with how
/// things stand when it ends: answers measured on one workspace mean nothing
/// in another - a new gating file under the same gate ids, a metadata export
/// that groups the files differently, a new cofactor, an edited rule.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct RunInputs {
    /// Each workspace file, by the name the metadata knows it by.
    pub files: Vec<(Arc<str>, PathBuf)>,
    /// What each file is compensated with.
    pub compensation: crate::compensation::groups::Compensation,
    /// File name to gating id, from the metadata.
    pub names: HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    /// Channel and cofactor for every arcsinh axis.
    pub cofactors: Vec<(Arc<str>, f32)>,
    pub metadata: crate::omiq::metadata::MetaDataFileMap,
    pub rules: RuleStore,
}

impl RunInputs {
    /// Everything a run reads, from the workspace as it stands. The one way
    /// both the Gate Rules tab and the tools for Claude put a run together,
    /// so they cannot measure different things.
    pub fn assemble(
        files: Option<&crate::file_load::FcsFiles>,
        compensation: &crate::compensation::groups::Compensation,
        metadata: &crate::omiq::metadata::MetaDataStore,
        axes: &crate::omiq::serialise::AxisSettings,
        rules: &RuleStore,
    ) -> Self {
        Self {
            // Each file with the name the metadata knows it by.
            files: files
                .map(|f| {
                    f.file_list()
                        .iter()
                        .map(|stub| (stub.name.clone(), stub.get_filepath().to_path_buf()))
                        .collect()
                })
                .unwrap_or_default(),
            compensation: compensation.clone(),
            names: metadata.file_name_to_gating_id().clone(),
            cofactors: Self::cofactors_of(axes),
            metadata: metadata.metadata().clone(),
            rules: rules.clone(),
        }
    }

    /// The arcsinh cofactors out of the axis settings, as a run reads them.
    pub fn cofactors_of(axes: &crate::omiq::serialise::AxisSettings) -> Vec<(Arc<str>, f32)> {
        let mut out = Vec::new();
        for (param, info) in axes.iter() {
            if info.is_arcsinh()
                && let Some(cofactor) = info.get_cofactor()
            {
                out.push((param.clone(), cofactor));
            }
        }
        out
    }
}

/// What a run is doing, for the progress line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Progress {
    /// Step 1: reading the files. Reading and measuring go together: one
    /// file is read, measured, and dropped before the next is opened.
    Measuring { done: usize, total: usize },
    /// Step 2: every file is in; each gate on each file is being positioned.
    Solving { done: usize, total: usize },
}

impl Progress {
    /// How far through its own step the run is. Each step fills the bar from
    /// empty: how long positioning takes against reading depends on how many
    /// rules there are - a few rules, and reading is nearly all of it; fifteen
    /// hundred, and positioning is - so no fixed share of one bar is right.
    pub fn fraction(self) -> f64 {
        match self {
            Progress::Measuring { done, total } | Progress::Solving { done, total }
                if total > 0 =>
            {
                done as f64 / total as f64
            }
            _ => 0.0,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Progress::Measuring { done, total } => {
                format!("Step 1 of 2 - reading file {done} of {total}")
            }
            Progress::Solving { done, total } if total > 0 => {
                format!("Step 2 of 2 - positioning gate {done} of {total}")
            }
            Progress::Solving { .. } => "Step 2 of 2 - positioning the gates...".to_string(),
        }
    }
}

/// The workspace's FCS files, each paired with the gating id the metadata
/// gives it.
///
/// Takes the files the workspace holds rather than reading a folder. The rules
/// tab used to walk its own folder field, which could point somewhere other
/// than the files the editor had open - two tabs measuring two different
/// experiments with nothing to say so.
///
/// Each file is looked up by its name in the program, not its name on disk:
/// for a file from a sub-folder those differ, and it is the program name the
/// metadata is written against.
pub fn files_to_read(
    files: &[(Arc<str>, PathBuf)],
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
) -> (Vec<(PathBuf, Arc<str>)>, Vec<String>) {
    let mut files = files.to_vec();
    // Measuring runs in parallel and the results are flattened in this order,
    // so it has to be fixed - see `measure_all`.
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut problems = Vec::new();
    let mut found = Vec::new();
    for (name, path) in files {
        // The metadata export's name, exactly. Matching on a stem instead is
        // how one donor's gates came to be scored against another's population.
        match names.get(&name) {
            Some(id) => found.push((path, id.clone())),
            None => problems.push(format!("{name}: no metadata row with this name")),
        }
    }
    (found, problems)
}

/// Read and measure every file, several at a time.
///
/// Each file is independent: reading is I/O and parsing, measuring builds an
/// R-tree over that file's events alone, and the gate state is only read. The
/// frame is dropped as soon as its measurements are taken, so what is alive at
/// once is one frame per worker rather than the whole experiment.
///
/// Results are collected **in path order**, not completion order: `collect` on
/// an indexed parallel iterator returns elements in the iterator's order, and
/// the flattening below preserves it. That is what keeps the answer the same
/// whichever thread finishes first - `position_all` answers once per specimen
/// and takes the first file of each, so an order that varied with scheduling
/// would make two identical runs disagree. `files_to_read` sorts, so the
/// iterator's order is the sorted one.
#[allow(clippy::too_many_arguments)]
pub fn measure_all(
    snapshot: &GateState,
    files: &[(Arc<str>, PathBuf)],
    compensation: &crate::compensation::groups::Compensation,
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    arcsinh: &[(Arc<str>, f32)],
    metadata: &crate::omiq::metadata::MetaDataFileMap,
    rules: &RuleStore,
    only: Option<&rustc_hash::FxHashSet<crate::gates::gate_store::NodeId>>,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> (
    Vec<crate::gate_rules::autogate::Measurement>,
    Vec<crate::gate_rules::autogate::Unmeasured>,
    Vec<String>,
) {
    let (mut per_store, problems) = measure_many(
        snapshot,
        files,
        compensation,
        names,
        arcsinh,
        metadata,
        std::slice::from_ref(rules),
        only,
        cancel,
        progress,
    );
    let (measured, unmeasured) = per_store.pop().unwrap_or_default();
    (measured, unmeasured, problems)
}

/// What one set of rules measured.
pub type Measured = (
    Vec<crate::gate_rules::autogate::Measurement>,
    Vec<crate::gate_rules::autogate::Unmeasured>,
);

/// [`measure_all`] for several sets of rules at once: each file is read once
/// and measured for every set - reading is most of the cost, so trying three
/// candidate rules costs little more than trying one. One result per set, in
/// the order given, and the problems reading files.
#[allow(clippy::too_many_arguments)]
pub fn measure_many(
    snapshot: &GateState,
    files: &[(Arc<str>, PathBuf)],
    compensation: &crate::compensation::groups::Compensation,
    names: &HashMap<Arc<str>, Arc<str>, FxBuildHasher>,
    arcsinh: &[(Arc<str>, f32)],
    metadata: &crate::omiq::metadata::MetaDataFileMap,
    stores: &[RuleStore],
    only: Option<&rustc_hash::FxHashSet<crate::gates::gate_store::NodeId>>,
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> (Vec<Measured>, Vec<String>) {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (files, mut problems) = files_to_read(files, names);
    let total = files.len();
    let done = AtomicUsize::new(0);

    let per_file: Vec<(Vec<Measured>, Vec<String>)> = files
        .par_iter()
        .map(|(path, id)| {
            let mut out: Vec<Measured> = (0..stores.len())
                .map(|_| (Vec::new(), Vec::new()))
                .collect();
            let mut trouble = Vec::new();
            if cancel.load(Ordering::Relaxed) {
                return (out, trouble);
            }
            // Compensated as its group says and scaled as it is drawn, as the
            // editor and the gallery read it, so a gate is placed on the
            // events a person sees. Only the cofactors this file carries: one
            // channel absent from one file used to fail every rule on every
            // file, the run reporting "Parameter AF P1-A not found" six times
            // and placing nothing.
            let frame = crate::events::read_scaled(path, &compensation.matrix_for(path), arcsinh);
            match frame {
                Ok(df) => {
                    for (store, slot) in stores.iter().zip(out.iter_mut()) {
                        match crate::gate_rules::autogate::measure_file_at(
                            snapshot, id, &df, metadata, store, only,
                        ) {
                            Ok((m, u)) => *slot = (m, u),
                            Err(e) => {
                                let said = format!("{id}: {e}");
                                if !trouble.contains(&said) {
                                    trouble.push(said);
                                }
                            }
                        }
                    }
                }
                Err(e) => trouble.push(format!("{}: {e}", path.display())),
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            (out, trouble)
        })
        .collect();

    let mut results: Vec<Measured> = (0..stores.len())
        .map(|_| (Vec::new(), Vec::new()))
        .collect();
    for (per_store, trouble) in per_file {
        for ((m, u), (all_m, all_u)) in per_store.into_iter().zip(results.iter_mut()) {
            all_m.extend(m);
            all_u.extend(u);
        }
        problems.extend(trouble);
    }
    (results, problems)
}

/// Everything a run produces, handed back in one piece.
pub struct RunOutcome {
    pub report: Report,
    pub placements: Vec<crate::gate_rules::autogate::Placement>,
    /// Whether it was stopped before solving; nothing is placed if so.
    pub cancelled: bool,
    /// A sample of the events behind every gate on every file the run
    /// measured, kept with the run once it is applied.
    pub events: crate::review::events::KeptEvents,
    /// Set when a pausing run stopped for a person to place gates by hand.
    pub paused: Option<Paused>,
}

impl RunOutcome {
    /// This part of a paused run followed by the part that went on after it:
    /// one run, as the Review tab keeps it.
    pub fn then(mut self, rest: RunOutcome) -> RunOutcome {
        self.report.positioned.extend(rest.report.positioned);
        self.report.unchanged.extend(rest.report.unchanged);
        self.report.reference.extend(rest.report.reference);
        self.report.skipped.extend(rest.report.skipped);
        self.report.unplaced.extend(rest.report.unplaced);
        self.placements.extend(rest.placements);
        self.events.metadata.extend(rest.events.metadata);
        self.events.samples.extend(rest.events.samples);
        self.cancelled = rest.cancelled;
        self.paused = rest.paused;
        self
    }
}

/// Below this confidence, a gate that other rules measure under stops a
/// pausing run.
pub const PAUSE_BELOW: f64 = 0.2;

/// A gate a person has to place on one specimen before a run goes on.
#[derive(Clone, Debug, PartialEq)]
pub struct NeedsPlacing {
    pub gate_id: crate::gates::gate_store::GateId,
    pub gate: Arc<str>,
    pub parent_gate: Option<Arc<str>>,
    /// The file to place it on.
    pub file: crate::gates::gate_store::FileId,
    /// `None` for a file with no sample id: placed on that file alone.
    pub specimen: Option<crate::omiq::metadata::MetaDataKey>,
    pub why: String,
}

/// Where a pausing run stopped, and what it needs before it goes on.
#[derive(Clone, Debug, PartialEq)]
pub struct Paused {
    /// The level to go on from with [`run_rules_pausing`].
    pub next_level: usize,
    pub needs: Vec<NeedsPlacing>,
}

/// The whole solve. Blocking: run it off any thread that has to stay
/// responsive.
///
/// It works against `gates` as given - a snapshot, for a caller that keeps
/// editing - and returns the placements rather than writing them, so a gate
/// moved while this runs is not silently overwritten. `cancel` stops it at the
/// next file while reading, and at the next gate while positioning.
pub fn run_rules(
    gates: &GateState,
    inputs: &RunInputs,
    progress: impl Fn(Progress) + Sync,
    cancel: &AtomicBool,
) -> RunOutcome {
    run_levels(gates, inputs, 0, false, progress, cancel)
}

/// [`run_rules`] from level `from_level`, stopping after any level where a
/// gate with ruled gates under it could not be placed, or was placed with a
/// confidence below [`PAUSE_BELOW`], on any specimen: a child measured under a
/// misplaced parent is measured on the wrong cells. What was placed up to
/// there is handed back with [`RunOutcome::paused`] saying what a person has to
/// place, and the level to go on from once they have.
pub fn run_rules_pausing(
    gates: &GateState,
    inputs: &RunInputs,
    from_level: usize,
    progress: impl Fn(Progress) + Sync,
    cancel: &AtomicBool,
) -> RunOutcome {
    run_levels(gates, inputs, from_level, true, progress, cancel)
}

fn run_levels(
    gates: &GateState,
    inputs: &RunInputs,
    from_level: usize,
    pause: bool,
    progress: impl Fn(Progress) + Sync,
    cancel: &AtomicBool,
) -> RunOutcome {
    use crate::gate_rules::autogate::{
        Skipped, apply_placements, linked_conflicts, rule_levels, rules_reaching_nothing,
        solve_all_reporting,
    };
    use std::sync::atomic::Ordering;

    let stopped = || RunOutcome {
        report: Report::default(),
        placements: Vec::new(),
        cancelled: true,
        events: Default::default(),
        paused: None,
    };

    // Level by level, down the tree: each level is measured on the gates as
    // the levels above it left them. See `rule_levels`.
    let levels = rule_levels(gates, &inputs.rules);
    let steps = levels.len().max(1);
    let mut working = gates.clone();
    let mut report = Report::default();
    let mut placements = Vec::new();
    let mut measured_all = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    let mut paused = None;

    // Said once, by the run's first part: a run that goes on after a pause
    // would otherwise say it again.
    if from_level == 0 {
        for target in rules_reaching_nothing(gates, &inputs.rules) {
            report.skipped.push(Skipped {
                file: Arc::from(""),
                gate: target.gate.clone(),
                parent_gate: target.parent.clone(),
                reason:
                    "this rule reaches no gate: no gate of this name is drawn under a parent of \
                         that name, or every one has a more specific rule"
                        .to_string(),
            });
        }
        for problem in crate::gate_rules::autogate::anchor_problems(gates, &inputs.rules) {
            report.skipped.push(Skipped {
                file: Arc::from(""),
                gate: problem.target.gate.clone(),
                parent_gate: problem.target.parent.clone(),
                reason: problem.reason,
            });
        }
        for conflict in linked_conflicts(gates, &inputs.rules) {
            report.skipped.push(Skipped {
                file: Arc::from(""),
                gate: conflict.gate.clone(),
                parent_gate: None,
                reason: conflict.reason,
            });
        }
    }

    for (step, level) in levels.iter().enumerate().skip(from_level) {
        let (measured, unmeasured, trouble) = measure_all(
            &working,
            &inputs.files,
            &inputs.compensation,
            &inputs.names,
            &inputs.cofactors,
            &inputs.metadata,
            &inputs.rules,
            Some(level),
            cancel,
            |done, total| {
                progress(Progress::Measuring {
                    done: step * total + done,
                    total: steps * total,
                })
            },
        );
        // Every level reads every file, so a file that cannot be read says so
        // once, not once a level.
        for problem in trouble {
            if !problems.contains(&problem) {
                problems.push(problem);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return stopped();
        }

        progress(Progress::Solving { done: 0, total: 0 });
        let (level_report, level_placements) = solve_all_reporting(
            &working,
            &inputs.rules,
            &measured,
            &unmeasured,
            &inputs.metadata,
            |done, total| progress(Progress::Solving { done, total }),
            cancel,
        );
        if cancel.load(Ordering::Relaxed) {
            return stopped();
        }
        // The next level reads its populations through these.
        apply_placements(&mut working, &level_placements, &inputs.metadata);
        let needs = if pause {
            needing_a_person(&working, &levels, &level_report, &inputs.rules)
        } else {
            Vec::new()
        };

        report.positioned.extend(level_report.positioned);
        report.unchanged.extend(level_report.unchanged);
        report.reference.extend(level_report.reference);
        report.skipped.extend(level_report.skipped);
        report.unplaced.extend(level_report.unplaced);
        placements.extend(level_placements);
        measured_all.extend(measured);
        if !needs.is_empty() {
            paused = Some(Paused {
                next_level: step + 1,
                needs,
            });
            break;
        }
    }

    for problem in problems {
        report.skipped.push(Skipped {
            file: Arc::from(""),
            gate: Arc::from(""),
            parent_gate: None,
            reason: problem,
        });
    }

    RunOutcome {
        report,
        placements,
        cancelled: false,
        events: crate::review::events::of_run(&measured_all, &inputs.metadata),
        paused,
    }
}

/// Put each gate a person has to place in the mode of positions by the column
/// its specimens are grouped by, where it stands now on every specimen, so
/// that moving it on one specimen moves it for nobody else. An edit is saved
/// at the level the gate came from, and a gate still in its drawn position
/// would move everywhere. A file with no specimen shows the gate as drawn.
pub fn hold_for_placing(
    state: &mut GateState,
    needs: &[NeedsPlacing],
    metadata: &crate::omiq::metadata::MetaDataFileMap,
) {
    use crate::gates::gate_positions::{Mode, set_mode};
    for need in needs {
        if let Some(specimen) = &need.specimen {
            // Refused for a gate not registered, or one a file cannot show:
            // there is nothing of it to hold.
            let _ = set_mode(
                state,
                &need.gate_id,
                &Mode::ByColumn(specimen.parameter.clone()),
                None,
                metadata,
            );
        }
    }
}

/// The gates of a level a person has to place before the levels under it can
/// be measured: unplaced, or placed with a confidence under [`PAUSE_BELOW`], on
/// any specimen, and with a gate of `ruled` somewhere under them.
pub(crate) fn needing_a_person(
    state: &GateState,
    ruled: &[rustc_hash::FxHashSet<crate::gates::gate_store::NodeId>],
    level: &Report,
    rules: &RuleStore,
) -> Vec<NeedsPlacing> {
    let parents = gates_above(state, ruled);
    let unplaced = level
        .unplaced
        .iter()
        .filter(|u| parents.contains(&u.gate_id))
        .map(|u| NeedsPlacing {
            gate_id: u.gate_id.clone(),
            gate: u.gate.clone(),
            parent_gate: u.parent_gate.clone(),
            file: u.file.clone(),
            specimen: u.specimen.clone(),
            why: u.reason.clone(),
        });
    let doubtful = level
        .positioned
        .iter()
        .filter(|p| p.confidence < PAUSE_BELOW && parents.contains(&p.gate_id))
        .map(|p| NeedsPlacing {
            gate_id: p.gate_id.clone(),
            gate: p.gate.clone(),
            parent_gate: p.parent_gate.clone(),
            file: p.file.clone(),
            specimen: Some(crate::omiq::metadata::MetaDataKey {
                parameter: rules.pairing.sample_id_column.clone(),
                group: p.specimen.clone(),
            }),
            why: match p.weakest {
                Some(weakest) => format!(
                    "placed with confidence {:.2}, held down by {weakest}",
                    p.confidence
                ),
                None => format!("placed with confidence {:.2}", p.confidence),
            },
        });
    unplaced.chain(doubtful).collect()
}

/// Every gate with one of `nodes` somewhere under it.
fn gates_above(
    state: &GateState,
    nodes: &[rustc_hash::FxHashSet<crate::gates::gate_store::NodeId>],
) -> rustc_hash::FxHashSet<crate::gates::gate_store::GateId> {
    let mut above = rustc_hash::FxHashSet::default();
    for node in nodes.iter().flatten() {
        let mut at = state.parent_node(node);
        while let Some(parent) = at {
            if let Some(gate) = state.gate_for_node(&parent) {
                above.insert(gate.clone());
            }
            at = state.parent_node(&parent);
        }
    }
    above
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(pairs: &[(&str, &str)]) -> HashMap<Arc<str>, Arc<str>, FxBuildHasher> {
        let mut map = HashMap::with_hasher(FxBuildHasher);
        for (file, id) in pairs {
            map.insert(Arc::from(*file), Arc::from(*id));
        }
        map
    }

    fn workspace(pairs: &[(&str, &str)]) -> Vec<(Arc<str>, PathBuf)> {
        pairs
            .iter()
            .map(|(name, path)| (Arc::from(*name), PathBuf::from(path)))
            .collect()
    }

    #[test]
    fn files_are_read_in_sorted_order() {
        // Measuring runs in parallel and the results are flattened in this
        // order, so two identical runs agree only if this order is fixed.
        let files = workspace(&[
            ("c.fcs", "/w/c.fcs"),
            ("a.fcs", "/w/a.fcs"),
            ("b.fcs", "/w/b.fcs"),
        ]);
        let names = named(&[("a.fcs", "A"), ("b.fcs", "B"), ("c.fcs", "C")]);
        let (found, problems) = files_to_read(&files, &names);

        let ids: Vec<&str> = found.iter().map(|(_, id)| id.as_ref()).collect();
        assert_eq!(ids, ["A", "B", "C"]);
        assert!(problems.is_empty());
    }

    #[test]
    fn a_file_with_no_metadata_row_is_reported_not_guessed() {
        // Matching on a stem rather than the exact name is how one donor's
        // gates came to be scored against another's population.
        let files = workspace(&[
            ("known.fcs", "/w/known.fcs"),
            ("stranger.fcs", "/w/stranger.fcs"),
        ]);
        let names = named(&[("known.fcs", "A")]);
        let (found, problems) = files_to_read(&files, &names);

        assert_eq!(found.len(), 1);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("stranger.fcs"), "{:?}", problems);
    }

    #[test]
    fn a_file_from_a_sub_folder_is_looked_up_by_its_program_name() {
        // Plate_10/A1.fcs is known as Plate_10_A1.fcs, and that is the name
        // the metadata has to carry. Looking it up by the name on disk would
        // find another plate's A1.
        let files = workspace(&[("Plate_10_A1.fcs", "/w/Plate_10/A1.fcs")]);
        let found_by_program_name = files_to_read(&files, &named(&[("Plate_10_A1.fcs", "P10")]));
        assert_eq!(found_by_program_name.0.len(), 1);
        assert_eq!(&*found_by_program_name.0[0].1, "P10");

        let by_disk_name = files_to_read(&files, &named(&[("A1.fcs", "WRONG")]));
        assert!(by_disk_name.0.is_empty(), "matched on the name on disk");
        assert_eq!(by_disk_name.1.len(), 1);
    }

    #[test]
    fn the_path_that_is_read_is_the_real_one() {
        // The name is for the metadata; opening the file needs its path.
        let files = workspace(&[("Plate_10_A1.fcs", "/w/Plate_10/A1.fcs")]);
        let (found, _) = files_to_read(&files, &named(&[("Plate_10_A1.fcs", "P10")]));
        assert_eq!(found[0].0, PathBuf::from("/w/Plate_10/A1.fcs"));
    }

    #[test]
    fn no_files_is_nothing_to_do_rather_than_a_problem() {
        let (found, problems) = files_to_read(&[], &named(&[]));
        assert!(found.is_empty());
        assert!(problems.is_empty());
    }

    // ── a whole run, from files on disk ──────────────────────────────────────
    //
    // Read every workspace file, measure the gates the rules name, solve
    // against each file's reference, and hand back the placements. Everything
    // below `run_rules` is tested piece by piece on frames built in memory;
    // these are the tests that start from FCS files, so they see the reading,
    // the metadata join and the solve agree about which file is which.

    mod a_run_from_files_on_disk {
        use super::*;
        use crate::file_load_tests::{scratch, write_fcs_rows};
        use crate::gate_rules::rule::{AboveTheNegativeRule, Rule};
        use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleTarget};
        use crate::gates::gate_store::GateSource;
        use crate::gates::gate_traits::DrawableGate;
        use rand::SeedableRng;
        use rand_distr::{Distribution, Normal, Uniform};
        use std::sync::atomic::AtomicBool;

        const X: &str = "FSC-A";
        const Y: &str = "SSC-A";

        /// A negative centred at `centre` and a positive well above it.
        fn population(centre: f32, seed: u64) -> Vec<Vec<f32>> {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let neg = Normal::new(centre, 40.0).unwrap();
            let pos = Uniform::new(centre + 250.0, centre + 400.0).unwrap();
            let mut rows: Vec<Vec<f32>> = (0..9_000)
                .map(|_| vec![neg.sample(&mut rng), 100.0])
                .collect();
            rows.extend((0..1_000).map(|_| vec![pos.sample(&mut rng), 100.0]));
            rows
        }

        /// A positive gate at the root whose left edge sits at 500 - right
        /// for a negative at 300.
        fn positive_gate() -> (GateState, Arc<str>) {
            let mut state = GateState::default();
            let geometry = flow_gates::create_rectangle_geometry(
                vec![(500.0, -1e16), (1e16, -1e16), (1e16, 1e16), (500.0, 1e16)],
                X,
                Y,
            )
            .unwrap();
            let id: Arc<str> = Arc::from("CD134+");
            let gate: Arc<dyn DrawableGate> = Arc::new(
                crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
                    flow_gates::Gate {
                        id: id.clone(),
                        name: "CD134+".into(),
                        geometry,
                        mode: flow_gates::GateMode::Global,
                        parameters: (Arc::from(X), Arc::from(Y)),
                        label_position: None,
                    },
                    true,
                )
                .unwrap(),
            );
            state.place_gate(&[id.clone()], &gate, &GateSource::Global);
            state.place_new_gate(None, id.clone()).unwrap();
            (state, id)
        }

        fn specimens() -> crate::omiq::metadata::MetaDataFileMap {
            let mut map = im::HashMap::with_hasher(FxBuildHasher);
            for (file, id) in [("fs_qc", "QC-A"), ("fs_b", "DONOR-B")] {
                let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
                columns.insert(Arc::from("SampleID"), Arc::from(id));
                columns.insert(Arc::from("SampleType"), Arc::from("FS"));
                map.insert(Arc::from(file) as Arc<str>, columns);
            }
            map
        }

        fn rules() -> RuleStore {
            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule::default()),
                },
            );
            store
        }

        /// The QC's negative at 300; the donor's has drifted to 600.
        fn workspace(name: &str) -> Vec<(Arc<str>, PathBuf)> {
            let dir = scratch(name);
            let channels = [(X, None), (Y, None)];
            write_fcs_rows(
                &dir.join("fs_qc.fcs"),
                &channels,
                &population(300.0, 1),
                &[],
            );
            write_fcs_rows(&dir.join("fs_b.fcs"), &channels, &population(600.0, 2), &[]);
            vec![
                (Arc::from("fs_qc.fcs"), dir.join("fs_qc.fcs")),
                (Arc::from("fs_b.fcs"), dir.join("fs_b.fcs")),
            ]
        }

        fn run(state: &GateState, files: Vec<(Arc<str>, PathBuf)>, cancel: bool) -> RunOutcome {
            run_rules(
                &state.clone(),
                &RunInputs {
                    files: files,
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                    cofactors: Vec::new(),
                    metadata: specimens(),
                    rules: rules(),
                },
                |_| {},
                &Arc::new(AtomicBool::new(cancel)),
            )
        }

        fn left_edge(state: &GateState, gate: &Arc<str>, file: &str) -> f32 {
            let g = state
                .gate_for_file(gate, &Arc::from(file), &specimens())
                .unwrap();
            crate::gate_rules::autogate::extent_on(&g.get_gate_ref(None).unwrap().geometry, X)
                .unwrap()
                .0
        }

        /// A run with a sort column: the report is put in the sort's order,
        /// and each placement the run applied has to stay with its own line
        /// of the report. They were sorted apart - the report sorted, the
        /// placements not - and the run record, which pairs them, gave each
        /// placement another specimen's position: every gate the sort had
        /// reordered read as moved the moment the run finished.
        #[test]
        fn a_sorted_run_keeps_each_placement_with_its_own_report_line() {
            use crate::gate_rules::rule::TailFractionRule;
            use crate::review::run_record::{
                PlacementStatus, RunRecord, Samples, placement_status,
            };
            let (mut state, id) = positive_gate();
            let dir = scratch("run-sorted-record");
            let channels = [(X, None), (Y, None)];
            // Files in one order, the sort column in the other.
            let donors = [
                ("fs_a", "DONOR-A", "3", 250.0),
                ("fs_b", "DONOR-B", "2", 300.0),
                ("fs_c", "DONOR-C", "1", 380.0),
            ];
            let mut files = Vec::new();
            let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
            for (at, (file, donor, visit, centre)) in donors.iter().enumerate() {
                let path = dir.join(format!("{file}.fcs"));
                write_fcs_rows(&path, &channels, &population(*centre, at as u64 + 10), &[]);
                files.push((Arc::from(format!("{file}.fcs").as_str()), path));
                let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
                columns.insert(Arc::from("SampleID"), Arc::from(*donor));
                columns.insert(Arc::from("SampleType"), Arc::from("FS"));
                columns.insert(Arc::from("Visit"), Arc::from(*visit));
                metadata.insert(Arc::from(*file) as Arc<str>, columns);
            }
            let mut rules = RuleStore::default();
            rules.pairing.sort_column = Some(Arc::from("Visit"));
            rules.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::Itself,
                    rule: Rule::TailFraction(TailFractionRule::new((0.05, 0.08))),
                },
            );
            let names = named(&[
                ("fs_a.fcs", "fs_a"),
                ("fs_b.fcs", "fs_b"),
                ("fs_c.fcs", "fs_c"),
            ]);
            let outcome = run_rules(
                &state.clone(),
                &RunInputs {
                    files,
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: names.clone(),
                    cofactors: Vec::new(),
                    metadata: metadata.clone(),
                    rules: rules.clone(),
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            );
            let placed: Vec<&str> = outcome.report.positioned.iter().map(|p| &*p.file).collect();
            assert_eq!(
                placed,
                vec!["fs_c", "fs_b", "fs_a"],
                "the premise: sorted by visit"
            );

            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &metadata,
            );
            let samples = Samples::new(&names, &metadata, &rules.pairing);
            let record =
                RunRecord::from_run(&outcome.report, &outcome.placements, &rules, &samples);
            assert_eq!(record.placed.len(), 3);
            for p in &record.placed {
                assert_eq!(
                    placement_status(p, &state, &metadata),
                    PlacementStatus::AsPlaced,
                    "{} reads as moved straight after the run",
                    p.sample.id
                );
                let drawn = left_edge_in(&state, &id, &p.sample.id, &metadata) as f64;
                assert!(
                    (drawn - p.to.unwrap()).abs() < 1e-3,
                    "{}: drawn {drawn}, recorded {:?}",
                    p.sample.id,
                    p.to
                );
                assert_eq!(
                    p.specimen,
                    metadata[p.sample.id.as_str()]["SampleID"].to_string()
                );
            }
        }

        /// A rectangle like `positive_gate`'s with its left edge at `edge`.
        fn gate_with_edge(id: &Arc<str>, edge: f32) -> Arc<dyn DrawableGate> {
            let geometry = flow_gates::create_rectangle_geometry(
                vec![(edge, -1e16), (1e16, -1e16), (1e16, 1e16), (edge, 1e16)],
                X,
                Y,
            )
            .unwrap();
            Arc::new(
                crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
                    flow_gates::Gate {
                        id: id.clone(),
                        name: "CD134+".into(),
                        geometry,
                        mode: flow_gates::GateMode::Global,
                        parameters: (Arc::from(X), Arc::from(Y)),
                        label_position: None,
                    },
                    true,
                )
                .unwrap(),
            )
        }

        /// The QC, a drifted donor, and a file no metadata row names a
        /// specimen for: one placed, one reference, one skipped.
        /// The QC, a drifted donor, and a file no metadata row names a
        /// specimen for: one placed, one reference, one skipped.
        fn three_way_inputs(name: &str) -> RunInputs {
            let mut files = workspace(name);
            let dir = files[0].1.parent().unwrap().to_path_buf();
            write_fcs_rows(
                &dir.join("fs_x.fcs"),
                &[(X, None), (Y, None)],
                &population(300.0, 3),
                &[],
            );
            files.push((Arc::from("fs_x.fcs"), dir.join("fs_x.fcs")));
            let mut metadata = specimens();
            let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
            columns.insert(Arc::from("SampleType"), Arc::from("FS"));
            metadata.insert(Arc::from("fs_x"), columns);
            RunInputs {
                files,
                compensation: crate::compensation::groups::Compensation::default(),
                names: named(&[
                    ("fs_qc.fcs", "fs_qc"),
                    ("fs_b.fcs", "fs_b"),
                    ("fs_x.fcs", "fs_x"),
                ]),
                cofactors: Vec::new(),
                metadata,
                rules: rules(),
            }
        }

        fn three_way_run(
            name: &str,
        ) -> (
            GateState,
            Arc<str>,
            RunOutcome,
            crate::omiq::metadata::MetaDataFileMap,
            RuleStore,
        ) {
            let (state, id) = positive_gate();
            let inputs = three_way_inputs(name);
            let outcome = run_rules(&state, &inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            (state, id, outcome, inputs.metadata, inputs.rules)
        }

        /// Where a workspace built by `workspace` lives.
        fn folder_of(inputs: &RunInputs) -> PathBuf {
            inputs.files[0].1.parent().unwrap().to_path_buf()
        }

        /// Run, apply, and keep the record, as the app and the tools do.
        fn applied(inputs: &RunInputs) -> (GateState, Arc<str>, crate::review::RunRecord) {
            let (mut state, id) = positive_gate();
            let outcome = run_rules(&state, inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &inputs.metadata,
            );
            let samples = crate::review::run_record::Samples::new(
                &inputs.names,
                &inputs.metadata,
                &inputs.rules.pairing,
            );
            let record = crate::review::RunRecord::from_run(
                &outcome.report,
                &outcome.placements,
                &inputs.rules,
                &samples,
            );
            record.save(&folder_of(inputs)).unwrap();
            (
                state,
                id,
                crate::review::RunRecord::load(&folder_of(inputs))
                    .unwrap()
                    .unwrap(),
            )
        }

        fn request(state: &GateState, id: &Arc<str>, sample: &str) -> crate::review::ReportRequest {
            crate::review::ReportRequest {
                node: state.nodes_for_gate(id)[0].clone(),
                sample: Arc::from(sample),
                problem: crate::review::Problem::TooLoose,
                note: "  lets in the negatives  ".into(),
            }
        }

        #[test]
        fn a_report_before_any_run_says_so_and_keeps_the_population() {
            use crate::review::report::{Decision, gather};
            let inputs = three_way_inputs("report-no-run");
            let (state, id) = positive_gate();
            let report = gather(
                &folder_of(&inputs),
                &request(&state, &id, "fs_b"),
                &state,
                &inputs,
                &Default::default(),
            )
            .unwrap();
            let Decision::NotPlaced { why } = &report.decision else {
                panic!("{:?}", report.decision);
            };
            assert!(why.contains("no rules run"), "{why}");
            assert_eq!(report.run_applied_at, None);
            // The workspace's own rule, as there was no run's.
            assert_eq!(report.rule.as_ref(), inputs.rules.rule_for("CD134+", None));
            assert_eq!(report.note, "lets in the negatives", "trimmed");
            assert_eq!(report.problem, crate::review::Problem::TooLoose);
            assert_eq!(report.path, "CD134+");
            assert_eq!(report.gate_id, "CD134+");
            assert_eq!(report.sample.name.as_deref(), Some("fs_b.fcs"));
            assert_eq!(report.sample_metadata["SampleID"], "DONOR-B");

            // The whole population, binned three ways and sampled.
            let data = &report.data;
            assert_eq!(data.events, 10_000);
            assert_eq!(data.histograms.0.parameter, X);
            assert_eq!(data.histograms.1.parameter, Y);
            assert_eq!(data.histograms.0.counts.iter().sum::<u32>(), 10_000);
            assert_eq!(data.histograms.1.counts.iter().sum::<u32>(), 10_000);
            assert_eq!(data.density.counts.iter().sum::<u32>(), 10_000);
            assert_eq!(
                data.events_subsample.len(),
                crate::review::report::SUBSAMPLE_EVENTS
            );
            // With no scaling, the axis is the data's own range.
            let xs = data.events_subsample.iter().map(|p| p.0 as f64);
            let lowest = xs.fold(f64::INFINITY, f64::min);
            assert!(data.histograms.0.lower <= lowest);
            // Where the gate was when reported.
            let on_x = data.gate_at.iter().find(|e| e.parameter == X).unwrap();
            assert_eq!(on_x.lower, Some(500.0));
            assert!(report.reference_data.is_none());
        }

        #[test]
        fn a_report_says_what_the_run_decided_for_each_kind_of_sample() {
            use crate::review::report::{Decision, gather};
            let inputs = three_way_inputs("report-kinds");
            let (state, id, record) = applied(&inputs);
            let folder = folder_of(&inputs);
            let report = |sample: &str| {
                gather(
                    &folder,
                    &request(&state, &id, sample),
                    &state,
                    &inputs,
                    &Default::default(),
                )
                .unwrap()
            };

            // The donor: the rule moved it, reading the QC - both populations kept.
            let donor = report("fs_b");
            let Decision::Placed(placed) = &donor.decision else {
                panic!("{:?}", donor.decision);
            };
            assert_eq!(**placed, record.placed[0]);
            assert_eq!(
                donor.run_applied_at.as_deref(),
                Some(record.applied_at.as_str())
            );
            assert_eq!(donor.rule.as_ref(), record.rules.rule_for("CD134+", None));
            let reference = donor
                .reference_data
                .as_ref()
                .expect("the QC's population too");
            assert_eq!(reference.sample.id, "fs_qc");
            assert_eq!(reference.events, 10_000);
            let on_x = donor
                .data
                .gate_at
                .iter()
                .find(|e| e.parameter == X)
                .unwrap();
            assert!((on_x.lower.unwrap() - placed.to.unwrap()).abs() < 1e-3);
            assert_eq!(donor.placed(), Some(&**placed));

            // The QC: kept as the reference, and it is its own population.
            let qc = report("fs_qc");
            let Decision::Kept(kept) = &qc.decision else {
                panic!("{:?}", qc.decision);
            };
            assert!(!kept.met_rule);
            assert!(qc.reference_data.is_none());

            // The unnamed file: the run could not place it, and says why.
            let unnamed = report("fs_x");
            let Decision::NotPlaced { why } = &unnamed.decision else {
                panic!("{:?}", unnamed.decision);
            };
            assert!(why.starts_with("the run could not place it"), "{why}");
            assert!(why.contains("SampleID"), "{why}");
        }

        #[test]
        fn a_report_that_cannot_be_made_says_why() {
            use crate::review::report::gather;
            let mut inputs = three_way_inputs("report-refused");
            let (state, id) = positive_gate();
            let folder = folder_of(&inputs);
            let mut root = request(&state, &id, "fs_b");
            root.node =
                crate::gates::gate_store::NodeId::from(crate::gates::gate_store::ROOTGATE.clone());
            let e = gather(&folder, &root, &state, &inputs, &Default::default()).unwrap_err();
            assert!(e.contains("no gate"), "{e}");
            let e = gather(
                &folder,
                &request(&state, &id, "nobody"),
                &state,
                &inputs,
                &Default::default(),
            )
            .unwrap_err();
            assert!(e.contains("no metadata row"), "{e}");
            inputs.files.retain(|(n, _)| &**n != "fs_b.fcs");
            let e = gather(
                &folder,
                &request(&state, &id, "fs_b"),
                &state,
                &inputs,
                &Default::default(),
            )
            .unwrap_err();
            assert!(e.contains("not in the workspace"), "{e}");
        }

        /// Three donors, each placed by a tail-fraction rule on itself.
        fn three_donor_inputs(name: &str) -> RunInputs {
            use crate::gate_rules::rule::TailFractionRule;
            let dir = scratch(name);
            let channels = [(X, None), (Y, None)];
            let donors = [
                ("fs_a", "DONOR-A", 250.0),
                ("fs_b", "DONOR-B", 300.0),
                ("fs_c", "DONOR-C", 380.0),
            ];
            let mut files = Vec::new();
            let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
            for (at, (file, donor, centre)) in donors.iter().enumerate() {
                let path = dir.join(format!("{file}.fcs"));
                write_fcs_rows(&path, &channels, &population(*centre, at as u64 + 20), &[]);
                files.push((Arc::from(format!("{file}.fcs").as_str()), path));
                let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
                columns.insert(Arc::from("SampleID"), Arc::from(*donor));
                columns.insert(Arc::from("SampleType"), Arc::from("FS"));
                metadata.insert(Arc::from(*file) as Arc<str>, columns);
            }
            let mut rules = RuleStore::default();
            rules.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::Itself,
                    rule: Rule::TailFraction(TailFractionRule::new((0.05, 0.08))),
                },
            );
            RunInputs {
                files,
                compensation: crate::compensation::groups::Compensation::default(),
                names: named(&[
                    ("fs_a.fcs", "fs_a"),
                    ("fs_b.fcs", "fs_b"),
                    ("fs_c.fcs", "fs_c"),
                ]),
                cofactors: Vec::new(),
                metadata,
                rules,
            }
        }

        #[test]
        fn a_reviewed_run_says_what_became_of_every_placement_and_goes_to_the_library() {
            use crate::review::board::LooksRight;
            use crate::review::report::{
                Outcome, gather, library_folder, mark_reviewed, reports_in,
            };
            use crate::review::run_record::KeptRecord;
            let inputs = three_donor_inputs("review-outcomes");
            let (mut state, id, mut record) = applied(&inputs);
            let folder = folder_of(&inputs);
            let metadata = &inputs.metadata;
            assert_eq!(record.placed.len(), 3, "{:?}", record.skipped);

            // fs_a and fs_b the rule was sure of - this fixture's rule moves
            // every gate far from where it was drawn, which on its own scores
            // them all 0 - and fs_c it was unsure of; the reviewer cleared
            // fs_c's flag.
            for p in &mut record.placed {
                p.confidence = 0.9;
            }
            let c = record
                .placed
                .iter_mut()
                .find(|p| p.sample.id == "fs_c")
                .unwrap();
            c.confidence = 0.1;
            // A placement of a gate the document has since lost.
            let mut lost = c.clone();
            lost.gate_id = "lost gate".into();
            record.placed.push(lost);
            // Two gates the run left alone, one of them later reported.
            let kept = |sample: &str| KeptRecord {
                gate_id: "kept gate".into(),
                gate: "Kept".into(),
                parent_gate: None,
                specimen: sample.into(),
                sample: crate::review::run_record::SampleRef {
                    id: sample.into(),
                    name: None,
                    sample_type: Some("FS".into()),
                },
                met_rule: true,
                achieved: None,
                above_the_line: None,
                line: None,
                shape: None,
                bound: None,
                measured_on: None,
            };
            record.kept = vec![kept("fs_a"), kept("fs_b")];
            record.save(&folder).unwrap();
            LooksRight::set(&folder, &record, "CD134+", "fs_c", true).unwrap();

            // fs_a reported; fs_b moved by hand, unreported; fs_c left.
            let reported = gather(
                &folder,
                &request(&state, &id, "fs_a"),
                &state,
                &inputs,
                &Default::default(),
            )
            .unwrap();
            reported.save(&folder).unwrap();
            let mut about_kept = reported.clone();
            about_kept.id = "kept-report".into();
            about_kept.gate_id = "kept gate".into();
            about_kept.sample.id = "fs_b".into();
            about_kept.save(&folder).unwrap();
            let mut elsewhere = reported.clone();
            elsewhere.id = "elsewhere".into();
            elsewhere.gate_id = "a gate the run never touched".into();
            elsewhere.save(&folder).unwrap();
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, 900.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );

            let library = scratch("review-outcomes-library");
            let (review, copied) =
                mark_reviewed(&folder, &state, metadata, Some(&library)).unwrap();
            let outcome_of = |sample: &str, gate: &str| {
                review
                    .placements
                    .iter()
                    .find(|p| p.placed.sample.id == sample && p.placed.gate_id == gate)
                    .unwrap()
            };
            assert_eq!(
                outcome_of("fs_a", "CD134+").outcome,
                Outcome::Reported {
                    reports: vec![reported.id.clone()]
                }
            );
            let Outcome::MovedUnreported { gate_at, gate } = &outcome_of("fs_b", "CD134+").outcome
            else {
                panic!("{:?}", outcome_of("fs_b", "CD134+").outcome);
            };
            let on_x = gate_at.iter().find(|e| e.parameter == X).unwrap();
            assert_eq!(on_x.lower, Some(900.0));
            // The gate itself is kept too, shape and all, as the reviewer left it.
            let left = gate.as_ref().expect("the gate as moved");
            assert_eq!(
                crate::gate_rules::autogate::extent_on(&left.geometry, X).map(|e| e.0),
                Some(900.0)
            );
            assert_eq!(outcome_of("fs_c", "CD134+").outcome, Outcome::Accepted);
            assert_eq!(outcome_of("fs_c", "lost gate").outcome, Outcome::Gone);

            // The flag the reviewer cleared, and those never raised.
            let c = &outcome_of("fs_c", "CD134+").flag;
            assert_eq!(c.flagged_on, vec!["low_confidence".to_string()]);
            assert!(c.flag_severity.unwrap() >= 3.0);
            assert!(c.looked_right);
            let a = &outcome_of("fs_a", "CD134+").flag;
            assert!(
                a.flagged_on.is_empty() && a.flag_severity.is_none() && !a.looked_right,
                "{a:?}"
            );

            // The gates left alone: accepted, or reported.
            let kept_outcome = |sample: &str| {
                review
                    .kept
                    .iter()
                    .find(|k| k.kept.sample.id == sample)
                    .unwrap()
                    .outcome
                    .clone()
            };
            assert_eq!(kept_outcome("fs_a"), Outcome::Accepted);
            assert_eq!(
                kept_outcome("fs_b"),
                Outcome::Reported {
                    reports: vec!["kept-report".into()]
                }
            );
            // And the report about a gate the run never touched.
            assert_eq!(review.other_reports, vec!["elsewhere".to_string()]);
            assert_eq!(
                (
                    review.accepted(),
                    review.reported(),
                    review.moved_unreported()
                ),
                (1, 2, 1)
            );
            assert_eq!(review.run_applied_at, record.applied_at);
            assert_eq!(review.rules, record.rules);

            // Kept in the workspace, and copied whole into the library.
            let here = folder.join("reviews").join("review.json");
            let text = std::fs::read_to_string(&here).unwrap();
            let back: crate::review::RunReview = serde_json::from_str(&text).unwrap();
            assert_eq!(back, review);
            let into = copied.expect("a library was given");
            assert_eq!(into, library_folder(&library, &folder, &record.applied_at));
            assert_eq!(
                std::fs::read_to_string(into.join("review.json")).unwrap(),
                text
            );
            let mut copied_reports: Vec<String> = std::fs::read_dir(into.join("reports"))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            copied_reports.sort();
            let mut ours: Vec<String> = reports_in(&folder)
                .iter()
                .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            ours.sort();
            assert_eq!(copied_reports, ours);

            // Marked again with no library: replaced here, nothing copied.
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(
                    &id,
                    record
                        .placed
                        .iter()
                        .find(|p| p.sample.id == "fs_b")
                        .unwrap()
                        .to
                        .unwrap() as f32,
                ),
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );
            let (again, copied) = mark_reviewed(&folder, &state, metadata, None).unwrap();
            assert!(copied.is_none());
            assert_eq!(
                outcome_of_in(&again, "fs_b"),
                Outcome::Accepted,
                "moved back: as placed"
            );
            let back: crate::review::RunReview =
                serde_json::from_str(&std::fs::read_to_string(&here).unwrap()).unwrap();
            assert_eq!(back, again);
        }

        #[test]
        fn a_flag_on_a_gate_moved_since_says_so_and_goes_to_the_back() {
            use crate::review::assess::assess;
            use crate::review::run_record::PlacementStatus;
            let inputs = three_donor_inputs("assess-status");
            let (mut state, id, mut record) = applied(&inputs);
            // Both unsure; fs_b the more so - the worse flag.
            for p in &mut record.placed {
                p.confidence = match p.sample.id.as_str() {
                    "fs_a" => 0.2,
                    "fs_b" => 0.05,
                    _ => 0.9,
                };
            }
            let now = (&state, &inputs.metadata);
            let order: Vec<String> = assess(&record, Some(now))
                .flags
                .iter()
                .map(|f| f.sample.id.clone())
                .collect();
            assert_eq!(order, vec!["fs_b", "fs_a"], "worst first");
            assert!(
                assess(&record, Some(now))
                    .flags
                    .iter()
                    .all(|f| f.status == Some(PlacementStatus::AsPlaced))
            );

            // The reviewer moves fs_b: most likely dealt with, so it goes last.
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, 900.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );
            let flags = assess(&record, Some((&state, &inputs.metadata))).flags;
            let order: Vec<&str> = flags.iter().map(|f| f.sample.id.as_str()).collect();
            assert_eq!(order, vec!["fs_a", "fs_b"]);
            assert_eq!(flags[1].status, Some(PlacementStatus::Moved));
            // Without the gates, no status is claimed.
            assert!(
                assess(&record, None)
                    .flags
                    .iter()
                    .all(|f| f.status.is_none())
            );
        }

        #[test]
        fn the_board_of_a_workspace_reads_its_run_reports_and_cleared_flags() {
            use crate::review::board::{LooksRight, Pile, board_in};
            use crate::review::report::gather;
            let inputs = three_donor_inputs("board-in");
            let folder = folder_of(&inputs);
            assert!(
                board_in(&folder, &GateState::default(), &inputs.metadata)
                    .unwrap()
                    .is_none(),
                "no run, no board"
            );
            let (mut state, id, mut record) = applied(&inputs);
            for p in &mut record.placed {
                p.confidence = if p.sample.id == "fs_c" { 0.9 } else { 0.1 };
            }
            record.save(&folder).unwrap();
            let pile_of = |state: &GateState, sample: &str| {
                board_in(&folder, state, &inputs.metadata)
                    .unwrap()
                    .unwrap()
                    .entries
                    .into_iter()
                    .find(|e| e.sample.id == sample)
                    .unwrap()
                    .pile
            };
            assert_eq!(pile_of(&state, "fs_a"), Pile::NeedsALook);
            assert_eq!(pile_of(&state, "fs_b"), Pile::NeedsALook);
            assert_eq!(pile_of(&state, "fs_c"), Pile::Passed);

            // Reported, cleared, moved: each lands in its pile.
            gather(
                &folder,
                &request(&state, &id, "fs_a"),
                &state,
                &inputs,
                &Default::default(),
            )
            .unwrap()
            .save(&folder)
            .unwrap();
            LooksRight::set(&folder, &record, "CD134+", "fs_b", true).unwrap();
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, 900.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_c"))),
            );
            assert_eq!(pile_of(&state, "fs_a"), Pile::Reported);
            assert_eq!(pile_of(&state, "fs_b"), Pile::Passed);
            assert_eq!(pile_of(&state, "fs_c"), Pile::Changed);
            let board = board_in(&folder, &state, &inputs.metadata)
                .unwrap()
                .unwrap();
            let b = board
                .entries
                .iter()
                .find(|e| e.sample.id == "fs_b")
                .unwrap();
            assert!(b.looks_right && b.flag.is_some());
            let a = board
                .entries
                .iter()
                .find(|e| e.sample.id == "fs_a")
                .unwrap();
            assert_eq!(a.reports, 1);
        }

        /// Run, apply and keep the run with its events, as the app and the
        /// tools do; then mark it reviewed, with a library if given.
        fn applied_and_kept(inputs: &RunInputs) -> (GateState, Arc<str>, crate::review::RunRecord) {
            let (mut state, id) = positive_gate();
            let outcome = run_rules(&state, inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &inputs.metadata,
            );
            let samples = crate::review::run_record::Samples::new(
                &inputs.names,
                &inputs.metadata,
                &inputs.rules.pairing,
            );
            let record = crate::review::RunRecord::from_run(
                &outcome.report,
                &outcome.placements,
                &inputs.rules,
                &samples,
            );
            record.applied(&folder_of(inputs), &outcome.events).unwrap();
            (
                state,
                id,
                crate::review::RunRecord::load(&folder_of(inputs))
                    .unwrap()
                    .unwrap(),
            )
        }

        #[test]
        fn a_replay_with_the_run_s_own_rules_puts_the_gates_where_the_run_did() {
            use crate::review::replay::{ReviewedRun, Verdict, replay_run};
            for inputs in [
                three_donor_inputs("replay-fidelity-tail"),
                three_way_inputs("replay-fidelity-negative"),
            ] {
                let (state, _, record) = applied_and_kept(&inputs);
                let run =
                    ReviewedRun::from_workspace(&folder_of(&inputs), &state, &inputs.metadata)
                        .unwrap();
                assert!(run.provisional, "not marked reviewed yet");
                let (summary, cases) = replay_run(&run, &[]);
                assert_eq!(summary.not_replayed, None);
                assert_eq!(
                    cases.len(),
                    record.placed.len() + record.kept.iter().filter(|k| k.met_rule).count()
                );
                let mut reproduced = 0;
                for c in &cases {
                    let base = c.baseline_decided.as_ref().expect("replayed");
                    // With no change tried, the replay is the baseline.
                    assert_eq!(c.replay_decided.as_ref(), Some(base));
                    assert_eq!(base.moved, c.run_decided.moved);
                    assert_eq!(base.measured_on, c.run_decided.measured_on);
                    assert!(c.provisional);
                    // Accepted, as nobody has moved anything: reproduced and
                    // still right, or honestly not reproduced - never judged
                    // fixed or broken by a replay that changed nothing.
                    match c.verdict {
                        Verdict::StillRight => {
                            assert!(c.reproduced);
                            reproduced += 1;
                        }
                        Verdict::NotReproduced => assert!(!c.reproduced),
                        other => panic!("{other:?}: {c:#?}"),
                    }
                }
                assert!(
                    reproduced * 3 >= cases.len() * 2,
                    "{reproduced} of {}",
                    cases.len()
                );
            }
            // The two tail placements away from the band's edge come back
            // exactly: the kept events hold the extremes the search starts from.
            let inputs = three_donor_inputs("replay-fidelity-exact");
            let (state, _, _) = applied_and_kept(&inputs);
            let run =
                ReviewedRun::from_workspace(&folder_of(&inputs), &state, &inputs.metadata).unwrap();
            let (_, cases) = replay_run(&run, &[]);
            for id in ["fs_a", "fs_b"] {
                let c = cases.iter().find(|c| c.sample.id == id).unwrap();
                let (was, now) = (
                    c.run_decided.line.at.unwrap(),
                    c.baseline_decided.as_ref().unwrap().line.at.unwrap(),
                );
                // To the 16-bit storage of the events: a hundred-thousandth
                // of the axis they span.
                assert!((was - now).abs() < 0.01, "{id}: {was} {now}");
            }
        }

        #[test]
        fn a_changed_band_breaks_the_accepted_gates_and_fixes_the_one_moved_by_hand() {
            use crate::gate_rules::rule::TailFractionRule;
            use crate::review::replay::{ReviewedRun, RuleChange, Truth, Verdict, replay_run};
            let inputs = three_donor_inputs("replay-change");
            let (mut state, id, record) = applied_and_kept(&inputs);
            // The reviewer wanted fs_b holding about a quarter: moved by hand.
            let b = record
                .placed
                .iter()
                .find(|p| p.sample.id == "fs_b")
                .unwrap();
            let events = crate::review::events::load(&folder_of(&inputs), &record.applied_at)
                .unwrap()
                .unwrap();
            let mut xs: Vec<f32> = events
                .samples
                .iter()
                .find(|e| e.file == "fs_b")
                .unwrap()
                .points
                .iter()
                .map(|p| p.0)
                .collect();
            xs.sort_by(f32::total_cmp);
            let quarter = xs[(xs.len() as f64 * 0.75) as usize];
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, quarter),
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );
            let run =
                ReviewedRun::from_workspace(&folder_of(&inputs), &state, &inputs.metadata).unwrap();

            let change = RuleChange {
                target: RuleTarget::named("CD134+"),
                rule: GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::Itself,
                    rule: Rule::TailFraction(TailFractionRule::new((0.2, 0.3))),
                },
            };
            let (_, before) = replay_run(&run, &[]);
            let (_, after) = replay_run(&run, &[change]);
            let find = |cases: &[crate::review::replay::Case], id: &str| {
                cases.iter().find(|c| c.sample.id == id).unwrap().clone()
            };
            let moved = find(&after, "fs_b");
            assert!(
                matches!(moved.truth, Truth::MovedByHand),
                "{:?}",
                moved.truth
            );
            assert!(
                (moved.right.beyond.unwrap() - 0.25).abs() < 0.01,
                "{:?}",
                moved.right
            );
            assert_eq!(find(&before, "fs_b").verdict, Verdict::StillWrong);
            assert_eq!(moved.verdict, Verdict::Fixed, "{moved:#?}");
            let now = moved.replay_decided.as_ref().unwrap().line.beyond.unwrap();
            assert!((0.2..=0.3).contains(&now), "{now}");
            // The ones accepted where the old band put them are broken by it.
            for id in ["fs_a", "fs_c"] {
                let c = find(&after, id);
                assert!(matches!(c.truth, Truth::AcceptedAsPlaced));
                assert!(
                    matches!(c.verdict, Verdict::Broken | Verdict::NotReproduced),
                    "{id}: {c:#?}"
                );
            }
            assert!(after.iter().any(|c| c.verdict == Verdict::Broken));
            let _ = b;
        }

        #[test]
        fn a_saved_fix_is_the_right_answer_and_a_report_without_one_only_says_what_was_wrong() {
            use crate::review::replay::{ReviewedRun, Truth, Verdict, replay_run};
            use crate::review::report::{gather, record_corrections};
            let inputs = three_donor_inputs("replay-reports");
            let (mut state, id, record) = applied_and_kept(&inputs);
            let folder = folder_of(&inputs);
            for sample in ["fs_a", "fs_c"] {
                gather(
                    &folder,
                    &request(&state, &id, sample),
                    &state,
                    &inputs,
                    &Default::default(),
                )
                .unwrap()
                .save(&folder)
                .unwrap();
            }
            // fs_a's fix, saved; fs_c's never made.
            let a_to = record
                .placed
                .iter()
                .find(|p| p.sample.id == "fs_a")
                .unwrap()
                .to
                .unwrap();
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, a_to as f32 + 30.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_a"))),
            );
            record_corrections(&folder, &state, &inputs.metadata);
            let run = ReviewedRun::from_workspace(&folder, &state, &inputs.metadata).unwrap();
            let (_, cases) = replay_run(&run, &[]);
            let a = cases.iter().find(|c| c.sample.id == "fs_a").unwrap();
            assert!(matches!(a.truth, Truth::Corrected { .. }), "{:?}", a.truth);
            assert!((a.right.at.unwrap() - (a_to + 30.0)).abs() < 1e-3);
            assert_eq!(
                a.verdict,
                Verdict::StillWrong,
                "the rule still puts it where it did"
            );
            let c = cases.iter().find(|c| c.sample.id == "fs_c").unwrap();
            let Truth::ReportedWithoutFix { problem, note } = &c.truth else {
                panic!("{:?}", c.truth);
            };
            assert_eq!(*problem, crate::review::Problem::TooLoose);
            assert_eq!(note, "lets in the negatives");
            assert_eq!(c.right.at, None);
            assert!(
                matches!(c.verdict, Verdict::Unchanged | Verdict::NotReproduced),
                "{c:#?}"
            );
        }

        #[test]
        fn a_run_without_what_a_replay_needs_says_why_and_judges_nothing() {
            use crate::review::replay::{ReviewedRun, Verdict, replay_run};
            let inputs = three_donor_inputs("replay-not-possible");
            let (state, _, record) = applied_and_kept(&inputs);
            let folder = folder_of(&inputs);
            // No events at all.
            std::fs::remove_file(crate::review::events::file_in(&folder)).unwrap();
            let run = ReviewedRun::from_workspace(&folder, &state, &inputs.metadata).unwrap();
            let (summary, cases) = replay_run(&run, &[]);
            assert!(
                summary
                    .not_replayed
                    .as_deref()
                    .unwrap()
                    .contains("no events")
            );
            assert!(cases.iter().all(|c| c.verdict == Verdict::NotReplayed));
            assert!(cases.iter().all(|c| c.why.is_some()));
            // Events from before gates and metadata were kept.
            let mut old = crate::review::events::of_run(&[], &Default::default());
            old.run_applied_at = record.applied_at.clone();
            crate::review::events::save(&folder, &old).unwrap();
            let run = ReviewedRun::from_workspace(&folder, &state, &inputs.metadata).unwrap();
            let (summary, _) = replay_run(&run, &[]);
            assert!(
                summary
                    .not_replayed
                    .as_deref()
                    .unwrap()
                    .contains("run the rules again")
            );
        }

        #[test]
        fn a_run_in_the_library_replays_as_it_did_in_its_workspace() {
            use crate::review::replay::{ReviewedRun, library_runs, replay};
            let inputs = three_donor_inputs("replay-library");
            let (state, _, _) = applied_and_kept(&inputs);
            let folder = folder_of(&inputs);
            let library = scratch("replay-library-lib");
            crate::review::report::mark_reviewed(&folder, &state, &inputs.metadata, Some(&library))
                .unwrap();
            let here = ReviewedRun::from_workspace(&folder, &state, &inputs.metadata).unwrap();
            assert!(!here.provisional, "marked reviewed");
            let (runs, problems) = library_runs(&library);
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(runs.len(), 1);
            let from_library = replay(&runs, &[]);
            let from_workspace = replay(std::slice::from_ref(&here), &[]);
            let strip = |r: &crate::review::replay::Replay| {
                r.cases
                    .iter()
                    .map(|c| (c.sample.id.clone(), c.verdict, c.replay_decided.clone()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(strip(&from_library), strip(&from_workspace));
            let total: usize = from_library
                .by_gate()
                .values()
                .flat_map(|v| v.values())
                .sum();
            assert_eq!(total, from_library.cases.len());
            // A folder that is not a library says so.
            let (runs, problems) = library_runs(&library.join("nothing here"));
            assert!(runs.is_empty() && problems.len() == 1);
        }

        fn outcome_of_in(
            review: &crate::review::RunReview,
            sample: &str,
        ) -> crate::review::report::Outcome {
            review
                .placements
                .iter()
                .find(|p| p.placed.sample.id == sample && p.placed.gate_id == "CD134+")
                .unwrap()
                .outcome
                .clone()
        }

        #[test]
        fn a_workspace_with_no_run_has_nothing_to_mark_reviewed() {
            let folder = scratch("review-no-run");
            let e = crate::review::report::mark_reviewed(
                &folder,
                &GateState::default(),
                &Default::default(),
                None,
            )
            .unwrap_err();
            assert!(e.contains("no rules run"), "{e}");
            assert!(!folder.join("reviews").join("review.json").exists());
        }

        #[test]
        fn a_correction_follows_the_gate_to_where_the_reviewer_left_it() {
            use crate::review::report::{gather, record_corrections, reports_in};
            let inputs = three_way_inputs("report-corrections");
            let (mut state, id, record) = applied(&inputs);
            let folder = folder_of(&inputs);
            let metadata = &inputs.metadata;
            for sample in ["fs_b", "fs_qc"] {
                gather(
                    &folder,
                    &request(&state, &id, sample),
                    &state,
                    &inputs,
                    &Default::default(),
                )
                .unwrap()
                .save(&folder)
                .unwrap();
            }
            // A report about a gate the document has since lost.
            let mut lost = reports_in(&folder)[0].1.clone();
            lost.id = "lost".into();
            lost.gate_id = "no such gate".into();
            lost.save(&folder).unwrap();
            let correction_of = |sample: &str| {
                reports_in(&folder)
                    .into_iter()
                    .map(|(_, r)| r)
                    .find(|r| r.sample.id == sample && r.gate_id == "CD134+")
                    .unwrap()
                    .correction
            };
            let edge_of = |c: &crate::review::report::Correction| {
                c.gate_at
                    .iter()
                    .find(|e| e.parameter == X)
                    .unwrap()
                    .lower
                    .unwrap()
            };

            // Nothing moved: nothing to record.
            assert_eq!(record_corrections(&folder, &state, metadata), 0);
            assert!(correction_of("fs_b").is_none() && correction_of("fs_qc").is_none());

            // The donor's gate moved by hand: its correction.
            let to = record.placed[0].to.unwrap() as f32;
            let place = |state: &mut GateState, sample: &str, edge: f32| {
                state.place_gate(
                    &[id.clone()],
                    &gate_with_edge(&id, edge),
                    &GateSource::Sample((id.clone(), Arc::from(sample))),
                );
            };
            place(&mut state, "fs_b", to + 40.0);
            assert_eq!(record_corrections(&folder, &state, metadata), 1);
            assert!((edge_of(&correction_of("fs_b").unwrap()) - (to as f64 + 40.0)).abs() < 1e-3);
            // Saved again unchanged: nothing new.
            assert_eq!(record_corrections(&folder, &state, metadata), 0);
            // Moved again: the correction follows.
            place(&mut state, "fs_b", to + 60.0);
            assert_eq!(record_corrections(&folder, &state, metadata), 1);
            assert!((edge_of(&correction_of("fs_b").unwrap()) - (to as f64 + 60.0)).abs() < 1e-3);
            // Put back where the rule had it: no correction after all.
            place(&mut state, "fs_b", to);
            assert_eq!(record_corrections(&folder, &state, metadata), 1);
            assert!(correction_of("fs_b").is_none());

            // The reference the rule did not move: moved since the report is a
            // correction.
            place(&mut state, "fs_qc", 520.0);
            assert_eq!(record_corrections(&folder, &state, metadata), 1);
            assert_eq!(edge_of(&correction_of("fs_qc").unwrap()), 520.0);
            // The lost gate's report is left alone throughout.
            let lost = reports_in(&folder)
                .into_iter()
                .find(|(_, r)| r.id == "lost")
                .unwrap()
                .1;
            assert!(lost.correction.is_none());
        }

        #[test]
        fn a_run_record_holds_each_placement_reference_and_skip_as_the_run_saw_it() {
            use crate::review::run_record::{
                PlacementStatus, RunRecord, Samples, placement_status,
            };
            let (mut state, id, outcome, metadata, rules) = three_way_run("record-three-way");
            let names = named(&[
                ("fs_qc.fcs", "fs_qc"),
                ("fs_b.fcs", "fs_b"),
                ("fs_x.fcs", "fs_x"),
            ]);
            let samples = Samples::new(&names, &metadata, &rules.pairing);
            let record =
                RunRecord::from_run(&outcome.report, &outcome.placements, &rules, &samples);

            // The donor, moved; its line of the report and its record agree
            // field for field.
            assert_eq!(record.placed.len(), 1, "{record:#?}");
            let line = &outcome.report.positioned[0];
            let placed = &record.placed[0];
            assert_eq!(placed.gate_id, "CD134+");
            assert_eq!(placed.gate, "CD134+");
            assert_eq!(placed.parent_gate, None);
            assert_eq!(placed.specimen_column, "SampleID");
            assert_eq!(placed.specimen, "DONOR-B");
            assert_eq!(placed.sample.id, "fs_b");
            assert_eq!(placed.sample.name.as_deref(), Some("fs_b.fcs"));
            assert_eq!(placed.sample.sample_type.as_deref(), Some("FS"));
            assert_eq!(placed.measured_on.id, "fs_qc");
            assert_eq!(placed.from, Some(line.from));
            assert_eq!(placed.to, Some(line.to));
            assert_eq!(placed.confidence, line.confidence);
            assert_eq!(placed.weakest.as_deref(), line.weakest);
            assert_eq!(placed.components.len(), line.components.len());
            for (c, l) in placed.components.iter().zip(&line.components) {
                assert_eq!((c.name.as_str(), c.score), (l.name, l.score));
            }
            assert_eq!(placed.reference_events, line.reference_events);
            assert_eq!(placed.in_band, line.in_band);
            // An above-the-negative rule: the two negatives it read, no valley
            // and no phenotype.
            let (reference, here) = placed.negative.as_ref().expect("the negatives it read");
            let (line_ref, line_here) = line.negative.as_ref().unwrap();
            assert_eq!(
                (reference.centre, here.centre),
                (line_ref.centre, line_here.centre)
            );
            assert!(
                here.centre > reference.centre + 200.0,
                "the donor's negative drifted up"
            );
            assert!(placed.valley.is_none() && placed.phenotype.is_none());
            // The donor's parent population, summarised on the rule's axis.
            let shape = placed
                .shape
                .as_ref()
                .expect("a summary of what the rule read");
            assert_eq!(shape.events, 10_000);
            assert!((shape.median() - 600.0).abs() < 20.0, "{}", shape.median());
            assert_eq!(placed.bound, Some(Bound::Above));
            // Where the placed gate sits: its left edge is the line.
            let on_x = placed.placed_at.iter().find(|e| e.parameter == X).unwrap();
            assert!(
                (on_x.lower.unwrap() - line.to).abs() < 1e-3,
                "{on_x:?} vs {}",
                line.to
            );

            // The QC is the reference: kept, not judged, with its line where a
            // person drew it.
            let reference = record
                .kept
                .iter()
                .find(|k| k.sample.id == "fs_qc")
                .expect("the reference is kept");
            assert!(!reference.met_rule);
            assert_eq!(reference.line, Some(500.0));
            assert_eq!(reference.shape.as_ref().map(|s| s.events), Some(10_000));
            assert_eq!(reference.specimen, "QC-A");

            // The file with no specimen is skipped, named, and says why.
            let skipped = record
                .skipped
                .iter()
                .find(|s| s.sample.id == "fs_x")
                .expect("fs_x is skipped");
            assert_eq!(skipped.sample.name.as_deref(), Some("fs_x.fcs"));
            assert!(skipped.reason.contains("SampleID"), "{}", skipped.reason);

            // Kept on disk and read back exactly.
            let folder = scratch("record-three-way-saved");
            record.save(&folder).unwrap();
            assert_eq!(RunRecord::load(&folder).unwrap().as_ref(), Some(&record));

            // Applied, every placement is as placed.
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &metadata,
            );
            assert_eq!(
                placement_status(placed, &state, &metadata),
                PlacementStatus::AsPlaced
            );
            let _ = id;
        }

        #[test]
        fn a_gate_that_already_meets_its_rule_is_kept_as_meeting_it() {
            use crate::gate_rules::rule::TailFractionRule;
            use crate::review::run_record::{RunRecord, Samples};
            let (state, _id) = positive_gate();
            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::Itself,
                    // The gate at 500 already admits the 10% positive.
                    rule: Rule::TailFraction(TailFractionRule::new((0.05, 0.15))),
                },
            );
            let files = workspace("record-kept");
            let names = named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]);
            let outcome = run_rules(
                &state,
                &RunInputs {
                    files: files[..1].to_vec(),
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: names.clone(),
                    cofactors: Vec::new(),
                    metadata: specimens(),
                    rules: store.clone(),
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            );
            let metadata = specimens();
            let samples = Samples::new(&names, &metadata, &store.pairing);
            let record =
                RunRecord::from_run(&outcome.report, &outcome.placements, &store, &samples);
            assert!(record.placed.is_empty(), "{record:#?}");
            assert_eq!(record.kept.len(), 1);
            let kept = &record.kept[0];
            assert!(kept.met_rule);
            assert_eq!(kept.sample.id, "fs_qc");
            assert_eq!(kept.line, Some(500.0));
            let achieved = kept.achieved.unwrap();
            assert!((0.05..=0.15).contains(&achieved), "{achieved}");
            assert_eq!(kept.bound, Some(Bound::Above));
            assert_eq!(kept.shape.as_ref().map(|s| s.events), Some(10_000));
        }

        #[test]
        fn measuring_several_rule_sets_in_one_read_gives_what_each_gives_alone() {
            use crate::gate_rules::rule::TailFractionRule;
            let inputs = three_way_inputs("run-measure-many");
            let (state, _) = positive_gate();
            let mut band = inputs.rules.clone();
            let mut rule = band.entries()[0].rule.clone();
            rule.rule = Rule::TailFraction(TailFractionRule::new((0.002, 0.005)));
            band.insert(RuleTarget::named("CD134+"), rule);
            let none = RuleStore::default();
            let stores = [inputs.rules.clone(), band.clone(), none];
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let (many, problems) = measure_many(
                &state,
                &inputs.files,
                &inputs.compensation,
                &inputs.names,
                &inputs.cofactors,
                &inputs.metadata,
                &stores,
                None,
                &AtomicBool::new(false),
                |_, _| {
                    calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                },
            );
            // Each file read once, for all three.
            assert_eq!(calls.into_inner(), inputs.files.len());
            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(many.len(), 3);
            for (store, (measured, _)) in stores.iter().zip(&many) {
                let (alone, _, _) = measure_all(
                    &state,
                    &inputs.files,
                    &inputs.compensation,
                    &inputs.names,
                    &inputs.cofactors,
                    &inputs.metadata,
                    store,
                    None,
                    &AtomicBool::new(false),
                    |_, _| {},
                );
                let key = |m: &crate::gate_rules::autogate::Measurement| {
                    (
                        m.file.to_string(),
                        m.gate_id.to_string(),
                        m.events,
                        m.kept_events.len(),
                    )
                };
                assert_eq!(
                    measured.iter().map(key).collect::<Vec<_>>(),
                    alone.iter().map(key).collect::<Vec<_>>()
                );
            }
            // No rules, nothing measured; the band keeps more events.
            assert!(many[2].0.is_empty());
            assert!(many[1].0[0].kept_events.len() > many[0].0[0].kept_events.len());
        }

        #[test]
        fn stop_is_heard_while_the_gates_are_being_positioned() {
            let inputs = three_way_inputs("run-stop-solving");
            let (state, _) = positive_gate();
            let flag = Arc::new(AtomicBool::new(false));
            let seen = std::sync::Mutex::new(Vec::new());
            // Stop pressed once every file has been read.
            let outcome = run_rules(
                &state,
                &inputs,
                |step| {
                    seen.lock().unwrap().push(step);
                    if matches!(step, Progress::Solving { .. }) {
                        flag.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                },
                &flag,
            );
            assert!(outcome.cancelled);
            assert!(outcome.placements.is_empty());
            assert!(outcome.report.positioned.is_empty());
            let seen = seen.into_inner().unwrap();
            // Every file was read; positioning stopped at the first gate.
            assert_eq!(
                seen.iter()
                    .filter(|s| matches!(s, Progress::Measuring { .. }))
                    .count(),
                3
            );
            assert!(
                seen.iter()
                    .filter(|s| matches!(s, Progress::Solving { done, .. } if *done > 1))
                    .count()
                    == 0,
                "{seen:?}"
            );

            // Unstopped, the same run places the donor.
            let flag = AtomicBool::new(false);
            let outcome = run_rules(&state, &inputs, |_| {}, &flag);
            assert!(!outcome.cancelled);
            assert_eq!(outcome.placements.len(), 1);
        }

        #[test]
        fn each_step_fills_the_bar_from_empty_and_says_which_step_it_is() {
            let reading = |done| Progress::Measuring { done, total: 4 };
            let placing = |done| Progress::Solving { done, total: 1_500 };
            assert_eq!(reading(0).fraction(), 0.0);
            assert_eq!(reading(2).fraction(), 0.5);
            assert_eq!(reading(4).fraction(), 1.0);
            assert_eq!(placing(0).fraction(), 0.0);
            assert_eq!(placing(750).fraction(), 0.5);
            assert_eq!(placing(1_500).fraction(), 1.0);
            assert_eq!(Progress::Solving { done: 0, total: 0 }.fraction(), 0.0);
            assert_eq!(reading(2).describe(), "Step 1 of 2 - reading file 2 of 4");
            assert_eq!(
                placing(750).describe(),
                "Step 2 of 2 - positioning gate 750 of 1500"
            );
            assert_eq!(
                Progress::Solving { done: 0, total: 0 }.describe(),
                "Step 2 of 2 - positioning the gates..."
            );
        }

        #[test]
        fn a_band_rule_keeps_enough_events_for_its_tail() {
            use crate::gate_rules::rule::TailFractionRule;
            let mut inputs = three_way_inputs("run-events-tail");
            let mut rule = inputs.rules.entries()[0].rule.clone();
            // 0.2% to 0.5% wants 50,000 events; each population has 10,000,
            // so every one of them is kept.
            rule.rule = Rule::TailFraction(TailFractionRule::new((0.002, 0.005)));
            inputs.rules.insert(RuleTarget::named("CD134+"), rule);
            let (state, _) = positive_gate();
            let outcome = run_rules(&state, &inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            assert!(!outcome.events.samples.is_empty());
            for e in &outcome.events.samples {
                assert_eq!((e.events, e.points.len()), (10_000, 10_000), "{}", e.file);
            }
        }

        #[test]
        fn a_run_keeps_the_events_behind_every_gate_on_every_file_it_measured() {
            use crate::review::events::{KEPT_EVENTS, load};
            let inputs = three_way_inputs("run-events");
            let (state, _) = positive_gate();
            let outcome = run_rules(&state, &inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            // The donor it placed, the QC it read, and the file it could not
            // place - measured all the same.
            let mut files: Vec<&str> = outcome
                .events
                .samples
                .iter()
                .map(|e| e.file.as_str())
                .collect();
            files.sort();
            assert_eq!(files, vec!["fs_b", "fs_qc", "fs_x"]);
            for e in &outcome.events.samples {
                assert_eq!(
                    (e.gate_id.as_str(), e.parent_gate.as_deref()),
                    ("CD134+", None)
                );
                assert_eq!((e.x.as_str(), e.y.as_str()), (X, Y));
                assert_eq!(e.events, 10_000);
                // The even sample and the extremes, less any extreme that
                // was on the even step already.
                assert!(
                    (KEPT_EVENTS - 4..=KEPT_EVENTS).contains(&e.points.len()),
                    "{}",
                    e.points.len()
                );
            }
            // The events are the file's: the donor's negative sits at 600,
            // the QC's at 300.
            let median_x = |file: &str| {
                let e = outcome
                    .events
                    .samples
                    .iter()
                    .find(|e| e.file == file)
                    .unwrap();
                let mut xs: Vec<f32> = e.points.iter().map(|p| p.0).collect();
                xs.sort_by(f32::total_cmp);
                xs[xs.len() / 2]
            };
            assert!(
                (median_x("fs_b") - 600.0).abs() < 30.0,
                "{}",
                median_x("fs_b")
            );
            assert!(
                (median_x("fs_qc") - 300.0).abs() < 30.0,
                "{}",
                median_x("fs_qc")
            );

            // Kept with the run on apply, for that run only.
            let folder = folder_of(&inputs);
            let samples = crate::review::run_record::Samples::new(
                &inputs.names,
                &inputs.metadata,
                &inputs.rules.pairing,
            );
            let record = crate::review::RunRecord::from_run(
                &outcome.report,
                &outcome.placements,
                &inputs.rules,
                &samples,
            );
            record.clone().applied(&folder, &outcome.events).unwrap();
            let kept = crate::review::RunRecord::load(&folder).unwrap().unwrap();
            let back = load(&folder, &kept.applied_at)
                .unwrap()
                .expect("kept with the run");
            assert_eq!(back.samples.len(), 3);
            // With each file's metadata, and the gate as it stood before.
            assert_eq!(back.metadata["fs_b"]["SampleID"], "DONOR-B");
            assert_eq!(back.metadata.len(), 3);
            for e in &back.samples {
                let gate = e.gate.as_ref().expect("the gate before the run");
                assert_eq!(gate.name, "CD134+");
                let (lo, _) = crate::gate_rules::autogate::extent_on(&gate.geometry, X).unwrap();
                assert_eq!(lo, 500.0, "where it was drawn, not where the run put it");
            }
            assert_eq!(load(&folder, "some other run").unwrap(), None);
            // A cancelled run keeps nothing.
            let cancelled = run_rules(&state, &inputs, |_| {}, &Arc::new(AtomicBool::new(true)));
            assert!(cancelled.cancelled && cancelled.events.samples.is_empty());
        }

        #[test]
        fn a_gate_left_alone_records_the_reference_the_rule_read() {
            use crate::gate_rules::rule::TailFractionRule;
            use crate::review::run_record::{RunRecord, Samples};
            let (state, _id) = positive_gate();
            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    // What the QC admits is already in the band, so every
                    // specimen's gate is left where it is.
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::TailFraction(TailFractionRule::new((0.05, 0.15))),
                },
            );
            let inputs = RunInputs {
                rules: store,
                ..three_way_inputs("record-kept-read")
            };
            let outcome = run_rules(&state, &inputs, |_| {}, &Arc::new(AtomicBool::new(false)));
            let samples = Samples::new(&inputs.names, &inputs.metadata, &inputs.rules.pairing);
            let record = RunRecord::from_run(
                &outcome.report,
                &outcome.placements,
                &inputs.rules,
                &samples,
            );
            let donor = record
                .kept
                .iter()
                .find(|k| k.sample.id == "fs_b")
                .expect("kept");
            assert!(donor.met_rule);
            let read = donor.measured_on.as_ref().expect("the file it read");
            assert_eq!(read.id, "fs_qc");
            assert_eq!(read.name.as_deref(), Some("fs_qc.fcs"));
            // The QC itself is the reference: it read nothing else.
            let qc = record.kept.iter().find(|k| k.sample.id == "fs_qc").unwrap();
            assert!(!qc.met_rule);
            assert_eq!(qc.measured_on, None);
        }

        #[test]
        fn a_placement_reads_as_moved_only_when_the_gate_on_its_sample_has_moved() {
            use crate::review::run_record::{
                PlacementStatus, RunRecord, Samples, placement_status,
            };
            let (mut state, id, outcome, metadata, rules) = three_way_run("record-status");
            let names = named(&[
                ("fs_qc.fcs", "fs_qc"),
                ("fs_b.fcs", "fs_b"),
                ("fs_x.fcs", "fs_x"),
            ]);
            let samples = Samples::new(&names, &metadata, &rules.pairing);
            let record =
                RunRecord::from_run(&outcome.report, &outcome.placements, &rules, &samples);
            let placed = record.placed[0].clone();
            let status = |state: &GateState, p: &crate::review::run_record::PlacedRecord| {
                placement_status(p, state, &metadata)
            };

            // Before the run's placements are applied, the gate is where it was.
            assert_eq!(status(&state, &placed), PlacementStatus::Moved);
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &metadata,
            );
            assert_eq!(status(&state, &placed), PlacementStatus::AsPlaced);

            // Rounding of the kind a save through Omiq's format does is not a move.
            let mut rounded = placed.clone();
            for e in &mut rounded.placed_at {
                e.lower = e.lower.map(|v| v * (1.0 + 1e-6));
            }
            assert_eq!(status(&state, &rounded), PlacementStatus::AsPlaced);
            // A real difference is.
            let mut off = placed.clone();
            for e in &mut off.placed_at {
                e.lower = e.lower.map(|v| v + 5.0);
            }
            assert_eq!(status(&state, &off), PlacementStatus::Moved);
            // Its parameters listed the other way round are the same gate.
            let mut swapped = placed.clone();
            swapped.placed_at.reverse();
            assert_eq!(status(&state, &swapped), PlacementStatus::AsPlaced);

            // A person moves the gate on another sample: this one is untouched.
            let to = placed.to.unwrap() as f32;
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, to + 40.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_qc"))),
            );
            assert_eq!(status(&state, &placed), PlacementStatus::AsPlaced);
            // ...and on this one: moved.
            state.place_gate(
                &[id.clone()],
                &gate_with_edge(&id, to + 40.0),
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );
            assert_eq!(status(&state, &placed), PlacementStatus::Moved);

            // A gate the document no longer has is gone.
            let mut other = placed.clone();
            other.gate_id = "no such gate".into();
            assert_eq!(status(&state, &other), PlacementStatus::Gone);
        }

        fn left_edge_in(
            state: &GateState,
            gate: &Arc<str>,
            file: &str,
            metadata: &crate::omiq::metadata::MetaDataFileMap,
        ) -> f32 {
            let g = state
                .gate_for_file(gate, &Arc::from(file), metadata)
                .unwrap();
            crate::gate_rules::autogate::extent_on(&g.get_gate_ref(None).unwrap().geometry, X)
                .unwrap()
                .0
        }

        /// BUG (docs/test-audit.md, B-AUTO-1): the default finder,
        /// `BelowTheGate`, refines from where the gate already sits on the
        /// sample - the reference's position. The donor's negative has
        /// drifted from 300 to 600, past that position, so the gate sees only
        /// the bottom of it, reads it low, and settles at 584: inside the
        /// negative, admitting 69% of the sample where the reference admits
        /// 10%. And it is scored 0.87, so a run ranks it among the
        /// placements least in need of review.
        #[test]
        #[ignore = "known bug B-AUTO-1: a negative that drifts past the gate puts it inside the negative, confidently"]
        fn the_gate_follows_the_donor_s_negative_and_leaves_the_qc_alone() {
            let (mut state, id) = positive_gate();
            let outcome = run(&state, workspace("run-follow"), false);
            assert!(!outcome.cancelled);
            assert!(
                outcome.report.skipped.iter().all(|s| &*s.file != "fs_b"),
                "{:?}",
                outcome
                    .report
                    .skipped
                    .iter()
                    .map(|s| &s.reason)
                    .collect::<Vec<_>>()
            );

            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &specimens(),
            );
            // The negative moved 300 up, so the gate should too - to within
            // what reading a noisy negative allows.
            let moved = left_edge(&state, &id, "fs_b");
            assert!(
                (moved - 800.0).abs() < 20.0,
                "the donor's gate is at {moved}"
            );
            assert!((left_edge(&state, &id, "fs_qc") - 500.0).abs() < 20.0);
        }

        /// The same run with the density finder, which ignores where the
        /// gate starts: it follows the drift.
        #[test]
        fn the_density_finder_follows_a_negative_that_drifted_past_the_gate() {
            use crate::gate_rules::rule::NegativeFinder;
            let (mut state, id) = positive_gate();
            let mut store = rules();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            let outcome = run_rules(
                &state.clone(),
                &RunInputs {
                    files: workspace("run-density"),
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                    cofactors: Vec::new(),
                    metadata: specimens(),
                    rules: store,
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            );
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &specimens(),
            );
            let moved = left_edge(&state, &id, "fs_b");
            assert!(
                (moved - 800.0).abs() < 30.0,
                "the donor's gate is at {moved}"
            );
        }

        /// Was B-GRP-2. A run positions a specimen: one position for every
        /// file of it, in the group tier. A file with an older position of its
        /// own - a per-file filter from the imported document, or a gate
        /// dragged on that one sample - used to resolve through the sample
        /// tier first, so the run's position never reached it while the
        /// report listed it as positioned. The newer position now applies.
        #[test]
        fn a_file_the_run_reports_positioned_is_drawn_where_it_was_put() {
            use crate::gate_rules::rule::NegativeFinder;
            let (mut state, id) = positive_gate();
            let global = state.registered_gate(&id).unwrap();
            let own = crate::gate_rules::autogate::translate_edge_to(
                &global,
                X,
                crate::gate_rules::rule_store::Bound::Above,
                450.0,
            )
            .unwrap();
            state.place_gate(
                &[id.clone()],
                &own,
                &GateSource::Sample((id.clone(), Arc::from("fs_b"))),
            );
            assert!(
                (left_edge(&state, &id, "fs_b") - 450.0).abs() < 1e-3,
                "the premise: fs_b has its own position"
            );

            let mut store = rules();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            let outcome = run_rules(
                &state.clone(),
                &RunInputs {
                    files: workspace("run-own-position"),
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                    cofactors: Vec::new(),
                    metadata: specimens(),
                    rules: store,
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            );
            let claimed = outcome
                .report
                .positioned
                .iter()
                .find(|p| &*p.file == "fs_b")
                .map(|p| p.to);
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &specimens(),
            );
            let shown = left_edge(&state, &id, "fs_b");
            if let Some(to) = claimed {
                assert!(
                    (f64::from(shown) - to).abs() < 1e-3,
                    "the report has fs_b positioned at {to}; it is drawn at {shown}"
                );
            }
        }

        #[test]
        fn an_unreadable_file_is_reported_and_the_rest_still_run() {
            let (state, _) = positive_gate();
            let mut files = workspace("run-bad");
            let bad = files[1].1.with_file_name("broken.fcs");
            std::fs::write(&bad, b"not an fcs file").unwrap();
            files[1].1 = bad;

            let outcome = run(&state, files, false);
            let said: Vec<&str> = outcome.report.skipped.iter().map(|s| &*s.reason).collect();
            assert!(
                said.iter().any(|r| r.contains("broken.fcs")),
                "the broken file should be reported by name: {said:?}"
            );
            assert!(
                outcome.report.positioned.iter().all(|p| &*p.file != "fs_b"),
                "nothing is placed for a file that could not be read"
            );
        }

        /// A marker on arcsinh: the events are transformed with the axis
        /// store's cofactor before they are measured, so the gate - drawn and
        /// stored in that transformed space - is positioned in it too.
        #[test]
        fn a_run_measures_in_the_arcsinh_space_the_gate_is_drawn_in() {
            use crate::gate_rules::rule::NegativeFinder;
            use flow_fcs::Transformable;
            const M: &str = "BV421-A";
            let t = flow_fcs::TransformType::Arcsinh { cofactor: 150.0 };
            let shown = |raw: f32| t.transform(&raw);

            // Raw negatives at 1,000 on the QC and 3,000 on the donor; the
            // same fraction of positives far above both.
            let rows = |centre: f32, seed: u64| -> Vec<Vec<f32>> {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                let neg = Normal::new(centre, centre * 0.1).unwrap();
                let pos = Uniform::new(60_000.0f32, 90_000.0).unwrap();
                let mut out: Vec<Vec<f32>> = (0..9_000)
                    .map(|_| vec![neg.sample(&mut rng), 100.0])
                    .collect();
                out.extend((0..1_000).map(|_| vec![pos.sample(&mut rng), 100.0]));
                out
            };
            let dir = scratch("run-arcsinh");
            let channels = [(M, Some("CD3")), (Y, None)];
            write_fcs_rows(&dir.join("fs_qc.fcs"), &channels, &rows(1_000.0, 3), &[]);
            write_fcs_rows(&dir.join("fs_b.fcs"), &channels, &rows(3_000.0, 4), &[]);
            let files = vec![
                (Arc::from("fs_qc.fcs"), dir.join("fs_qc.fcs")),
                (Arc::from("fs_b.fcs"), dir.join("fs_b.fcs")),
            ];

            // The gate's edge a little above the QC's negative, in display
            // units - where a person would have drawn it on that plot.
            let edge = shown(2_500.0);
            let mut state = GateState::default();
            let id: Arc<str> = Arc::from("CD3+");
            let gate: Arc<dyn DrawableGate> = Arc::new(
                crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
                    flow_gates::Gate {
                        id: id.clone(),
                        name: "CD3+".into(),
                        geometry: flow_gates::create_rectangle_geometry(
                            vec![(edge, -1e16), (1e16, -1e16), (1e16, 1e16), (edge, 1e16)],
                            M,
                            Y,
                        )
                        .unwrap(),
                        mode: flow_gates::GateMode::Global,
                        parameters: (Arc::from(M), Arc::from(Y)),
                        label_position: None,
                    },
                    true,
                )
                .unwrap(),
            );
            state.place_gate(&[id.clone()], &gate, &GateSource::Global);
            state.place_new_gate(None, id.clone()).unwrap();

            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD3+"),
                GateRule {
                    parameter: Arc::from(M),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            let outcome = run_rules(
                &state.clone(),
                &RunInputs {
                    files: files,
                    compensation: crate::compensation::groups::Compensation::default(),
                    names: named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                    cofactors: vec![(Arc::from(M), 150.0)],
                    metadata: specimens(),
                    rules: store,
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            );
            crate::gate_rules::autogate::apply_placements(
                &mut state,
                &outcome.placements,
                &specimens(),
            );

            let placed = state
                .gate_for_file(&id, &Arc::from("fs_b"), &specimens())
                .unwrap();
            let at = crate::gate_rules::autogate::extent_on(
                &placed.get_gate_ref(None).unwrap().geometry,
                M,
            )
            .unwrap()
            .0;
            // In display space the donor's negative sits shown(3000) -
            // shown(1000) higher, so the gate should have moved by about as
            // much, and still be a display coordinate - single digits, not
            // thousands.
            let expected = edge + (shown(3_000.0) - shown(1_000.0));
            assert!(at < 10.0, "the gate was placed in raw units: {at}");
            assert!(
                (at - expected).abs() < 0.3,
                "the gate is at {at}, expected about {expected}"
            );
        }

        /// Was B-FCS-2: a file whose data could not be read tripped an
        /// assertion inside flow_fcs, and files are read in parallel, so that
        /// panic took the whole run with it - the donor's gate was never
        /// placed because of a different file. flow_fcs now refuses such a
        /// file with an error: it is reported, and the rest still run.
        #[test]
        fn a_file_with_a_damaged_data_offset_is_reported_and_the_rest_still_run() {
            let (state, _) = positive_gate();
            let mut files = workspace("run-damaged");
            let damaged = files[0].1.with_file_name("damaged.fcs");
            crate::file_load_tests::with_data_offsets_damaged(&damaged);
            files.push((Arc::from("damaged.fcs"), damaged));

            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut names = named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]);
                names.insert(Arc::from("damaged.fcs"), Arc::from("damaged"));
                run_rules(
                    &state.clone(),
                    &RunInputs {
                        files: files,
                        compensation: crate::compensation::groups::Compensation::default(),
                        names: names,
                        cofactors: Vec::new(),
                        metadata: specimens(),
                        rules: rules(),
                    },
                    |_| {},
                    &Arc::new(AtomicBool::new(false)),
                )
            }));
            let outcome = outcome.expect("the run finished");
            assert!(
                outcome.report.positioned.iter().any(|p| &*p.file == "fs_b"),
                "the donor was still placed"
            );
            assert!(
                outcome
                    .report
                    .skipped
                    .iter()
                    .any(|s| s.reason.contains("damaged.fcs")),
                "and the damaged file reported: {:?}",
                outcome
                    .report
                    .skipped
                    .iter()
                    .map(|s| &s.reason)
                    .collect::<Vec<_>>()
            );
        }

        /// The donor's events as its cytometer recorded them: its SSC spilling
        /// into FSC at twice its value, so every event reads 200 higher on X
        /// until compensated.
        fn spilt_workspace(name: &str) -> Vec<(Arc<str>, PathBuf)> {
            let files = workspace(name);
            let spilt: Vec<Vec<f32>> = population(600.0, 2)
                .into_iter()
                .map(|e| vec![e[0] + 2.0 * e[1], e[1]])
                .collect();
            write_fcs_rows(&files[1].1, &[(X, None), (Y, None)], &spilt, &[]);
            files
        }

        fn peak_rules() -> RuleStore {
            use crate::gate_rules::rule::NegativeFinder;
            let mut store = RuleStore::default();
            store.insert(
                RuleTarget::named("CD134+"),
                GateRule {
                    parameter: Arc::from(X),
                    bound: Bound::Above,
                    measured_on: MeasuredOn::File(Arc::from("fs_qc")),
                    rule: Rule::AboveTheNegative(AboveTheNegativeRule {
                        find: NegativeFinder::NegativePeak,
                        ..AboveTheNegativeRule::default()
                    }),
                },
            );
            store
        }

        fn run_with(
            state: &GateState,
            files: Vec<(Arc<str>, PathBuf)>,
            compensation: crate::compensation::groups::Compensation,
        ) -> RunOutcome {
            run_rules(
                &state.clone(),
                &RunInputs {
                    files: files,
                    compensation: compensation,
                    names: named(&[("fs_qc.fcs", "fs_qc"), ("fs_b.fcs", "fs_b")]),
                    cofactors: Vec::new(),
                    metadata: specimens(),
                    rules: peak_rules(),
                },
                |_| {},
                &Arc::new(AtomicBool::new(false)),
            )
        }

        /// A run measures the events as compensated. The donor's negative is
        /// really at 600, which puts its gate at 800; read uncompensated it
        /// sits at 800, and the gate at 1000.
        #[test]
        fn a_run_places_gates_on_compensated_events() {
            use crate::compensation::Spillover;
            use crate::compensation::groups::{Compensation, Source};
            let (state, id) = positive_gate();
            let files = spilt_workspace("run-compensated");

            let mut compensation = Compensation::default();
            for (_, path) in &files {
                compensation.add_file(path.clone(), Ok(None));
            }
            let group = compensation.new_group("Recomputed");
            let matrix =
                Spillover::new(vec![Arc::from(X), Arc::from(Y)], vec![1.0, 0.0, 2.0, 1.0]).unwrap();
            compensation
                .set_source(
                    group,
                    Source::Loaded {
                        path: "/m.csv".into(),
                        matrix: Arc::new(matrix),
                    },
                )
                .unwrap();
            compensation.move_file(&files[1].1, group).unwrap();

            let mut compensated = state.clone();
            let outcome = run_with(&state, files.clone(), compensation);
            crate::gate_rules::autogate::apply_placements(
                &mut compensated,
                &outcome.placements,
                &specimens(),
            );
            let at = left_edge(&compensated, &id, "fs_b");
            assert!(
                (at - 800.0).abs() < 30.0,
                "compensated, the donor's gate is at {at}"
            );

            let mut raw = state.clone();
            let outcome = run_with(&state, files, Compensation::default());
            crate::gate_rules::autogate::apply_placements(
                &mut raw,
                &outcome.placements,
                &specimens(),
            );
            let at = left_edge(&raw, &id, "fs_b");
            assert!(
                (at - 1000.0).abs() < 30.0,
                "the premise: uncompensated it is at {at}"
            );
        }

        /// A file that cannot be compensated as its group says is reported,
        /// and not measured uncompensated in its place.
        #[test]
        fn a_file_that_cannot_be_compensated_is_reported_not_measured_raw() {
            use crate::compensation::groups::Compensation;
            let (state, _) = positive_gate();
            let files = workspace("run-uncompensatable");
            let mut compensation = Compensation::default();
            compensation.add_file(files[0].1.clone(), Ok(None));
            compensation.add_file(
                files[1].1.clone(),
                Err("its $SPILLOVER is not a spillover matrix".into()),
            );
            let outcome = run_with(&state, files, compensation);
            assert!(
                !outcome.report.positioned.iter().any(|p| &*p.file == "fs_b"),
                "the donor was not placed"
            );
            let reasons: Vec<&String> = outcome.report.skipped.iter().map(|s| &s.reason).collect();
            assert!(
                reasons
                    .iter()
                    .any(|r| r.contains("fs_b.fcs") && r.contains("can't be compensated")),
                "{reasons:?}"
            );
        }

        #[test]
        fn a_cancelled_run_places_nothing() {
            let (state, _) = positive_gate();
            let outcome = run(&state, workspace("run-cancel"), true);
            assert!(outcome.cancelled);
            assert!(outcome.placements.is_empty());
        }
    }
}
