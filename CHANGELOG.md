# Changelog

## Unreleased

### Added

- **A rule that finds a population by what it is, rather than where it sat.**
  The cells inside the gate on the reference sample are described by where they
  sit across markers you choose, and that description is used to find the same
  cells in every other sample; the gate is then fitted to wherever they turn out
  to be. For populations the threshold rules cannot reach - a smear with no dip,
  several clusters near each other, anything that moves in both axes at once.

  The markers are per rule, because which ones define a population is knowledge
  about the biology that nothing in the data supplies. MAIT cells are TCR Va7.2
  and CD161 and CD127; a monocyte marker is not wrong about them, it is silent,
  and including it spends the distance budget on noise.

  Two ways to fit, also per rule. *Keep the shape* moves and resizes the gate as
  drawn and stays the kind of gate it is, for an outline that means something the
  data does not - a quadrant, a shape agreed with somebody else, a gate that has
  to stay comparable with how it was drawn before. *Draw a polygon* traces a
  fresh boundary round the matched cells on every sample, and turns the gate into
  a polygon whatever it was.

  Nothing is normalised between samples: each one's markers are read against its
  own parent population, so donor differences are carried rather than flattened.

- **A verification table for it.** A gate drawn round the wrong cells looks
  exactly like one drawn round the right cells until you look. Per sample: how
  many cells matched against how many the hand-drawn gate held, what fraction of
  the fitted gate's contents are actually the population, how much of the
  population it holds, how many separate clouds they formed, and - per marker -
  where they sat on the reference against where they sit here. The last is the
  check that these are the same cells: a marker reading +8 on the reference and
  +1 here has not been matched on, whatever the distance said.

- **Messages are toasts.** Every report of something that just happened - a file
  written, a rule saved, a run finished, a path that did not resolve - now
  appears briefly in the corner and clears itself. They used to be inline notes
  that stayed until something else replaced them, so the editor accumulated
  stale claims like "Loaded 150 gates from ..." long after the fact, and a note
  could only be seen on the tab that wrote it - a run finishing while you were
  looking at the plots said nothing at all. Four of the editor's own reports
  were set on a signal nothing rendered, so a failed axis rescale had been
  silent entirely.

  State that describes the form *right now* stays inline, because it has to be
  readable while you act on it: which rule is being edited, how a pairing column
  resolves, how far through a run the solver is.

- **A Workspace tab, first, in place of `file_paths.txt`.** The FCS files, the
  metadata, the scaling and the gating file are chosen in the app instead of
  named by line number in a text file beside the binary and read once at
  startup. Open a folder and each part is recognised: FCS files anywhere under
  it, sub-folders included; the gating file (`.omiqgt`) and the two CSVs (with
  "metadata" and "scaling" in their names) at the top level only. A part that
  is missing, or that more than one file could be, is reported and chosen by
  hand - nothing is guessed. Each part can be replaced on its own, FCS files
  added and removed one at a time, and the gating file written from the same
  tab. Every path has a box beside its dialog, because the dialog is a service
  a machine may not be running.

  A file from a sub-folder is known in the program by its folders and its name
  joined with underscores - `Plate_10/A1.fcs` is `Plate_10_A1.fcs` - so two
  plates' `A1.fcs` cannot be confused. Nothing on disk is renamed, but the
  metadata has to use that name; a file it has no row for is flagged on the
  Workspace tab and says so in place of its plot.

  The last workspace is remembered - in the user's configuration folder, not
  beside the binary - and offered at the next launch rather than opened.
  Rules are not carried from one workspace to the next: they, and the sample
  pairing that travels with them, are exported and imported on the rules tab.

  Replacing the scaling carries the gates across: every channel whose
  transform or range changed goes through the same rescale the editor's
  cofactor and range boxes use, so drawn, per-specimen and per-sample positions
  all come through. Replacing the metadata re-imports the gating file, because
  it defines the groups per-specimen positions are keyed by. Both that and
  replacing the gating file, or opening another workspace, discard gate
  positions changed since the import, and ask first.

- **Browse buttons beside every path field**, on all three tabs: the gating file
  to load and to write, the rules sidecar to load and to save, the FCS folder,
  and the exported contact sheet. The OS file dialog, through `rfd` - already in
  the tree via dioxus-desktop, so no new dependency.

  Three dialogs, not one: choosing a file that exists is not the same dialog as
  naming one to write, and an open dialog cannot name a file that is not there
  yet - which most of these are. So the rules sidecar carries two, one on the
  field for Load and one joined to Save.

  The text field stays everywhere. A pasted path reaches a mounted share that a
  dialog makes hard work of, and the dialog is a separate D-Bus service that not
  every machine runs. Where it does not open, that is now said rather than the
  button appearing to do nothing.

