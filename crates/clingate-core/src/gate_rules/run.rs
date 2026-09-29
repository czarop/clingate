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

use crate::gate_rules::autogate::{Report, measure_file};
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
    /// Reading and measuring go together: one file is read, measured, and
    /// dropped before the next is opened.
    Measuring { done: usize, total: usize },
    /// Every file is in; the rules are being solved.
    Solving { done: usize, total: usize },
}

impl Progress {
    pub fn fraction(self) -> f64 {
        match self {
            // Solving is the tail after the reading, so the bar carries on
            // through it rather than stopping dead on one message.
            Progress::Measuring { done, total } if total > 0 => 0.95 * (done as f64 / total as f64),
            Progress::Measuring { .. } => 0.0,
            Progress::Solving { done, total } if total > 0 => {
                0.95 + 0.05 * (done as f64 / total as f64)
            }
            Progress::Solving { .. } => 0.95,
        }
    }

    pub fn describe(self) -> String {
        match self {
            Progress::Measuring { done, total } => format!("Measuring file {done} of {total}"),
            Progress::Solving { done, total } if total > 0 => {
                format!("Solving rule {done} of {total}")
            }
            Progress::Solving { .. } => "Solving the rules...".to_string(),
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
    cancel: &std::sync::atomic::AtomicBool,
    progress: impl Fn(usize, usize) + Sync,
) -> (
    Vec<crate::gate_rules::autogate::Measurement>,
    Vec<crate::gate_rules::autogate::Unmeasured>,
    Vec<String>,
) {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (files, mut problems) = files_to_read(files, names);
    let total = files.len();
    let done = AtomicUsize::new(0);

    type Measured = (
        Vec<crate::gate_rules::autogate::Measurement>,
        Vec<crate::gate_rules::autogate::Unmeasured>,
        Vec<String>,
    );

    let per_file: Vec<Measured> = files
        .par_iter()
        .map(|(path, id)| {
            let mut out: Measured = (Vec::new(), Vec::new(), Vec::new());
            if cancel.load(Ordering::Relaxed) {
                return out;
            }
            // Compensated as its group says and scaled as it is drawn, as the
            // editor and the gallery read it, so a gate is placed on the
            // events a person sees. Only the cofactors this file carries: one
            // channel absent from one file used to fail every rule on every
            // file, the run reporting "Parameter AF P1-A not found" six times
            // and placing nothing.
            let frame = crate::events::read_scaled(path, &compensation.matrix_for(path), arcsinh);
            match frame {
                Ok(df) => match measure_file(snapshot, id, &df, metadata, rules) {
                    Ok((m, u)) => {
                        out.0 = m;
                        out.1 = u;
                    }
                    Err(e) => out.2.push(format!("{id}: {e}")),
                },
                Err(e) => out.2.push(format!("{}: {e}", path.display())),
            }
            progress(done.fetch_add(1, Ordering::Relaxed) + 1, total);
            out
        })
        .collect();

    let mut measured = Vec::new();
    let mut unmeasured = Vec::new();
    for (m, u, p) in per_file {
        measured.extend(m);
        unmeasured.extend(u);
        problems.extend(p);
    }
    (measured, unmeasured, problems)
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
}

/// The whole solve. Blocking: run it off any thread that has to stay
/// responsive.
///
/// It works against `gates` as given - a snapshot, for a caller that keeps
/// editing - and returns the placements rather than writing them, so a gate
/// moved while this runs is not silently overwritten. `cancel` stops it at the
/// next file.
pub fn run_rules(
    gates: &GateState,
    inputs: &RunInputs,
    progress: impl Fn(Progress) + Sync,
    cancel: &AtomicBool,
) -> RunOutcome {
    use std::sync::atomic::Ordering;

    let (measured, unmeasured, mut problems) = measure_all(
        gates,
        &inputs.files,
        &inputs.compensation,
        &inputs.names,
        &inputs.cofactors,
        &inputs.metadata,
        &inputs.rules,
        cancel,
        |done, total| progress(Progress::Measuring { done, total }),
    );

    if cancel.load(Ordering::Relaxed) {
        return RunOutcome {
            report: Report::default(),
            placements: Vec::new(),
            cancelled: true,
            events: Default::default(),
        };
    }

    progress(Progress::Solving { done: 0, total: 0 });
    let (mut report, placements) = crate::gate_rules::autogate::solve_all_reporting(
        gates,
        &inputs.rules,
        &measured,
        &unmeasured,
        &inputs.metadata,
        |done, total| progress(Progress::Solving { done, total }),
    );

    for problem in problems.drain(..) {
        report.skipped.push(crate::gate_rules::autogate::Skipped {
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
        events: crate::review::events::of_run(&measured, &inputs.metadata),
    }
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

            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
            let Outcome::MovedUnreported { gate_at } = &outcome_of("fs_b", "CD134+").outcome else {
                panic!("{:?}", outcome_of("fs_b", "CD134+").outcome);
            };
            let on_x = gate_at.iter().find(|e| e.parameter == X).unwrap();
            assert_eq!(on_x.lower, Some(900.0));
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
                assert_eq!(e.points.len(), KEPT_EVENTS);
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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

            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);
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
            crate::gate_rules::autogate::apply_placements(&mut state, &outcome.placements);

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
            crate::gate_rules::autogate::apply_placements(&mut compensated, &outcome.placements);
            let at = left_edge(&compensated, &id, "fs_b");
            assert!(
                (at - 800.0).abs() < 30.0,
                "compensated, the donor's gate is at {at}"
            );

            let mut raw = state.clone();
            let outcome = run_with(&state, files, Compensation::default());
            crate::gate_rules::autogate::apply_placements(&mut raw, &outcome.placements);
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
