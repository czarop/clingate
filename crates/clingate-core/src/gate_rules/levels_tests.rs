//! A run places gates level by level down the tree, and says why it left a
//! gate alone.
//!
//! The case that went wrong: a rule on a gate whose parent another rule moves
//! in the same run. A run used to measure every gate first and place them all
//! after, so the child was measured on its parent as it was *before* the run -
//! the wrong cells - and nothing noticed because nothing tested it. The oracle
//! here is doing it by hand: run the parent's rule, apply it, then run the
//! child's. One run has to give exactly that, however the rules are listed.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rand::SeedableRng;
use rand_distr::{Distribution, Uniform};
use rustc_hash::FxBuildHasher;

use crate::file_load_tests::{scratch, write_fcs_rows};
use crate::gate_rules::autogate::{apply_placements, extent_on, linked_conflicts, rule_levels};
use crate::gate_rules::rule::{Rule, TailFractionRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::run::{Progress, RunInputs, RunOutcome, run_rules};
use crate::gates::GateState;
use crate::gates::gate_store::GateSource;
use crate::gates::gate_traits::DrawableGate;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";
const BIG: f32 = 1e16;

/// A rectangle from `x0` up and from `y0` up.
fn rect(id: &str, name: &str, x0: f32, y0: f32) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(x0, y0), (BIG, y0), (BIG, BIG), (x0, BIG)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from(id),
                name: name.into(),
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

fn add(state: &mut GateState, gate: Arc<dyn DrawableGate>, parent: Option<&str>) -> Arc<str> {
    let id: Arc<str> = Arc::from(gate.get_id());
    state.place_gate(&[id.clone()], &gate, &GateSource::Global);
    state
        .place_new_gate(parent.map(Arc::from), id.clone())
        .unwrap();
    id
}

/// Lymph at the top holding everything, CD69+ under it, IFNy+ under that.
/// Every gate starts open to the whole axis, so any rule moves it.
fn three_deep() -> GateState {
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(&mut state, rect("cd69", "CD69+", -BIG, -1.0), Some("lymph"));
    add(&mut state, rect("ifng", "IFNy+", -1.0, -BIG), Some("cd69"));
    state
}

/// X uniform over 0-1000 and Y tracking it: what a gate holds on Y depends on
/// where its parent cut X, which is the whole point.
fn events(seed: u64) -> Vec<Vec<f32>> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let x = Uniform::new(0.0f32, 1000.0).unwrap();
    let noise = Uniform::new(-50.0f32, 50.0).unwrap();
    (0..20_000)
        .map(|_| {
            let v = x.sample(&mut rng);
            vec![v, v + noise.sample(&mut rng)]
        })
        .collect()
}

fn specimens() -> crate::omiq::metadata::MetaDataFileMap {
    let mut map = im::HashMap::with_hasher(FxBuildHasher);
    for (file, id) in [("fs_a", "DONOR-A"), ("fs_b", "DONOR-B")] {
        let mut columns: rustc_hash::FxHashMap<Arc<str>, Arc<str>> = Default::default();
        columns.insert(Arc::from("SampleID"), Arc::from(id));
        columns.insert(Arc::from("SampleType"), Arc::from("FS"));
        map.insert(Arc::from(file) as Arc<str>, columns);
    }
    map
}

fn files(name: &str) -> Vec<(Arc<str>, PathBuf)> {
    let dir = scratch(name);
    let channels = [(X, None), (Y, None)];
    write_fcs_rows(&dir.join("fs_a.fcs"), &channels, &events(1), &[]);
    write_fcs_rows(&dir.join("fs_b.fcs"), &channels, &events(2), &[]);
    vec![
        (Arc::from("fs_a.fcs"), dir.join("fs_a.fcs")),
        (Arc::from("fs_b.fcs"), dir.join("fs_b.fcs")),
    ]
}

/// Keep the top `band` of the parent on `parameter`, read on each sample.
fn top(parameter: &str, band: (f64, f64)) -> GateRule {
    GateRule {
        parameter: Arc::from(parameter),
        bound: Bound::Above,
        measured_on: MeasuredOn::Itself,
        rule: Rule::TailFraction(TailFractionRule::new(band)),
    }
}

fn lymph_rule() -> (RuleTarget, GateRule) {
    (RuleTarget::named("Lymph"), top(X, (0.49, 0.51)))
}
fn cd69_rule() -> (RuleTarget, GateRule) {
    (RuleTarget::under("CD69+", "Lymph"), top(Y, (0.19, 0.21)))
}
fn ifng_rule() -> (RuleTarget, GateRule) {
    (RuleTarget::under("IFNy+", "CD69+"), top(X, (0.24, 0.26)))
}

fn store(rules: &[(RuleTarget, GateRule)]) -> RuleStore {
    let mut store = RuleStore::default();
    for (target, rule) in rules {
        store.insert(target.clone(), rule.clone());
    }
    store
}

fn inputs(files: &[(Arc<str>, PathBuf)], rules: RuleStore) -> RunInputs {
    RunInputs {
        files: files.to_vec(),
        compensation: crate::compensation::groups::Compensation::default(),
        names: [("fs_a.fcs", "fs_a"), ("fs_b.fcs", "fs_b")]
            .into_iter()
            .map(|(a, b)| (Arc::from(a), Arc::from(b)))
            .collect::<std::collections::HashMap<_, _, FxBuildHasher>>(),
        cofactors: Vec::new(),
        metadata: specimens(),
        rules,
    }
}

fn run(state: &GateState, files: &[(Arc<str>, PathBuf)], rules: RuleStore) -> RunOutcome {
    run_rules(
        state,
        &inputs(files, rules),
        |_| {},
        &AtomicBool::new(false),
    )
}

/// Where `gate`'s lower edge on `parameter` sits for `file` once `placed`.
fn edge(state: &GateState, gate: &str, parameter: &str, file: &str) -> f32 {
    let g = state
        .gate_for_file(&Arc::from(gate), &Arc::from(file), &specimens())
        .unwrap();
    extent_on(&g.get_gate_ref(None).unwrap().geometry, parameter)
        .unwrap()
        .0
}

fn applied(state: &GateState, outcome: &RunOutcome) -> GateState {
    let mut after = state.clone();
    apply_placements(&mut after, &outcome.placements, &specimens());
    after
}

/// Every edge the three gates have, on both files, after `state`.
fn edges(state: &GateState) -> Vec<f32> {
    let mut all = Vec::new();
    for file in ["fs_a", "fs_b"] {
        all.push(edge(state, "lymph", X, file));
        all.push(edge(state, "cd69", Y, file));
        all.push(edge(state, "ifng", X, file));
    }
    all
}

// ─── the levels themselves ────────────────────────────────────────────────────

fn level_names(state: &GateState, rules: &RuleStore) -> Vec<Vec<String>> {
    rule_levels(state, rules)
        .iter()
        .map(|level| {
            let mut names: Vec<String> = level
                .iter()
                .map(|node| {
                    let id = state.gate_for_node(node).unwrap();
                    state.registered_gate(id).unwrap().get_name().to_string()
                })
                .collect();
            names.sort();
            names
        })
        .collect()
}

#[test]
fn a_ruled_gate_comes_after_every_ruled_gate_above_it() {
    let state = three_deep();
    let rules = store(&[ifng_rule(), cd69_rule(), lymph_rule()]);
    assert_eq!(
        level_names(&state, &rules),
        [["Lymph"], ["CD69+"], ["IFNy+"]],
        "listed bottom-up, placed top-down"
    );
}

#[test]
fn a_gate_with_no_rule_between_does_not_add_a_level() {
    // CD69+ has no rule: it does not move, so IFNy+ only waits for Lymph.
    let state = three_deep();
    let rules = store(&[ifng_rule(), lymph_rule()]);
    assert_eq!(level_names(&state, &rules), [["Lymph"], ["IFNy+"]]);
}

#[test]
fn gates_with_no_ruled_gate_above_them_are_one_level() {
    let state = three_deep();
    let rules = store(&[cd69_rule()]);
    assert_eq!(level_names(&state, &rules), [["CD69+"]]);
}

#[test]
fn no_rules_is_no_levels() {
    assert!(rule_levels(&three_deep(), &RuleStore::default()).is_empty());
}

// ─── one run, level by level ──────────────────────────────────────────────────

