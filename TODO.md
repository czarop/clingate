# TODO

Open work, roughly in the order it needs doing. Items carry enough context to
be picked up cold. Read `HANDOVER.md` first for the architecture and the format
facts these depend on.

**Suggested order:** the two gate-editing bugs (they block normal use), then
ghost collection, then autogating. The Omiq load test is the user's to do and
gates everything in Export.

## Linked gates

Omiq keys nodes separately from filter containers, so one gate can be applied
at several points in the tree. A real export had 250 node placements over 146
containers: 38 linked, one of them at nine points.

Import, the editor and export now all agree on this, for single gates and for
composites. **This section is done** apart from the `is_ghost` note below,
which belongs to ghost collection rather than to linking.

Worth exercising by hand, since none of the Dioxus wiring is covered by tests:
link two gates and check both rows carry the badge; navigate to the *second*
placement of a linked gate (that path was broken until late in the session);
link two quadrants and confirm all four corners move together; unlink one and
confirm you get an independent quadrant.

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
- [x] **Confirm before linking.** Picking a target no longer writes anything: a
      confirmation names both gates and says whose geometry is discarded, and
      refusals are shown in the pane rather than printed to a console. The
      discarded gate is kept as a ghost, so it is still in the document, but
      nothing in the UI brings it back - there is still no undo.
- [x] **Link composites as a group.** `link_composite` re-points every corner
      to the matching corner of the target, matched by position in
      `get_inner_gate_ids` (a fixed geometric order), all-or-nothing, refusing a
      mismatched arity or a composite-to-single link. Deleting one instance of a
      composite takes the whole group with it, since a three-cornered quadrant
      is not a gate. Checked against the real export's linked skewed quadrant.
- [x] **Unlink composites.** `with_new_group_id` on all three composite types
      mints an id for the group and a positional one per corner (`_BL`, `_BR`,
      `_TR`, `_TL`; `_L`, `_R`), and `unlink_composite` re-points every corner
      at that plot to the copy. Positional because an imported composite's
      corners carry Omiq's container ids, which say nothing about which corner
      they are.
- [ ] **`is_ghost` reports every composite.** A composite has no container of
      its own in the file - only its corners do - but clingate registers it
      under its own id with no node, so `is_ghost` calls it a ghost. Harmless
      today because nothing calls it in anger, but it would make the
      ghost-collection sweep above try to collect every composite.

      Fix by excluding a gate whose `get_id()` differs from the key it is
      registered under, or whose corners have placements. Wants a test that a
      freshly added quadrant is not a ghost.

Names are shared per gate, matching Omiq, which stores the name on the
container and not on the node. Per-placement names are not representable in a
gating file.

## Gate editing

Reported from testing, not yet investigated.

- [ ] **Ellipse rotation only flips.** Reported from testing. Rotating an
      ellipse no longer follows the pointer through intermediate angles - it
      jumps straight to a complete flip.

      Start at `EllipseGate::rotate_gate` in
      `src/gate_editor/gates/gate_single/ellipse_gate.rs`, and at its
      interaction with `source_handles`. This session added `EllipseHandles` so
      an unedited imported ellipse exports byte-identically to what Omiq wrote;
      a rotation drops those handles and re-derives a principal-axis pair. The
      degenerate-case handling was also hardened then (a circle reads 0 rather
      than an arbitrary eigenvector; the axis-aligned test uses a scaled
      tolerance instead of `f == 0.0`). Check whether the re-derivation is
      snapping the angle to the principal axis rather than taking the
      pointer's. `gate_single_tests.rs` and `gate_drag_tests.rs` cover rotation
      about a pivot - extend those rather than starting fresh.
- [ ] **A rectangle or line gate's right point cannot cross its left.**
      Reported from testing. Dragging the right-hand point past the left-hand
      one is refused, so a gate cannot be dragged through itself.

      Expect a min/max assumption in `replace_point` on `RectangleGate` /
      `LineGate` that clamps where it should swap: once the points cross, the
      "right" point is the smaller one and the geometry needs rebuilding with
      them exchanged, not pinned. Check `create_rectangle_geometry` too - the
      `Rectangle { min, max }` geometry is order-dependent and
      `filter_events_to_mask` compares `gt(minx) & lt(maxx)`, so an inverted
      rectangle would admit nothing.

## Ghost containers

A gate with no node but which a live boolean still references. Omiq leaves
these behind and we keep them verbatim so the boolean stays evaluable.
`GateState::is_ghost` is the predicate.

- [ ] **Collect ghosts when nothing references them.** Deleting the last
      boolean that references a ghost leaves it stranded: nothing sweeps it.
      They accumulate for the life of a session and are all written back on
      export.

      The machinery already exists: `collect_reachable` in `gate_store.rs` is
      the walk the importer uses to decide which containers a live node or
      boolean can reach. Run the same walk after a delete and drop anything it
      does not reach. Two cautions: fix the `is_ghost` composite false positive
      below first, or the sweep will try to collect every composite; and
      `omiq_rebuild.ghost_containers` holds raw JSON for containers that were
      already nodeless at import, which must be swept on the same rule rather
      than kept forever.
      A real export had 6 ghosts on import, so this is not hypothetical.

## Export

- [ ] **Verify Omiq actually accepts a clingate-written file.** THE open
      question. Everything else rests on it and only a real load test settles
      it - the round trip proves clingate and our model of the format agree
      with each other, not that Omiq agrees with either. This is the user's to
      do. Until it passes, treat every export-side decision as provisional.
- [ ] **Name new composite corners `Q1..Q4`.** A composite created in the
      editor names its corners after their ids (`{uuid}_BL`), so the hierarchy
      pane shows a UUID. Omiq names them for the populations they select
      (`CD45RA-CD197+` and so on). Editor-side naming, not a writer bug: see
      `subgate_names` in the composite constructors, which already takes names
      when the importer supplies them.
- [x] **Choose a placement when adding a child under a linked parent.** The
      sidebar passes the node, so a child attaches to the position the user was
      looking at. `as_parent_node` still resolves a bare gate id to its first
      position, for callers that have not been converted.

## Autogating

The point of the project, and entirely unstarted.

This is the point of the project and has had no attention. Everything built so
far - import, the node model, export - is the substrate it needs.

- [ ] **Wire `gate_move` to the gate store.** `src/gate_move/` holds KDE, peak
      finding, bandwidth selection, population-shift recovery, smear scoring,
      density binning and FFT cross-correlation, with 41 asserting tests. None
      of it is called from the editor. The join is: for a gate on a QC sample,
      compute the shift to the same population on another sample, and write the
      moved gate as a **sample override** - `GateSubStore::insert_for_source`
      with `GateSource::Sample`, which is the tier the importer already uses for
      Omiq's `perFileFilters`. So the output has somewhere to go that already
      round-trips.
- [ ] **Define the ruleset format** - how a gate is told to follow a population
      between samples, and where it is stored. Open questions: per gate or per
      gate-and-channel; which `gate_move` strategy (peak, quantile, smear) and
      its parameters; whether rules are exported to Omiq at all (they have no
      representation in a gating file, so probably a clingate-side sidecar).
- [ ] **Export scaling and metadata.** The user has said these should eventually
      be written back too, alongside gate positions. Not started; the importers
      are in `axis_store.rs` (`read_axis_configs`) and `omiq/metadata.rs`
      (`parse_metadata_csv`), and both are plain functions, so a writer can be
      tested the same way.

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
