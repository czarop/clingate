//! Random sequences of the edits a person makes to a gating document.
//!
//! The gate store (`gate_editor::gates::gate_store`) keeps four things in
//! step: the gates, the tree of positions they are placed at, the composites'
//! corners, and which gates are still reachable. Each edit - adding a gate of
//! any kind under any position, deleting a position, linking one position to
//! another's gate, unlinking - has its own tests. What they cannot show is
//! that the four stay in step across any *sequence* of edits, and that
//! whatever the sequence left behind can be written out (`omiq::serialise`)
//! and read back (`omiq::deserialise`) as the same document.
//!
//! Seeded, so a failure prints the sequence that caused it.

mod common;

use clingate::gate_editor::gates::GateState;
use clingate::gate_editor::gates::gate_store::{GateStateImplExt, NodeId, ROOTGATE};
use clingate::gate_editor::gates::gate_types::PrimaryGateType;
use clingate::gate_editor::plots::axis_store::PlotMapper;
use clingate::omiq::rebuild::OmiqDocumentHeader;
use clingate::omiq::serialise::to_omiq_document_with_header;
use common::*;
use flow_fcs::TransformType;
use rand::prelude::*;
use std::sync::Arc;

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

fn header() -> OmiqDocumentHeader {
    OmiqDocumentHeader {
        date: "2020-01-01T00:00:00.000Z".into(),
        dataset_id: 2,
        inverted: false,
        task_id: 1,
        url: "https://example.invalid/".into(),
        workflow_id: 1,
        extra: Default::default(),
    }
}

const KINDS: [PrimaryGateType; 5] = [
    PrimaryGateType::Rectangle,
    PrimaryGateType::Ellipse,
    PrimaryGateType::Quadrant,
    PrimaryGateType::Bisector,
    PrimaryGateType::SkewedQuadrant,
];

/// Every position, in an order that does not depend on the random ids gates
/// are given - so a seed picks the same positions every run and a failure
/// can be replayed. Keyed by the names from the root down, and a composite
/// corner's suffix.
fn nodes(state: &GateState) -> Vec<NodeId> {
    let root = NodeId::from(ROOTGATE.clone());
    let key = |node: &NodeId| -> String {
        let mut parts = Vec::new();
        let mut at = Some(node.clone());
        while let Some(n) = at {
            if n == root {
                break;
            }
            let name = state
                .gate_for_node(&n)
                .and_then(|g| state.registered_gate(g))
                .map(|g| g.get_name().to_string())
                .unwrap_or_default();
            let suffix = n.as_str().rsplit_once('_').map(|(_, s)| s).unwrap_or("");
            parts.push(format!("{name}_{suffix}"));
            at = state.parent_node(&n);
        }
        parts.reverse();
        parts.join("/")
    };
    let mut all: Vec<(String, NodeId)> = state
        .placements()
        .map(|(n, _)| (key(n), n.clone()))
        .collect();
    all.sort();
    all.into_iter().map(|(_, n)| n).collect()
}

/// Everything that makes the document what it is, independent of the order
/// it is held in: each position's gate and parent, and every registered gate.
fn layout(state: &GateState) -> (Vec<(String, String, Option<String>)>, Vec<String>) {
    let mut placements: Vec<_> = state
        .placements()
        .map(|(node, _)| {
            (
                node.as_str().to_string(),
                state
                    .gate_for_node(node)
                    .map(|g| g.to_string())
                    .unwrap_or_default(),
                state.parent_node(node).map(|p| p.as_str().to_string()),
            )
        })
        .collect();
    placements.sort();
    let mut gates: Vec<String> = state
        .registered_ids()
        .iter()
        .map(|g| g.to_string())
        .collect();
    gates.sort();
    (placements, gates)
}

/// What must hold after every edit.
fn check(state: &GateState) -> Result<(), String> {
    let root = NodeId::from(ROOTGATE.clone());
    for node in nodes(state) {
        let Some(gate) = state.gate_for_node(&node) else {
            return Err(format!("{node} names no gate"));
        };
        if !state.is_registered(gate) {
            return Err(format!("{node} names {gate}, which is not registered"));
        }
        match state.parent_node(&node) {
            None => return Err(format!("{node} has no parent, not even the root")),
            Some(p) if p != root && state.gate_for_node(&p).is_none() => {
                return Err(format!("{node}'s parent {p} is not a position"));
            }
            _ => {}
        }
        // One gate per level between it and the root.
        let mut depth = 0;
        let mut at = node.clone();
        while let Some(p) = state.parent_node(&at) {
            if p == root {
                break;
            }
            depth += 1;
            if depth > 1_000 {
                return Err(format!("{node}'s ancestry does not end"));
            }
            at = p;
        }
        let chain = state.gate_chain_for_node(&node);
        if chain.len() != depth + 1 {
            return Err(format!("{node}: chain of {} at depth {depth}", chain.len()));
        }
    }
    // Nothing registered that nothing reaches: every registered id is placed,
    // or is a composite whose corners are.
    for id in state.registered_ids() {
        if state.placement_count(&id) > 0 {
            continue;
        }
        let Some(gate) = state.registered_gate(&id) else {
            return Err(format!("{id} is listed but does not resolve"));
        };
        if state.is_ghost(&id) {
            // Kept because a boolean still reaches it - the fixture has one.
            continue;
        }
        let corners_placed = gate.is_composite()
            && gate.get_id() == id
            && gate
                .get_inner_gate_ids()
                .iter()
                .any(|c| state.placement_count(c) > 0);
        if !corners_placed {
            return Err(format!("{id} is registered but on no plot"));
        }
    }
    Ok(())
}

