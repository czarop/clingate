# The test reports

Two reports, each made by one button and kept in the workspace's
`test_reports` folder, to send on: the **test report** of the rule picker on
the whole workspace, and the **rule report** of how one rule did in the last
applied run.

## The test report

One button on the Gate Rules tab - **Make a test report** - runs a fixed set of
checks on the workspace as it stands and writes what happened to one Markdown
file, kept in the workspace's `test_reports` folder as
`test-report-<date>_<time>.md`. Send that file (or **Copy the report** and
paste it): it holds everything needed to see what the program did, without
the data. The tools for Claude make the same report with `test_report`.

Nothing in the workspace changes: no rule, no gate, no kept pick. It reads the
gating as it is in the app, saved or not, and the pick settings as typed in
**Pick the best rule** above it - whether or not a pick has been made with
them. The tools for Claude use the settings last kept (`rules/pick_settings.json`).

### What it checks

The report's first line names its tests and their version (`rule picker,
version 3`), so a report can be read against what it ran.

| Section | What it says |
| --- | --- |
| Header | The tests and their version, clingate's version, when it was made, the workspace, the number of threads. |
| Workspace | The FCS files and how many have a metadata row, by sample type; the specimens; the gates drawn, the rules by kind and how many are picked for; the arcsinh channels. |
| Copying rules to siblings | For each population with rules of its own, against each population with gates of the same names: how many rules a copy would write, and each it would leave out with why. Nothing is copied. |
| Compensation and unmixing | In the first population at the top of the gating, on up to 24 specimens spread through the workspace with their unstained files: each channel rising with another alike on every sample (the spillover value to change), on some samples only (their dye), and autofluorescence left in, with the channel to look in on the unstained - as the Workspace tab's "Check compensation and unmixing" says them. |
| Pick settings | The FMX band tried first, the agreement below which a sample is off, and the share of samples that may be off. |
| Picks | For every gate a rule places (not one placed from another gate): passed or FLAGGED, the rule picked and what it is measured on, its typical and lowest agreement with the hand gates, how many samples are off, how much it gains on the rule as it stands, and how many rules were tried. Then, per gate, each kind tried in order - searched or not, its closest rule, its typical agreement, its samples off, whether it passes - the rule as it stands, and the rules tied with the pick. |
| Sample by sample | Per gate, every sample, least agreeing first: what the picked rule did (moved, kept, reference, not placed), its agreement with the hand gate and the line below which it is off, how much of the hand gate it catches and holds beyond it, the hand gate's and the rule's share of the parent and their difference, how far it moved the edge in the parent's interquartile ranges, and the rule as it stands on the same sample. |
| Problems | Files that could not be read, rules refused, steps that failed or panicked - each said once. |
| Timings | How long the pick and each scoring took. |

A step that fails is a problem in the report, not the end of it: a pick that
cannot run says why and the rest of the report is still written.

### Adding a check

The report is built in `crates/clingate-core/src/test_report.rs` by
`test_report`, one section after another. A new thing to check is a new
section function called from there, its numbers taken from the code it
checks - as the picks and scores are - so the report says what the program
did rather than what a copy of it would do. Raise `SUITE_VERSION` whenever
what the report says changes, and add a test beside the others in
`test_report_tests.rs` and `gate_rules/runs_tests.rs`.

## The rule report

On the Review tab, **Report on this rule** reports on the gate chosen there:
every sample the last applied run placed it on, numbered, with its plot. It is
kept in a folder of its own, `test_reports/rule-report-<gate>-<date>_<time>/`,
holding `report.md` and the pictures; send the folder (or **Copy the report**
for the Markdown alone). The tools for Claude make the same report with
`rule_report`, and see the pictures. Nothing in the workspace changes.

| Section | What it says |
| --- | --- |
| Header | What it tells and its version (`rule run, version 1`), clingate's version, when it was made, the workspace, when the run was applied, the gate. |
| The rule | The rule the run placed the gate by - its kind, settings, parameter, side and what it reads - and whether it is the same now, changed (and to what) or removed. |
| Summary | How many placements, moved or already meeting the rule; how many flagged by the review and how many of those judged to look right; the Review tab's piles for the gate; what the run could not place. |
| Sample by sample | The flagged worst first, then the rest by sample: the number its plot carries, the sample and its type, flagged or not, its pile on the Review tab (and whether the gate has been moved since), where the line went from and to, the rule's confidence and what held it down, in band or not, what the placed gate admitted when placed, what the gate holds of the sample's parent now, the file the rule read where not the sample, and why the review flagged it. |
| Pictures | Which numbers are in which picture: `flagged-1.png`, ... then `not-flagged-1.png`, ..., nine plots to a picture, left to right, top to bottom. Each plot is the gate's parent population on the gate's two parameters, drawn as the gallery draws it, with the gate as it stands now and the sample's number in the corner. |
| Problems | A sample that could not be drawn - its tile is left grey with its number, so the rest keep their places. |
| Timings | How long drawing took. |

It is built in `crates/clingate-core/src/rule_report.rs`; raise its
`SUITE_VERSION` when what it says changes, and test it in
`rule_report_tests.rs`.
