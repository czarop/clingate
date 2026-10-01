//! Tests for what a rule can be written against.

#![cfg(test)]

use crate::axis_store::PlotMapper;
use crate::gate_rules::choices::choices;
use crate::gates::GateState;
use crate::gates::gate_store::{GateStateImplExt, ROOTGATE};
use crate::gates::gate_types::PrimaryGateType;
use flow_fcs::TransformType;
use std::sync::Arc;

const X: &str = "FSC-A";
const Y: &str = "SSC-A";

fn mapper() -> PlotMapper {
    PlotMapper::new(
        600.0,
        600.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        0.0..=1000.0,
        TransformType::Linear,
        TransformType::Linear,
    )
}

fn add(
    state: &mut GateState,
    name: &str,
    kind: PrimaryGateType,
    parent: Option<Arc<str>>,
) -> Arc<str> {
    let before: Vec<Arc<str>> = state
        .placements()
        .map(|(n, _)| n.as_arc().clone())
        .collect();
    state
        .add_gate(
            &mapper(),
            300.0,
            300.0,
            Arc::from(X),
            Arc::from(Y),
            // Polygons need their points; the other kinds derive theirs.
            Some(vec![(100.0, 100.0), (300.0, 100.0), (200.0, 300.0)]),
            parent,
            kind,
            Some(name.to_string()),
        )
        .expect("a gate can be added");
    state
        .placements()
        .map(|(n, p)| (n.as_arc().clone(), p.gate_id.clone()))
        .find(|(n, _)| !before.contains(n))
        .map(|(_, id)| id)
        .expect("the new placement")
}

fn everything_offered(state: &GateState) -> Vec<String> {
    let c = choices(state);
    c.children
        .iter()
        .flat_map(|(_, kids)| kids.iter())
        .map(|k| k.to_string())
        .collect()
}

#[test]
fn a_quadrants_container_is_not_offered_but_its_corners_are() {
    // The container carries the composite group's id as its name, so it shows
    // up in a list as something like `Mzk4` - not a gate anyone drew, and not
    // one a rule positioning a single line could act on.
    let mut state = GateState::default();
    let parent = add(
        &mut state,
        "CD4+CD8-",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    let quad = add(
        &mut state,
        "Q",
        PrimaryGateType::Quadrant,
        Some(parent.clone()),
    );

    let composite = state.registered_gate(&quad).expect("the quadrant");
    assert!(
        composite.is_composite(),
        "the fixture should be a composite"
    );

    let offered = everything_offered(&state);
    assert!(
        !offered.contains(&composite.get_name().to_string()),
        "the composite container should not be offered, got {offered:?}"
    );
}

#[test]
fn an_ordinary_gate_is_still_offered() {
    // The exclusion has to be narrow: a plain gate under a parent stays.
    let mut state = GateState::default();
    let parent = add(
        &mut state,
        "CD4+",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    let _ = add(
        &mut state,
        "Ki67+",
        PrimaryGateType::Rectangle,
        Some(parent),
    );

    assert!(everything_offered(&state).contains(&"Ki67+".to_string()));
}

#[test]
fn a_gate_at_the_root_is_not_offered() {
    // Nothing to be a fraction of.
    let mut state = GateState::default();
    let _ = add(
        &mut state,
        "Cells",
        PrimaryGateType::Polygon,
        Some(ROOTGATE.clone()),
    );
    assert!(everything_offered(&state).is_empty());
}

// ── carrying a rule's gate across a change of population ──────────────────
//
// The Edit button exists so one rule can be moved onto a second population
// without retyping it, which only pays off if the gate survives the move.

use crate::gate_rules::choices::{GateChoices, carry_over};

fn two_populations() -> GateChoices {
    GateChoices {
        parents: vec![Arc::from("CD4+"), Arc::from("CD8+")],
        children: vec![
            (
                Arc::from("CD4+"),
                vec![Arc::from("Ki67+"), Arc::from("TIGIT+")],
            ),
            (Arc::from("CD8+"), vec![Arc::from("Ki67+")]),
        ],
        parameters: vec![
            (
                Arc::from("Ki67+"),
                vec![Arc::from("Ki-67"), Arc::from("CD3")],
            ),
            (Arc::from("TIGIT+"), vec![Arc::from("TIGIT")]),
        ],
    }
}

#[test]
fn a_gate_the_new_population_also_holds_is_kept() {
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "Ki67+", "Ki-67");
    assert_eq!(gate, "Ki67+");
    assert_eq!(parameter, "Ki-67");
}

#[test]
fn a_gate_the_new_population_does_not_hold_is_cleared() {
    // TIGIT+ is drawn on CD4+ only. Carrying the name over would let the form
    // name a population that does not exist.
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "TIGIT+", "TIGIT");
    assert_eq!(gate, "");
    assert_eq!(parameter, "");
}

