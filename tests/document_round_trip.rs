//! A gating document written by the program and read back by it.
//!
//! The import (`omiq::deserialise`), the three-tier gate store
//! (`gate_editor::gates::gate_store`), the metadata grouping and the export
//! (`omiq::serialise`) each have their own tests. What none of them can show
//! alone is the loop a person actually runs: gates adjusted per sample in
//! this session, the file saved, and the file opened again. A position that
//! comes back on the wrong sample - or on every sample - is the failure that
//! matters, and it only exists across the whole loop.

mod common;

use clingate::gate_editor::gates::GateState;
use clingate::gate_editor::gates::gate_store::{GateId, GateSource};
use clingate::gate_editor::gates::gate_traits::DrawableGate;
use clingate::gate_rules::autogate::{extent_on, place_for_specimen, translate_edge_to};
use clingate::gate_rules::rule_store::Bound;
use clingate::omiq::metadata::MetaDataKey;
use clingate::omiq::serialise::to_omiq_document;
use common::*;
use std::sync::Arc;

const FIXTURE: &str = "quadrant_with_boolean_child.omiqgt";

fn import(path: &std::path::Path) -> GateState {
    let mut state = GateState::default();
    state
        .upload_gates_from_file(path.to_path_buf(), &fixture_metadata(), fixture_axes())
        .unwrap_or_else(|e| panic!("{} imports: {e}", path.display()));
    state
}

/// Save the state and open what was saved.
fn saved_and_reopened(state: &GateState, name: &str) -> GateState {
    let written = to_omiq_document(state, &fixture_metadata(), &fixture_axes())
        .expect("an imported document can be written");
    let path = scratch(name).join("saved.omiqgt");
    std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();
    import(&path)
}

/// A gate with a finite lower edge on its x parameter, to move.
fn movable(state: &GateState) -> (GateId, Arc<dyn DrawableGate>, Arc<str>, f32) {
    state
        .registered_ids()
        .into_iter()
        .find_map(|id| {
            let gate = state.registered_gate(&id)?;
            if gate.is_composite() {
                return None;
            }
            let (x, _) = gate.get_params();
            let (low, _) = extent_on(&gate.get_gate_ref(None)?.geometry, &x)?;
            (low.is_finite() && low.abs() < 1e9).then_some((id, gate, x, low))
        })
        .expect("the fixture holds a gate with a finite x edge")
}

fn low_edge_for(state: &GateState, gate: &GateId, file: &str, x: &str) -> f32 {
    let resolved = state
        .gate_for_file(gate, &Arc::from(file), &fixture_metadata())
        .unwrap_or_else(|| panic!("{gate} resolves for {file}"));
    extent_on(&resolved.get_gate_ref(None).unwrap().geometry, x)
        .unwrap()
        .0
}

/// Equal to f32 precision. The moves below are a tenth of the edge's value,
/// so a position that did not move cannot pass for one that did - the edge
/// the fixture offers is on FSC-A, around a million, where a fixed nudge of
/// half a unit would vanish inside any tolerance a float needs.
fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(1.0)
}

/// A move big enough to see: a tenth of where the edge is.
fn nudge(from: f32) -> f32 {
    (from.abs() * 0.1).max(1.0)
}

#[test]
fn a_position_set_for_one_sample_comes_back_on_that_sample_only() {
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let to = from + nudge(from);
    let moved = translate_edge_to(&gate, &x, Bound::Above, to as f64).unwrap();
    state.place_gate(
        &[id.clone()],
        &moved,
        &GateSource::Sample((id.clone(), Arc::from("sample1"))),
    );

    let back = saved_and_reopened(&state, "persample");
    let (one, two) = (
        low_edge_for(&back, &id, "sample1", &x),
        low_edge_for(&back, &id, "sample2", &x),
    );
    assert!(close(one, to), "sample1 came back at {one}, set to {to}");
    assert!(
        close(two, from),
        "sample2 came back at {two}, left at {from}"
    );
}

