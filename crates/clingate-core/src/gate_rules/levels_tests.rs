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
    apply_placements(&mut after, &outcome.placements);
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
