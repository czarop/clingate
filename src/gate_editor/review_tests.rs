//! What the review wiring decides, on the shared test workspace's gating:
//! what a plot's and a gallery tile's Report buttons report, where a Review
//! tile draws a placement, and where "Open in editor" takes the editor.
#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use clingate_core::axis_store::Param;
use clingate_core::gates::GateState;
use clingate_core::gates::gate_store::{GateId, NodeId};
use clingate_core::review::run_record::SampleRef;
use clingate_core::session::Session;
use clingate_core::test_workspace::two_samples_with_a_rule;

use crate::gate_editor::review::{drawn_target, gates_on_plot, selected_target};
use crate::gate_editor::review_window::{EditorFocus, file_of, focus_on, where_drawn};

/// The workspace, Tmem's gate id, its node and the population above it.
fn tmem(name: &str) -> (Session, GateId, NodeId, NodeId) {
    let session = Session::open(&two_samples_with_a_rule(name)).unwrap();
    let state = session.gates();
    let id = state
        .registered_ids()
        .into_iter()
        .find(|id| state.registered_gate(id).unwrap().get_name() == "Tmem")
        .expect("the fixture has Tmem");
    let node = state.nodes_for_gate(&id)[0].clone();
    let parent = state.parent_node(&node).unwrap();
    (session, id, node, parent)
}

fn params(session: &Session) -> Vec<Param> {
    session.axes().sorted_settings.iter().cloned().collect()
}

fn name_of(state: &GateState, node: &NodeId) -> String {
    let id = state.gate_for_node(node).unwrap();
    state.registered_gate(id).unwrap().get_name().to_string()
}

#[test]
fn a_plot_reports_the_gate_selected_on_it_at_its_place_under_the_plot() {
    let (session, id, node, parent) = tmem("review-selected");
    let state = session.gates();
    let target = selected_target(
        state,
        Some(&id),
        Some(&parent),
        Some(Arc::from("sample1")),
        "sample1_FMX.fcs",
    )
    .expect("a target");
    assert_eq!(target.node, node);
    assert_eq!(target.gate, "Tmem");
    assert_eq!(&*target.sample, "sample1");
    assert_eq!(target.sample_name, "sample1_FMX", "without .fcs");
    assert!(target.choices.is_empty());

    // Nothing selected, a sample no metadata names, or a gate not in the
    // document: nothing to report.
    assert!(selected_target(state, None, Some(&parent), Some(Arc::from("sample1")), "x").is_none());
    assert!(selected_target(state, Some(&id), Some(&parent), None, "x").is_none());
    let nowhere: GateId = Arc::from("no such gate");
    assert!(
        selected_target(
            state,
            Some(&nowhere),
            Some(&parent),
            Some(Arc::from("sample1")),
            "x"
        )
        .is_none()
    );
}

#[test]
fn a_gallery_tile_reports_a_gate_drawn_on_it_and_offers_the_others() {
    let (session, id, node, parent) = tmem("review-drawn");
    let state = session.gates();
    let (x, y) = state.registered_gate(&id).unwrap().get_params();

    let drawn = gates_on_plot(state, &parent, &x, &y);
    assert!(
        drawn.iter().any(|(n, g)| *n == node && g == "Tmem"),
        "{drawn:?}"
    );
    // Every gate offered is a child of the population, on the page's axes.
    for (child, _) in &drawn {
        assert_eq!(state.parent_node(child).as_ref(), Some(&parent));
        let (a, b) = state
            .registered_gate(state.gate_for_node(child).unwrap())
            .unwrap()
            .get_params();
        assert!((a == x && b == y) || (a == y && b == x));
    }
    // Drawn the other way round, the same gates.
    assert_eq!(gates_on_plot(state, &parent, &y, &x), drawn);

    let target = drawn_target(
        state,
        &parent,
        &x,
        &y,
        Some(Arc::from("sample2")),
        "sample2_FS.fcs",
    )
    .expect("a gate is drawn");
    assert_eq!((target.node.clone(), target.gate.clone()), drawn[0].clone());
    assert_eq!(target.choices, drawn);
    assert_eq!(target.sample_name, "sample2_FS");

    // A page on axes no gate under it uses draws nothing to report.
    assert!(gates_on_plot(state, &parent, "FSC-A", "no such channel").is_empty());
    assert!(
        drawn_target(
            state,
            &parent,
            "FSC-A",
            "no such channel",
            Some(Arc::from("sample2")),
            "x"
        )
        .is_none()
    );
    // Nor does a tile whose sample no metadata names.
    assert!(drawn_target(state, &parent, &x, &y, None, "x").is_none());
}

