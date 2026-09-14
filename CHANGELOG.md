# Changelog

## Unreleased

### Fixed

- **Y axis transform read from the X axis.** `extract_axis_range_from_axis_settings`
  returned `x_axis.transform` for both axes, so every quadrant and skewed-quadrant
  gate imported with the wrong infinite bounds on Y whenever the two axes used
  different scaling.
- **Gate deletion left the tree inconsistent.** `remove_gate` read each doomed
  gate's parent *after* `delete_subtree` had already unlinked it, so the view-index
  key was built against the root and deleted gates kept rendering on their plot. It
  also dropped position overrides for the requested gate only, never for its
  descendants, and never removed a composite's own registry entry - leaving deleted
  composites present in every resolver built from the registry.
- **Boolean gates were unusable as soon as they were created.** The sidebar creates
  AND/OR gates with a single operand, but `filter_events_to_mask` rejected fewer
  than two, so using a freshly created boolean gate as a parent always errored. A
  single operand now folds to that operand's own mask.
- **Unsupported Omiq gate types aborted the whole import.** `GateSerialized::get_params`
  called `panic!` and `to_drawable` called `todo!()` on the `Unknown` catch-all that
  exists specifically to future-proof against new Omiq gate types. Both now report
  an error. `get_params` returns `Option`.
- **Gate-move errors panicked the desktop app.** The mouse handlers in `draw_gates`
  used `.expect("Gate Move Failed")`; they now log and carry on. An unguarded
  `unwrap` on the plot mapper in `onmouseup` was made consistent with the guarded
  path directly below it.
- **Numerical robustness in `gate_move`.** `kde_1d` divided by a zero bandwidth and
  by an empty population, filling the grid with NaN that propagated silently into
  every downstream shift and drift classification; `n_points` of 0 underflowed the
  step divisor. `kde_peak` panicked on NaN densities and indexed an empty slice via
  the eagerly-evaluated `unwrap_or` fallback. `silverman_bandwidth` returned zero
  for a population whose middle 50% is identical. `quantile` underflowed on an
  empty slice. Sort comparators no longer `unwrap` a `partial_cmp`.
- **Composite position overrides were never updated on edit.** The write loop
  iterated all of a composite's ids but inserted under the resolved gate's own
  key each time, so only the composite's entry moved. Import creates per-subgate
  override entries and filtering resolves subgates by id, so a composite moved on
  an overridden sample rendered in its new position while still gating the old
  one. The same mistake was in `match_gates_to_plot`, where the inner binding
  also shadowed the outer one.
- **Gates added with no parent were unreachable.** `add_gate` keyed the view
  index on a bare `None` while parenting the gate under `ROOTGATE`;
  `remove_gate` and `get_gates_for_plot` both ask for `Some(ROOTGATE)`, so such
  a gate could never be found again to redraw or delete.
- **Every channel without a marker name was dropped from the scaling import.** A
  blank "Feature Name (Secondary)" reads back as null, and `?` on it discarded
  the whole row - losing FSC, SSC and Time, which then silently fell back to
  default axis settings instead of the exported ones.
- **An unsupported scaling type aborted the scaling import.** `unreachable!()`
  on an unrecognised value meant one unsupported row cost every other axis in
  the file; such rows are now skipped with a warning.
- **Degenerate ellipse reconstruction.** A circle read its rotation off an
  arbitrary eigenvector, and the axis-aligned test compared `f == 0.0` exactly,
  so an ellipse aligned to within float noise took `atan2` with two near-zero
  arguments.
- **Both axis selectors opened on the wrong channel.** The memo backing each
  selector's index read the axis store with `peek`, which does not subscribe,
  so it kept the value computed before the scaling export had loaded - 0 - and
  never recomputed. Both selectors therefore displayed whichever channel the
  export listed first, regardless of the axes actually in use. It also matched
  on the whole `Param`, which carries the marker name from the export, so the
  hardcoded FSC-A/SSC-A defaults could never match by equality. Now matched on
  the channel, against a subscribed read.
- **Broken doctests in `gate_hierarchy`.** All twelve imported `flow_gates::GateHierarchy`
  (the type lives in this crate) and called `add_child` without its `order`
  argument, so `cargo test` failed on the doc tests alone.

### Added

- Test suite for `GateHierarchy` (35 tests): relationships, ancestry and gating
  chains, sibling ordering by Omiq's `ord`, cycle rejection, reparenting,
  the several deletion modes, and topological ordering.
- Asserting test suite for `gate_move` (41 tests): KDE, peak finding, bandwidth
  selection, population-shift recovery, smear scoring, density binning, FFT
  cross-correlation and the `GateRules` constraints - including a regression test
  for each numerical fix above. The pre-existing `flow_tests` modules print their
  results and pass unconditionally, including on `Err`, so they cannot catch a
  regression; they are left in place as exploratory harnesses.
- Test suite for the Omiq interchange layer (`src/omiq/tests.rs`, 30 tests): tree
  and node parsing, every gate geometry Omiq emits, the `Unknown` catch-all,
  composite group-id classification, per-file and per-group override routing,
  boolean operand resolution through nested compounds, skewed-quadrant winding,
  and ellipse reconstruction from Omiq's four control points.