#[test]
fn a_gate_under_a_moved_parent_is_placed_on_the_parent_as_moved() {
    let files = files("levels-two");
    let state = three_deep();

    // By hand: Lymph's rule, applied; then CD69+'s on what that left.
    let by_hand = {
        let first = run(&state, &files, store(&[lymph_rule()]));
        let after_first = applied(&state, &first);
        let second = run(&after_first, &files, store(&[cd69_rule()]));
        applied(&after_first, &second)
    };
    let in_one_run = applied(
        &state,
        &run(&state, &files, store(&[lymph_rule(), cd69_rule()])),
    );

    for file in ["fs_a", "fs_b"] {
        assert_eq!(
            edge(&in_one_run, "cd69", Y, file),
            edge(&by_hand, "cd69", Y, file),
            "{file}: one run puts CD69+ where doing it by hand does"
        );
        assert_eq!(
            edge(&in_one_run, "lymph", X, file),
            edge(&by_hand, "lymph", X, file)
        );
    }

    // And that is a different answer from measuring CD69+ on Lymph unmoved -
    // which is what a run used to do, so this test can tell the two apart.
    let on_the_old_parent = applied(&state, &run(&state, &files, store(&[cd69_rule()])));
    for file in ["fs_a", "fs_b"] {
        let right = edge(&in_one_run, "cd69", Y, file);
        let old = edge(&on_the_old_parent, "cd69", Y, file);
        assert!(
            (right - old).abs() > 50.0,
            "{file}: CD69+ at {right} on the moved Lymph, {old} on the unmoved one - \
             these have to differ for the test to mean anything"
        );
    }
}

#[test]
fn three_levels_in_one_run_are_three_steps_by_hand() {
    let files = files("levels-three");
    let state = three_deep();

    let mut by_hand = state.clone();
    for rule in [lymph_rule(), cd69_rule(), ifng_rule()] {
        let outcome = run(&by_hand, &files, store(&[rule]));
        by_hand = applied(&by_hand, &outcome);
    }
    let outcome = run(
        &state,
        &files,
        store(&[lymph_rule(), cd69_rule(), ifng_rule()]),
    );
    assert!(
        outcome.report.skipped.is_empty(),
        "{:?}",
        outcome
            .report
            .skipped
            .iter()
            .map(|s| &s.reason)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        outcome.report.positioned.len(),
        6,
        "three gates on two specimens"
    );
    assert_eq!(edges(&applied(&state, &outcome)), edges(&by_hand));
}

#[test]
fn the_order_the_rules_are_listed_in_changes_nothing() {
    let files = files("levels-order");
    let state = three_deep();
    let listed_down = run(
        &state,
        &files,
        store(&[lymph_rule(), cd69_rule(), ifng_rule()]),
    );
    let listed_up = run(
        &state,
        &files,
        store(&[ifng_rule(), cd69_rule(), lymph_rule()]),
    );
    let listed_mixed = run(
        &state,
        &files,
        store(&[cd69_rule(), ifng_rule(), lymph_rule()]),
    );

    let down = edges(&applied(&state, &listed_down));
    assert_eq!(edges(&applied(&state, &listed_up)), down);
    assert_eq!(edges(&applied(&state, &listed_mixed)), down);
}

#[test]
fn the_events_kept_for_review_are_the_ones_each_level_measured() {
    // A review compares a placement with the population it was measured on;
    // for CD69+ that has to be the population under the moved Lymph.
    let files = files("levels-events");
    let state = three_deep();
    let outcome = run(&state, &files, store(&[lymph_rule(), cd69_rule()]));
    let kept = |gate: &str, file: &str| {
        outcome
            .events
            .samples
            .iter()
            .find(|s| &*s.gate_id == gate && &*s.file == file)
            .unwrap_or_else(|| panic!("events for {gate} on {file}"))
            .events
    };
    for file in ["fs_a", "fs_b"] {
        let lymph_all = kept("lymph", file);
        let cd69_parent = kept("cd69", file);
        // Lymph's parent is the whole file; CD69+'s is what Lymph kept - half.
        assert_eq!(lymph_all, 20_000);
        let half = cd69_parent as f64 / lymph_all as f64;
        assert!(
            (0.48..=0.52).contains(&half),
            "{file}: CD69+ was measured on {cd69_parent} events - {half:.3} of the file, \
             not the half the moved Lymph holds"
        );
    }
}

#[test]
fn stopping_between_levels_places_nothing() {
    let files = files("levels-stop");
    let state = three_deep();
    let stop = AtomicBool::new(false);
    let outcome = run_rules(
        &state,
        &inputs(&files, store(&[lymph_rule(), cd69_rule()])),
        |p| {
            // Stop once the second level starts reading.
            if let Progress::Measuring { done, total } = p
                && done > total / 2
            {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        },
        &stop,
    );
    assert!(outcome.cancelled);
    assert!(
        outcome.placements.is_empty(),
        "nothing half-done is handed back"
    );
}

#[test]
fn reading_counts_up_once_across_every_level() {
    let files = files("levels-progress");
    let state = three_deep();
    let seen = std::sync::Mutex::new(Vec::new());
    run_rules(
        &state,
        &inputs(&files, store(&[lymph_rule(), cd69_rule(), ifng_rule()])),
        |p| {
            if let Progress::Measuring { done, total } = p {
                seen.lock().unwrap().push((done, total));
            }
        },
        &AtomicBool::new(false),
    );
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 6, "two files, three levels");
    assert!(seen.iter().all(|(_, total)| *total == 6), "{seen:?}");
    let mut done: Vec<usize> = seen.iter().map(|(d, _)| *d).collect();
    done.sort();
    assert_eq!(
        done,
        [1, 2, 3, 4, 5, 6],
        "the bar fills once, not once a level"
    );
}

// ─── what a run says it left alone ────────────────────────────────────────────

fn reasons(outcome: &RunOutcome) -> Vec<String> {
    outcome
        .report
        .skipped
        .iter()
        .map(|s| format!("{} | {} | {}", s.gate, s.file, s.reason))
        .collect()
}

#[test]
fn a_rule_that_reaches_no_gate_says_so() {
    let files = files("levels-nothing");
    let state = three_deep();
    let outcome = run(
        &state,
        &files,
        store(&[
            (RuleTarget::named("CD69 +"), top(Y, (0.1, 0.2))),
            (RuleTarget::under("CD69+", "Lymphs"), top(Y, (0.1, 0.2))),
        ]),
    );
    let said = reasons(&outcome);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said.iter().all(|r| r.contains("reaches no gate")),
        "{said:?}"
    );
    assert!(said[0].starts_with("CD69 +"), "{said:?}");
    assert!(outcome.placements.is_empty());
}

#[test]
fn the_same_problem_on_every_file_is_one_line_that_names_a_file_and_counts_the_rest() {
    // A rule on a parameter the gate is not drawn on cannot be measured
    // anywhere: one line, not one per file, but still saying where.
    let files = files("levels-one-line");
    let state = three_deep();
    let outcome = run(
        &state,
        &files,
        store(&[(RuleTarget::named("Lymph"), top("BV421-A", (0.1, 0.2)))]),
    );
    let said = reasons(&outcome);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("| fs_a |"),
        "names the first file: {said:?}"
    );
    assert!(
        said[0].ends_with("and the same on 1 other file"),
        "and how many shared it: {said:?}"
    );
}

#[test]
fn a_reference_file_the_workspace_does_not_have_is_named_as_the_problem() {
    let files = files("levels-no-file");
    let state = three_deep();
    let mut rule = top(X, (0.1, 0.2));
    rule.measured_on = MeasuredOn::File(Arc::from("QC1_FS_Plate_001.fcs"));
    let outcome = run(&state, &files, store(&[(RuleTarget::named("Lymph"), rule)]));
    let said = reasons(&outcome);
    assert_eq!(said.len(), 2, "one per specimen: {said:?}");
    assert!(
        said.iter()
            .all(|r| r
                .contains("\"QC1_FS_Plate_001.fcs\", which is not one of this workspace's files")),
        "{said:?}"
    );
}

#[test]
fn a_specimen_without_the_partner_the_rule_reads_is_named() {
    let files = files("levels-no-partner");
    let state = three_deep();
    let mut rule = top(X, (0.1, 0.2));
    rule.measured_on = MeasuredOn::Partner(Arc::from("FMX"));
    let outcome = run(&state, &files, store(&[(RuleTarget::named("Lymph"), rule)]));
    let said = reasons(&outcome);
    assert_eq!(said.len(), 2, "{said:?}");
    assert!(
        said.iter()
            .any(|r| r.contains("no file with SampleID DONOR-A has the sample type FMX")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|r| r.contains("no file with SampleID DONOR-B has the sample type FMX")),
        "{said:?}"
    );
}

#[test]
fn a_reference_that_could_not_be_measured_says_why() {
    let files = files("levels-unmeasured-reference");
    let mut state = three_deep();
    // CD69+'s parent holds nothing on fs_a: Lymph moved past every event there.
    let lymph = state.registered_gate(&Arc::from("lymph")).unwrap();
    let empty =
        crate::gate_rules::autogate::translate_edge_to(&lymph, X, Bound::Above, 5_000.0).unwrap();
    crate::gate_rules::autogate::place_for_specimen(
        &mut state,
        &Arc::from("lymph"),
        &crate::omiq::metadata::MetaDataKey {
            parameter: Arc::from("SampleID"),
            group: Arc::from("DONOR-A"),
        },
        &empty,
    );
    let mut rule = top(Y, (0.1, 0.2));
    rule.measured_on = MeasuredOn::File(Arc::from("fs_a"));
    let outcome = run(&state, &files, store(&[(RuleTarget::named("CD69+"), rule)]));
    let said = reasons(&outcome);
    assert!(
        said.iter().any(|r| r.starts_with("CD69+ | fs_b |")
            && r.contains("the reference file fs_a could not be measured: its parent population holds 0 events")),
        "{said:?}"
    );
}