- **Load a different gating file without restarting.** A Load box on the editor
  tab, beside the export one, replaces every gate with the ones in another Omiq
  file. A replacement, not an addition: uploading over a loaded document used to
  leave the old gates in the registry, unreachable from the new tree but still
  resolved into every sample and still written back out on export. The new
  document is parsed on a worker thread into a state of its own and swapped in
  only once it has read, so a mistyped path leaves what is on screen alone. The
  selected position returns to the root, since a node id from the discarded
  document answers to nothing in the new one.

- **Gate gallery (third tab).** Pick a gate in the hierarchy and see it on every
  sample in the run at once - the question the editor cannot answer, because a
  rule that works on the reference and drifts on a third of the cohort looks
  fine one sample at a time. Specimens come from the same `pair_files` grouping
  the editor uses, so the sort column chosen there orders this too and the FMX
  is always the left plot of a pair. Ten specimens to a page.

  Plots are pictures, not editors: the rendered bitmap with a static outline
  over it and no event handlers at all, so a stray click cannot move a gate on a
  sample someone was only looking at. Percentages are computed through the same
  `get_percent_and_counts_gate` the editor uses, against the same R-tree, so the
  two tabs cannot disagree.

  Rendered images are cached against a fingerprint of every gate the picture
  depends on. Gates are never mutated in place - moving one replaces the `Arc` -
  so pointer identity answers "has this gone stale", and the fingerprint holds
  the `Arc`s it hashed to keep those addresses from being reused. Paging back is
  free; re-running the autogater invalidates exactly the plots whose gates moved.

- **Contact-sheet PDF export.** The whole run for one gate, six specimens to an
  A4 landscape page, re-rendered at print resolution with a count and a Stop.
  JPEGs are embedded unchanged as `DCTDecode` images and outlines are drawn as
  page operators from the same flattened primitives the screen uses, so the
  exported sheet is the page you looked at rather than a second drawing of it.
  Written directly rather than through a PDF library - no new dependency.

- **Tests for carrying gates through a change of scaling** - the change a new
  cofactor makes, typed in or arriving with a replacement scaling file. Each
  asks what a person relies on: does the gate hold the same raw events after
  the rescale as before it. Rectangles, line gates, quadrants and bisectors
  must match exactly; polygons, ellipses and skewed quadrants, whose slanted
  edges become curves on the new scale, are held to what was measured (98.4%
  to 99.7%). Also covered: all three tiers a gate can sit in, a gate shared
  between tiers or registered under several keys being carried once, a gate
  on other channels being left alone, a round trip there and back, and a gate
  drawn after a rescale living in the same space as the ones carried by it.
  A further test holds the scaling import to producing only linear and
  arcsinh axes, since the rescale has nothing for a biexponential one.

- **A test audit, and the bugs it found as failing tests.** Every test file
  was read for tests that could not fail - 41 scenario tests in `gate_move`
  printed their results and passed whatever happened - and every module for
  code no test reached. `docs/test-audit.md` lists what was repaired, what
  was added, where each module reaches into the others, and 25 bugs, each
  pinned as a test that fails today and is `#[ignore]`d with the bug's id:
  `cargo test --no-default-features --no-fail-fast -- --ignored` runs them.
  The four most serious: a metadata row without a file name shifts every
  later file onto the wrong group; typing an axis limit below the lower one
  crashes the app, and retyping one moves quadrants for good; and with the
  default finder, above-the-negative puts a gate inside a negative that
  drifted past it and scores that 0.87.

- **Integration tests** in `tests/`, driving the library with real files:
  a plate folder to each file's metadata, a document saved and reopened
  with per-sample positions, a scaling file replaced under drawn gates on
  real stores, every gate counted by the filter and by the on-screen index,
  and the rules run from FCS files on disk.

### Changed

- **A plot no longer needs every channel the scaling file names.** The cofactors
  handed to `apply_arcsinh_transforms` come from the axis settings, which
  describe the whole panel; it errors on the first name it cannot find, so one
  channel absent from one file lost the entire plot. The gallery now passes only
  the channels the file carries. Nothing measured changes - a transform for a
  column that is not there could not have reached the plot's axes or its gating
  chain, which are columns that are.

- **Editing a rule keeps its gate when the population changes.** The Edit button
  exists so one rule can be moved onto a second population without retyping it,
  and clearing the gate and parameter on every change of parent made that three
  picks instead of one. A gate the new population also holds is now kept, and
  the parameter with it; a name the new parent does not hold is still cleared,
  since carrying it over would let the form name a combination the document does
  not have.

### Fixed

- **A metadata row with no file name no longer shifts the rows after it.**
  Such a row was left out, but every file after it was then given the row
  before its own - its group, and so the gates it was given, were wrong
  without a word. Each row is now read whole. Rows with no id or no file
  name are left out and named in a warning; an empty row is passed over. Two
  rows with the same id are refused. Two with the same file name - Omiq allows
  two plates' `A1.fcs` - are both kept for the gating file, but no file of
  that name is given either's metadata, and a warning says so; the later row
  used to win silently.

