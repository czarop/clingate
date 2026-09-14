# TODO

Open work, roughly in the order it needs doing. Items carry enough context to
be picked up cold. Read `HANDOVER.md` first for the architecture and the format
facts these depend on.

**Suggested order:** the ellipse rotation bug (the rectangle one is fixed),
then ghost collection, then autogating. The Omiq load test is the user's to do
and gates everything in Export.

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

- [ ] **Ellipse rotation only flips.** Reported from testing.

      The maths is not at fault, and neither is this branch: `rotate_gate`,
      `calculate_projected_radii`, `update_ellipse_geometry` and both node
      helpers are byte-identical at `273645d`, and a probe of the pure layer
      sweeps continuously - pointer at 100 degrees gives 10, at 120 gives 30, at
      150 gives 60. Angles round-trip exactly, because
      `create_ellipse_geometry` derives the angle as `atan2(right - centre)`,
      which inverts `calculate_ellipse_nodes_y_up`. So the TODO's original guess
      - that a rotation snaps to the principal axis - is wrong.

      What disagrees is the handle. Its data position in `ellipse_gate.rs` is
      `(cx, cy + ry)`, fixed and independent of `angle`, and `draw_gates` then
      rotates the *rendered* handle by an SVG transform of `-angle`. With the y
      axis inverted that lands it at data angle `90 - angle`, and mousedown
      reads the rendered position back through `pixel_to_data` - so grabbing the
      handle on an ellipse at angle t computes `-t` and mirrors it. At t = 0 it
      is a no-op, which is why a fresh axis-aligned ellipse looks fine. During a
      drag the transform then adds `rotation_deg()` on top of an `angle` that is
      already being updated, rotating the handle twice per frame.

      Smallest fix: make the handle's data position track the angle
      (`(cx - ry*sin t, cy + ry*cos t)`) and drop the now-redundant SVG rotate.
      That touches neither the geometry nor `source_handles`, so the
      byte-identical export of an untouched import is unaffected - which matters
      while the Omiq load test below is still outstanding.

      Related but separate: the two node helpers disagree in y.
      `calculate_ellipse_nodes` puts "top" at `(cx, cy - ry)` and
      `calculate_ellipse_nodes_y_up` at `(cx, cy + ry)`. `try_new` uses the
      first for the drawn points, `update_ellipse_geometry` the second for the
      geometry. Resizing tolerates it because `calculate_projected_radii` takes
      `abs()`, so it has been invisible. Worth reconciling, carefully, since it
      moves the drawn handles for every ellipse.

- [x] **A rectangle or line gate's right point could not cross its left.**
      Not a clamp and not a regression from this branch - every function on the
      drag path was byte-identical at `273645d`. `create_rectangle_geometry`
      normalises to min/max, so each rebuild rewinds the corners into a fixed
      order while the drag held the `point_index` it captured on mousedown. Once
      the pointer crossed, that index named a different corner and the gate
      collapsed into a sliver trailing it.

      The drag now carries an anchor - the corner or edge that must not move -
      read once before the first write and held for the rest of the drag.
      `drag_anchor` is `None` on the trait by default; only the two geometries
      stored as a normalised min/max rectangle override it. The rectangle's drag
      preview had the same stale-index assumption and now uses the drag's
      anchor too.

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
on it silently stops updating. Three bit that way: the axis selectors, the gate
resolver, and the filtered frame.

The fourth was the opposite mistake. Fixing the filtered frame by tracking the
whole resolver - which is rebuilt on every gate write - made every gate edit
anywhere re-filter the dataframe and rebuild the event index for every open
plot. A plot's data depends on its own gating chain and nothing below it, so it
now tracks a memo over just that chain. Too wide a dependency is as much a bug
as too narrow a one, and only the second kind is visible as a stale screen.

- [x] **Audit the reactive closures for the same shape.** Went through all 38
      `use_memo` / `use_effect` / `use_resource` closures. Found one more real
      bug - the filtered dataframe peeked the resolver, so dragging a parent
      gate moved its outline but left every plot below it showing the old
      population - and one fragile-but-correct case, the event index, now
      tracked too. The two deliberate peeks (`upload_succeded`,
      `axes_initialised`) are latches, and `match_gates_to_plot` peeks on
      purpose; all three are commented as such.
- [x] **A Dioxus test runtime.** It is not hard: `dioxus-signals` tests its own
      reactivity with a headless `VirtualDom`, a run counter and
      `render_immediate(&mut NoOpMutations)`, and so do we now - no renderer, no
      GTK, running in the ordinary `--no-default-features` suite.
      `reactivity_tests.rs` pins all three shapes: a peeked dependency that
      never arrives, a wide one that puts every write on the expensive path, and
      a narrow memo that shields the expensive work.

      Two mechanics to know, both of which produce a vacuously passing test if
      missed: a memo is lazy, so its closure only re-runs when the value is
      *read* after invalidation; and a chain of memos needs one extra render
      pass per level to propagate.

- [ ] **Extend it to `PlotWindow` itself.** The tests above cover the pattern,
      not the component. Standing up the real thing means providing its four
      contexts - gate, metadata, axis and plot stores - which is fixture work
      rather than anything novel. The assertion worth having: editing a gate off
      a plot's chain does not re-run its filtered-frame resource. That is this
      session's bug, stated directly.

## Housekeeping

- [ ] **Consider committing `Cargo.lock`.** It is gitignored, but clingate is
      an application, not a library. A stale lock on one machine pinned
      `ethnum` 1.5.2, which fails to build on current Rust.
