# Test audit

A file-by-file pass over the test suite: tests that could not fail, code with
no test, bugs found on the way, and where each module reaches into another -
the seams the integration tests in the second pass are written against.

## Conventions

- **A known bug is a failing test, marked `#[ignore]`.** The reason names the
  bug's id below: `#[ignore = "known bug B-KDE-1: ..."]`. `cargo test` stays
  green; `cargo test -- --ignored` runs every known bug, and every one of them
  should fail. One that passes has been fixed - delete its `#[ignore]` and
  move its entry to *Fixed*. (Doctests are the exception: rustdoc never runs
  a block marked `ignore`, even under `--ignored`, and reports it as passing.
  The one such block, `clone_subtree`'s, has a failing unit test beside it.)
- **A vacuous test** is one that cannot fail whatever the code does: no
  assertion, an assertion that always holds, or an early `return` that turns a
  missing input into a pass. Each one found is listed with what was done.
- Tests that need a real file (`OMIQ_GATING_FILE`, `CLINGATE_FCS_DIR` and the
  like) skip when the variable is unset. That is deliberate and listed here so
  nobody mistakes a green run for coverage of them.

## Where things stand

- **1,052 unit tests and 21 integration tests pass**, plus 11 doctests.
- **42 known-bug tests (36 bugs) are pinned as failing tests** (`#[ignore]`d
  with their id); all of them fail today. One more (B-BUILD-1) was fixed outright.
- **42 vacuous tests dealt with**: 41 scenario tests in `gate_move` that
  printed their results and passed whatever happened (40 now assert, one
  loop over the others deleted), and one FCS equality test that discarded
  its answer. 15 more that asserted only that *something* was returned, or
  that an edit was refused, now assert what came back and that a refused
  edit changed nothing.

Run everything without GTK:

    cargo test --no-default-features                          # the suite
    cargo test --no-default-features --no-fail-fast -- --ignored   # the known bugs: every one should fail

The table is ordered by severity. The **High** entries change what a
person sees or gates without saying so, or crash the app, and are the ones
to fix first.

## Summary of bugs

| Id | Where | What | Severity |
|---|---|---|---|
| B-AUTO-1 | `gate_rules::rule::AboveTheNegativeRule` with the default `NegativeFinder::BelowTheGate` (`threshold::refine_from`) | Refines from where the gate sits on the sample - the reference's position. A negative that drifted past it (300 -> 600 in the test) is seen only from below, read low, and the gate settles at 584, inside the negative: 69% of the sample admitted against 10% on the reference, **scored 0.87**, so a run ranks it as needing no review. The `NegativePeak` finder follows the same drift to 800 | **High** - confidently wrong gating |
| B-META-1 | `omiq::metadata::parse_metadata_csv` | A row with no id or file name is skipped when ids are collected, but metadata is then read by position in the shortened list: every later file gets the previous row's metadata, so its group - and the gates it is given - are wrong | **High** - silent wrong gating |
| B-AX-1 | quadrant / skewed quadrant `recalculate_gate_for_new_axis_limits` (via `main_window`'s limit boxes) | The boxes apply every keystroke and nothing checks lower < upper; the relimit's `f32::clamp(lower + buffer, upper - buffer)` then panics - typing `-5` in the upper box of a linear axis crashes the app | **High** - crash |
| B-AX-2 | the same | Each keystroke's intermediate limit (4, 40, 400 ... on the way to 400,000) clamps a quadrant's centre into that range, and nothing restores it: retyping a limit moves the quadrant for good. The cofactor box applies every keystroke the same way, through the rescale's own clamp | **High** - silent change to gating |
| B-AX-3 | the same `clamp`, reached from files: `axis_store::read_axis_configs` and the gating import | Nothing validates a scaling file: a range the wrong way round (Min above Max), a cofactor of 0 (infinite bounds, then a NaN in the clamp) or a negative one each crash the app when it replaces the scaling (every quadrant on the channel is relimited) and when a gating file is imported over it | **High** - loading a file crashes the app |
| B-OMIQ-2 | `GateState::upload_gates_from_file` (the node depth sort) | Walks each node's `parentId` to the root with nothing to notice a node already seen: a gating file in which a node is its own parent never finishes importing - the Workspace tab hangs on "Loading" instead of calling the file damaged | **High** - a damaged file hangs the app |
| B-SCALE-1 | `plots::axis_store::fetch_axes_from_omiq_csv` | The scaling export is read with a fixed schema, which polars applies by position and ignores the header: an export with its columns in another order loads silently with the wrong ones (the test's CD3 gets cofactor -500 and a range of -6.7 to -0.3), and one with a column missing reads every later value shifted | **High** - silently wrong scaling for every gate on the channel |
| B-GRP-1 | `GateState::get_current_sample` and `gate_for_file` (group tier), fed by `omiq::deserialise` and `autogate::place_for_specimen` | The import keys a gate's per-group positions by the column the file names (`md`); the autogater keys its positions by the pairing's sample id column. Nothing removes one when the other is written, and a sample in a group of each gets whichever column its metadata hash map yields first. In the test the run's new position for `QCVn` is ignored - the plot, the statistics and the export keep the file's (-1.21 against -0.21 placed), while the run reports the gate placed. The export then names one of the two columns, also by hash order (`group_override_column`) | **High** - a run's result silently not applied |
| B-GRP-2 | `autogate::apply_placements` (group tier) under `get_current_sample`'s precedence (sample, then group) | A run writes one position per specimen, in the group tier. A file with a position of its own - a per-file filter from the imported document, or a gate dragged on that one sample - resolves through the sample tier first, so the run's position never reaches it, yet the report lists it as positioned: 815.7 in the test, beside a plot drawn at 450 | **High** - a run's result silently not applied, and reported as applied |
| B-FCS-1 | `file_load::FcsSampleStub::open` (via flow_fcs `Metadata::validate_guid`) | `validate_guid` looks for `GUID`, never finds it among keys stored as `$GUID`, and writes a random `$GUID` over the file's own. Equality "by `$GUID`" compares random numbers: two copies of one acquisition, or one file opened twice, are unequal. Root cause is upstream in `czarop/flow` | Medium - identity of an acquisition is lost |
| B-CONF-1 | `gate_rules::confidence::Component::new` / `Confidence::from_components` | `NaN.clamp(0, 1)` is NaN, and the `f64::min` fold ignores NaN: an unmeasurable component leaves the overall score untouched, ranking the gate as trustworthy | Medium - review ranking |
| B-RULE-1 | `gate_rules::rule::ValleyRule::min_depth_fraction` | Documented as the depth below which a valley placement is flagged; edited in the Gate Rules tab and saved, but read nowhere - a placement scores the same (0.5315 in the test) whether the bar is 0.9 or 0.05 | Medium - a setting that does nothing |
| B-PHEN-1 | `gate_rules::phenotype::Baseline::of` | Non-finite values are not dropped: the first median is sorted with NaN in it (by a comparator that is not an order) and lands on one, so the baseline comes back `median: NaN` and the marker is disabled for the match | Medium - one corrupt event |
| B-CNT-1 | `gate_filtering::filter_events_to_mask` vs `gate_stats` (`EventIndex`) | The filter admits strictly inside a rectangle, the index counts the edge: the percentage on a gate counts events the population drawn under it does not hold (240 vs 246 in the test, the six edge events) | Medium - whole-number scatter values meet round edges |
| B-PDF-1 | `gallery::export::contact_sheet` (the Export PDF button) | A plot that fails to render (`render_plot(..).ok()`) and a paired file with no metadata row (skipped while the jobs are built) both leave an empty slot, and an empty slot is printed "no paired file". On screen the same plot shows why it failed. The QC record says a specimen had no such file when it did, and the run is reported as written | Medium - a QC record that misstates what was checked |
| B-PAIR-1 | `plots::sample_pairs::pair_files`, shown through `take(2)` in `main_window` and `gallery::window` | A specimen gets one slot per display-order type, filled by the *first* file of that type, and both screens show a pair's first two slots. A second file of one type (a tube re-acquired after a clog), a third file of a specimen with no named type, and every file of a third type typed into "Plot order" are listed in the editor but never drawn - picking one shows the specimen's other files - and never appear in the gallery or its PDF. Nothing warns: the pairing controls count only untyped files and types missing from the order | Medium - a file that cannot be viewed or edited, and a QC record that silently omits it |
| B-WS-1 | `workspace::program_name` | "Outside the workspace" is decided by `strip_prefix`, which does not resolve `..`; `/w/../elsewhere/A1.fcs` is named `.._elsewhere_A1.fcs` | Low - dialogs and `fcs_under` give clean paths |
| B-FCS-2 | `file_load::FcsSampleStub::open` | Checks a file's header and keywords but not that its data segment holds the `$TOT` events promised. A file with one header offset digit damaged is accepted into the workspace, and reading its events trips an assertion in flow_fcs; a rules run reads files in parallel, so that one file ends the *whole* run ("The run did not finish") and no gate is placed. (A file merely cut short is refused cleanly when its events are read.) | Medium - one damaged file stops every run |
| B-NAV-1 | `plots::sample_pairs::step_from` (the editor's Previous / Next, moved out of `main_window` to be tested) | Lands on the arrived-at specimen's *left* file; a specimen with no FMO keeps its left side empty, so nothing is selected, and pressing again steps from the same place to the same specimen. The buttons cannot get past it in either direction | Low - the file list still reaches it |
| B-OMIQ-1 | `omiq::serialise` (label position) | `"labelLoc": {}` (label not placed) is read as (0, 0) and exported as an explicit `{"f1Val": 0, "f2Val": 0}` - an unedited gate's label pinned to the origin | Low |
| B-THR-1 | `gate_rules::threshold::valley_in` | `NoValley::OnlyOnePeak { events }` is always built with `events: 0`, so the refusal says "one peak ... over 0 events" | Low - a misleading report |
| B-CONF-2 | `gate_rules::confidence::displacement_score` | Divides by `displacement_limit`, read from the rules file, without the guard `stability_score` has; 0 scores an unmoved gate as NaN (then hidden by B-CONF-1) | Low |
| B-RS-1 | `gate_rules::rule_store::human_order` | Calls distinct names equal (`D02`/`D2`, `a1`/`A1`), so sorted lists keep whatever order the hash map gave | Low |
| B-STAT-1 | `gate_stats::get_percent_and_counts_gate` | `count / parent * 100` unguarded: a gate over an empty parent shows NaN% | Low |
| B-GRID-3 | `gate_move::density_grid::apply_constraints` | Capping a move scales `dx_data`/`dy_data` but not `dx_bins`/`dy_bins` | Low |
| B-KDE-1 | `gate_move::kde::kde_negative_shift` | Negative width is the std-dev of everything below the axis midpoint, so a smeared positive reads as a widened negative (ratio 2.15 for an identical negative) | Low - not called by the app |
| B-KDE-2 | `gate_move::kde_shift::analyse_population_shift` | A widened negative's KDE peak moves by noise (0.127) past the 0.1 significance threshold; the same scenario is `CompensationIssue` on X and `Ambiguous` on Y | Low - not called by the app |
| B-KDE-3 | `gate_move::kde_shift::compute_smear_score` | Entropy term is normalised by `ln(grid points)`: a tight cluster scores ~0.35-0.49, never near its documented 0, the score changes with the grid, and the peak/median blend it drives follows noise for a smear | Low - not called by the app |
| B-HIER-1 | `gate_editor::gates::gate_hierarchy::GateHierarchy::clone_subtree` | Both branches after `add_child` return `Err` ("possible cycle" on failure, "no order for child" on success), so cloning any subtree with a child fails | Low - no caller yet |
| B-HIER-2 | `gate_hierarchy::GateHierarchy::would_create_cycle` (used by `add_child`, `reparent`, `delete_node_keep_children`) | Checks whether the parent is among the child's descendants; a gate is not its own, so a gate can be made its own parent - directly, or by deleting a gate and handing its children to one of them. `get_ancestors` then loops forever. Found by the random edit sequences | Medium - reachable from a damaged file (B-OMIQ-2) |
| B-DOC-1 | `GateState::link_node_to_gate` / `link_composite` | Re-pointing a position keeps the gate it replaced registered "since a boolean may still reference it", whether one does or not; deleting collects such a gate (`collect_stranded_ghosts`) because it accumulates and is exported as a container on no plot. Found by the random document edits | Low - file bloat, invisible containers in Omiq |
| B-FCS-3 | flow_fcs `Fcs::open` (upstream, `czarop/flow`) | Slices by the header's offsets and asserts event counts without checking them: damaged or truncated files panic ("range end index 312 out of range for slice of length 110"). Every caller runs it on a worker thread, so the app survives; files the workspace refuses never reach it; B-FCS-2 is the case that does | Low here - upstream |
| B-GRID-1 | `gate_move::density_grid::DensityGrid::from_column` | A NaN coordinate casts to cell 0 and is counted | Low - not called by the app |
| B-GRID-2 | `DensityGrid::from_column` | `unwrap`s `.f64()`: a Float32 column (FCS data) panics | Low - not called by the app |
| B-GRID-4 | `gate_move::density_grid::make_gaussian_kernel` | `sigma = 0` gives a NaN kernel; the blur fills the grid with NaN and `cross_correlate` then panics on `partial_cmp().unwrap()` | Low - not called by the app |
| B-GRID-5 | `gate_move::density_grid::calculate_dynamic_radii` | The "noise, not a cluster" guard compares a spread measured on half the axis with 25% of the whole axis, and only on X; it cannot fire | Low - not called by the app |
| B-BUILD-1 (fixed) | `Cargo.toml` | The binary needs `dioxus::desktop`, so `cargo test --no-default-features` - documented as the way to test without GTK - failed building it for any target but `--lib`. Fixed: `required-features = ["desktop"]` on the `[[bin]]` | - |
## Modules

### gate_move

**Reach.** Only `kde::kde_1d`, `kde::kde_peak` and `kde::silverman_bandwidth`
are called from outside the module - by `gate_rules::threshold`. Everything
else (`density_grid`, `kde_negative_shift`, `analyse_population_shift`) is
exploratory code with no caller in the application, so its bugs are latent.

**Vacuous tests found and repaired (41).**

- `density_grid::flow_tests` - 9 scenario tests printed their translation and
  passed whether it was right, wrong or an `Err`. `run` now asserts the shift to
  within one bin (the correlation has no sub-bin refinement: 0.3 comes back as
  0.2578, three bins of 0.086) and a finite, positive peak.
- `kde::flow_tests` - 9 scenario tests, the same. `run` now asserts the peak
  shift and which axes are flagged as widened; one exposed B-KDE-1.
- `kde_shift::flow_tests_kde_shift` - its first 9 tests were verbatim copies of
  the `kde.rs` ones and never called this file's `analyse_population_shift`.
  They now do, asserting the negative shift, the positive shift (or its
  absence) and the `DriftType`; one exposed B-KDE-2. `DriftType` gained
  `Debug, Clone, Copy, PartialEq, Eq` to be compared.
- `kde_shift::extended_flow_tests` - 13 scenario tests printed only; they now
  assert the same way. One exposed B-KDE-3.
- `kde_shift::test_all_scenarios` looped over the first nine scenarios and
  printed; deleted, as the nine now assert individually.

**Added.** Tests for `gaussian_blur` (mass and symmetry), `make_gaussian_kernel`,
`find_lower_quadrant_peak`, `isolate_peak_elliptical`, half-open binning,
infinite events, `compute_negative_shift` (follows the negative, not the
positive; errors with no negative), `compute_total_shift` (recovers a
translation; an empty test is an error), `kde_negative_shift` errors,
`quantile`, `extract_region`.

### file_load, workspace, searchable_select

**Reach.** `FcsFiles` is built by the Workspace tab
(`gate_editor::workspace_window`) and read by every tab through the
`Shell` context: the sample list and plots (`main_window`, `plot_window`),
the pairing (`pairing_controls`), the gallery, and the rules window's
`files_to_read`. `program_name` is the join key for the metadata
(`omiq::metadata::file_name_to_gating_id`), so a file's program name must be
what the metadata's file-name column says. `Remembered` is written by
`workspace_window::remember` and read on start-up.

**Vacuous test found and repaired (1).**
`comparing_two_files_does_not_need_a_guid` did `let _ = a == b;` - it
compared two different files and discarded the answer. It now asserts they
are unequal. Writing the tests beside it found B-FCS-1.

**Added.** The FCS fixture takes channels, labels and extra keywords
(`write_fcs_with`). Tests for the extension check (any case; `.txt` and none
refused), a missing file, the program name vs the file's own, the transform
chosen per channel (scatter and `Time` linear, the rest the default), labels
falling back to the channel name, case-insensitive `find_parameter` and
`find_mutable_parameter`, keyword lookup across keyword types, GUID
equality. In the workspace: `Found::one` for several candidates,
`Remembered::is_empty`, a corrupt or older remembered file, a failed file
being retried when added again, a name clash reported once, only `.fcs`
collected from sub-folders.

`searchable_select`'s three copies of the search filter are one tested
predicate, `matches_search`.

### omiq

**Reach.** `deserialise` is driven by `GateState::upload_gates_from_file`
(`gate_editor::gates::gate_store`), which builds every drawable gate, the
node tree, the per-group overrides keyed by `MetaDataKey`, and the
`OmiqRebuildStore` (`rebuild`) the export needs. It reads the axis settings
(`AxisStore`) for composite ranges and infinite bounds. `serialise` is driven
by the Workspace tab's *Write gating file* and reads the same three: gates,
metadata (`MetaDataFileMap`) and axes. `metadata::parse_metadata_csv` feeds
`MetaDataStore`, whose `file_name_to_gating_id` is the join between an FCS
file's program name (`workspace::program_name`) and its gating id.

**Weak tests strengthened (9).** Five link/unlink/delete tests asserted only
`is_err()`; they now also assert the refused edit left every placement and
registration as it was (`layout`). `a_missing_axis_setting_is_an_error` now
checks the error names the axis. `every_atomic_container_keeps_its_type`
checked a type was present, not that it was the same one; it now compares,
and fails if the fixture gave it nothing to compare. `every_gate_keeps_the_
label_it_came_in_with` likewise checked presence only; it now compares at f32
precision (coordinates are held as f32 and written widened, so `51.0513`
comes back as `51.051300048828125`) - which found B-OMIQ-1.

**Dead code.** `deserialise::validate_metadata_requirements` is never
called and only prints. Nothing checks that a gating file's groups exist in
the metadata - see the integration pass.

**Added.** A new skewed quadrant is written as four `AngleGate`s grouped
`_SKEWEDQUAD0..3`; a new quadrant, skewed quadrant and bisector each come
back from export and re-import as the same kind with the same pieces. In
metadata: incomplete rows left out, blank values absent, numbers kept as
text, a missing id column or file an error; B-META-1.

**Observation.** Two metadata rows with the same file name are accepted
silently, the later winning. With files now named by their sub-folder path
that is less likely, but a file would still be given another's group with no
warning.

### gate_rules: threshold, confidence

**Reach.** `threshold` is plain arithmetic over `&[f64]`, used by `rule`
(`Rule::solve`, which picks the axis and builds each gate's *shadow* for
`negative_below` / `refine_from`) and through it `autogate`. It borrows
`kde_1d`, `silverman_bandwidth` and `kde_peak` from `gate_move::kde`.
`confidence` scores `Threshold`s for `rule` and `MatchEvidence` for the
phenotype rule (`autogate`); its `ConfidenceLimits` are serialised in the
rules sidecar (`rule_store`).

**Weak tests strengthened (4).** The four "no boundary" valley tests asserted
only `is_err()`; each now names the refusal it expects (`OnlyOnePeak` for a
merged hump and a smear, `NothingDeepEnough` at the right place for a
shoulder wobble and a tail ripple), so a fixture that fails for another
reason no longer passes.

**Added.** `negative_below` and `refine_from` had no direct test: the
negative under a gate, sliding the gate, too little below it, refining from a
gate set too high, not running off the axis from one set too low, and a gate
already in place staying put. `interquartile_spread`, the count swing in a gap
and in a continuum, `first_valley`'s refusals of bad input. `assess_match`
- the whole phenotype confidence model - had no test; it now has six, plus
clamping, empty confidence, the zero `swing_half` guard and the limits'
round trip through serde.

### gate_rules: rule, rule_store, shape_fit, phenotype, autogate

**Reach.** `autogate` is where the rules meet everything else: it reads the
FCS files (`file_load`, through `measure_file` into `parent_values` and the
event index `flow_gates` builds), the gate store (`GateState::gate_for_file`,
`resolve_drawable`, the per-file overrides), the metadata
(`MetaDataFileMap`, for specimens and sample types via `rule_store`'s
pairing), and writes placements back with `apply_placements`
(`GateState::set_gate_for_file`). The Gate Rules tab
(`gate_editor::gate_rules_window`) drives it through `files_to_read`,
`measure_all` and `run_solve`, and saves `RuleStore` as JSON.

**Found.** B-RULE-1: the test that claims to check a shallow valley is
flagged against the bar (`a_shallow_valley_is_placed_and_flagged_rather_than_refused`)
passes 0.9 as the bar, but its "shallower than the bar" assertion checks the
fixture, not the code - with the bar at 0 the result is identical. The new
test shows it.

**Added.** `ValleyRule::calibrate` and `place` directly (the offset from the
bottom, following a shifted dip, refusing one hump), `describe`,
`accepted_band` for every kind, calibrated rules refusing to solve alone,
every threshold rule being assessable. `simplify` (the contour thinning that
makes a traced outline a drawable gate) had no test: corners kept, area kept,
small outlines untouched, never below a triangle, degenerate input.
`Rows::select`, `z_into`. `human_order` on case and prefixes.

**Env-gated, and pass when unset.** `harness_tests::a_real_workflow_shows_what_the_manual_gates_capture`
(`OMIQ_GATING_FILE`, `OMIQ_METADATA_FILE`, `OMIQ_SCALING_FILE`,
`OMIQ_FCS_DIR`), and two tools that write a file for looking at by hand and
assert nothing: `shape_fit_tests::a_fitted_shape_can_be_looked_at`
(`SHAPE_FIT_OUT`) and `rule_store_tests::a_phenotype_sidecar_can_be_written_for_trying_the_app`.

**Observations.**
- `RuleStore::reference_file` takes "the one file" of the wanted type in a
  specimen; with two it returns whichever the map yields first. Refusing, as
  the workspace does with two candidate files, would be consistent.
- `Rule::solve` / `Rule::apply` refuse the calibrated rules with
  `SolveError::BadBand { band: (0, 0) }`, whose message ("a band of 0 to 0 is
  not a fraction range") names the wrong reason. `autogate` never reaches it,
  since those rules take their own branch.
- `RuleStore::save` writes in place; `Remembered::save_to` writes beside and
  renames. A crash mid-save loses the rules file.

### gate_editor/gates

**Reach.** `GateState` (`gate_store`) is the document: the three-tier gate
store (global registry, per-group and per-sample overrides keyed by
`MetaDataKey` / file id), the node tree (`gate_hierarchy`), and the Omiq
rebuild data. It is written by the import (`omiq::deserialise`), the editor
(drag, rotate, draw - `gate_single`, `gate_composite`, `gate_drag`), the
autogater (`place_gate`, per-file overrides), the rescale (`rescale_channel`,
`relimit_channel`, from `axis_store` and the Workspace tab) and link/unlink.
It is read by filtering (`gate_filtering`, a polars mask), the on-screen
statistics (`gate_stats`, an R-tree `EventIndex`), drawing (`draw_gates`),
the gallery, and the export (`omiq::serialise`).

**Weak tests strengthened (2).** `an_ellipse_can_be_rotated` only checked a
rotation returned something; it now checks the angle changed and the size
and centre did not. `composite_figures_are_retrievable_by_subgate_id` only
checked each quarter had a figure; it now checks the quarters' counts tile
the ten events and their percentages sum to 100.

**Added.** `swap_tests`: every gate type transposed onto swapped axes holds
exactly the same events - in the same named piece for a composite - and
transposing twice gives the gate back; the ellipse, line gate, skewed
quadrant and bisector had no swap test. `gate_store`: which files have a
position of their own (what the export writes per file), collecting a
stranded ghost, walking the tree (`root_nodes`, `child_nodes`,
`parent_node`, `gate_chain_for_node`, `node_order`), reading a UI id as a
position (`as_parent_node`), `place_new_gate`. `draw_gates::was_gate_clicked`:
an edge selects its gate, the interior and empty space do not, the nearer of
two edges wins.

**Observations.**
- The same question - which events a gate holds - is answered two ways: a
  polars mask in `gate_filtering` (used for the plotted population) and an
  R-tree in `gate_stats` (used for the percentage shown). Nothing checked
  they agree; the integration pass does.
- `filter_events_by_hierarchy_to_mask` and `rescale_helper` print to stdout
  on every call.

### gate_editor: axis_info, plots, gallery, windows

**Reach.** `AxisStore` (`plots::axis_store`) is filled from the scaling file
(`read_axis_configs`) and edited in the editor's axis boxes
(`main_window` -> `update_lower` / `update_upper` -> `GateState::set_current_axis_limits`,
which relimits the composites) and cofactor box (-> `rescale_gates`). Its
settings reach the import (composite ranges), the export (infinite bounds),
the plots (`draw_plot`'s `PlotMapper`), the gallery and the rules window
(`cofactors_carried_by`). The gallery (`select`, `cache`, `render`,
`overlay`, `pdf`) reads `GateState` through a resolver per file and caches
pictures by the addresses of the gates they depend on.

**Found.** B-AX-1 and B-AX-2, from reading `main_window`'s limit handlers
against the quadrant relimit. The handlers also print errors to stdout
rather than showing them.

**Added.** `data_helpers`: plotted points skip incomplete events and keep
their order, a missing column is an error, the event index covers the two
named columns, a non-Float32 column is refused. Gallery: a picture depends
on its filter chain then its drawn gates (`dependencies`), moving a drawn
gate on one file stales that file's picture and no other,
`flatten_gates`. `param_for_fluoro`. `workspace_window` had no tests: the
rule for which actions discard gates (moved into `Pending::discards_gates`
so it can be tested beside the text that warns about it), what each
confirmation names, `Part`, `Which`, `flatten`, `file_name`.

**Not unit-tested, and why.** The Dioxus components themselves -
`main_window`, `plot_window`, `gate_sidebar`, `route`, the `Handles`
operations in `workspace_window`, the `components/` widgets - need a
running runtime with stores and signals. Their logic is tested where it
has been pulled out into plain functions; the rest was checked by driving
the app (see the Workspace entries in the changelog).

## Second pass: integration tests

In `tests/`, against the library's public API, with files written to disk
where the seam is a file (`tests/common` writes FCS, metadata and scaling
files). Run with `cargo test --no-default-features --tests`.

- `workspace_to_metadata` - folder -> `detect` -> `FcsFiles` (program names
  for plate sub-folders) -> `parse_metadata_csv` -> each file's row; a
  metadata export using bare well names reaches no file rather than the
  wrong one; a remembered workspace reopens the same files under the same
  names, without a file removed by hand; B-META-1 end to end.
- `gate_counting` - every gate type counted by the filter and by the
  on-screen index over the same events: they agree away from edges, the
  quadrant's quarters account for every event once; B-CNT-1 (edges),
  B-STAT-1 (empty parent).
- `document_round_trip` - a real fixture imported with metadata; a position
  set for one sample, and one set per specimen (as the autogater writes),
  survive save and reopen on exactly the samples they were set for; a
  second save changes nothing; every gate keeps its placements and parent.
  The moves are a tenth of the edge's value: the first draft used a fixed
  0.5 on an edge near a million, inside any float tolerance, so it could
  not have failed.
- `scaling_replace` - scaling files on disk, real stores in a headless
  `VirtualDom`, and the Workspace tab's own `carry_to_scaling` (moved out of
  its loader for this): a new cofactor leaves rectangles and quadrants
  holding exactly the same cells and an ellipse over 98%; the same scaling
  changes nothing; a channel the new file drops is reported and its gates
  left alone. A guard asserts the uncarried gates *would* differ, so the
  fixture can tell carrying from doing nothing.
- `gate_rules_window::tests::a_run_from_files_on_disk` (in-crate, since
  `run_solve` is private to the tab) - the Run button's pipeline from FCS
  files on disk: an unreadable file is reported by name and the rest run, a
  cancelled run places nothing, the density finder follows a drifted
  negative; B-AUTO-1.

## Leftover output

Debug `println!`s that run in normal use, noted rather than removed since
removing them is not a test change: `gate_single::rescale_helper` (every
point of every rescaled gate), `gate_filtering::filter_events_by_hierarchy_to_mask`
(on every filtered plot), `GateState`'s import (a line per gate built), `plots::data_helpers::get_filtered_dataframe` (the
whole gate chain), `plots::axis_store::read_axis_configs` (skipped channels),
`deserialise::validate_metadata_requirements` (dead). `main_window`'s axis
boxes print their errors instead of showing them.

### Doctests

Eight of `gate_hierarchy`'s twelve doctests wrapped their example in
`# fn example() -> Result<..> { .. }` and never called it, so rustdoc
compiled them and ran nothing - their assertions had never executed. Each now
ends `# example().unwrap();`. Run, three failed: `reparent` and
`reparent_subtree` moved a gate under a parent that was never added, which
the functions correctly refuse (the examples were wrong, and now add it);
`clone_subtree` failed on a real bug, B-HIER-1, and its example is marked
`ignore` with a pointer here until it is fixed.

### Random edit sequences

`gate_hierarchy_tests::any_sequence_of_edits_leaves_a_valid_tree` runs 2,000
seeded sequences of 40 random edits - add, reparent, reparent a subtree,
delete a subtree, delete keeping children, delete - and checks `validate()`
after every step. It found B-HIER-2 twice: a direct self-edge on the first
seed, and on seed 21 a 25-step sequence that shrinks (by removing steps while
it still fails) to two: `add_child(g6, g7); delete_node_keep_children(g6,
Some(g7))`. Both routes are pinned by the B-HIER-2 test and stepped around
in the random sequences, which otherwise pass.

### Random document edits

`tests/document_fuzz.rs` applies seeded random sequences of the edits the
editor makes - add a gate of every kind (rectangle, ellipse, polygon, line,
quadrant, bisector, skewed quadrant) under any position or the root, delete a
position, link one position to another's gate, unlink - to an empty document
or to the checked-in fixture (which brings a boolean and the ghost it keeps
alive). After every edit: every position names a registered gate, every
parent is a position, every chain is as long as the position is deep, and
nothing is registered that nothing reaches. After a sequence, the document is
exported and re-imported and must come back identical. 600 sequences of 30
edits and 150 round trips; over 200 seeds every add and delete succeeded,
about a third of the links (the rest correctly refused), and unlinks are
aimed at linked positions.

It found B-DOC-1 on its second seed. Positions are chosen by a key built from
gate names rather than ids, since gates are given random UUIDs and a seed
would otherwise pick different positions each run.

flow_gates prints `🔧 [TRANSFORM]` lines on every pixel conversion; they fill
the output of any test that adds gates. That is upstream, in `czarop/flow`.

### Random gate edits

`gate_editor::gates::edit_fuzz_tests`, over every gate type: 25 random drags
each followed by the opposite drag leave the gate holding exactly the cells
it held (with a guard that a drag moves cells at all - except the bisector,
whose whole-gate drag slides its handle along the split by design); and 200
random moves of the handles each gate draws never panic or leave a
coordinate that is not finite. An early draft pulled handle index 5 on every
type and hit the skewed quadrant's `unreachable!()` - it draws five handles,
so that index cannot come from the editor, and the test now pulls only drawn
handles.

### Damaged gating files

`tests/import_robustness.rs` imports the fixture with one random damage at a
time - 600 trials of a key removed, a value nulled, turned into a string, or
made enormous - and requires each to load or be refused, within five seconds,
without a panic. None panicked or hung; loops through `parentId` are left
out as the known B-OMIQ-2. The import also prints `CREATED GLOBAL GATE!` /
`CREATED FILE-SPECIFIC GATE!` for every gate it builds.

### Damaged FCS files

`tests/fcs_robustness.rs` changes random bytes in a valid file's header and
keywords (400 trials) and cuts files short at random (200), and requires the
workspace's reader, `FcsSampleStub::open`, to refuse each without a panic -
it does. flow_fcs's full reader panics on many of the same files (B-FCS-3,
pinned there). What matters is the overlap: of 3,000 damaged files, one was
accepted by the workspace and then panicked the event reader - a header
data-offset digit read as whitespace, so the data segment appeared to hold
88 events for 50. That is B-FCS-2, reproduced deterministically in
`file_load_tests` and, for its consequence, in the rules run tests.

A first draft of the rules-run test cut a file short in its data and
claimed the run died; it did not - flow_fcs refuses that case cleanly - so
that claim was dropped and the passing behaviour is now a test of its own.

### Damaged metadata and scaling exports

`tests/csv_robustness.rs` damages each export 500 times - a cell blanked, a
cell holding a comma, a row cut short, an enormous number, a whole column
dropped - and requires each to be read or refused without a panic; none
panicked. Checking what the scaling reader *accepted* found B-SCALE-1: a
dropped column was read shifted, not refused. A decimal cofactor or range
(`150.5`, `-500.5`) refuses the whole file with a polars parse error, since
every number is read as `Int64` - worth knowing if Omiq ever writes one.

### The band search, over random populations

`autogate_tests::the_band_search_lands_in_any_band_a_population_can_satisfy`:
120 random populations of one to three clusters, a third of them rounded to
whole numbers so values tie, each given a random band that some edge can
actually reach. A gate open to either side is slid by `position_by_capture`
and must land inside the band, and what it reports holding must equal what
the moved gate holds when asked afresh. It does, every time.

### The exported contact sheet

The PDF tests looked for strings in the output, which a file with one
cross-reference offset wrong still contains - and most readers then call it
damaged. `gallery::tests::assert_well_formed` checks what a reader checks
first: `startxref` finds the table, every entry is twenty bytes and points
at its object's first byte, `/Size` agrees, every stream is exactly its
`/Length`, and no coordinate is `NaN` or `inf`. It holds for multi-page
sheets and for 200 random headings, titles and file names drawn from
brackets, backslashes, accents and control characters, each of whose text
strings must close on its own line.

The export's render-and-lay-out step was inside the Export button's
closure; it is now `export::contact_sheet`, called from the same place,
so it can be driven with FCS files on disk. Plots land in their own
specimen's slot whatever order they finish in, a gate is drawn with its
percentage, a stopped run writes nothing. Reading it found B-PDF-1.

Checked and found sound: the editor and gallery plot sizes are fixed (600;
200/260/340), so the pixel mapping's panic on an empty plotting area cannot
be reached; the frames reaching the event filter are single-chunk (flow_fcs
builds each column from one `Vec`), so the ellipse and polygon filters'
contiguous-slice requirement always holds; a rotation handle's angle wraps
at +/-180 degrees, but the rotation itself is taken from the pointer's
absolute position, so the wrap only affects the preview and is invisible.

### Which files reach the screen

`sample_pairs_tests::on_screen` states the rule both screens follow - a
file is drawn only from one of its pair's first two slots - and the tests
hold every file of a folder to it. A folder of one FMX and one FS per
specimen, some missing one and some unknown to the metadata, passes.
A re-acquired full stain and a three-file untyped specimen do not
(B-PAIR-1). The rules run is not affected in the same way: a specimen
shares one position, read from its highest-ranked file, by design.

Previous and Next were a closure in `main_window`; the step is now
`sample_pairs::step_from`, beside `pair_of`, and is tested: it lands on
the next specimen's left file and wraps both ways, and a file no pair
holds steps from the first specimen. Found B-NAV-1. `listing_order` (the
editor's file list) had no tests; it now has four, one of them over 300
random pairings: every file listed exactly once, a file in no slot kept
beside its specimen, stale indices dropped.

### A gate positioned by two columns

The fixture groups `QCVn` by `Type` and `0lmI` by `test`.
`document_round_trip` gives each a per-specimen position through the
autogater's own `place_for_specimen`: under the file's column the new
position replaces the old, on screen and in the export; under the other
column it is ignored for one of the two gates, whichever way the hash
falls (B-GRP-1). The test runs both ways round for that reason.

The same precedence hides a run from any file with a position of its own
(B-GRP-2): `a_run_from_files_on_disk` gives the donor's file a per-sample
position before the run, and the run then reports that file positioned
at 815.7 while it is drawn at 450. The test accepts either fix - the file
drawn where the report says, or the report not claiming it.