#[test]
fn a_reference_named_in_the_metadata_but_not_loaded_says_so() {
    let files = files("levels-not-loaded");
    let state = three_deep();
    let mut rule = top(X, (0.1, 0.2));
    rule.measured_on = MeasuredOn::File(Arc::from("fs_a"));
    // Only fs_b is loaded.
    let outcome = run(
        &state,
        &files[1..],
        store(&[(RuleTarget::named("Lymph"), rule)]),
    );
    let said = reasons(&outcome);
    assert!(
        said.iter()
            .any(|r| r.contains("the reference file fs_a was not read in this run")),
        "{said:?}"
    );
}

// ─── a linked gate reached by two rules ───────────────────────────────────────

/// "Shared" linked under both A and B: one gate, two places.
fn linked() -> GateState {
    let mut state = GateState::default();
    add(&mut state, rect("a", "A", -1.0, -BIG), None);
    add(&mut state, rect("b", "B", -1.0, -BIG), None);
    add(&mut state, rect("shared", "Shared", -BIG, -1.0), Some("a"));
    add(&mut state, rect("copy", "Shared", -BIG, -1.0), Some("b"));
    let under_a = state.nodes_for_gate(&Arc::from("shared"))[0].clone();
    let under_b = state.nodes_for_gate(&Arc::from("copy"))[0].clone();
    state.link_node_to_gate(&under_b, &under_a).unwrap();
    assert!(state.is_linked(&Arc::from("shared")));
    state
}

#[test]
fn a_linked_gate_set_by_two_rules_is_left_alone_and_the_run_says_why() {
    let files = files("levels-linked-two");
    let state = linked();
    let rules = store(&[
        (RuleTarget::under("Shared", "A"), top(Y, (0.1, 0.2))),
        (RuleTarget::under("Shared", "B"), top(Y, (0.3, 0.4))),
    ]);
    let conflicts = linked_conflicts(&state, &rules);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].rules, ["Shared of A", "Shared of B"]);
    assert!(
        conflicts[0].reason.contains("2 rules set it"),
        "{}",
        conflicts[0].reason
    );

    let outcome = run(&state, &files, rules);
    assert!(outcome.placements.is_empty(), "neither rule wins silently");
    let said = reasons(&outcome);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("would fight over its one position"),
        "{said:?}"
    );
}

#[test]
fn one_rule_reaching_a_linked_gate_at_two_places_is_left_alone_too() {
    // Two populations to read for one position is the same fight.
    let files = files("levels-linked-one");
    let state = linked();
    let rules = store(&[(RuleTarget::named("Shared"), top(Y, (0.1, 0.2)))]);
    let conflicts = linked_conflicts(&state, &rules);
    assert_eq!(conflicts.len(), 1);
    assert!(
        conflicts[0].reason.contains("Name the parent"),
        "{}",
        conflicts[0].reason
    );
    let outcome = run(&state, &files, rules);
    assert!(outcome.placements.is_empty());
}

#[test]
fn a_linked_gate_set_by_one_rule_at_one_place_moves_everywhere_it_is_drawn() {
    let files = files("levels-linked-fine");
    let state = linked();
    let rules = store(&[(RuleTarget::under("Shared", "A"), top(Y, (0.1, 0.2)))]);
    assert!(linked_conflicts(&state, &rules).is_empty());
    let outcome = run(&state, &files, rules);
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    assert_eq!(outcome.report.positioned.len(), 2, "one per specimen");
    let after = applied(&state, &outcome);
    // One gate: wherever it is drawn, it is where the rule put it.
    for file in ["fs_a", "fs_b"] {
        assert!(edge(&after, "shared", Y, file) > 0.0, "moved from -1");
    }
}

#[test]
fn an_unlinked_gate_of_the_same_name_under_two_parents_is_no_conflict() {
    let mut state = GateState::default();
    add(&mut state, rect("a", "A", -1.0, -BIG), None);
    add(&mut state, rect("b", "B", -1.0, -BIG), None);
    add(&mut state, rect("one", "Shared", -BIG, -1.0), Some("a"));
    add(&mut state, rect("two", "Shared", -BIG, -1.0), Some("b"));
    let rules = store(&[(RuleTarget::named("Shared"), top(Y, (0.1, 0.2)))]);
    assert!(linked_conflicts(&state, &rules).is_empty());
    assert_eq!(level_names(&state, &rules), [["Shared", "Shared"]]);
}

// ─── a gate that follows another ──────────────────────────────────────────────
//
// "In the same position as the main CD4-CD8+ gate", "aligned to the left edge
// of CD19+CD14-". The anchor is placed first, on each sample, and this gate
// takes its position from it there.

use crate::gate_rules::autogate::anchor_problems;
use crate::gate_rules::rule::{EdgeFrom, FromGateRule, Side};

/// A rectangle with all four edges given.
fn boxed(id: &str, name: &str, x: (f32, f32), y: (f32, f32)) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(x.0, y.0), (x.1, y.0), (x.1, y.1), (x.0, y.1)],
        X,
        Y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from(id),
                name: name.into(),
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

fn follows(same_shape_as: Option<RuleTarget>, edges: Vec<EdgeFrom>) -> GateRule {
    GateRule {
        // Ignored for this rule; given anyway, as a hand-written file might.
        parameter: Arc::from("anything"),
        bound: Bound::Above,
        measured_on: MeasuredOn::Partner(Arc::from("FMX")),
        rule: Rule::FromAnotherGate(FromGateRule {
            same_shape_as,
            edges,
        }),
    }
}

fn edge_from(
    anchor: RuleTarget,
    parameter: &str,
    side: Side,
    anchor_side: Side,
    gap: f64,
) -> EdgeFrom {
    EdgeFrom {
        anchor,
        parameter: Arc::from(parameter),
        side,
        anchor_side,
        gap,
    }
}

fn extent(state: &GateState, gate: &str, parameter: &str, file: &str) -> (f32, f32) {
    let g = state
        .gate_for_file(&Arc::from(gate), &Arc::from(file), &specimens())
        .unwrap();
    extent_on(
        &g.get_gate_ref(Some(gate))
            .or_else(|| g.get_gate_ref(None))
            .unwrap()
            .geometry,
        parameter,
    )
    .unwrap()
}

/// Give `gate` its own position for one donor, as a person placing it by hand
/// for that specimen would.
fn place_by_hand(state: &mut GateState, gate: &str, donor: &str, moved: Arc<dyn DrawableGate>) {
    crate::gate_rules::autogate::place_for_specimen(
        state,
        &Arc::from(gate),
        &crate::omiq::metadata::MetaDataKey {
            parameter: Arc::from("SampleID"),
            group: Arc::from(donor),
        },
        &moved,
    );
}

/// [`three_deep`], with "CD69 copy" under Lymph beside CD69+, drawn wide open
/// so any placement moves it.
fn with_a_copy() -> GateState {
    let mut state = three_deep();
    add(
        &mut state,
        rect("copy", "CD69 copy", -BIG, -500.0),
        Some("lymph"),
    );
    state
}

fn copy_rule() -> (RuleTarget, GateRule) {
    (
        RuleTarget::under("CD69 copy", "Lymph"),
        follows(Some(RuleTarget::under("CD69+", "Lymph")), Vec::new()),
    )
}

/// [`three_deep`], with "CD69 copy" under a top-level gate of its own,
/// holding every event as Lymph does: a copy of CD69+'s shape beside CD69+
/// would be over it.
fn with_a_copy_elsewhere() -> GateState {
    let mut state = three_deep();
    add(&mut state, rect("elsewhere", "Elsewhere", -1.0, -BIG), None);
    add(
        &mut state,
        rect("copy", "CD69 copy", -BIG, -500.0),
        Some("elsewhere"),
    );
    state
}

fn copy_elsewhere_rule() -> (RuleTarget, GateRule) {
    (
        RuleTarget::under("CD69 copy", "Elsewhere"),
        follows(Some(RuleTarget::under("CD69+", "Lymph")), Vec::new()),
    )
}