- Test suite for the axis transforms (`axis_info`, 24 tests): arcsinh round-trips,
  cofactor changes preserving raw positions, and axis-limit edits.
- Test suite for event filtering (`gate_filtering_tests.rs`, 21 tests): rectangle,
  polygon and ellipse masks including boundary and concave cases, all three
  boolean operations and their nesting, operand-resolution failures, and
  hierarchy chain narrowing. `GateOverrideResolver` has public fields, so these
  build a resolver by hand rather than standing up a Dioxus store.
- Regression tests for the per-axis transform lookup, covering the Y-axis bug
  above directly.
- Test suites for the previously untested interaction and geometry layers:
  `gate_drag` (20 tests - drag rebasing, offset sign, rotation about a pivot),
  `gate_types` (24 - shape classification, offset/restyle cloning, statistics),
  `PlotMapper` (13 - pixel/data round trips on linear and arcsinh axes, axis
  direction, hit tolerance), `gate_single` (25 - rectangle/polygon/ellipse/line
  construction, translation, vertex edits, axis transposition, hit testing),
  `gate_composite` (19 - subgate identity and count, orthogonality, handle
  derivation, clamping), and `gate_stats` (15 - counts, parent-relative
  percentages, quadrant partitioning, draft rendering at each click stage).
- `Debug` derives on `GateRenderShape`, `ShapeType`, `DrawingStyle`, `Direction`
  and `GateText`, so render shapes can be inspected and compared in assertions.
- The stores' plain-data logic moved off the Dioxus lenses onto `GateState`,
  `GateSubStore`, `AxisStore` and free parsing functions, so it runs without a
  runtime. The store methods remain thin wrappers at the same write
  granularity, leaving reactivity unchanged. 44 tests follow: store writes and
  override precedence, add/remove, and the metadata and scaling imports.
- Opening axes are chosen once, when the scaling export lands: the scatter
  pair if the file has it, otherwise the first two channels in display order,
  taking the marker names from the export rather than assuming them. A latch
  keeps it to once, so switching sample files leaves the axes and the selected
  gate where the user put them. 6 tests.
- `EllipseGate` keeps the four control points Omiq wrote (`EllipseHandles`), so
  an unedited ellipse exports byte-identically instead of a canonicalised
  equivalent. A move carries them; a rotation drops them and derives a
  principal-axis pair.

- `omiq::rebuild` captures what a gating file carries beyond geometry - the
  document header, and per gate the node id, parent, `ord`, `collapsed` flag,
  container type, `groupId`, metadata column, source gate type and per-file id
  list. Kept beside the registry keyed by gate id, so it survives every edit.
  Unreachable containers are kept verbatim as raw JSON.
- `omiq::serialise` converts a gate back into the filter Omiq stores for it,
  driven by the captured source type - a quadrant corner is a rectangle in the
  file but a polygon once imported, and a skewed corner an angle gate. Every
  gate in both fixtures round-trips against what the file actually said.
- `Serialize` on the wire types, so one definition owns the format in both
  directions. Absent optionals are omitted rather than written as null.
- `to_omiq_document` assembles a whole gating file from scratch: nodes, filter
  containers, per-file position fan-out, boolean compounds, the document header
  with its unmodelled fields, and the verbatim pass-through of unreachable
  containers.
- Linked gates are preserved. Omiq keys nodes separately from filter
  containers, so one gate can be applied at several points in the tree - in a
  real file, 38 of 146 containers were shared this way and one appeared at nine
  points. `OmiqRebuildData` records every placement.
- Gates created in the editor export as first-class Omiq gates. Previously only
  gates that came in from a file carried the node placement, `groupId` and
  source type the writer needs, so a gate drawn here was written as a filter
  container that nothing in the tree pointed at - present in the file, invisible
  in Omiq. The writer now mints a node id for every gate without a captured
  placement, resolves `parentId` through a first pass over both new and imported
  gates, takes `ord` from the hierarchy, and synthesises the `_QUAD0..3`,
  `_SPLIT0..1` and `_SKEWEDQUAD0..3` group ids a new composite needs so its
  corners stay tied together. 12 tests cover it, including re-importing a file
  written from gates that were only ever created here.
- `to_omiq_document` reports an error rather than writing a header of zeros when
  the session never imported a gating file; `to_omiq_document_with_header` takes
  one explicitly for that case.
- `a_real_gating_file_survives_a_round_trip`, gated on `OMIQ_GATING_FILE`,
  checks the whole path against a real export. Skipped when unset.

### Testing

    cargo test                        # needs the GTK system packages below
    cargo test --no-default-features  # no system packages needed

439 unit tests and 12 doctests pass either way. Every test lives in the library,
so `--no-default-features` is enough to run them: it drops dioxus's `desktop`
feature, which pulls in `gdk-sys` and probes pkg-config for `gdk-3.0`. Without
those packages a plain `cargo test` fails at that probe before running anything.

Building the desktop binary (or testing with default features) needs:

    libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev
    libayatana-appindicator3-dev librsvg2-dev