#[test]
fn a_position_set_for_a_specimen_comes_back_on_its_samples_only() {
    // What the autogater writes: one position per metadata group.
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let to = from + nudge(from);
    let moved = translate_edge_to(&gate, &x, Bound::Above, to as f64).unwrap();
    place_for_specimen(
        &mut state,
        &id,
        &MetaDataKey {
            parameter: Arc::from("test"),
            group: Arc::from("two"),
        },
        &moved,
    );

    let back = saved_and_reopened(&state, "perspecimen");
    assert!(close(low_edge_for(&back, &id, "sample2", &x), to));
    assert!(close(low_edge_for(&back, &id, "sample1", &x), from));
}

#[test]
fn saving_twice_changes_nothing_the_second_time() {
    // A document that drifts each time it is saved - rounding, a sentinel
    // re-read as a coordinate, a group re-keyed - is corrupted a little more
    // by every save.
    let mut state = import(&fixture(FIXTURE));
    let (id, gate, x, from) = movable(&state);
    let moved = translate_edge_to(&gate, &x, Bound::Above, (from + nudge(from)) as f64).unwrap();
    state.place_gate(
        &[id.clone()],
        &moved,
        &GateSource::Sample((id, Arc::from("sample1"))),
    );

    let once = saved_and_reopened(&state, "twice-a");
    let twice = saved_and_reopened(&once, "twice-b");
    let write = |s: &GateState| to_omiq_document(s, &fixture_metadata(), &fixture_axes()).unwrap();
    assert_eq!(write(&once)["tree"], write(&twice)["tree"]);
}

#[test]
fn every_gate_and_position_survives_a_save() {
    let state = import(&fixture(FIXTURE));
    let back = saved_and_reopened(&state, "everything");

    let mut before = state.registered_ids();
    let mut after = back.registered_ids();
    before.sort();
    after.sort();
    assert_eq!(before, after, "the same gates");
    for id in &before {
        assert_eq!(
            state.placement_count(id),
            back.placement_count(id),
            "{id} is on the same number of plots"
        );
        assert_eq!(
            state.hierarchy_parent(id),
            back.hierarchy_parent(id),
            "{id} keeps its parent"
        );
    }
}

/// BUG (docs/test-audit.md, B-AX-3), on import: a gating file's quadrants are
/// built against the axis ranges the scaling gave, and a range the wrong way
/// round reaches the same `clamp` - opening the gating file crashes rather
/// than reporting the scaling.
#[test]
fn a_gating_file_imported_over_an_inverted_range_is_an_error_not_a_crash() {
    let mut axes = fixture_axes();
    for (_, axis) in axes.iter_mut() {
        std::mem::swap(&mut axis.axis_lower, &mut axis.axis_upper);
    }
    let outcome = std::panic::catch_unwind(|| {
        let mut state = GateState::default();
        state
            .upload_gates_from_file(fixture(FIXTURE), &fixture_metadata(), axes)
            .is_ok()
    });
    assert!(outcome.is_ok(), "importing over an inverted range panicked");
}

/// Import a copy of the fixture with its JSON edited by `edit`, on a thread,
/// giving up after `seconds`. `None` means it never finished.
fn import_edited(
    name: &str,
    edit: impl FnOnce(&mut serde_json::Value),
    seconds: u64,
) -> Option<bool> {
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture(FIXTURE)).unwrap()).unwrap();
    edit(&mut doc);
    let path = scratch(name).join("edited.omiqgt");
    std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut state = GateState::default();
        let ok = state
            .upload_gates_from_file(path, &fixture_metadata(), fixture_axes())
            .is_ok();
        let _ = done.send(ok);
    });
    finished
        .recv_timeout(std::time::Duration::from_secs(seconds))
        .ok()
}

/// Was B-OMIQ-2: the import sorts nodes by depth by walking each one's
/// `parentId` up to the root, with nothing to notice a node it had already
/// seen. A file in which a node is its own parent sent the walk round
/// forever, hanging the Workspace tab on "Loading". Now it is refused as
/// damaged.
#[test]
fn a_gating_file_whose_tree_loops_is_refused_rather_than_hanging() {
    let outcome = import_edited(
        "loop",
        |doc| {
            let nodes = doc["tree"]["nodes"].as_object_mut().unwrap();
            let (id, node) = nodes.iter_mut().next().unwrap();
            node["parentId"] = serde_json::Value::String(id.clone());
        },
        10,
    );
    assert_eq!(
        outcome,
        Some(false),
        "the import never finished (None) or accepted it"
    );
}

