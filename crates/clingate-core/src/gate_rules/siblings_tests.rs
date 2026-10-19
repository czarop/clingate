//! Copying the rules under one population to the same gates under another.
//! Every expectation is read off the tree built here by hand: which gates each
//! parent has, and what each is drawn on.

use std::sync::Arc;

use crate::gate_rules::rule::{FromGateRule, Rule, TailFractionRule, ValleyOrSmearRule};
use crate::gate_rules::rule_store::{Bound, GateRule, MeasuredOn, RuleStore, RuleTarget};
use crate::gate_rules::siblings::copy_rules;
use crate::gates::GateState;
use crate::gates::gate_store::{GateSource, NodeId};
use crate::gates::gate_traits::DrawableGate;

fn rect(id: &str, name: &str, x: &str, y: &str) -> Arc<dyn DrawableGate> {
    let geometry = flow_gates::create_rectangle_geometry(
        vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)],
        x,
        y,
    )
    .unwrap();
    Arc::new(
        crate::gates::gate_single::rectangle_gate::RectangleGate::try_new(
            flow_gates::Gate {
                id: Arc::from(id),
                name: name.into(),
                geometry,
                mode: flow_gates::GateMode::Global,
                parameters: (Arc::from(x), Arc::from(y)),
                label_position: None,
            },
            true,
        )
        .unwrap(),
    )
}

fn add(state: &mut GateState, id: &str, name: &str, axes: (&str, &str), parent: Option<&str>) {
    let gate = rect(id, name, axes.0, axes.1);
    let id: Arc<str> = Arc::from(id);
    state.place_gate(&[id.clone()], &gate, &GateSource::Global);
    state.place_new_gate(parent.map(Arc::from), id).unwrap();
}

/// A and B side by side. A has X and Y on CD3 and CD4, and Z; B has X on the
/// same axes and Y on CD8 and CD4, and no Z.
fn tree() -> GateState {
    let mut state = GateState::default();
    add(&mut state, "a", "A", ("FSC-A", "SSC-A"), None);
    add(&mut state, "b", "B", ("FSC-A", "SSC-A"), None);
    add(&mut state, "ax", "X", ("CD3", "CD4"), Some("a"));
    add(&mut state, "ay", "Y", ("CD3", "CD4"), Some("a"));
    add(&mut state, "az", "Z", ("CD3", "CD4"), Some("a"));
    add(&mut state, "bx", "X", ("CD3", "CD4"), Some("b"));
    add(&mut state, "by", "Y", ("CD8", "CD4"), Some("b"));
    state
}

fn node(state: &GateState, id: &str) -> NodeId {
    state.nodes_for_gate(&Arc::from(id))[0].clone()
}

fn band(parameter: &str, low: f64) -> GateRule {
    GateRule {
        parameter: parameter.into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::Partner("FMX".into()),
        rule: Rule::TailFraction(TailFractionRule::new((low, low * 2.0))),
    }
}

fn shape_of(gate: &str, parent: &str) -> GateRule {
    GateRule {
        parameter: "".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::Itself,
        rule: Rule::FromAnotherGate(FromGateRule {
            same_shape_as: Some(RuleTarget::under(gate, parent)),
            ..FromGateRule::default()
        }),
    }
}

fn copied_to(copy: &crate::gate_rules::siblings::Copy) -> Vec<String> {
    copy.copied.iter().map(|c| c.target.describe()).collect()
}

fn left_out(copy: &crate::gate_rules::siblings::Copy) -> Vec<(String, String)> {
    copy.not_copied
        .iter()
        .map(|n| (n.target.describe(), n.reason.clone()))
        .collect()
}

#[test]
fn each_rule_under_a_parent_is_copied_to_the_gate_of_its_name_under_the_other() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("CD3", 0.01));
    rules.insert(RuleTarget::under("Z", "A"), band("CD3", 0.02));
    // Written for every Y: it reaches B's already, so it is not A's to copy.
    rules.insert(RuleTarget::named("Y"), band("CD3", 0.03));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert_eq!(copied_to(&copy), ["X of B"]);
    assert_eq!(copy.copied[0].rule, band("CD3", 0.01));
    assert_eq!(copy.copied[0].replaces, None);
    assert_eq!(
        left_out(&copy),
        [("Z of B".to_string(), "B has no gate called Z".to_string())]
    );
    let written = copy.written_into(&rules);
    assert_eq!(
        written.get(&RuleTarget::under("X", "B")),
        Some(&band("CD3", 0.01))
    );
    assert_eq!(
        written.get(&RuleTarget::under("X", "A")),
        Some(&band("CD3", 0.01))
    );
}

