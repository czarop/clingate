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
- [ ] **Link composites as a group.** Omiq does link them: in a real export a
      skewed quadrant has all four corners placed under the same three parents,
      each corner keeping its own `ord`. So a composite link is one action over
      four containers. Linking needs no copying - it only re-points nodes - so
      the work is lifting the operation from the node to the group: resolve the
      other corners through `get_inner_gate_ids()`, match corner to corner on
      the `groupId` suffix (`_QUAD0` to `_QUAD0`), reject a mismatched arity
      (a quadrant to a bisector), and apply all four or none. The right-click
      menu acts on the group from a click on any one corner.
- [ ] **Unlink composites.** The hard half: a copy needs a new composite id,
      a new id per corner, and a new `groupId` tying them together, then all
      corners re-pointed. Wants a `with_new_group_id` alongside
      `with_new_id`.
- [ ] **`is_ghost` reports every composite.** A composite has no container of
      its own in the file - only its corners do - but clingate registers it
      under its own id with no node, so the predicate calls it a ghost. Not
      reachable today, but it would make the ghost-collection sweep try to
      collect every composite.
- [ ] **Linking composites (superseded by the two items above).** Refused for now: a composite is registered under
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

## Reactivity

The store refactor moved plain-data logic off the Dioxus lenses. Where a Store
wrapper then reads through `peek`, it subscribes to nothing, and any memo built
on it silently stops updating. Two have bitten already: the axis selectors, and
the gate resolver.

- [ ] **Audit the remaining Store wrappers for the same shape.** A `peek` inside
      a wrapper that a `use_memo` depends on is the pattern. There is no test
      coverage for reactivity at all - the store logic is tested without a
      runtime, which is exactly why these got through.
- [ ] **Consider a Dioxus test runtime** for the handful of memos that matter
      (resolver, axis index, gate list), so a lost subscription fails a test
      rather than being found by hand.

## Housekeeping

- [ ] **Consider committing `Cargo.lock`.** It is gitignored, but clingate is
      an application, not a library. A stale lock on one machine pinned
      `ethnum` 1.5.2, which fails to build on current Rust.