/// Two nodes that are each other's parent, and the error saying so.
#[test]
fn a_gating_file_whose_nodes_are_each_others_parents_is_refused_by_name() {
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(fixture(FIXTURE)).unwrap()).unwrap();
    let ids: Vec<String> = doc["tree"]["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .take(2)
        .cloned()
        .collect();
    doc["tree"]["nodes"][&ids[0]]["parentId"] = serde_json::Value::String(ids[1].clone());
    doc["tree"]["nodes"][&ids[1]]["parentId"] = serde_json::Value::String(ids[0].clone());
    let path = scratch("mutual-loop").join("edited.omiqgt");
    std::fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();
    let error = GateState::default()
        .upload_gates_from_file(path, &fixture_metadata(), fixture_axes())
        .expect_err("a looping tree is refused")
        .to_string();
    assert!(
        error.contains("damaged") && error.contains("loops"),
        "{error}"
    );
    assert!(
        ids.iter().any(|id| error.contains(id.as_str())),
        "names a node in the loop: {error}"
    );
}

#[test]
fn a_gating_file_naming_a_parent_it_does_not_contain_still_opens() {
    // The comment in the depth sort says this "shouldn't happen with clean
    // data"; what happens if it does is that it finishes, rather than hangs.
    let outcome = import_edited(
        "orphan",
        |doc| {
            let nodes = doc["tree"]["nodes"].as_object_mut().unwrap();
            let (_, node) = nodes.iter_mut().next().unwrap();
            node["parentId"] = serde_json::Value::String("not-a-node".into());
        },
        10,
    );
    assert!(outcome.is_some(), "the import never finished");
}

/// Random positions per sample and per specimen, on random gates, survive a
/// save and reopen: every gate resolves, for every file, to where it did.
#[test]
fn random_per_file_and_per_specimen_positions_survive_a_save() {
    use rand::prelude::*;
    let files = ["sample1", "sample2"];
    let groups = [("test", "one"), ("test", "two")];
    for seed in 0..40u64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut state = import(&fixture(FIXTURE));
        let mut ids: Vec<GateId> = state
            .registered_ids()
            .into_iter()
            .filter(|id| {
                state.registered_gate(id).is_some_and(|g| {
                    !g.is_composite()
                        && extent_on(&g.get_gate_ref(None).unwrap().geometry, &g.get_params().0)
                            .is_some_and(|(lo, _)| lo.is_finite() && lo.abs() < 1e9)
                })
            })
            .collect();
        ids.sort();
        let mut log = Vec::new();
        for _ in 0..6 {
            let id = ids[rng.random_range(0..ids.len())].clone();
            let gate = state.registered_gate(&id).unwrap();
            let x = gate.get_params().0;
            let (lo, _) = extent_on(&gate.get_gate_ref(None).unwrap().geometry, &x).unwrap();
            let to = lo + nudge(lo) * rng.random_range(-1.0..1.0);
            let Ok(moved) = translate_edge_to(&gate, &x, Bound::Above, to as f64) else {
                continue;
            };
            if rng.random_bool(0.5) {
                let file = files[rng.random_range(0..files.len())];
                state.place_gate(
                    &[id.clone()],
                    &moved,
                    &GateSource::Sample((id.clone(), Arc::from(file))),
                );
                log.push(format!("{id} for {file} at {to}"));
            } else {
                let (column, group) = groups[rng.random_range(0..groups.len())];
                place_for_specimen(
                    &mut state,
                    &id,
                    &MetaDataKey {
                        parameter: Arc::from(column),
                        group: Arc::from(group),
                    },
                    &moved,
                );
                log.push(format!("{id} for group {group} at {to}"));
            }
        }

        // Guard: the edits must have given the two files different positions
        // somewhere, or this would be comparing the defaults with themselves.
        let differs = ids.iter().any(|id| {
            let x = state.registered_gate(id).unwrap().get_params().0;
            !close(
                low_edge_for(&state, id, "sample1", &x),
                low_edge_for(&state, id, "sample2", &x),
            )
        });
        assert!(
            differs,
            "seed {seed}: no file got a position of its own\n{}",
            log.join("\n")
        );

        let back = saved_and_reopened(&state, &format!("random-{seed}"));
        for id in &ids {
            let x = state.registered_gate(id).unwrap().get_params().0;
            for file in files {
                let (before, after) = (
                    low_edge_for(&state, id, file, &x),
                    low_edge_for(&back, id, file, &x),
                );
                assert!(
                    close(before, after),
                    "seed {seed}: {id} on {file} was at {before}, came back at {after}\n{}",
                    log.join("\n")
                );
            }
        }
    }
}