- **Changing an axis no longer moves a quadrant.** A new cofactor or a new
  axis range pulled a quadrant or skewed quadrant whose centre sat near either
  end of the axis inwards, and snapped a skewed quadrant's slanted arms to the
  new edges, turning them - the quarters held different cells afterwards, and
  widening the axis again did not put them back. An imported Omiq quadrant
  whose centre lay beyond the axes was moved onto them. The centre and the
  direction of every arm now stay exactly where they were; only where the
  lines are drawn changes, and a centre left off the plot by a narrowed axis
  has its handle drawn at the plot's edge, where it can still be grabbed.

- **The axis boxes no longer crash the app or move gates while you type.** A
  limit or cofactor is applied when you press Enter or leave the box, not at
  every keystroke - typing 400000 used to apply 4, 40, 400 on the way, each
  pulling any quadrant on the axis with it. A value that would leave the axis
  unusable (an upper limit below the lower one, a cofactor below 1) is refused
  with a warning and the box goes back to what it was; an upper limit below
  the lower one used to crash the app.

- **A scaling file is checked before it is used.** Its columns are found by
  their names, so their order no longer matters, and a file missing one is
  refused by name - columns used to be read by position, so a reordered file
  loaded silently with the wrong values. A file with a channel that cannot be
  drawn (Min not below Max, a cofactor of 0 or less, a value missing) is
  refused with a warning naming each one, and nothing is changed; such files
  used to crash the app. Decimal cofactors and ranges are now accepted.

- **A damaged gating file whose tree loops is refused.** A node that is its
  own parent, or two that are each other's, used to hang the import on
  "Loading"; it is now refused as damaged, with a warning naming the node.
  Every file that fails to load now raises a warning, not only a status line.

- **The pairing boxes show the columns actually in use.** The Sample type box
  read "SampleID" while pairing by SampleType: its options arrive with the
  metadata, after its value was set, and it fell back to the first. The same
  fix covers Sort by and the rules form's file pickers.

- **Every file of a specimen can be seen.** A specimen showed one file per
  type in its plot order, and only two plots: a re-acquired tube, a third
  file type, or a third file of a specimen with no type was listed in the
  editor but never drawn, and missing from the gallery and its PDF. The
  editor now always shows the file you select - the FMO stays in the first
  plot - and when a specimen has more than one other file, a selector above
  the second plot picks which. That choice carries over as you step between
  specimens: pick the unstained control and the next specimen shows its
  unstained control too. In the gallery, a specimen's extra files get rows of
  their own, "D1 (2)", under the column of their type.

- **The PDF contact sheet says when a plot could not be drawn.** A paired
  file that failed to render, or had no row in the metadata, was printed as
  "no paired file" - the QC record said the specimen had no such file - and
  the export reported success. Such a slot is now framed like a plot, names
  the file and says why it could not be drawn, and the message after export
  lists them.

- **The most recent per-specimen position is the one that applies.** A gate
  can be positioned per group under more than one metadata column - the
  gating file groups it by one, a rules run by the pairing's Sample ID
  column, and a run after that column is changed by another. A sample in a
  group under each used to get whichever column its metadata happened to
  list first, so a run's answer could be silently ignored. Now the position
  written last applies, and an older one still holds for the samples nothing
  newer covers. A save names a grouping column only when grouping by it gives
  every sample its position; otherwise it writes the positions sample by
  sample, so reopening the file shows exactly what was saved.
  The same holds between a sample's own position and its specimen's: whichever
  was set last applies. A rules run used to be hidden from any sample that
  had been adjusted by hand (or had its own position in the gating file),
  while its report said the sample had been moved.

- **A rules run never writes to a workspace it did not measure.** A run
  carries on while another tab is in front, and one that finished after the
  gating file, metadata or scaling was replaced wrote its answers into the new
  document. A run now stops as soon as the gates, files, metadata, scaling or
  rules change, and before writing anything it checks that all of them are
  still what it started from; if not, it says so and moves nothing. Selecting a
  gate or looking at another file does not count as a change.

- **Previous and Next get past a specimen with no FMO.** They landed on the
  specimen's left-hand file, which such a specimen does not have, so nothing
  was selected and the buttons stuck there. They now land on the first file
  the specimen shows.

- **`cargo test --no-default-features` builds.** The binary needs
  `dioxus::desktop`, so every test target but `--lib` failed to build
  without GTK. It now declares `required-features = ["desktop"]`.