#[test]
fn a_parameter_that_gate_is_not_drawn_on_is_cleared_but_the_gate_stays() {
    // The gate survives the move; the parameter did not come with it, so the
    // form asks for that one field rather than both.
    let (gate, parameter) = carry_over(&two_populations(), "CD8+", "Ki67+", "TIGIT");
    assert_eq!(gate, "Ki67+");
    assert_eq!(parameter, "");
}

#[test]
fn clearing_the_population_clears_the_gate() {
    let (gate, parameter) = carry_over(&two_populations(), "", "Ki67+", "Ki-67");
    assert_eq!(gate, "");
    assert_eq!(parameter, "");
}

// ── how a phenotype rule reads back ──────────────────────────────────────

use crate::axis_store::Param;

fn param(marker: &str, fluoro: &str) -> Param {
    Param {
        marker: Arc::from(marker),
        fluoro: Arc::from(fluoro),
    }
}

#[test]
fn a_phenotype_rule_names_its_markers_as_they_were_ticked() {
    use crate::gate_rules::choices::describe_phenotype;
    use crate::gate_rules::rule::{PhenotypeRule, ShapeFit};
    // The rule stores the column it reads, because that is what a DataFrame is
    // indexed by. Nobody ticks a column called BV421-A.
    let panel = vec![
        param("CD279", "BV421-A"),
        param("Va7_2", "BV711-A"),
        param("CD161", "BB700-A"),
    ];
    let rule = PhenotypeRule {
        markers: vec![Arc::from("BV421-A"), Arc::from("BB700-A")],
        fit: ShapeFit::DrawPolygon,
        ..Default::default()
    };
    let described = describe_phenotype(&rule, &panel);
    assert!(described.contains("CD279"), "got: {described}");
    assert!(described.contains("CD161"), "got: {described}");
    assert!(
        !described.contains("BV421-A"),
        "the column leaked: {described}"
    );
}

#[test]
fn a_marker_the_panel_does_not_carry_is_shown_as_the_rule_stores_it() {
    use crate::gate_rules::choices::marker_label;
    // The honest answer: that is the column the rule will look for, and saying
    // so is how a person finds out the panel has changed under them.
    assert_eq!(marker_label("PE-A", &[param("CD279", "BV421-A")]), "PE-A");
}

#[test]
fn a_phenotype_rule_with_nothing_ticked_says_it_uses_every_marker() {
    use crate::gate_rules::choices::describe_phenotype;
    use crate::gate_rules::rule::PhenotypeRule;
    let described = describe_phenotype(&PhenotypeRule::default(), &[]);
    assert!(described.contains("every marker"), "got: {described}");
}

// ─── the phenotype rule's marker picker ───────────────────────────────────────

fn panel() -> Vec<crate::axis_store::Param> {
    vec![
        param("FSC-A", "FSC-A"),
        param("CD161", "BUV395-A"),
        param("TCRVa7.2", "BV421-A"),
        param("CD3", "BUV805-A"),
    ]
}