#[test]
fn a_copy_takes_the_anchor_s_position_on_each_sample_after_the_anchor_s_rule() {
    let files = files("follow-copy");
    let state = with_a_copy_elsewhere();
    // Listed before the rule it depends on.
    let outcome = run(&state, &files, store(&[copy_elsewhere_rule(), cd69_rule()]));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&state, &outcome);
    for file in ["fs_a", "fs_b"] {
        let anchor = extent(&after, "cd69", Y, file);
        assert_eq!(extent(&after, "copy", Y, file), anchor, "{file}");
        assert_eq!(
            extent(&after, "copy", X, file),
            extent(&after, "cd69", X, file)
        );
        assert!(
            anchor.0 > 0.0,
            "{file}: the anchor moved, from -1 to {}",
            anchor.0
        );
    }
    // And by hand: the anchor's rule, applied, then the copy's.
    let first = applied(&state, &run(&state, &files, store(&[cd69_rule()])));
    let by_hand = applied(
        &first,
        &run(&first, &files, store(&[copy_elsewhere_rule()])),
    );
    for file in ["fs_a", "fs_b"] {
        assert_eq!(
            extent(&after, "copy", Y, file),
            extent(&by_hand, "copy", Y, file)
        );
    }
    // The two donors' anchors differ, so the copies do: it is per sample.
    assert_ne!(
        extent(&after, "copy", Y, "fs_a"),
        extent(&after, "copy", Y, "fs_b")
    );
}

#[test]
fn a_copy_of_a_gate_placed_by_hand_takes_each_donor_s_own_position() {
    let files = files("follow-hand");
    let mut state = with_a_copy_elsewhere();
    for (donor, at) in [("DONOR-A", 300.0), ("DONOR-B", 600.0)] {
        let cd69 = state.registered_gate(&Arc::from("cd69")).unwrap();
        let moved =
            crate::gate_rules::autogate::translate_edge_to(&cd69, Y, Bound::Above, at).unwrap();
        place_by_hand(&mut state, "cd69", donor, moved);
    }
    let outcome = run(&state, &files, store(&[copy_elsewhere_rule()]));
    let after = applied(&state, &outcome);
    assert_eq!(extent(&after, "copy", Y, "fs_a").0, 300.0);
    assert_eq!(extent(&after, "copy", Y, "fs_b").0, 600.0);
    // What the copy holds is reported, before and after.
    let placed = &outcome.report.positioned;
    assert_eq!(placed.len(), 2);
    for p in placed {
        assert!(
            p.to.is_finite() && p.from.is_finite(),
            "{} {}",
            p.from,
            p.to
        );
        assert!(
            p.to < p.from,
            "moved up from -500, it holds less: {} -> {}",
            p.from,
            p.to
        );
        assert_eq!(p.confidence, 1.0, "copied, not estimated");
    }
}

#[test]
fn a_copy_already_where_the_anchor_is_is_left_in_place() {
    let files = files("follow-kept");
    let state = with_a_copy();
    let rules = store(&[copy_rule(), cd69_rule()]);
    let once = applied(&state, &run(&state, &files, rules.clone()));
    let again = run(&once, &files, rules);
    let kept: Vec<&str> = again
        .report
        .unchanged
        .iter()
        .filter(|u| &*u.gate == "CD69 copy")
        .map(|u| &*u.file)
        .collect();
    assert_eq!(kept.len(), 2, "both samples: {kept:?}");
    assert!(
        !again.placements.iter().any(|p| &*p.gate_id == "copy"),
        "nothing to write for the copy"
    );
}

#[test]
fn a_gate_waits_for_every_gate_it_follows_whatever_its_place_in_the_tree() {
    // The copy sits under Lymph (level 1 once Lymph has a rule), but its
    // anchor CD69+ is level 1 too - so the copy is level 2.
    let state = with_a_copy();
    let rules = store(&[copy_rule(), lymph_rule(), cd69_rule()]);
    assert_eq!(
        level_names(&state, &rules),
        [vec!["Lymph"], vec!["CD69+"], vec!["CD69 copy"]]
    );
    // An anchor with no rule is read as it stands: no wait for it.
    let rules = store(&[copy_rule(), lymph_rule()]);
    assert_eq!(
        level_names(&state, &rules),
        [vec!["Lymph"], vec!["CD69 copy"]]
    );
}

#[test]
fn an_edge_is_set_against_the_anchor_s_edge_and_a_rectangle_s_other_edges_stay() {
    // "The CD19- gate aligned to the left edge of the CD19+CD14- gate."
    let files = files("follow-edge");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(&mut state, rect("pos", "CD19+", 0.0, -BIG), Some("lymph"));
    add(
        &mut state,
        boxed("neg", "CD19-", (-100.0, 900.0), (-50.0, 950.0)),
        Some("lymph"),
    );
    let pos_rule = (RuleTarget::under("CD19+", "Lymph"), top(X, (0.49, 0.51)));
    let neg_rule = (
        RuleTarget::under("CD19-", "Lymph"),
        follows(
            None,
            vec![edge_from(
                RuleTarget::under("CD19+", "Lymph"),
                X,
                Side::Upper,
                Side::Lower,
                0.0,
            )],
        ),
    );
    let outcome = run(&state, &files, store(&[neg_rule, pos_rule]));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&state, &outcome);
    for file in ["fs_a", "fs_b"] {
        let pos_left = extent(&after, "pos", X, file).0;
        assert!(
            (400.0..600.0).contains(&pos_left),
            "{file}: CD19+ moved to {pos_left}"
        );
        assert_eq!(
            extent(&after, "neg", X, file),
            (-100.0, pos_left),
            "{file}: its right edge against CD19+'s left; its left edge where it was"
        );
        assert_eq!(
            extent(&after, "neg", Y, file),
            (-50.0, 950.0),
            "{file}: y untouched"
        );
    }
}

#[test]
fn a_gap_is_added_to_the_anchor_s_edge() {
    let files = files("follow-gap");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(
        &mut state,
        boxed("pos", "Pos", (400.0, 900.0), (-BIG, BIG)),
        Some("lymph"),
    );
    add(
        &mut state,
        boxed("neg", "Neg", (-100.0, 900.0), (-50.0, 950.0)),
        Some("lymph"),
    );
    let rule = (
        RuleTarget::named("Neg"),
        follows(
            None,
            vec![edge_from(
                RuleTarget::named("Pos"),
                X,
                Side::Upper,
                Side::Lower,
                -10.0,
            )],
        ),
    );
    let after = applied(&state, &run(&state, &files, store(&[rule])));
    assert_eq!(extent(&after, "neg", X, "fs_a"), (-100.0, 390.0));
}

#[test]
fn two_edges_can_each_follow_their_own_gate() {
    // "The MAIT CD4-CD8- gate adjacent to the left edge of the CD4+CD8- gate
    // and the bottom of the CD4-CD8+ gate."
    let files = files("follow-two-edges");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(
        &mut state,
        boxed("cd4", "CD4+CD8-", (300.0, BIG), (-BIG, 500.0)),
        Some("lymph"),
    );
    add(
        &mut state,
        boxed("cd8", "CD4-CD8+", (-BIG, 300.0), (600.0, BIG)),
        Some("lymph"),
    );
    add(
        &mut state,
        boxed("dn", "CD4-CD8-", (-100.0, 800.0), (-100.0, 800.0)),
        Some("lymph"),
    );
    let rule = (
        RuleTarget::named("CD4-CD8-"),
        follows(
            None,
            vec![
                edge_from(
                    RuleTarget::named("CD4+CD8-"),
                    X,
                    Side::Upper,
                    Side::Lower,
                    0.0,
                ),
                edge_from(
                    RuleTarget::named("CD4-CD8+"),
                    Y,
                    Side::Upper,
                    Side::Lower,
                    0.0,
                ),
            ],
        ),
    );
    let after = applied(&state, &run(&state, &files, store(&[rule])));
    assert_eq!(extent(&after, "dn", X, "fs_a"), (-100.0, 300.0));
    assert_eq!(extent(&after, "dn", Y, "fs_a"), (-100.0, 600.0));
}

#[test]
fn a_polygon_slides_whole_until_its_edge_is_there() {
    let files = files("follow-polygon");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(
        &mut state,
        boxed("pos", "Pos", (450.0, 900.0), (-BIG, BIG)),
        Some("lymph"),
    );
    let polygon: Arc<dyn DrawableGate> = Arc::new(
        crate::gates::gate_single::polygon_gate::PolygonGate::try_new(
            flow_gates::Gate {
                id: Arc::from("poly"),
                name: "Poly".into(),
                geometry: flow_gates::create_polygon_geometry(
                    vec![(100.0, 100.0), (300.0, 100.0), (200.0, 400.0)],
                    X,
                    Y,
                )
                .unwrap(),
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(X), Arc::from(Y)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    );
    add(&mut state, polygon, Some("lymph"));
    let rule = (
        RuleTarget::named("Poly"),
        follows(
            None,
            vec![edge_from(
                RuleTarget::named("Pos"),
                X,
                Side::Upper,
                Side::Lower,
                0.0,
            )],
        ),
    );
    let after = applied(&state, &run(&state, &files, store(&[rule])));
    assert_eq!(
        extent(&after, "poly", X, "fs_a"),
        (250.0, 450.0),
        "slid 150 to meet it, width kept"
    );
    assert_eq!(extent(&after, "poly", Y, "fs_a"), (100.0, 400.0));
}

#[test]
fn a_gate_drawn_the_other_way_round_is_copied_turned() {
    let files = files("follow-turned");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    // The anchor is drawn with SSC-A across and FSC-A up.
    let turned: Arc<dyn DrawableGate> = Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from("anchor"),
                name: "Anchor".into(),
                geometry: flow_gates::create_rectangle_geometry(
                    vec![
                        (100.0, 500.0),
                        (200.0, 500.0),
                        (200.0, 700.0),
                        (100.0, 700.0),
                    ],
                    Y,
                    X,
                )
                .unwrap(),
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(Y), Arc::from(X)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    );
    add(&mut state, turned, Some("lymph"));
    // Under a parent of its own: a copy beside its anchor would be over it.
    add(&mut state, rect("apart", "Apart", -1.0, -BIG), None);
    add(
        &mut state,
        boxed("mine", "Mine", (0.0, 10.0), (0.0, 10.0)),
        Some("apart"),
    );
    let rule = (
        RuleTarget::named("Mine"),
        follows(Some(RuleTarget::named("Anchor")), Vec::new()),
    );
    let outcome = run(&state, &files, store(&[rule]));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&state, &outcome);
    assert_eq!(extent(&after, "mine", X, "fs_a"), (500.0, 700.0));
    assert_eq!(extent(&after, "mine", Y, "fs_a"), (100.0, 200.0));
    assert_eq!(
        after
            .gate_for_file(&Arc::from("mine"), &Arc::from("fs_a"), &specimens())
            .unwrap()
            .get_params(),
        (Arc::from(X), Arc::from(Y)),
        "still drawn its own way round"
    );
}