#[test]
fn a_gate_not_drawn_on_the_rule_s_parameter_is_left_out() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("Y", "A"), band("CD3", 0.01));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert!(copy.copied.is_empty());
    assert_eq!(
        left_out(&copy),
        [(
            "Y of B".to_string(),
            "the gate is drawn on CD8 and CD4, not CD3".to_string()
        )]
    );
}

/// [`tree`], with a Z under B drawn as A's, and a C at the top with an X.
fn tree_with_z_under_b_and_c() -> GateState {
    let mut state = tree();
    add(&mut state, "bz", "Z", ("CD3", "CD4"), Some("b"));
    add(&mut state, "c", "C", ("FSC-A", "SSC-A"), None);
    add(&mut state, "cx", "X", ("CD3", "CD4"), Some("c"));
    state
}

#[test]
fn a_gate_read_under_the_old_parent_is_read_under_the_new_one_and_elsewhere_stays() {
    let state = tree_with_z_under_b_and_c();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), shape_of("Z", "A"));
    let elsewhere = GateRule {
        parameter: "CD3".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::File("qc".into()),
        rule: Rule::ValleyOrSmear(ValleyOrSmearRule {
            fallback: Some(RuleTarget::under("X", "C")),
            ..ValleyOrSmearRule::default()
        }),
    };
    let mut other = rules.clone();
    other.insert(RuleTarget::under("X", "A"), elsewhere.clone());

    let into_b = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );
    let kept = copy_rules(
        &state,
        &other,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert_eq!(left_out(&into_b), []);
    assert_eq!(into_b.copied[0].rule, shape_of("Z", "B"));
    assert_eq!(left_out(&kept), []);
    assert_eq!(
        kept.copied[0].rule, elsewhere,
        "the fallback is under neither"
    );
}

#[test]
fn the_gates_a_next_to_or_an_edge_rule_reads_are_read_under_the_new_parent() {
    use crate::gate_rules::rule::{EdgeFrom, NextToRule, Side};
    let state = tree_with_z_under_b_and_c();
    let next_to = |parent: &str| GateRule {
        parameter: "".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::Itself,
        rule: Rule::NextToGate(NextToRule {
            anchor: RuleTarget::under("Z", parent),
            parameter: "CD3".into(),
            side: Side::Lower,
            meet: Default::default(),
            gap: 0.0,
        }),
    };
    let edge = |parent: &str| GateRule {
        parameter: "".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::Itself,
        rule: Rule::FromAnotherGate(FromGateRule {
            same_shape_as: None,
            edges: vec![EdgeFrom {
                anchor: RuleTarget::under("Z", parent),
                parameter: "CD3".into(),
                side: Side::Lower,
                anchor_side: Side::Upper,
                gap: 0.0,
            }],
        }),
    };
    let (a, b) = (node(&state, "a"), node(&state, "b"));

    for (rule, expected) in [(next_to("A"), next_to("B")), (edge("A"), edge("B"))] {
        let mut rules = RuleStore::default();
        rules.insert(RuleTarget::under("X", "A"), rule);

        let copy = copy_rules(&state, &rules, &a, &b, false);

        assert_eq!(left_out(&copy), []);
        assert_eq!(copy.copied[0].rule, expected);
    }
}

#[test]
fn a_copy_update_rule_would_refuse_is_left_out_with_its_reason() {
    let state = tree();
    let mut rules = RuleStore::default();
    // Y is drawn on CD3 and CD4 under A, as X is; under B on CD8 and CD4.
    rules.insert(RuleTarget::under("X", "A"), shape_of("Y", "A"));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert!(copy.copied.is_empty());
    assert_eq!(
        left_out(&copy),
        [(
            "X of B".to_string(),
            "Y of B is drawn on CD8 and CD4, and X of B on CD3 and CD4, so it cannot take that \
             shape"
                .to_string()
        )]
    );
}

#[test]
fn a_copy_that_would_give_one_gate_drawn_in_two_places_two_rules_is_left_out() {
    let mut state = tree_with_z_under_b_and_c();
    // B's X is drawn under C too, as one gate.
    let x_of_c = node(&state, "cx");
    state
        .link_node_to_gate(&x_of_c, &node(&state, "bx"))
        .unwrap();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("CD3", 0.01));
    rules.insert(RuleTarget::under("X", "C"), band("CD3", 0.05));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert!(copy.copied.is_empty(), "{:?}", copy.copied);
    let reasons = left_out(&copy);
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].1.contains("X of B") && reasons[0].1.contains("X of C"),
        "{reasons:?}"
    );
}