#[test]
fn a_new_phenotype_rule_starts_with_the_gate_s_own_two_markers_ticked() {
    use crate::gate_rules::choices::plot_markers;
    let drawn = [Arc::from("BV421-A"), Arc::from("BUV395-A")];
    assert_eq!(
        plot_markers(&drawn, &panel()),
        ["BV421-A", "BUV395-A"],
        "ticked as the channels the rule stores, in the plot's order"
    );
}

#[test]
fn a_gate_drawn_on_marker_names_ticks_the_same_boxes() {
    // A gating document can name its axes by marker. The box to tick is still
    // the channel's - there is one box per channel.
    use crate::gate_rules::choices::plot_markers;
    let drawn = [Arc::from("TCRVa7.2"), Arc::from("CD161")];
    assert_eq!(plot_markers(&drawn, &panel()), ["BV421-A", "BUV395-A"]);
}

#[test]
fn a_gate_drawn_twice_on_one_channel_ticks_it_once() {
    use crate::gate_rules::choices::plot_markers;
    let drawn = [Arc::from("CD3"), Arc::from("BUV805-A")];
    assert_eq!(plot_markers(&drawn, &panel()), ["BUV805-A"]);
}

#[test]
fn a_gate_axis_the_panel_does_not_carry_is_kept_as_named() {
    // Shown and ticked as the rule will look for it, rather than dropped.
    use crate::gate_rules::choices::plot_markers;
    let drawn = [Arc::from("CD3"), Arc::from("Time")];
    assert_eq!(plot_markers(&drawn, &panel()), ["BUV805-A", "Time"]);
}

#[test]
fn the_picker_offers_each_channel_once() {
    use crate::gate_rules::choices::marker_panel;
    let mut doubled = panel();
    doubled.push(param("BUV805-A", "BUV805-A")); // the same channel, unnamed
    let offered: Vec<String> = marker_panel(&doubled)
        .iter()
        .map(|p| p.fluoro.to_string())
        .collect();
    assert_eq!(offered, ["FSC-A", "BUV395-A", "BV421-A", "BUV805-A"]);
}

#[test]
fn a_parameter_is_shown_with_its_marker_and_channel() {
    use crate::gate_rules::choices::marker_label;
    assert_eq!(marker_label("BUV395-A", &panel()), "CD161-BUV395");
    assert_eq!(marker_label("FSC-A", &panel()), "FSC-A");
    assert_eq!(marker_label("Time", &panel()), "Time", "unknown: as stored");
}

// ─── the form for a rule from another gate ────────────────────────────────────

mod following {
    use crate::gate_rules::choices::{
        EdgeForm, GateChoices, every_target, follow_from_form, follow_to_form,
    };
    use crate::gate_rules::rule::{FromGateRule, Side};
    use crate::gate_rules::rule_store::RuleTarget;
    use std::sync::Arc;

    fn offered() -> GateChoices {
        GateChoices {
            parents: vec![Arc::from("CD45+"), Arc::from("Lymph")],
            children: vec![
                (
                    Arc::from("CD45+"),
                    vec![Arc::from("CD19+CD14-"), Arc::from("CD19-")],
                ),
                (Arc::from("Lymph"), vec![Arc::from("CD19-")]),
            ],
            parameters: Vec::new(),
        }
    }

    fn edge(anchor: &str, parameter: &str, side: &str, anchor_side: &str, gap: &str) -> EdgeForm {
        EdgeForm {
            anchor: anchor.into(),
            parameter: parameter.into(),
            side: side.into(),
            anchor_side: anchor_side.into(),
            gap: gap.into(),
        }
    }

    #[test]
    fn a_valley_rule_falls_back_to_the_same_gate_under_another_parent() {
        use crate::gate_rules::choices::fallback_targets;
        let named = |gate, parent| -> Vec<String> {
            fallback_targets(&offered(), gate, parent)
                .iter()
                .map(|t| t.describe())
                .collect()
        };
        assert_eq!(named("CD19-", "CD45+"), ["CD19- of Lymph"]);
        assert_eq!(named("CD19-", "Lymph"), ["CD19- of CD45+"]);
        assert!(named("CD19+CD14-", "CD45+").is_empty(), "drawn once");
    }