#[test]
fn a_quadrant_follows_another_quadrant_on_each_sample() {
    // "Position gate 47 according to the position of the same gates on the
    // CD4-CD8+ population."
    use crate::gates::gate_composite::quadrant_gate::QuadrantGate;
    use crate::gates::gate_types::PrimaryGateType;
    let files = files("follow-quadrant");
    let mapper = crate::axis_store::PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        flow_fcs::TransformType::Linear,
        flow_fcs::TransformType::Linear,
    );
    let mut state = GateState::default();
    add(&mut state, rect("cd8", "CD8", -1.0, -BIG), None);
    add(&mut state, rect("gd", "TCRgd", -1.0, -BIG), None);
    let mut quadrant = |name: &str, parent: &str, at: (f32, f32)| -> Arc<str> {
        let before: Vec<_> = state.placements().map(|(n, _)| n.clone()).collect();
        state
            .add_gate(
                &mapper,
                at.0,
                at.1,
                Arc::from(X),
                Arc::from(Y),
                None,
                Some(Arc::from(parent)),
                PrimaryGateType::Quadrant,
                Some(name.to_string()),
            )
            .unwrap();
        let corner = state
            .placements()
            .find(|(n, _)| !before.contains(n))
            .map(|(_, p)| p.gate_id.clone())
            .unwrap();
        corner
    };
    let anchor_corner = quadrant("Memory", "cd8", (300.0, 300.0));
    let mine_corner = quadrant("Memory gd", "gd", (100.0, 500.0));
    // The CD8 quadrant placed by hand, differently for each donor.
    for (donor, pixel) in [("DONOR-A", (200.0, 250.0)), ("DONOR-B", (400.0, 350.0))] {
        let q = state.registered_gate(&anchor_corner).unwrap();
        let moved: Arc<dyn DrawableGate> =
            Arc::from(q.replace_point(pixel, 0, None, &mapper).unwrap());
        place_by_hand(&mut state, &anchor_corner, donor, moved);
    }
    // Named by one corner each, as the tree names them.
    let anchor_name = state.population_name(&anchor_corner).unwrap();
    let mine_name = state.population_name(&mine_corner).unwrap();
    assert_ne!(&*anchor_name, "Memory", "a corner, not the quadrant");
    let rule = (
        RuleTarget::under(mine_name.as_ref(), "TCRgd"),
        follows(
            Some(RuleTarget::under(anchor_name.as_ref(), "CD8")),
            Vec::new(),
        ),
    );
    let outcome = run(&state, &files, store(&[rule]));
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    assert_eq!(
        outcome.placements.len(),
        2,
        "one per donor - not one per corner"
    );
    let after = applied(&state, &outcome);
    let centre = |state: &GateState, corner: &Arc<str>, file: &str| {
        let q = state
            .gate_for_file(corner, &Arc::from(file), &specimens())
            .unwrap();
        q.as_any()
            .downcast_ref::<QuadrantGate>()
            .unwrap()
            .points()
            .center
    };
    for file in ["fs_a", "fs_b"] {
        assert_eq!(
            centre(&after, &mine_corner, file),
            centre(&after, &anchor_corner, file),
            "{file}"
        );
    }
    assert_ne!(
        centre(&after, &mine_corner, "fs_a"),
        centre(&after, &mine_corner, "fs_b")
    );
    // Every corner of the moved quadrant, and the quadrant itself, read the
    // same new position - not the corner the rule named alone.
    let mine = after.registered_gate(&mine_corner).unwrap();
    let mut ids = mine.get_inner_gate_ids();
    ids.push(mine.get_id());
    for id in ids {
        let q = after
            .gate_for_file(&id, &Arc::from("fs_a"), &specimens())
            .unwrap();
        assert_eq!(
            q.as_any()
                .downcast_ref::<QuadrantGate>()
                .unwrap()
                .points()
                .center,
            centre(&after, &anchor_corner, "fs_a"),
            "{id}"
        );
    }
}

#[test]
fn a_rule_that_cannot_follow_says_why_once_and_places_nothing() {
    let files = files("follow-refused");
    let mut state = with_a_copy();
    // Two unrelated gates called "Twin" under different parents.
    add(&mut state, rect("twin1", "Twin", -BIG, -1.0), Some("lymph"));
    add(&mut state, rect("twin2", "Twin", -BIG, -1.0), Some("cd69"));
    let cases: Vec<(GateRule, &str)> = vec![
        (
            follows(Some(RuleTarget::named("Nowhere")), Vec::new()),
            "the gate it follows, Nowhere, is not in the gating",
        ),
        (
            follows(Some(RuleTarget::under("CD69+", "CD3")), Vec::new()),
            "is not drawn under that parent - it is drawn under Lymph",
        ),
        (
            follows(Some(RuleTarget::named("Twin")), Vec::new()),
            "names 2 different gates - name its parent",
        ),
        (
            follows(Some(RuleTarget::named("CD69 copy")), Vec::new()),
            "it names itself",
        ),
        (
            follows(None, Vec::new()),
            "needs same_shape_as, or at least one edge",
        ),
        (
            follows(
                Some(RuleTarget::named("CD69+")),
                vec![edge_from(
                    RuleTarget::named("CD69+"),
                    Y,
                    Side::Lower,
                    Side::Lower,
                    0.0,
                )],
            ),
            "not both",
        ),
    ];
    for (rule, expected) in cases {
        let rules = store(&[(RuleTarget::under("CD69 copy", "Lymph"), rule.clone())]);
        let problems = anchor_problems(&state, &rules);
        assert_eq!(problems.len(), 1, "{expected}: {problems:?}");
        assert!(
            problems[0].reason.contains(expected),
            "{}",
            problems[0].reason
        );
        let outcome = run(&state, &files, rules);
        assert!(outcome.placements.is_empty(), "{expected}");
        let said = reasons(&outcome);
        assert_eq!(said.len(), 1, "once, not per sample: {said:?}");
        assert!(said[0].contains(expected), "{said:?}");
    }
}

#[test]
fn gates_that_follow_each_other_round_a_loop_are_left_alone() {
    let files = files("follow-loop");
    let mut state = with_a_copy();
    add(
        &mut state,
        rect("other", "Other", -BIG, -1.0),
        Some("lymph"),
    );
    let rules = store(&[
        (
            RuleTarget::named("CD69 copy"),
            follows(Some(RuleTarget::named("Other")), Vec::new()),
        ),
        (
            RuleTarget::named("Other"),
            follows(Some(RuleTarget::named("CD69 copy")), Vec::new()),
        ),
    ]);
    let problems = anchor_problems(&state, &rules);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems
            .iter()
            .all(|p| p.reason.contains("lead back to it"))
    );
    assert!(rule_levels(&state, &rules).is_empty());
    let outcome = run(&state, &files, rules);
    assert!(outcome.placements.is_empty());
}