#[test]
fn a_review_tile_draws_the_population_above_the_gate_on_the_gate_s_axes() {
    let (session, id, node, parent) = tmem("review-where");
    let state = session.gates();
    let params = params(&session);
    let parent_name = name_of(state, &parent);
    let (x, y) = state.registered_gate(&id).unwrap().get_params();

    let drawn = where_drawn(state, &params, &id, Some(&parent_name)).expect("drawn");
    assert_eq!(drawn.gate_node, node);
    assert_eq!(&*drawn.parent, parent.as_str());
    assert_eq!(drawn.x.fluoro, x);
    assert_eq!(drawn.y.fluoro, y);
    // With the marker names the scaling gives.
    assert_eq!(Some(&drawn.x), params.iter().find(|p| p.fluoro == x));

    // A channel the scaling does not have is drawn by its own name.
    let bare = where_drawn(state, &[], &id, Some(&parent_name)).unwrap();
    assert_eq!(
        (bare.x.marker.clone(), bare.x.fluoro.clone()),
        (x.clone(), x)
    );
    // A gate the document has lost is not drawn at all.
    assert!(where_drawn(state, &params, "no such gate", None).is_none());
}

#[test]
fn a_tile_finds_its_sample_s_file_by_the_name_the_run_recorded() {
    let files: Vec<(Arc<str>, PathBuf)> = vec![
        (Arc::from("a.fcs"), PathBuf::from("/w/a.fcs")),
        (
            Arc::from("Plate_10_b.fcs"),
            PathBuf::from("/w/Plate_10/b.fcs"),
        ),
    ];
    let sample = |name: Option<&str>| SampleRef {
        id: "x".into(),
        name: name.map(str::to_string),
        sample_type: None,
    };
    assert_eq!(
        file_of(&files, &sample(Some("Plate_10_b.fcs"))),
        Some((
            Arc::from("Plate_10_b.fcs"),
            PathBuf::from("/w/Plate_10/b.fcs")
        ))
    );
    assert_eq!(file_of(&files, &sample(Some("gone.fcs"))), None);
    assert_eq!(file_of(&files, &sample(None)), None);
}

#[test]
fn open_in_editor_sets_the_population_the_axes_and_the_sample() {
    let (session, id, _, parent) = tmem("review-focus");
    let params = params(&session);
    let (x, y) = session.gates().registered_gate(&id).unwrap().get_params();
    let files: Vec<Arc<str>> = vec![Arc::from("sample1_FMX.fcs"), Arc::from("sample2_FS.fcs")];
    let focus = EditorFocus {
        parent: Arc::from(parent.as_str()),
        x: x.clone(),
        y: y.clone(),
        sample_name: Arc::from("sample2_FS.fcs"),
    };
    let to = focus_on(&focus, &params, &files);
    assert_eq!(&*to.parent, parent.as_str());
    assert_eq!(to.x.as_ref().map(|p| p.fluoro.clone()), Some(x));
    assert_eq!(to.y.as_ref().map(|p| p.fluoro.clone()), Some(y));
    assert_eq!(to.file, Some(1));

    // A channel the scaling lacks leaves that axis alone; a file no longer in
    // the workspace leaves the sample alone.
    let odd = EditorFocus {
        x: Arc::from("no such channel"),
        sample_name: Arc::from("gone.fcs"),
        ..focus
    };
    let to = focus_on(&odd, &params, &files);
    assert_eq!(to.x, None);
    assert!(to.y.is_some());
    assert_eq!(to.file, None);
}

