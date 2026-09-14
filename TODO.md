# TODO

Open work, roughly in the order it needs doing. Items carry enough context to
be picked up cold.

## Linked gates

Omiq keys nodes separately from filter containers, so one gate can be applied
at several points in the tree. A real export had 250 node placements over 146
containers: 38 linked, one of them at nine points.

Import, the editor and export now all agree on this. What is left is
composites, and the safety of the link action itself.

- [x] **Stage 1 - split placement from gate.** `NodeId` newtype, `GatePlacement`,
      and the `placements` / `nodes_by_gate` tables in `GateState`. Node ids
      equal gate ids for now, so no behaviour change.
- [x] **Stage 2 - a node per Omiq `GatingNode` on import.** The tree is keyed
      on `node.id`; 250 placements rather than 146 on a real file. Chains are
      node-scoped, which fixed a linked gate's statistics being computed
      against one arbitrary parent chain.
- [x] **Fold the exporter onto the node table.** One Omiq node per position in
      the tree, so an edit made here reaches the file.
- [x] **Stage 3 - link / unlink / delete-one-instance.** `link_node_to_gate`,
      `unlink_node` and `delete_placement` on `GateState`, wired to the
      hierarchy pane's right-click menu, with a pick mode for choosing a link
      target.
- [ ] **Confirm before linking.** Linking discards the right-clicked gate's own
      geometry, and there is no undo. The discarded gate is kept as a ghost
      rather than deleted, so it is still in the document, but nothing in the UI
      can bring it back. Wants a confirmation step before the link is applied.
- [x] **Link composites as a group.** `link_composite` re-points every corner
      to the matching corner of the target, matched by position in
      `get_inner_gate_ids` (a fixed geometric order), all-or-nothing, refusing a
      mismatched arity or a composite-to-single link. Deleting one instance of a
      composite takes the whole group with it, since a three-cornered quadrant
      is not a gate. Checked against the real export's linked skewed quadrant.
- [ ] **Unlink composites.** The hard half, and the asymmetry a user will hit:
      a composite can now be linked but not unlinked. A copy needs a new
      composite id, a new id per corner, and a new `groupId` tying them
      together, then all corners re-pointed. Wants a `with_new_group_id`
      alongside `with_new_id`. Until then `unlink_node` refuses with "this kind
      of gate cannot be copied", shown in the pane.
- [ ] **`is_ghost` reports every composite.** A composite has no container of
      its own in the file - only its corners do - but clingate registers it
      under its own id with no node, so the predicate calls it a ghost. Not
      reachable today, but it would make the ghost-collection sweep try to
      collect every composite.

Names are shared per gate, matching Omiq, which stores the name on the
container and not on the node. Per-placement names are not representable in a
gating file.

## Gate editing

Reported from testing, not yet investigated.

- [ ] **Ellipse rotation only flips.** Rotating an ellipse no longer follows the
      pointer through intermediate angles - it jumps straight to a complete
      flip. Suspect `rotate_gate` in `ellipse_gate.rs`, and the interaction with
      `source_handles`: rotation drops the imported handles and re-derives a
      principal-axis pair, which was changed when Omiq's four control points
      were preserved. Check whether the re-derivation is snapping the angle
      rather than taking the pointer's.
- [ ] **A rectangle or line gate's right edge cannot cross its left.** Dragging
      the right-hand point past the left-hand one is refused, so a gate cannot
      be inverted or dragged through itself. Expect a min/max assumption in
      `replace_point` that needs the two swapped rather than clamped.

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
- [x] **Choose a placement when adding a child under a linked parent.** The
      sidebar passes the node, so a child attaches to the position the user was
      looking at. `as_parent_node` still resolves a bare gate id to its first
      position, for callers that have not been converted.

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

- [x] **Audit the reactive closures for the same shape.** Went through all 38
      `use_memo` / `use_effect` / `use_resource` closures. Found one more real
      bug - the filtered dataframe peeked the resolver, so dragging a parent
      gate moved its outline but left every plot below it showing the old
      population - and one fragile-but-correct case, the event index, now
      tracked too. The two deliberate peeks (`upload_succeded`,
      `axes_initialised`) are latches, and `match_gates_to_plot` peeks on
      purpose; all three are commented as such.
- [ ] **Consider a Dioxus test runtime** for the handful of memos that matter
      (resolver, axis index, gate list), so a lost subscription fails a test
      rather than being found by hand.

## Housekeeping

- [ ] **Consider committing `Cargo.lock`.** It is gitignored, but clingate is
      an application, not a library. A stale lock on one machine pinned
      `ethnum` 1.5.2, which fails to build on current Rust.