#[test]
fn an_edge_that_cannot_be_set_is_refused_on_that_sample_with_the_reason() {
    let files = files("follow-bad-edge");
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    // Pos is open to the right: it has no upper edge on X.
    add(&mut state, rect("pos", "Pos", 400.0, -BIG), Some("lymph"));
    add(
        &mut state,
        boxed("neg", "Neg", (500.0, 900.0), (-50.0, 950.0)),
        Some("lymph"),
    );
    let open = (
        RuleTarget::named("Neg"),
        follows(
            None,
            vec![edge_from(
                RuleTarget::named("Pos"),
                X,
                Side::Lower,
                Side::Upper,
                0.0,
            )],
        ),
    );
    let said = reasons(&run(&state, &files, store(&[open])));
    assert!(
        said.iter()
            .any(|r| r.contains("the upper edge of Pos on FSC-A is open")),
        "{said:?}"
    );
    // Setting Neg's upper edge to Pos's lower (400) would cross its own lower
    // edge (500).
    let crossing = (
        RuleTarget::named("Neg"),
        follows(
            None,
            vec![edge_from(
                RuleTarget::named("Pos"),
                X,
                Side::Upper,
                Side::Lower,
                0.0,
            )],
        ),
    );
    let said = reasons(&run(&state, &files, store(&[crossing])));
    assert!(
        said.iter()
            .any(|r| r.contains("would put it past its other edge")),
        "{said:?}"
    );
}

// ─── a run that pauses for a person ───────────────────────────────────────────

fn pausing(
    state: &GateState,
    files: &[(Arc<str>, PathBuf)],
    rules: RuleStore,
    from_level: usize,
) -> RunOutcome {
    crate::gate_rules::run::run_rules_pausing(
        state,
        &inputs(files, rules),
        from_level,
        |_| {},
        &AtomicBool::new(false),
    )
}

/// Lymph's rule read on each specimen's FMO, which no specimen has: it can
/// place Lymph on nobody.
fn lymph_rule_with_no_reference() -> (RuleTarget, GateRule) {
    let (target, mut rule) = lymph_rule();
    rule.measured_on = MeasuredOn::Partner(Arc::from("FMO"));
    (target, rule)
}

#[test]
fn a_parent_that_cannot_be_placed_stops_the_run_before_its_child() {
    let files = files("pause-unplaced");
    let state = three_deep();
    let outcome = pausing(
        &state,
        &files,
        store(&[lymph_rule_with_no_reference(), cd69_rule()]),
        0,
    );

    let paused = outcome.paused.expect("CD69+ is measured under Lymph");
    assert_eq!(paused.next_level, 1);
    let mut on: Vec<(&str, &str)> = paused
        .needs
        .iter()
        .map(|n| (&*n.gate, &*n.specimen.as_ref().unwrap().group))
        .collect();
    on.sort();
    assert_eq!(on, [("Lymph", "DONOR-A"), ("Lymph", "DONOR-B")]);
    assert!(paused.needs.iter().all(|n| n.why.contains("FMO")));
    assert!(
        outcome
            .report
            .positioned
            .iter()
            .all(|p| &*p.gate != "CD69+")
            && outcome.report.unchanged.iter().all(|u| &*u.gate != "CD69+"),
        "nothing under the unplaced gate is measured"
    );
}

#[test]
fn the_same_run_without_pausing_goes_on_as_before() {
    let files = files("pause-not-asked");
    let outcome = run(
        &three_deep(),
        &files,
        store(&[lymph_rule_with_no_reference(), cd69_rule()]),
    );
    assert!(outcome.paused.is_none());
    assert!(
        outcome
            .report
            .positioned
            .iter()
            .any(|p| &*p.gate == "CD69+")
    );
}

#[test]
fn a_gate_with_no_ruled_gate_under_it_does_not_stop_the_run() {
    // The Review tab is for those.
    let files = files("pause-leaf");
    let outcome = pausing(
        &three_deep(),
        &files,
        store(&[lymph_rule_with_no_reference()]),
        0,
    );
    assert!(outcome.paused.is_none());
    assert_eq!(outcome.report.unplaced.len(), 2, "one line per specimen");
}

#[test]
fn a_parent_placed_with_low_confidence_stops_the_run_and_a_confident_one_does_not() {
    use crate::gate_rules::run::{PAUSE_BELOW, needing_a_person};
    let files = files("pause-confidence");
    let state = three_deep();
    let rules = store(&[lymph_rule(), cd69_rule()]);
    let below = &rule_levels(&state, &rules)[1..];
    let mut lymph_only = run(&state, &files, store(&[lymph_rule()])).report;
    assert_eq!(lymph_only.positioned.len(), 2);
    for placed in &mut lymph_only.positioned {
        placed.confidence = 0.9;
    }
    assert!(needing_a_person(&state, below, &lymph_only, &rules).is_empty());

    lymph_only.positioned[0].confidence = PAUSE_BELOW - 0.01;
    let needs = needing_a_person(&state, below, &lymph_only, &rules);
    assert_eq!(needs.len(), 1);
    assert_eq!(needs[0].file, lymph_only.positioned[0].file);
    assert!(needs[0].why.contains("0.19"), "{}", needs[0].why);

    lymph_only.positioned[0].confidence = PAUSE_BELOW;
    assert!(needing_a_person(&state, below, &lymph_only, &rules).is_empty());
}

#[test]
fn going_on_after_a_pause_measures_the_child_under_the_parent_as_placed_by_hand() {
    let files = files("pause-resume");
    let state = three_deep();
    let rules = store(&[lymph_rule_with_no_reference(), cd69_rule()]);
    let first = pausing(&state, &files, rules.clone(), 0);
    let paused = first.paused.expect("stopped for Lymph");

    // "By hand": Lymph where its own-sample rule would put it.
    let placed_by_hand = applied(&state, &run(&state, &files, store(&[lymph_rule()])));
    let rest = pausing(&placed_by_hand, &files, rules, paused.next_level);
    assert!(rest.paused.is_none());
    let resumed = applied(&placed_by_hand, &rest);

    let cd69_alone = applied(
        &placed_by_hand,
        &run(&placed_by_hand, &files, store(&[cd69_rule()])),
    );
    for file in ["fs_a", "fs_b"] {
        assert_eq!(
            edge(&resumed, "cd69", Y, file),
            edge(&cd69_alone, "cd69", Y, file)
        );
        assert_eq!(
            edge(&resumed, "lymph", X, file),
            edge(&placed_by_hand, "lymph", X, file)
        );
    }
}

#[test]
fn going_on_after_a_pause_does_not_repeat_what_the_whole_run_said() {
    let files = files("pause-said-once");
    let state = three_deep();
    let nowhere = (RuleTarget::named("Nowhere"), top(X, (0.1, 0.2)));
    let rules = store(&[lymph_rule(), cd69_rule(), nowhere]);
    let says_nowhere =
        |outcome: &RunOutcome| outcome.report.skipped.iter().any(|s| &*s.gate == "Nowhere");
    assert!(says_nowhere(&pausing(&state, &files, rules.clone(), 0)));
    assert!(!says_nowhere(&pausing(&state, &files, rules, 1)));
}

#[test]
fn a_paused_run_and_the_rest_of_it_are_kept_as_one_run() {
    let files = files("pause-joined");
    let state = three_deep();
    let rules = store(&[lymph_rule_with_no_reference(), cd69_rule()]);
    let first = pausing(&state, &files, rules.clone(), 0);
    let (skipped_first, events_first) = (first.report.skipped.len(), first.events.samples.len());
    let placed_by_hand = applied(&state, &run(&state, &files, store(&[lymph_rule()])));
    let rest = pausing(&placed_by_hand, &files, rules, 1);
    let (positioned_rest, placements_rest, events_rest) = (
        rest.report.positioned.len(),
        rest.placements.len(),
        rest.events.samples.len(),
    );
    assert!(positioned_rest > 0 && events_first > 0 && events_rest > 0);

    let whole = first.then(rest);
    assert!(whole.paused.is_none(), "the rest finished");
    assert_eq!(whole.report.positioned.len(), positioned_rest);
    assert_eq!(whole.placements.len(), placements_rest);
    assert_eq!(
        whole.report.skipped.len(),
        skipped_first,
        "the rest skipped nothing"
    );
    assert_eq!(whole.events.samples.len(), events_first + events_rest);
    assert!(whole.report.unplaced.iter().any(|u| &*u.gate == "Lymph"));
}

#[test]
fn a_gate_held_for_placing_is_where_it_was_and_moves_for_its_specimen_alone() {
    use crate::gates::gate_store::GateSource;
    let files = files("pause-hold");
    let mut state = three_deep();
    let outcome = pausing(
        &state,
        &files,
        store(&[lymph_rule_with_no_reference(), cd69_rule()]),
        0,
    );
    let needs: Vec<_> = outcome
        .paused
        .unwrap()
        .needs
        .into_iter()
        .filter(|n| &*n.file == "fs_a")
        .collect();
    let before = (
        edge(&state, "lymph", X, "fs_a"),
        edge(&state, "lymph", X, "fs_b"),
    );

    crate::gate_rules::run::hold_for_placing(&mut state, &needs, &specimens());

    assert_eq!(
        (
            edge(&state, "lymph", X, "fs_a"),
            edge(&state, "lymph", X, "fs_b")
        ),
        before
    );
    let source = |file: &str| {
        state
            .gate_and_source_for_file(&Arc::from("lymph"), &Arc::from(file), &specimens())
            .unwrap()
            .0
    };
    assert!(
        matches!(source("fs_a"), GateSource::Group((_, ref key)) if &*key.group == "DONOR-A"),
        "a drag on fs_a is saved to DONOR-A's own position"
    );
    assert!(
        matches!(source("fs_b"), GateSource::Group((_, ref key)) if &*key.group == "DONOR-B"),
        "fs_b has a position of its own, where it was"
    );
}

