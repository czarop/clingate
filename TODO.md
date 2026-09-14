# TODO

Open work, roughly in the order it needs doing. Items carry enough context to
be picked up cold.

## Linked gates

Omiq keys nodes separately from filter containers, so one gate can be applied
at several points in the tree. A real export had 250 node placements over 146
containers: 38 linked, one of them at nine points.

Export already preserves all 250 (`OmiqRebuildData.nodes` is a `Vec`). The
editor does not model them: `GateHierarchy` is single-parent, and `add_child`
unlinks the previous parent before adding, so the last node processed wins and
the other placements vanish.

- [x] **Stage 1 - split placement from gate.** `NodeId` newtype, `GatePlacement`,
      and the `placements` / `nodes_by_gate` tables in `GateState`. Node ids
      equal gate ids for now, so no behaviour change.
- [ ] **Stage 2 - a node per Omiq `GatingNode` on import.** In
      `gate_store.rs`, `upload_gates_from_file` keys the tree on
      `node.filter_container_id`; key it on `node.id` instead, and take the
      parent straight from `node.parent_id` rather than through the
      `node_to_gate_id` indirection, which then goes away. The sidebar already
      recurses through `get_children`, so placements render with no change
      there beyond keying `GateNode` on the node and looking the gate up.
      `get_chain_to_root` becomes node-scoped, which also fixes a real bug: a
      linked gate's statistics are currently computed against one arbitrary
      parent chain.
- [x] **Stage 3 - link / unlink / delete-one-instance.** `link_node_to_gate`,
      `unlink_node` and `delete_placement` on `GateState`, wired to the
      hierarchy pane's right-click menu, with a pick mode for choosing a link
      target.
- [ ] **Confirm before linking.** Linking discards the right-clicked gate's own
      geometry, and there is no undo. The discarded gate is kept as a ghost
      rather than deleted, so it is still in the document, but nothing in the UI
      can bring it back. Wants a confirmation step before the link is applied.
- [ ] **Linking composites.** Refused for now: a composite is registered under
      its own id *and* each corner's, and Omiq treats the group as
      all-or-nothing, so a copy has to mint an id per corner and rewrite the
      `groupId` that ties them together. `DrawableGate::with_new_id` returns
      `None` for them, which is what the refusal keys off.

Names are shared per gate, matching Omiq, which stores the name on the
container and not on the node. Per-placement names are not representable in a
gating file.

## Ghost containers

A gate with no node but which a live boolean still references. Omiq leaves
these behind and we keep them verbatim so the boolean stays evaluable.
`GateState::is_ghost` is the predicate.

- [ ] **Collect ghosts when nothing references them.** Deleting the last
      boolean that references a ghost leaves it stranded: nothing sweeps it.
      They accumulate for the life of a session and are all written back on
      export. Needs a reachability sweep on delete - the same walk the importer
      already does to decide which containers are reachable.

## Export

- [ ] **Verify Omiq actually accepts a clingate-written file.** Everything
      else rests on this and only a real load test settles it. The round trip
      proves clingate and our model of the format agree with each other, not
      that Omiq agrees with them.
- [ ] **Name new composite corners `Q1..Q4`.** They currently default to the
      raw subgate id (`{uuid}_BL`). Editor-side naming, not a writer bug.
- [ ] **Choose a placement when adding a child under a linked parent.** Today
      it attaches to the first placement silently. Stage 2 makes this
      well-defined; until then it is a guess.

## Autogating

The point of the project, and entirely unstarted.

- [ ] **Wire `gate_move` to the gate store.** The shift/drift machinery is
      tested in isolation (41 tests) but nothing calls it from the editor.
- [ ] **Define the ruleset format** - how a gate is told to follow a
      population between samples, and where that is stored.

## Housekeeping

- [ ] **Consider committing `Cargo.lock`.** It is gitignored, but clingate is
      an application, not a library. A stale lock on one machine pinned
      `ethnum` 1.5.2, which fails to build on current Rust.