#[test]
fn a_rule_for_every_gate_of_the_name_is_kept_unless_replaced() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("CD3", 0.01));
    rules.insert(RuleTarget::named("X"), band("CD3", 0.05));
    let (a, b) = (node(&state, "a"), node(&state, "b"));

    let kept = copy_rules(&state, &rules, &a, &b, false);
    let replaced = copy_rules(&state, &rules, &a, &b, true);

    assert!(kept.copied.is_empty());
    assert!(
        left_out(&kept)[0]
            .1
            .starts_with("already reached by the rule for every X gate, kept: "),
        "{:?}",
        left_out(&kept)
    );
    assert_eq!(copied_to(&replaced), ["X of B"]);
    assert_eq!(replaced.copied[0].replaces, Some(band("CD3", 0.05)));
}

#[test]
fn a_phenotype_rule_moves_no_one_parameter_but_its_pinned_edges_must_be_on_the_gate() {
    use crate::gate_rules::rule::PhenotypeRule;
    let state = tree();
    let phenotype = |pinned: &[&str]| GateRule {
        parameter: "SSC-A".into(),
        bound: Bound::Above,
        measured_on: MeasuredOn::File("qc".into()),
        rule: Rule::MatchThePhenotype(PhenotypeRule {
            pinned: pinned.iter().map(|m| Arc::from(*m)).collect(),
            ..PhenotypeRule::default()
        }),
    };
    let (a, b) = (node(&state, "a"), node(&state, "b"));
    let copy_of = |rule: GateRule| {
        let mut rules = RuleStore::default();
        rules.insert(RuleTarget::under("Y", "A"), rule);
        copy_rules(&state, &rules, &a, &b, false)
    };

    let unpinned = copy_of(phenotype(&[]));
    let pinned = copy_of(phenotype(&["CD3"]));

    assert_eq!(copied_to(&unpinned), ["Y of B"]);
    assert_eq!(
        left_out(&pinned),
        [(
            "Y of B".to_string(),
            "Y of B is drawn on CD8 and CD4, so it has no edge on CD3 to pin".to_string()
        )]
    );
}

#[test]
fn a_rule_with_no_parameter_where_it_needs_one_is_left_out() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("", 0.01));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert_eq!(
        left_out(&copy),
        [(
            "X of B".to_string(),
            "X of B is drawn on CD3 and CD4, so a rule for it slides its edge along one of \
             those - not "
                .to_string()
        )]
    );
}

#[test]
fn a_gate_read_that_the_new_parent_lacks_leaves_the_rule_out() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), shape_of("Z", "A"));

    let copy = copy_rules(
        &state,
        &rules,
        &node(&state, "a"),
        &node(&state, "b"),
        false,
    );

    assert_eq!(
        left_out(&copy),
        [(
            "X of B".to_string(),
            "it reads Z, which B does not have".to_string()
        )]
    );
}

#[test]
fn a_gate_s_own_rule_is_kept_unless_replaced_and_the_same_rule_is_not_copied_again() {
    let state = tree();
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("CD3", 0.01));
    rules.insert(RuleTarget::under("X", "B"), band("CD3", 0.05));
    let (a, b) = (node(&state, "a"), node(&state, "b"));

    let kept = copy_rules(&state, &rules, &a, &b, false);
    let replaced = copy_rules(&state, &rules, &a, &b, true);
    let again = copy_rules(&state, &replaced.written_into(&rules), &a, &b, true);

    assert!(kept.copied.is_empty());
    assert!(
        left_out(&kept)[0]
            .1
            .starts_with("already has a rule of its own, kept: ")
    );
    assert_eq!(copied_to(&replaced), ["X of B"]);
    assert_eq!(replaced.copied[0].replaces, Some(band("CD3", 0.05)));
    assert_eq!(
        left_out(&again),
        [("X of B".to_string(), "already has this rule".to_string())]
    );
}

#[test]
fn a_copy_is_offered_from_a_population_with_rules_to_those_sharing_a_gate_name() {
    use crate::gate_rules::siblings::{populations_with_rules, siblings_of};
    let mut state = tree();
    add(&mut state, "c", "C", ("FSC-A", "SSC-A"), None);
    add(&mut state, "cw", "W", ("CD3", "CD4"), Some("c"));
    let mut rules = RuleStore::default();
    rules.insert(RuleTarget::under("X", "A"), band("CD3", 0.01));
    rules.insert(RuleTarget::named("W"), band("CD3", 0.01));
    let names = |found: Vec<(NodeId, Arc<str>)>| -> Vec<String> {
        found
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect()
    };

    assert_eq!(names(populations_with_rules(&state, &rules)), ["A"]);
    assert_eq!(names(siblings_of(&state, &node(&state, "a"))), ["B"]);
    assert_eq!(
        names(siblings_of(&state, &node(&state, "c"))),
        Vec::<String>::new()
    );
}