// ─── who a pause names ────────────────────────────────────────────────────────

/// Lymph's rule on a channel no file has: it cannot be measured anywhere.
fn lymph_rule_on_a_missing_channel() -> (RuleTarget, GateRule) {
    let (target, mut rule) = lymph_rule();
    rule.parameter = Arc::from("No-Such-Channel");
    (target, rule)
}

fn files_named(name: &str, names: &[&str]) -> Vec<(Arc<str>, PathBuf)> {
    let dir = scratch(name);
    let channels = [(X, None), (Y, None)];
    names
        .iter()
        .enumerate()
        .map(|(seed, file)| {
            let path = dir.join(format!("{file}.fcs"));
            write_fcs_rows(&path, &channels, &events(seed as u64 + 1), &[]);
            (Arc::from(format!("{file}.fcs")), path)
        })
        .collect()
}

fn inputs_for(
    files: &[(Arc<str>, PathBuf)],
    metadata: crate::omiq::metadata::MetaDataFileMap,
    rules: RuleStore,
) -> RunInputs {
    RunInputs {
        files: files.to_vec(),
        compensation: crate::compensation::groups::Compensation::default(),
        names: files
            .iter()
            .map(|(name, _)| (name.clone(), Arc::from(name.trim_end_matches(".fcs"))))
            .collect::<std::collections::HashMap<_, _, FxBuildHasher>>(),
        cofactors: Vec::new(),
        metadata,
        rules,
    }
}

fn row(columns: &[(&str, &str)]) -> rustc_hash::FxHashMap<Arc<str>, Arc<str>> {
    columns
        .iter()
        .map(|(column, value)| (Arc::from(*column), Arc::from(*value)))
        .collect()
}

#[test]
fn a_file_with_no_sample_id_is_named_by_itself_and_shows_the_gate_as_drawn() {
    use crate::gates::gate_store::GateSource;
    let files = files_named("pause-no-id", &["fs_a", "loose", "loose_2"]);
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    metadata.insert(
        Arc::from("fs_a") as Arc<str>,
        row(&[("SampleID", "DONOR-A"), ("SampleType", "FS")]),
    );
    for loose in ["loose", "loose_2"] {
        metadata.insert(Arc::from(loose) as Arc<str>, row(&[("SampleType", "FS")]));
    }
    let mut state = three_deep();
    let outcome = crate::gate_rules::run::run_rules_pausing(
        &state,
        &inputs_for(
            &files,
            metadata.clone(),
            store(&[lymph_rule_on_a_missing_channel(), cd69_rule()]),
        ),
        0,
        |_| {},
        &AtomicBool::new(false),
    );
    let needs = outcome.paused.expect("Lymph was measured nowhere").needs;
    let mut named: Vec<(String, Option<String>)> = needs
        .iter()
        .map(|n| {
            (
                n.file.to_string(),
                n.specimen.as_ref().map(|s| s.group.to_string()),
            )
        })
        .collect();
    named.sort();
    assert_eq!(
        named,
        [
            ("fs_a".to_string(), Some("DONOR-A".to_string())),
            ("loose".to_string(), None),
            ("loose_2".to_string(), None)
        ]
    );

    crate::gate_rules::run::hold_for_placing(&mut state, &needs, &metadata);
    let source = state
        .gate_and_source_for_file(&Arc::from("lymph"), &Arc::from("loose"), &metadata)
        .unwrap()
        .0;
    // The gate is held by SampleID, which the file has no value in.
    assert!(matches!(source, GateSource::Global), "{source:?}");
}

#[test]
fn a_specimen_with_several_files_is_named_once_on_the_file_to_place_it_on() {
    // Each full stain sorts before its FMO, so the order the files are read
    // in does not pick it; DONOR-C has two, and the first is the one read.
    let files = files_named(
        "pause-one-each",
        &["a_1", "a_2", "b_1", "b_2", "c_1", "c_2", "c_3"],
    );
    let mut metadata = im::HashMap::with_hasher(FxBuildHasher);
    for (file, donor, kind) in [
        ("a_1", "DONOR-A", "FS"),
        ("a_2", "DONOR-A", "FMO"),
        ("b_1", "DONOR-B", "FS"),
        ("b_2", "DONOR-B", "FMO"),
        ("c_1", "DONOR-C", "FS"),
        ("c_2", "DONOR-C", "FMO"),
        ("c_3", "DONOR-C", "FS"),
    ] {
        metadata.insert(
            Arc::from(file) as Arc<str>,
            row(&[("SampleID", donor), ("SampleType", kind)]),
        );
    }
    let outcome = crate::gate_rules::run::run_rules_pausing(
        &three_deep(),
        &inputs_for(
            &files,
            metadata,
            store(&[lymph_rule_on_a_missing_channel(), cd69_rule()]),
        ),
        0,
        |_| {},
        &AtomicBool::new(false),
    );
    let mut named: Vec<(String, String)> = outcome
        .paused
        .expect("Lymph was measured nowhere")
        .needs
        .iter()
        .map(|n| {
            (
                n.specimen.as_ref().unwrap().group.to_string(),
                n.file.to_string(),
            )
        })
        .collect();
    named.sort();
    assert_eq!(
        named,
        [
            ("DONOR-A".to_string(), "a_1".to_string()),
            ("DONOR-B".to_string(), "b_1".to_string()),
            ("DONOR-C".to_string(), "c_1".to_string())
        ],
        "one each, on the full stain rather than the FMO"
    );
}

#[test]
fn two_gates_missed_on_one_specimen_are_each_named() {
    let files = files("pause-two-gates");
    let mut state = GateState::default();
    // A and B on plots of their own, so neither waits for the other.
    add(&mut state, rect("a", "A", -1.0, -BIG), None);
    add(&mut state, rect("p", "P", -1.0, -BIG), None);
    add(&mut state, rect("b", "B", -1.0, -BIG), Some("p"));
    add(
        &mut state,
        rect("under_a", "Under A", -BIG, -1.0),
        Some("a"),
    );
    add(
        &mut state,
        rect("under_b", "Under B", -BIG, -1.0),
        Some("b"),
    );
    let rules = store(&[
        (RuleTarget::named("A"), top("No-Such-Channel", (0.49, 0.51))),
        (RuleTarget::named("B"), top("No-Such-Channel", (0.49, 0.51))),
        (RuleTarget::under("Under A", "A"), top(Y, (0.19, 0.21))),
        (RuleTarget::under("Under B", "B"), top(Y, (0.19, 0.21))),
    ]);
    let outcome = crate::gate_rules::run::run_rules_pausing(
        &state,
        &inputs(&files, rules),
        0,
        |_| {},
        &AtomicBool::new(false),
    );
    let mut named: Vec<(String, String)> = outcome
        .paused
        .expect("neither A nor B was measured")
        .needs
        .iter()
        .map(|n| (n.gate.to_string(), n.file.to_string()))
        .collect();
    named.sort();
    assert_eq!(
        named,
        [
            ("A".to_string(), "fs_a".to_string()),
            ("A".to_string(), "fs_b".to_string()),
            ("B".to_string(), "fs_a".to_string()),
            ("B".to_string(), "fs_b".to_string())
        ]
    );
}

// ─── a valley rule's fallback ─────────────────────────────────────────────────
//
// X is one peak - a smear with no dip - so a valley rule on it finds
// nothing, and one with a fallback puts its edge where the same gate's is
// under another parent.

/// fs_a and fs_b with X one normal peak, and Y the same.
fn smears(name: &str) -> Vec<(Arc<str>, PathBuf)> {
    let dir = scratch(name);
    let channels = [(X, None), (Y, None)];
    ["fs_a", "fs_b"]
        .iter()
        .enumerate()
        .map(|(seed, file)| {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64 + 1);
            let x = rand_distr::Normal::new(500.0f32, 100.0).unwrap();
            let rows: Vec<Vec<f32>> = (0..20_000)
                .map(|_| {
                    let v = x.sample(&mut rng);
                    vec![v, v]
                })
                .collect();
            let path = dir.join(format!("{file}.fcs"));
            write_fcs_rows(&path, &channels, &rows, &[]);
            (Arc::from(format!("{file}.fcs")), path)
        })
        .collect()
}

/// IFNy+ under A and under B, both open from -1 on X.
fn ifng_twice() -> GateState {
    let mut state = GateState::default();
    add(&mut state, rect("a", "A", -1.0, -BIG), None);
    add(&mut state, rect("b", "B", -1.0, -BIG), None);
    add(&mut state, rect("ifng_a", "IFNy+", -1.0, -BIG), Some("a"));
    add(&mut state, rect("ifng_b", "IFNy+", -1.0, -BIG), Some("b"));
    state
}