    #[test]
    fn every_gate_is_offered_as_a_rule_names_it_under_each_parent() {
        let named: Vec<String> = every_target(&offered())
            .iter()
            .map(|t| t.describe())
            .collect();
        assert_eq!(
            named,
            ["CD19+CD14- of CD45+", "CD19- of CD45+", "CD19- of Lymph"],
            "a gate under two parents is two choices"
        );
    }

    #[test]
    fn the_same_shape_is_the_gate_chosen() {
        let rule =
            follow_from_form(Some("CD19- of Lymph"), &[], &every_target(&offered())).unwrap();
        assert_eq!(
            rule.same_shape_as,
            Some(RuleTarget::under("CD19-", "Lymph"))
        );
        assert!(
            rule.edges.is_empty(),
            "edges on the form are not carried along"
        );
    }

    #[test]
    fn edges_are_read_off_the_form_with_an_empty_gap_as_none() {
        let rule = follow_from_form(
            None,
            &[
                edge("CD19+CD14- of CD45+", "BUV395-A", "Upper", "Lower", ""),
                edge("CD19- of Lymph", "BV421-A", "Lower", "Upper", "-0.5"),
            ],
            &every_target(&offered()),
        )
        .unwrap();
        assert_eq!(rule.same_shape_as, None);
        assert_eq!(rule.edges.len(), 2);
        assert_eq!(
            rule.edges[0].anchor,
            RuleTarget::under("CD19+CD14-", "CD45+")
        );
        assert_eq!(&*rule.edges[0].parameter, "BUV395-A");
        assert_eq!(
            (rule.edges[0].side, rule.edges[0].anchor_side),
            (Side::Upper, Side::Lower)
        );
        assert_eq!(rule.edges[0].gap, 0.0);
        assert_eq!(rule.edges[1].gap, -0.5);
    }

    #[test]
    fn what_the_form_cannot_make_a_rule_of_is_said() {
        let targets = every_target(&offered());
        let said = |same: Option<&str>, edges: &[EdgeForm]| {
            follow_from_form(same, edges, &targets).unwrap_err()
        };
        assert_eq!(said(Some(""), &[]), "Choose the gate it follows");
        assert!(said(Some("CD3+ of CD45+"), &[]).contains("no gate CD3+ of CD45+"));
        assert_eq!(said(None, &[]), "Add an edge to set");
        assert!(
            said(None, &[edge("CD19- of Lymph", "", "Upper", "Lower", "")]).contains("parameter")
        );
        assert!(
            said(
                None,
                &[edge("CD19- of Lymph", "X", "Upper", "Lower", "a bit")]
            )
            .contains("not a number")
        );
        assert!(
            said(None, &[edge("CD19- of Lymph", "X", "", "Lower", "")]).contains("not an edge")
        );
    }

    #[test]
    fn a_saved_rule_opens_in_the_form_as_it_was_written() {
        let targets = every_target(&offered());
        for (same, edges) in [
            (Some("CD19- of Lymph"), Vec::new()),
            (
                None,
                vec![
                    edge("CD19+CD14- of CD45+", "BUV395-A", "Upper", "Lower", ""),
                    edge("CD19- of Lymph", "BV421-A", "Lower", "Upper", "-0.5"),
                ],
            ),
        ] {
            let rule: FromGateRule = follow_from_form(same, &edges, &targets).unwrap();
            let (same_back, edges_back) = follow_to_form(&rule);
            assert_eq!(same_back.as_deref(), same);
            assert_eq!(edges_back, edges);
            assert_eq!(
                follow_from_form(same_back.as_deref(), &edges_back, &targets).unwrap(),
                rule,
                "and saves back unchanged"
            );
        }
    }
}