- **An ellipse survives a change of scaling.** Rescaling read the ellipse's
  `radius_x` as if it lay along X. It only does when the ellipse is unrotated
  and wider than it is tall in data units - and against a linear scatter axis,
  where SSC-A runs to hundreds of thousands and a marker to about eight, an
  ellipse is almost always the other way round. The scatter-sized radius went
  through the marker's transform, overflowed, and the gate came back with an
  infinite radius admitting everything to one side of it. The centre and one
  end of each axis are now carried as points and the ellipse is refitted
  through them, whatever its angle; a gate that still cannot be carried is
  reported and left as it was, rather than replaced with a broken one.

- **One bad FCS file no longer takes the application down.** The header reader
  `expect`ed every step, and flow_fcs slices the file by its header's offsets
  without checking them, so an empty, truncated or mislabelled file panicked.
  Each is now an error naming the file and the reason, and the rest of the
  folder still loads. Comparing two files also stopped panicking on one without
  a GUID.

- **Loading a second scaling file replaces the first rather than merging
  into it.** Merging - how the store behaved when the scaling could only load
  once - would have kept the old file's settings for every channel the new one
  does not mention, and the old display order for every channel they share.

- **The axes survive a scaling replace.** They were picked once, when the
  first scaling arrived, and never again. Each now keeps its channel when the
  new scaling has it and falls back to the default when it does not.

- **Edit on a rule brings back the gate and the parameter.** Both were coming
  back empty, so editing a rule meant re-picking them, and a threshold rule
  refused to save until you did. Their menus are built from the field above -
  gates from the population, parameters from the gate - and Edit sets both
  halves in one go, so the value could reach the menu before the matching entry
  existed. A menu given a value it has no entry for falls back to its first one.

- **A phenotype rule now insists on a named reference sample.** Only a sample
  gated by hand can say what a population is. Measured on "its FMX" the rule
  would resolve a different file per specimen and could land on a control -
  which has, by definition, no signal in the channel it drops, usually the very
  marker the population is defined by. The phenotype would be described from
  cells that cannot show it, and the result would look like an answer. Measured
  on "itself" it would describe the population from the gate it is about to
  move. The form only ever writes a named file; this is for a sidecar written by
  hand, where nothing else would catch it.

- **A run no longer fails on every file because one channel is missing from
  one.** The third and last place with this fault: `apply_arcsinh_transforms`
  errors on the first parameter it cannot find, and the cofactors describe the
  whole panel as the scaling file defines it, so a channel absent from a single
  file failed every rule on every file - a real run reported "Parameter AF P1-A
  not found" six times and placed nothing. The gallery and the editor were fixed
  earlier; the autogate solver reads its own frames and was still doing it.

- **The sample pairing controls no longer overlap themselves on the rules tab.**

  They carry a grid of their own, and dropping them into a cell of that tab's
  form grid squeezed it into the 11rem label column: the labels wrapped and the
  warning underneath was drawn over them. They take the full width of the row.

- **A plot no longer needs every channel the scaling file names** - in the
  editor too, not only the gallery. `apply_arcsinh_transforms` errors on the
  first parameter it cannot find, and the editor swallowed that error into an
  endless "Rendering Plot..." spinner, which reads as slowness rather than as a
  failure. Both tabs now go through one filter, so they cannot answer
  differently.

- **The editor no longer scrolls back to the top on every change of sample.** A
  plot that was loading, failed, or had no resolver rendered a placeholder sized
  to its contents, so the page lost 600 pixels of height and the browser clamped
  the scroll position to fit what was left. Every state a plot can be in now
  occupies the same square.


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
- Linked gates are modelled, not just preserved. The gating tree is keyed by
  node rather than by gate, so a gate Omiq applies at several points occupies
  several positions instead of collapsing to whichever node the import saw
  last - 250 placements rather than 146 in a real export. Each position carries
  its own gating chain, which also fixes a linked gate's statistics: they were
  computed against whichever parent chain survived the import.
- Link, unlink and delete-one-instance, on the hierarchy pane's right-click
  menu. Linking re-points a position at another gate, discarding the gate it
  showed - which is kept as a ghost, since a boolean may reference it. It is
  refused across different parameter pairs, and for composites, which Omiq
  treats as all-or-nothing across their corners. Unlinking copies the shared
  gate under a fresh id for that position alone.
- `a_real_gating_file_survives_a_round_trip`, gated on `OMIQ_GATING_FILE`,
  checks the whole path against a real export. Skipped when unset.

### Testing

    cargo test                        # needs the GTK system packages below
    cargo test --no-default-features  # no system packages needed

472 unit tests and 12 doctests pass either way. Every test lives in the library,
so `--no-default-features` is enough to run them: it drops dioxus's `desktop`
feature, which pulls in `gdk-sys` and probes pkg-config for `gdk-3.0`. Without
those packages a plain `cargo test` fails at that probe before running anything.

Building the desktop binary (or testing with default features) needs:

    libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev
    libayatana-appindicator3-dev librsvg2-dev