fn valley_on_b(fallback: Option<RuleTarget>) -> (RuleTarget, GateRule) {
    (
        RuleTarget::under("IFNy+", "B"),
        GateRule {
            parameter: Arc::from(X),
            bound: Bound::Above,
            measured_on: MeasuredOn::File(Arc::from("fs_a")),
            rule: Rule::InTheValley(crate::gate_rules::rule::ValleyRule {
                fallback,
                ..Default::default()
            }),
        },
    )
}

fn band_on_a() -> (RuleTarget, GateRule) {
    (RuleTarget::under("IFNy+", "A"), top(X, (0.19, 0.21)))
}

#[test]
fn a_valley_rule_with_no_dip_takes_its_fallback_s_edge_after_the_fallback_s_rule() {
    use crate::gate_rules::autogate::FLAGGED_CONFIDENCE;
    use crate::gate_rules::confidence::FALLBACK;
    let files = smears("valley-fallback");
    let state = ifng_twice();
    let fallback = Some(RuleTarget::under("IFNy+", "A"));
    // Listed before the rule it falls back to.
    let rules = store(&[valley_on_b(fallback), band_on_a()]);
    assert_eq!(level_names(&state, &rules), [["IFNy+"], ["IFNy+"]]);

    let outcome = run(&state, &files, rules);
    assert!(outcome.report.skipped.is_empty(), "{:?}", reasons(&outcome));
    let after = applied(&state, &outcome);
    // By hand: the band rule applied, then its edge read off.
    let by_hand = applied(&state, &run(&state, &files, store(&[band_on_a()])));
    let theirs = extent(&by_hand, "ifng_a", X, "fs_b").0;
    assert!(theirs > 500.0, "the band rule moved it, to {theirs}");
    assert_eq!(extent(&after, "ifng_b", X, "fs_b").0, theirs);
    assert_eq!(
        extent(&after, "ifng_b", Y, "fs_b"),
        extent(&state, "ifng_b", Y, "fs_b"),
        "only the edge the valley sets"
    );
    // The reference is gated by hand, and stays as it was drawn.
    assert_eq!(
        extent(&after, "ifng_b", X, "fs_a"),
        extent(&state, "ifng_b", X, "fs_a")
    );
    assert!(
        outcome
            .report
            .reference
            .iter()
            .any(|r| &*r.gate_id == "ifng_b" && &*r.file == "fs_a")
    );
    let placed: Vec<_> = outcome
        .report
        .positioned
        .iter()
        .filter(|p| &*p.gate_id == "ifng_b")
        .collect();
    assert_eq!(placed.len(), 1, "the specimen that is not the reference");
    for p in placed {
        assert_eq!(p.confidence, 0.25);
        assert_eq!(p.weakest, Some(FALLBACK));
        assert!(
            p.components[0].detail.contains("IFNy+ of A"),
            "{}",
            p.components[0].detail
        );
    }
    assert!(
        crate::gate_rules::run::PAUSE_BELOW <= FLAGGED_CONFIDENCE
            && FLAGGED_CONFIDENCE < crate::review::assess::REVIEW_FLOOR,
        "reviewed, but no pause"
    );
}

#[test]
fn a_valley_rule_with_no_dip_and_no_fallback_is_left_unplaced() {
    let files = smears("valley-no-fallback");
    let state = ifng_twice();
    let outcome = run(&state, &files, store(&[valley_on_b(None), band_on_a()]));
    assert!(
        outcome
            .report
            .positioned
            .iter()
            .all(|p| &*p.gate_id != "ifng_b")
    );
    assert!(
        reasons(&outcome).iter().any(|r| r.starts_with("IFNy+ |")),
        "{:?}",
        reasons(&outcome)
    );
}

#[test]
fn a_fallback_that_cannot_give_an_edge_says_both_why() {
    let files = smears("valley-fallback-open");
    let mut state = ifng_twice();
    add(&mut state, rect("open", "Open", -BIG, -BIG), Some("a"));
    let rules = store(&[valley_on_b(Some(RuleTarget::under("Open", "A")))]);
    let outcome = run(&state, &files, rules);
    let said = reasons(&outcome);
    assert!(
        said.iter()
            .any(|r| r.contains("its fallback failed") && r.contains("is open")),
        "{said:?}"
    );
}

#[test]
fn a_fallback_naming_itself_or_no_gate_is_refused_before_the_run() {
    use crate::gate_rules::autogate::anchor_problems;
    let state = ifng_twice();
    for (fallback, says) in [
        (RuleTarget::under("IFNy+", "B"), "names itself"),
        (RuleTarget::under("Nothing", "A"), "Nothing"),
    ] {
        let problems = anchor_problems(&state, &store(&[valley_on_b(Some(fallback))]));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].reason.contains(says), "{}", problems[0].reason);
    }
}

// ─── never over another gate on the plot ──────────────────────────────────────
//
// X is uniform over 0-1000 under Lymph: the top 80% starts at 200. Neg holds
// everything up to 400 on the same plot.

/// Lymph, and under it Neg up to 400 and Pos from 600, side by side.
fn neg_and_pos() -> GateState {
    let mut state = GateState::default();
    add(&mut state, rect("lymph", "Lymph", -1.0, -BIG), None);
    add(
        &mut state,
        boxed("neg", "Neg", (-BIG, 400.0), (-BIG, BIG)),
        Some("lymph"),
    );
    add(&mut state, rect("pos", "Pos", 600.0, -BIG), Some("lymph"));
    state
}

fn pos_to_the_top_80() -> (RuleTarget, GateRule) {
    (RuleTarget::under("Pos", "Lymph"), top(X, (0.79, 0.81)))
}

/// Neg's upper edge where it keeps the bottom half: 500.
fn neg_to_the_bottom_half() -> (RuleTarget, GateRule) {
    (
        RuleTarget::under("Neg", "Lymph"),
        GateRule {
            bound: Bound::Below,
            ..top(X, (0.49, 0.51))
        },
    )
}

fn upper(state: &GateState, gate: &str, file: &str) -> f32 {
    extent(state, gate, X, file).1
}

#[test]
fn a_line_rule_is_held_back_where_its_gate_meets_one_no_rule_moves() {
    use crate::gate_rules::autogate::FLAGGED_CONFIDENCE;
    use crate::gate_rules::confidence::HELD_BACK;
    let files = files("clear-held");
    let state = neg_and_pos();
    let outcome = run(&state, &files, store(&[pos_to_the_top_80()]));
    let after = applied(&state, &outcome);
    for file in ["fs_a", "fs_b"] {
        assert_eq!(edge(&after, "pos", X, file), 400.0, "{file}: Neg's edge");
    }
    for placed in &outcome.report.positioned {
        let held = placed
            .components
            .iter()
            .find(|c| c.name == HELD_BACK)
            .expect("said to be held back");
        assert_eq!(held.score, FLAGGED_CONFIDENCE);
        assert!(held.detail.contains("Neg"), "{}", held.detail);
        assert!(placed.confidence <= FLAGGED_CONFIDENCE);
        assert!((placed.to - 400.0).abs() < 1e-3, "{}", placed.to);
        assert!(
            (placed.achieved - 0.6).abs() < 0.02,
            "what it holds from 400: {}",
            placed.achieved
        );
    }
}

#[test]
fn a_gate_beside_one_listed_first_is_held_against_where_that_one_went() {
    let files = files("clear-order");
    let state = neg_and_pos();
    let rules = store(&[neg_to_the_bottom_half(), pos_to_the_top_80()]);
    assert_eq!(level_names(&state, &rules), [["Neg"], ["Pos"]]);
    let after = applied(&state, &run(&state, &files, rules));
    for file in ["fs_a", "fs_b"] {
        let neg = upper(&after, "neg", file);
        assert!((neg - 500.0).abs() < 15.0, "{file}: Neg to {neg}");
        assert_eq!(edge(&after, "pos", X, file), neg, "{file}");
    }

    // Listed the other way round, Pos goes first and Neg is held at it.
    let rules = store(&[pos_to_the_top_80(), neg_to_the_bottom_half()]);
    assert_eq!(level_names(&state, &rules), [["Pos"], ["Neg"]]);
    let after = applied(&state, &run(&state, &files, rules));
    for file in ["fs_a", "fs_b"] {
        let pos = edge(&after, "pos", X, file);
        assert!((pos - 200.0).abs() < 15.0, "{file}: Pos to {pos}");
        assert_eq!(upper(&after, "neg", file), pos, "{file}");
    }
}

#[test]
fn a_copy_that_would_lie_over_another_gate_is_left_where_it_was() {
    let files = files("clear-copy");
    let state = with_a_copy();
    let outcome = run(&state, &files, store(&[copy_rule()]));
    assert!(outcome.placements.is_empty());
    let said = reasons(&outcome);
    assert!(
        said.iter().all(|r| r.contains("would overlap CD69+")),
        "{said:?}"
    );
    assert_eq!(said.len(), 2, "{said:?}");
}