// ── a gate positioned by two metadata columns ────────────────────────────

/// Place a new position for sample1's specimen on `id`, grouped by `column`,
/// and return (the x channel, where the edge was put).
fn place_by(state: &mut GateState, id: &GateId, column: &str) -> (Arc<str>, f32) {
    let current = state
        .gate_for_file(id, &Arc::from("sample1"), &fixture_metadata())
        .expect("the gate resolves for sample1");
    let (x, _) = current.get_params();
    let from = extent_on(&current.get_gate_ref(None).unwrap().geometry, &x)
        .unwrap()
        .0;
    let to = from + nudge(from);
    let moved = translate_edge_to(&current, &x, Bound::Above, to as f64).unwrap();
    // sample1 is "one" under both of the fixture's columns.
    place_for_specimen(
        state,
        id,
        &MetaDataKey {
            parameter: Arc::from(column),
            group: Arc::from("one"),
        },
        &moved,
    );
    (x, to)
}

/// What sample1 is drawn and filtered with: the editor's resolver, which is
/// what `gate_for_file` (the export's view) must agree with.
fn low_edge_on_screen(state: &GateState, id: &GateId, x: &str) -> f32 {
    let groups = fixture_metadata()
        .get(&Arc::from("sample1") as &Arc<str>)
        .cloned()
        .unwrap();
    let resolver = state.get_current_sample(Arc::from("sample1"), &groups);
    let gate = resolver.resolve_drawable(id).unwrap();
    extent_on(&gate.get_gate_ref(None).unwrap().geometry, x)
        .unwrap()
        .0
}

#[test]
fn a_position_placed_under_the_files_own_column_replaces_it() {
    // The two gates the fixture groups, each by its own column.
    for (id, column) in [("QCVn", "Type"), ("0lmI", "test")] {
        let mut state = import(&fixture(FIXTURE));
        let id: GateId = Arc::from(id);
        assert_eq!(
            state.group_override_column(&id).as_deref(),
            Some(column),
            "the premise: {id} arrives grouped by {column}"
        );
        let (x, to) = place_by(&mut state, &id, column);
        assert!(close(low_edge_for(&state, &id, "sample1", &x), to));
        assert!(close(low_edge_on_screen(&state, &id, &x), to));
    }
}

/// Was B-GRP-1. The file groups a gate's positions by one metadata column;
/// the autogater positions by the pairing's sample id column, which need not
/// be the same one. Both kinds of position are kept, and a sample in a group
/// of each got whichever column the hash map of its metadata yielded first -
/// not the position just placed. Now the newest applies.
///
/// Run both ways round - each gate grouped by one column in the file and
/// placed by the other - so no hash order can make it pass by luck.
#[test]
fn a_position_placed_under_another_column_is_the_one_its_samples_get() {
    for (id, grouped_by, placed_by) in [("QCVn", "Type", "test"), ("0lmI", "test", "Type")] {
        let mut state = import(&fixture(FIXTURE));
        let id: GateId = Arc::from(id);
        assert_eq!(
            state.group_override_column(&id).as_deref(),
            Some(grouped_by)
        );
        let (x, to) = place_by(&mut state, &id, placed_by);
        let (exported, shown) = (
            low_edge_for(&state, &id, "sample1", &x),
            low_edge_on_screen(&state, &id, &x),
        );
        assert!(
            close(exported, to) && close(shown, to),
            "{id}, grouped by {grouped_by} in the file and placed by {placed_by}: \
             sample1 is at {exported} for the export and {shown} on screen, placed at {to}"
        );
    }
}

