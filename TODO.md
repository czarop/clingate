# TODO

Open work, roughly in the order it needs doing. Items carry enough context to
be picked up cold. Read `HANDOVER.md` first for the architecture and the format
facts these depend on.

**Suggested order:** autogating - the gate-editing bugs, the `is_ghost` false
positive and ghost collection are all done. The Omiq load test is the user's to
do and gates everything in Export.

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
- [x] **`is_ghost` reported every composite.** A composite has no container of
      its own in the file - only its corners do - but clingate registers it
      under its own id with no node, so counting placements on that key always
      answered zero. It now asks whether any corner is on the tree, which is
      the question that was meant. Would have had the sweep below collect every
      composite.

Names are shared per gate, matching Omiq, which stores the name on the
container and not on the node. Per-placement names are not representable in a
gating file.

## Gate editing

Both reported bugs are resolved. Ellipse rotation was investigated at length
and works correctly in use - the probe of the pure layer sweeps continuously
and the handle behaves - so the entry was removed rather than left as a
standing doubt. The two ellipse node helpers do order their minor-axis pair
opposite ways round; that is now documented in both, and it is harmless,
because the pair is symmetric about the centre and the resize path takes
`abs()`.

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

- [x] **Collect ghosts when nothing references them.**
      `collect_stranded_ghosts` runs the importer's reachability idea over the
      live store: start from every gate holding a position in the tree, follow
      booleans to their operands to a fixed point, and drop whatever the walk
      does not reach - from the registry, both override tiers, the rebuild
      entries and the boolean link table. Called at the end of `remove_gate`,
      which is where the triggering case lives: deleting the last boolean that
      referenced a ghost.

      One deliberate limit. `omiq_rebuild.ghost_containers` is left alone. Those containers were
      already unreachable in the file Omiq wrote and are kept verbatim so a
      round trip returns the document it was given; sweeping them on this rule
      would drop every one on the first delete, and
      `unreachable_containers_are_kept_verbatim` asserts otherwise. Collecting
      what this session stranded is a different thing from discarding what Omiq
      shipped.

      `delete_placement` sweeps too, after the whole composite group rather
      than inside the loop - a composite is still reachable while any corner
      holds a position, so a sweep between corners would see a half-deleted
      group and do nothing.

      The test that used to say a ghost is always kept was asserting the
      mechanism rather than the reason for it: it ran against a fixture with no
      boolean in it. It now turns on whether anything actually reaches the gate,
      which is the only question that matters - one test for a ghost a boolean
      still needs, one for a gate nothing reaches.

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

## Gate rules

The rules approach to autogating: a written rule says where a gate belongs, it
is evaluated on one sample, and the position is applied to that sample's
partners. `src/gate_rules/` holds it. The population-drift approach in
`gate_move` is separate and still unwired - see Autogating below.

**A rule belongs to a parameter, never to an axis.** Which axis a marker is
drawn on is a property of the plot and has to be read off the gate every time.
CD279 is usually on y and CD134 on x, and in one real file CD279 appears on
both - so a rule that records "x lower" is wrong the moment the same marker is
plotted the other way round. The harness learned this the hard way: reading the
x edge unconditionally made it read the wrong marker entirely, and gates came
out capturing their whole parent population. Nothing in `rule.rs` names an
axis, and nothing should.

- [x] **Solve for a threshold.** `tail_fraction` for "the FMO gate should hold
      0.2% to 0.5%", `percentile_offset` for a visual step above a percentile.
      Pure functions over a slice of f64.
- [x] **A rule is a type,** with a confidence model of its own.
- [x] **Score against a hand-gated workflow.** `harness_tests.rs`, gated on four
      environment variables. Over 50 placements of the four gates that really
      use the FMO band rule, the confident ones agree with hand placement to a
      median of 0.054 arcsinh units against a typical hand adjustment of 0.18,
      and 97% fall within one such adjustment. The 15 it held back were mostly
      flagged on event count, with parent populations of 38 to 368.

- [ ] **A third rule: the edge of the negative peak.** GranzymeB and Ki67 in
      this panel are gated by finding where the negative population ends and
      sitting just above it - no FMO involved, and no fraction to aim at. It is
      why both came out 0.46 arcsinh units from hand placement under a tail
      fraction rule, which is the right answer to the wrong question.

      Distinct from `percentile_offset`, which steps a *fixed* distance above a
      percentile: this has to find the edge, so it needs the density rather than
      an order statistic. The KDE in `gate_move` is the obvious starting point.

- [ ] **A rule store.** Attach a `Rule` to a gate - its band, its parameter,
      which sample role it is measured on - and persist it to a sidecar, since
      Omiq has no representation for rules. The harness currently hardcodes one
      band for every gate, which measures the band rather than the solver. This
      is the bookkeeping that stands between the solvers and real use.

- [ ] **The Gate Rules tab.** Choose a rule type per gate and set its
      parameters. Wants the rule store first.

- [ ] **Apply a solved threshold to a gate.** Rebuild the geometry and write it
      through `insert_for_source` with `GateSource::Sample`. Real files
      *translate* the rectangle rather than resize it: across 41 gates over 117
      files the width stayed constant to seven figures while the bounding edge
      moved.

## Autogating

The point of the project, and entirely unstarted.

The second of the two approaches: measure how far a population moved from a QC
sample and carry the gate with it. Unwired. The rules approach above is the
direct replacement for how gating is done by hand today and is where the work
has gone so far.

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
