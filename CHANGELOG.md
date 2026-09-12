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

### Testing

    cargo test                        # needs the GTK system packages below
    cargo test --no-default-features  # no system packages needed

202 unit tests and 12 doctests pass either way. Every test lives in the library,
so `--no-default-features` is enough to run them: it drops dioxus's `desktop`
feature, which pulls in `gdk-sys` and probes pkg-config for `gdk-3.0`. Without
those packages a plain `cargo test` fails at that probe before running anything.

Building the desktop binary (or testing with default features) needs:

    libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev
    libayatana-appindicator3-dev librsvg2-dev
