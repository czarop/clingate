# Test audit

A file-by-file pass over the test suite: tests that could not fail, code with
no test, bugs found on the way, and where each module reaches into another -
the seams the integration tests in the second pass are written against.

## Conventions

- **A known bug is a failing test, marked `#[ignore]`.** The reason names the
  bug's id below: `#[ignore = "known bug B-KDE-1: ..."]`. `cargo test` stays
  green; `cargo test -- --ignored` runs every known bug, and every one of them
  should fail. One that passes has been fixed - delete its `#[ignore]` and
  move its entry to *Fixed*.
- **A vacuous test** is one that cannot fail whatever the code does: no
  assertion, an assertion that always holds, or an early `return` that turns a
  missing input into a pass. Each one found is listed with what was done.
- Tests that need a real file (`OMIQ_GATING_FILE`, `CLINGATE_FCS_DIR` and the
  like) skip when the variable is unset. That is deliberate and listed here so
  nobody mistakes a green run for coverage of them.

## Summary of bugs

| Id | Where | What | Severity |
|---|---|---|---|
| B-KDE-1 | `gate_move::kde::kde_negative_shift` | Negative width is the std-dev of everything below the axis midpoint, so a smeared positive reads as a widened negative (ratio 2.15 for an identical negative) | Low - not called by the app |
| B-KDE-2 | `gate_move::kde_shift::analyse_population_shift` | A widened negative's KDE peak moves by noise (0.127) past the 0.1 significance threshold; the same scenario is `CompensationIssue` on X and `Ambiguous` on Y | Low - not called by the app |
| B-KDE-3 | `gate_move::kde_shift::compute_smear_score` | Entropy term is normalised by `ln(grid points)`: a tight cluster scores ~0.35-0.49, never near its documented 0, the score changes with the grid, and the peak/median blend it drives follows noise for a smear | Low - not called by the app |
| B-FCS-1 | `file_load::FcsSampleStub::open` (via flow_fcs `Metadata::validate_guid`) | `validate_guid` looks for `GUID`, never finds it among keys stored as `$GUID`, and writes a random `$GUID` over the file's own. Equality "by `$GUID`" compares random numbers: two copies of one acquisition, or one file opened twice, are unequal. Root cause is upstream in `czarop/flow` | Medium - identity of an acquisition is lost |
| B-WS-1 | `workspace::program_name` | "Outside the workspace" is decided by `strip_prefix`, which does not resolve `..`; `/w/../elsewhere/A1.fcs` is named `.._elsewhere_A1.fcs` | Low - dialogs and `fcs_under` give clean paths |
| B-META-1 | `omiq::metadata::parse_metadata_csv` | A row with no id or file name is skipped when ids are collected, but metadata is then read by position in the shortened list: every later file gets the previous row's metadata, so its group - and the gates it is given - are wrong | **High** - silent wrong gating |
| B-OMIQ-1 | `omiq::serialise` (label position) | `"labelLoc": {}` (label not placed) is read as (0, 0) and exported as an explicit `{"f1Val": 0, "f2Val": 0}` - an unedited gate's label pinned to the origin | Low |
| B-THR-1 | `gate_rules::threshold::valley_in` | `NoValley::OnlyOnePeak { events }` is always built with `events: 0`, so the refusal says "one peak ... over 0 events" | Low - a misleading report |
| B-CONF-1 | `gate_rules::confidence::Component::new` / `Confidence::from_components` | `NaN.clamp(0, 1)` is NaN, and the `f64::min` fold ignores NaN: an unmeasurable component leaves the overall score untouched, ranking the gate as trustworthy | Medium - review ranking |
| B-CONF-2 | `gate_rules::confidence::displacement_score` | Divides by `displacement_limit`, read from the rules file, without the guard `stability_score` has; 0 scores an unmoved gate as NaN (then hidden by B-CONF-1) | Low |
| B-GRID-1 | `gate_move::density_grid::DensityGrid::from_column` | A NaN coordinate casts to cell 0 and is counted | Low - not called by the app |
| B-GRID-2 | `DensityGrid::from_column` | `unwrap`s `.f64()`: a Float32 column (FCS data) panics | Low - not called by the app |
| B-GRID-3 | `gate_move::density_grid::apply_constraints` | Capping a move scales `dx_data`/`dy_data` but not `dx_bins`/`dy_bins` | Low |
| B-GRID-4 | `gate_move::density_grid::make_gaussian_kernel` | `sigma = 0` gives a NaN kernel; the blur fills the grid with NaN and `cross_correlate` then panics on `partial_cmp().unwrap()` | Low - not called by the app |
| B-GRID-5 | `gate_move::density_grid::calculate_dynamic_radii` | The "noise, not a cluster" guard compares a spread measured on half the axis with 25% of the whole axis, and only on X; it cannot fire | Low - not called by the app |

## Modules

### gate_move

**Reach.** Only `kde::kde_1d`, `kde::kde_peak` and `kde::silverman_bandwidth`
are called from outside the module - by `gate_rules::threshold`. Everything
else (`density_grid`, `kde_negative_shift`, `analyse_population_shift`) is
exploratory code with no caller in the application, so its bugs are latent.

**Vacuous tests found and repaired (31).**

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