// ── composites: a quadrant moved for one sample, or one specimen ─────────

/// Every quarter's extent on both axes, for one file.
fn quarters_for(state: &GateState, composite: &GateId, file: &str) -> Vec<(GateId, [f32; 4])> {
    let gate = state
        .gate_for_file(composite, &Arc::from(file), &fixture_metadata())
        .unwrap_or_else(|| panic!("{composite} resolves for {file}"));
    let (x, y) = gate.get_params();
    let mut ids = gate.get_inner_gate_ids();
    ids.sort();
    ids.into_iter()
        .map(|sub| {
            // Resolved through the subgate's own id, as filtering and the
            // statistics read it - not through the composite's.
            let piece = state
                .gate_for_file(&sub, &Arc::from(file), &fixture_metadata())
                .unwrap_or_else(|| panic!("{sub} resolves for {file}"));
            let geometry = &piece.get_gate_ref(Some(&sub)).unwrap().geometry;
            let (x0, x1) = extent_on(geometry, &x).unwrap();
            let (y0, y1) = extent_on(geometry, &y).unwrap();
            (sub, [x0, x1, y0, y1])
        })
        .collect()
}

/// Equal, where the axis's own infinite bound counts as equal to itself.
fn same_quarters(a: &[(GateId, [f32; 4])], b: &[(GateId, [f32; 4])]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((ia, ea), (ib, eb))| {
            ia == ib
                && ea
                    .iter()
                    .zip(eb)
                    .all(|(p, q)| (p.is_infinite() && p == q) || close(*p, *q))
        })
}

/// The fixture's quadrant with its centre handle dragged to `to`, as the
/// editor drags it: on a plot drawn over the fixture's own axes.
fn dragged_quadrant(state: &GateState, to: (f32, f32)) -> (GateId, Arc<dyn DrawableGate>) {
    use clingate::gate_editor::plots::axis_store::PlotMapper;
    let id = state
        .registered_ids()
        .into_iter()
        .find(|id| {
            state
                .registered_gate(id)
                .is_some_and(|g| g.is_composite() && g.get_id() == *id)
        })
        .expect("the fixture holds a composite");
    let gate = state.registered_gate(&id).unwrap();
    let (x, y) = gate.get_params();
    let axes = fixture_axes();
    let (xa, ya) = (axes.get(&x).unwrap(), axes.get(&y).unwrap());
    let mapper = PlotMapper::new(
        600.0,
        600.0,
        xa.axis_lower..=xa.axis_upper,
        ya.axis_lower..=ya.axis_upper,
        xa.axis_lower..=xa.axis_upper,
        ya.axis_lower..=ya.axis_upper,
        xa.transform.clone(),
        ya.transform.clone(),
    );
    let moved = gate
        .replace_point(to, 0, None, &mapper)
        .expect("a quadrant's centre can be dragged");
    (id, Arc::from(moved))
}

#[test]
fn a_quadrant_moved_for_one_sample_comes_back_on_that_sample_only() {
    use clingate::gate_editor::gates::gate_store::GateSubStore;
    let mut state = import(&fixture(FIXTURE));
    let (id, moved) = dragged_quadrant(&state, (1.0, 2.0));
    let before_other = quarters_for(&state, &id, "sample2");
    state.place_gate(
        &GateSubStore::ids_for(&moved, &id),
        &moved,
        &GateSource::Sample((id.clone(), Arc::from("sample1"))),
    );
    let placed = quarters_for(&state, &id, "sample1");
    assert!(
        !same_quarters(&placed, &before_other),
        "the drag must move the quarters, or this compares a quadrant with itself"
    );

    let back = saved_and_reopened(&state, "quadrant-sample");
    let (one, two) = (
        quarters_for(&back, &id, "sample1"),
        quarters_for(&back, &id, "sample2"),
    );
    assert!(
        same_quarters(&one, &placed),
        "sample1: {one:?}\nplaced: {placed:?}"
    );
    assert!(
        same_quarters(&two, &before_other),
        "sample2: {two:?}\nwas: {before_other:?}"
    );
}

