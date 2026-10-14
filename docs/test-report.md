# The test report

One button on the Gate Rules tab - **Make a test report** - runs a fixed set of
checks on the workspace as it stands and writes what happened to one Markdown
file, kept in the workspace's `test_reports` folder as
`test-report-<date>_<time>.md`. Send that file (or **Copy the report** and
paste it): it holds everything needed to see what the program did, without
the data. The tools for Claude make the same report with `test_report`.

Nothing in the workspace changes: no rule, no gate, no kept pick. It reads the
gating as it is in the app, saved or not, and the pick settings last used in
**Pick the best rule** (kept in `rules/pick_settings.json`).

## What it checks

The report's first line names its tests and their version (`rule picker,
version 1`), so a report can be read against what it ran.

| Section | What it says |
| --- | --- |
| Header | The tests and their version, clingate's version, when it was made, the workspace, the number of threads. |
| Workspace | The FCS files and how many have a metadata row, by sample type; the specimens; the gates drawn, the rules by kind and how many are picked for; the arcsinh channels. |
| Pick settings | The FMX band tried first, the agreement below which a sample is off, and the share of samples that may be off. |
| Picks | For every gate a rule places (not one placed from another gate): passed or FLAGGED, the rule picked and what it is measured on, its typical and lowest agreement with the hand gates, how many samples are off, how much it gains on the rule as it stands, and how many rules were tried. Then, per gate, each kind tried in order - searched or not, its closest rule, its typical agreement, its samples off, whether it passes - the rule as it stands, and the rules tied with the pick. |
| Sample by sample | Per gate, every sample, least agreeing first: what the picked rule did (moved, kept, reference, not placed), its agreement with the hand gate and the line below which it is off, how much of the hand gate it catches and holds beyond it, the hand gate's and the rule's share of the parent and their difference, how far it moved the edge in the parent's interquartile ranges, and the rule as it stands on the same sample. |
| Problems | Files that could not be read, rules refused, steps that failed or panicked - each said once. |
| Timings | How long the pick and each scoring took. |

A step that fails is a problem in the report, not the end of it: a pick that
cannot run says why and the rest of the report is still written.

## Adding a check

The report is built in `crates/clingate-core/src/test_report.rs` by
`test_report`, one section after another. A new thing to check is a new
section function called from there, its numbers taken from the code it
checks - as the picks and scores are - so the report says what the program
did rather than what a copy of it would do. Raise `SUITE_VERSION` whenever
what the report says changes, and add a test beside the others in
`test_report_tests.rs` and `gate_rules/runs_tests.rs`.