/// The document a sequence starts from: empty, or - for odd seeds - the
/// checked-in fixture, which brings a boolean and the ghost it keeps alive.
fn starting_point(seed: u64) -> GateState {
    let mut state = GateState::default();
    if seed % 2 == 1 {
        state
            .upload_gates_from_file(
                fixture("quadrant_deleted_boolean_survives.omiqgt"),
                &Default::default(),
                fixture_axes(),
            )
            .expect("the fixture imports");
    }
    state
}

fn random_edits(seed: u64, steps: usize) -> (GateState, Vec<String>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut state = starting_point(seed);
    let mut log = Vec::new();
    for step in 0..steps {
        let existing = nodes(&state);
        let pick = |rng: &mut StdRng| -> Option<NodeId> {
            (!existing.is_empty()).then(|| existing[rng.random_range(0..existing.len())].clone())
        };
        let did = match rng.random_range(0..10) {
            // Adding is the most common edit, so the documents grow.
            0..=4 => {
                let kind = match rng.random_range(0..KINDS.len() + 2) {
                    k if k < KINDS.len() => KINDS[k],
                    k if k == KINDS.len() => PrimaryGateType::Polygon,
                    _ => PrimaryGateType::Line(Some(rng.random_range(100.0..900.0))),
                };
                let points = matches!(kind, PrimaryGateType::Polygon).then(|| {
                    (0..3)
                        .map(|_| (rng.random_range(50.0..950.0), rng.random_range(50.0..950.0)))
                        .collect::<Vec<(f32, f32)>>()
                });
                let parent = if rng.random_bool(0.3) {
                    None
                } else {
                    pick(&mut rng)
                };
                let name = format!("g{step}");
                let result = state.add_gate(
                    &mapper(),
                    rng.random_range(150.0..450.0),
                    rng.random_range(150.0..450.0),
                    Arc::from("FSC-A"),
                    Arc::from("SSC-A"),
                    points,
                    parent.as_ref().map(|p| p.as_arc().clone()),
                    kind,
                    Some(name.clone()),
                );
                format!("add {kind:?} {name} under {parent:?} -> {}", result.is_ok())
            }
            5 | 6 => match pick(&mut rng) {
                Some(n) => format!("delete {n} -> {}", state.delete_placement(&n).is_ok()),
                None => continue,
            },
            7 | 8 => match (pick(&mut rng), pick(&mut rng)) {
                (Some(a), Some(b)) => {
                    let linked = state.link_node_to_gate(&a, &b).is_ok();
                    // Linking leaves the gate it replaced registered even
                    // when nothing reaches it - B-DOC-1, pinned in the omiq
                    // tests. Collected here, as the fix would, so the
                    // sequences can go on to find anything else.
                    state.collect_stranded_ghosts();
                    format!("link {a} to {b} -> {linked}")
                }
                _ => continue,
            },
            _ => {
                // Only a linked position can be unlinked, and a random one
                // rarely is - so aim at the linked ones when there are any.
                let linked: Vec<NodeId> = existing
                    .iter()
                    .filter(|n| state.gate_for_node(n).is_some_and(|g| state.is_linked(g)))
                    .cloned()
                    .collect();
                let target = if linked.is_empty() {
                    pick(&mut rng)
                } else {
                    Some(linked[rng.random_range(0..linked.len())].clone())
                };
                match target {
                    Some(n) => format!("unlink {n} -> {}", state.unlink_node(&n).is_ok()),
                    None => continue,
                }
            }
        };
        log.push(did);
        if let Err(e) = check(&state) {
            panic!("seed {seed}, step {step}: {e}\n{}", log.join("\n"));
        }
    }
    (state, log)
}

#[test]
fn any_sequence_of_edits_keeps_the_document_consistent() {
    for seed in 0..600 {
        random_edits(seed, 30);
    }
}

#[test]
fn whatever_the_edits_left_is_written_and_read_back_as_the_same_document() {
    for seed in 0..150 {
        let (state, log) = random_edits(seed, 20);
        let header = state.omiq_rebuild().header.clone().unwrap_or_else(header);
        let written =
            to_omiq_document_with_header(&state, &Default::default(), &fixture_axes(), header)
                .unwrap_or_else(|e| panic!("seed {seed}: export failed: {e}\n{}", log.join("\n")));
        let path = scratch(&format!("fuzz-{seed}")).join("doc.omiqgt");
        std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();

        let mut back = GateState::default();
        back.upload_gates_from_file(path, &Default::default(), fixture_axes())
            .unwrap_or_else(|e| panic!("seed {seed}: reimport failed: {e}\n{}", log.join("\n")));
        assert_eq!(
            layout(&back),
            layout(&state),
            "seed {seed}: the document changed on the way through\n{}",
            log.join("\n")
        );
    }
}