#[test]
fn a_quadrant_moved_for_one_specimen_comes_back_on_its_samples_only() {
    let mut state = import(&fixture(FIXTURE));
    let (id, moved) = dragged_quadrant(&state, (2.5, 0.5));
    let before_other = quarters_for(&state, &id, "sample1");
    place_for_specimen(
        &mut state,
        &id,
        &MetaDataKey {
            parameter: Arc::from("test"),
            group: Arc::from("two"),
        },
        &moved,
    );
    let placed = quarters_for(&state, &id, "sample2");
    assert!(!same_quarters(&placed, &before_other));

    let back = saved_and_reopened(&state, "quadrant-specimen");
    assert!(same_quarters(&quarters_for(&back, &id, "sample2"), &placed));
    assert!(same_quarters(
        &quarters_for(&back, &id, "sample1"),
        &before_other
    ));
}

/// Omiq keeps one grouping column per gate; the session can hold positions
/// under several. The export must name a column only if grouping by it gives
/// every file the position the session gives it - otherwise reopening hands a
/// whole group whichever of its files is read first.
///
/// Here both files share a donor. Positions, oldest first: the donor's (both
/// files), sample1's specimen's under `test`, and another donor's (neither
/// file). The newest column is Donor, but grouping by it would put sample1
/// back at the donor's position; `test` separates the two correctly.
#[test]
fn the_export_names_a_grouping_column_only_if_it_holds_every_files_position() {
    let mut metadata = fixture_metadata();
    for file in ["sample1", "sample2"] {
        let mut columns = metadata.get(&Arc::from(file) as &Arc<str>).unwrap().clone();
        columns.insert(Arc::from("Donor"), Arc::from("D"));
        metadata.insert(Arc::from(file), columns);
    }
    let mut state = GateState::default();
    state
        .upload_gates_from_file(fixture(FIXTURE), &metadata, fixture_axes())
        .unwrap();
    let id: GateId = Arc::from("0lmI");
    let global = state.registered_gate(&id).unwrap();
    let x = global.get_params().0;
    let from = extent_on(&global.get_gate_ref(None).unwrap().geometry, &x)
        .unwrap()
        .0;
    let at = |to: f32| translate_edge_to(&global, &x, Bound::Above, to as f64).unwrap();
    let key = |column: &str, group: &str| MetaDataKey {
        parameter: Arc::from(column),
        group: Arc::from(group),
    };
    let (donor, specimen) = (from + 1.0, from + 2.0);
    place_for_specimen(&mut state, &id, &key("Donor", "D"), &at(donor));
    place_for_specimen(&mut state, &id, &key("test", "one"), &at(specimen));
    place_for_specimen(&mut state, &id, &key("Donor", "E"), &at(from + 3.0));

    let edge = |s: &GateState, file: &str| {
        let g = s.gate_for_file(&id, &Arc::from(file), &metadata).unwrap();
        extent_on(&g.get_gate_ref(None).unwrap().geometry, &x)
            .unwrap()
            .0
    };
    assert!(close(edge(&state, "sample1"), specimen), "the premise");
    assert!(close(edge(&state, "sample2"), donor), "the premise");

    let written = to_omiq_document(&state, &metadata, &fixture_axes()).unwrap();
    let path = scratch("grouping-column").join("saved.omiqgt");
    std::fs::write(&path, serde_json::to_string(&written).unwrap()).unwrap();
    let mut back = GateState::default();
    back.upload_gates_from_file(path, &metadata, fixture_axes())
        .unwrap();
    assert!(
        close(edge(&back, "sample1"), specimen),
        "sample1 came back at {}, was at {specimen}",
        edge(&back, "sample1")
    );
    assert!(
        close(edge(&back, "sample2"), donor),
        "sample2 came back at {}, was at {donor}",
        edge(&back, "sample2")
    );
}