mod beside {
    use super::*;
    use crate::gate_editor::review_window::{Compare, comparisons};
    use clingate_core::review::assess::Flag;
    use clingate_core::review::board::{Entry, Pile};

    fn sample(id: &str, kind: &str) -> SampleRef {
        SampleRef {
            id: id.into(),
            name: Some(format!("{id}.fcs")),
            sample_type: Some(kind.into()),
        }
    }

    fn files() -> Vec<(Arc<str>, PathBuf)> {
        ["fs1", "fmx1", "qc", "fs7"]
            .iter()
            .map(|n| {
                (
                    Arc::from(format!("{n}.fcs").as_str()),
                    PathBuf::from(format!("/w/{n}.fcs")),
                )
            })
            .collect()
    }

    fn entry(rule_read: Option<SampleRef>, peer: Option<SampleRef>) -> Entry {
        Entry {
            gate_id: "g".into(),
            gate: "CD69+".into(),
            parent_gate: None,
            sample: sample("fs1", "FS"),
            moved: true,
            confidence: Some(0.7),
            weakest: None,
            pile: Pile::NeedsALook,
            flag: peer.map(|p| Flag {
                gate_id: "g".into(),
                gate: "CD69+".into(),
                parent_gate: None,
                sample: sample("fs1", "FS"),
                moved: true,
                confidence: Some(0.7),
                weakest: None,
                severity: 4.0,
                reasons: Vec::new(),
                status: None,
                peers: 30,
                confident_peers: true,
                typical_peer: Some(p),
            }),
            looks_right: false,
            reports: 0,
            status: None,
            rule_read,
        }
    }

    fn labels(b: &[crate::gate_editor::review_window::Beside]) -> Vec<String> {
        b.iter().map(|b| b.label.clone()).collect()
    }

    #[test]
    fn an_fs_is_shown_beside_the_fmx_its_rule_read_and_its_typical_peer() {
        let e = entry(Some(sample("fmx1", "FMX")), Some(sample("fs7", "FS")));
        let both = comparisons(&e, Compare::Both, &files());
        assert_eq!(
            labels(&both),
            vec!["the FMX the rule read: fmx1", "a typical peer: fs7"]
        );
        assert_eq!(both[0].path, PathBuf::from("/w/fmx1.fcs"));
        assert_eq!(
            labels(&comparisons(&e, Compare::RuleRead, &files())),
            vec!["the FMX the rule read: fmx1"]
        );
        assert_eq!(
            labels(&comparisons(&e, Compare::Peer, &files())),
            vec!["a typical peer: fs7"]
        );
        assert!(comparisons(&e, Compare::Nothing, &files()).is_empty());
    }

    #[test]
    fn a_reference_of_the_same_kind_is_named_as_the_reference() {
        let e = entry(Some(sample("qc", "FS")), None);
        assert_eq!(
            labels(&comparisons(&e, Compare::Both, &files())),
            vec!["the reference the rule read: qc"]
        );
    }

    #[test]
    fn nothing_is_shown_twice_nor_what_the_workspace_no_longer_has() {
        // The typical peer is the reference too: once.
        let e = entry(Some(sample("fs7", "FS")), Some(sample("fs7", "FS")));
        assert_eq!(comparisons(&e, Compare::Both, &files()).len(), 1);
        // A rule that read the sample itself, or a file that has gone.
        let e = entry(Some(sample("fs1", "FS")), Some(sample("gone", "FS")));
        assert!(comparisons(&e, Compare::Both, &files()).is_empty());
        // Unflagged, a tile has no typical peer: only what the rule read.
        let e = entry(Some(sample("fmx1", "FMX")), None);
        assert_eq!(comparisons(&e, Compare::Peer, &files()).len(), 0);
        assert_eq!(comparisons(&e, Compare::Both, &files()).len(), 1);
    }

    #[test]
    fn every_choice_reads_back_from_its_key() {
        for c in Compare::ALL {
            assert_eq!(Compare::from_key(c.key()), Some(c));
        }
        assert_eq!(Compare::from_key("x"), None);
    }
}
