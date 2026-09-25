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
