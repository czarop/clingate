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

use crate::gate_editor::gates::GateState;
use crate::gate_rules::autogate::{Report, measure_file};
use crate::gate_rules::rule_store::RuleStore;

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
        use crate::gate_editor::gates::gate_store::GateSource;
        use crate::gate_editor::gates::gate_traits::DrawableGate;
        use crate::gate_rules::rule::{AboveTheNegativeRule, Rule};
        use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleTarget};
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
                crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate::try_new(
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
                crate::gate_editor::gates::gate_single::rectangle_gate::RectangleGate::try_new(
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
