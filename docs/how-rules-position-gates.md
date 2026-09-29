# How gate rules position gates

This is an exact description of what clingate's gate rules do, written so
that a person - or Claude, through the `explain_gate_positioning` tool - can
reason about why a gate landed where it did and how a rule could do better.
Every step names the function that does it, so the description can be
checked against the code (`read_positioning_code` returns the source).

Numbers are in the plot's display space: the arcsinh-scaled values the plots
show, not raw channel values.

## 1. The words

- **Specimen** - one donor or sample, named by the pairing's
  `sample_id_column` in the metadata. A specimen has several files: the full
  stain, and controls such as an FMO or an FMX (a "fluorescence minus X"
  control).
- **Sample type** - which kind of file it is (FS, FMX, FMO, ...). Read from
  the pairing's `sample_type_column`, or derived from other columns when the
  Gate Rules tab says how (`SamplePairing::sample_type_of`).
- **display_order** - the sample types in order, controls first and the full
  stain last. It decides which file of a specimen is *the* file being gated.
- **Rule target** - a gate name, optionally with its parent's name ("CD69+ of
  CD4+"). A rule naming the parent wins over one that does not
  (`RuleStore::rule_for`).
- **GateRule** - `parameter` (the marker it positions along, never an axis),
  `bound` (`Above` keeps events above the line - a positive gate; `Below`
  keeps those below), `measured_on` (which file it reads) and `rule` (how).
- **measured_on**:
  - `Partner(type)` - the file of the *same specimen* with that sample type,
    e.g. the specimen's FMX. A hand-picked reference override for a file wins
    over the pairing (`RuleStore::reference_file`).
  - `Itself` - the file being gated.
  - `File(id)` - one named file for every specimen, e.g. a QC sample gated by
    hand.
- **The line** - the gate's leading side on the rule's parameter: its lower
  extent for an `Above` rule, its upper extent for `Below`
  (`LineReading::current`).

## 2. A run, end to end

`gate_rules::run` drives it; `autogate` does the work.

0. **Levels** (`rule_levels`). A gate's population is its parent's events,
   so a gate under another gate a rule moves has to be measured after that
   gate is placed. The ruled gates are put in levels by the tree: level 0 has
   no ruled gate above it, level 1 has one, and so on. Steps 1-6 run once per
   level, each measuring on the gates as the levels above left them, and the
   next level reads through those placements. A gate whose rule is from
   another gate also waits for every gate it follows that a rule places. The
   order the rules are listed in plays no part. A rule from another gate
   whose anchor is not one gate, is itself, or leads round in a loop back to
   it is reported and left alone (`anchor_problems`). A run with no ruled gate under another is one level,
   and reads each file once; each further level reads every file again.
   Before any level: a rule that reaches no gate is reported
   (`rules_reaching_nothing`), and a linked gate the rules reach at more than
   one place - two rules, or one rule at two parents - is left alone and
   reported (`linked_conflicts`): it has one position, and two readings of it
   would fight over it.
   A trial (`try_rules`) and a profile read the gates as they stand, not as
   a run would leave the levels above; a replay re-solves each gate on the
   events its run kept, so a changed parent rule does not re-filter the
   gates under it.
1. **Measure** (`measure_file` -> `measure_population`). For every file and
   every gate a rule names, the gate's parent population is filtered exactly
   as the plot filters it (same gate chain, same override resolution, same
   compensation and scaling), and read into a `Measurement`:
   - `values` - the parent population on the rule's parameter;
   - `current` - where the gate's line sits on this file now;
   - `shadow` - each event paired with its distance from the gate's boundary
     *at that event's own height on the other axis*, so a slanted or curved
     boundary is handled: sliding the gate by `d` puts exactly the events
     with offset below `d` behind it. Events the gate never reaches on its
     other axis are left out.
   - `index` - the population indexed as the plot indexes it, so "what does
     this gate admit" is answered by the same statistic the screen shows
     (`admitted_by`).
   Refused here (reported as skipped): the rule's parameter is not one of
   the gate's axes; the gate is a shape that cannot slide along one axis; its
   line is unbounded; the parent has fewer than 2 events. The same reason on
   several files is one line, naming the first file and counting the rest.
2. **One file per specimen** (`solve_all_reporting`). A specimen's files
   share one gate position, so only one of them is solved: the file whose
   sample type comes *latest* in `display_order` (`gated_rank`) - normally
   the full stain. If no file has a sample type the order names, a warning is
   reported and each rule reads whichever file sorted first.
3. **References are kept.** If the rule is `File(id)` and the file belongs to
   the specimen being positioned, that specimen is the hand-gated reference:
   it is reported as a reference and never moved (moving it would make the
   next run calibrate from a moved gate).
4. **Resolve the reference file** (`resolve_reference`): `Itself` is the
   gated file, `File(id)` is that file, `Partner(type)` is the override for
   this file if one exists, otherwise the specimen's file of that type. No
   reference file measured -> skipped, saying which: the named file is not
   in the workspace; the specimen has no file of that type; the reference
   was read but could not be measured (and why); or it is in the metadata
   but was not loaded (`why_no_reference`).
5. **Position** (`position_one`, section 3).
6. **Write back** (`apply_placements`). The moved gate is written as a
   per-specimen (group) override, so every file of the specimen - FMX and
   full stain alike - shows the new position. A later per-file move by hand
   wins over it (most recent wins).

Every list in the report is then sorted by the pairing's sort column, with
each placement kept beside its own line (`sort_report`).

## 3. Positioning one gate

`position_one` works with two measurements: the **sample** (the gated file)
and the **reference** (the file `measured_on` names; for `Itself` the same
file). `current_gate` is the gate as it stands *on the reference file*.

### 3.0 Already right? (band rules only)

For a rule with a band (only `TailFraction`), the fraction of the reference
population the current gate admits is counted. If it is already inside the
band the gate is **kept** where it is ("unchanged", met the rule). Every other
rule is always re-solved.

### 3.1 TailFraction - "capture 0.2% to 0.5% of the reference population"

Stored as `band: (low, high)` in fractions of the parent.

The gate is **slid rigidly** along the parameter and asked what it holds -
no line is solved and no corner anchored, so slanted shapes work
(`slide_to_capture`):

- The search bracket is from `min(reference values) - current - margin` to
  `max(reference values) - current + margin`, as slide distances, with
  `margin = max(max - min, 1)` (`bracket_for`). Note it is built from the
  *reference* population's extremes and the *sample's* current line.
- Bisection, at most 48 steps. Each probe slides the gate to the middle of
  the bracket and counts the fraction of the **reference** population it
  admits (`admitted_by`). Too many admitted -> move further along the
  parameter (for `Above`), too few -> back.
- **With `aim: AnywhereInBand` (the default) it stops at the first probe
  inside the band.** It does not aim for the middle of the band. Where in the band it stops depends on the bisection
  path, which is fixed by the search range - from the reference
  population's dimmest event less the population's width to its brightest
  plus the width - so moving the single brightest or dimmest event can move
  where the gate lands anywhere within the band. Where the gate started
  does not change the path; it matters only in that a gate already in the
  band is kept where it is (3.0). (The band's midpoint is used only to
  choose the best probe when no probe ever lands inside the band.)
- **With `aim: Middle`** it does not stop there: it carries on bisecting
  towards the band's middle fraction and keeps the probe nearest it, so
  every sample lands at the same fraction as near as its events allow,
  whatever its extreme events. A gate is then left where it is (3.0) only if
  it already holds within a tenth of the band's width of the middle.
- No probe could be evaluated -> skipped ("no position along this axis holds
  the band").

The separate function `threshold::tail_fraction` - which does aim for the
middle of the band and puts the line midway between the two events either
side of that count - is **not** what a run uses to place a gate; it is used
by `Rule::solve`/`GateRule::solve` only.

The placement is judged on the reference population: `achieved` is the
fraction of the reference (for `Partner(FMX)`, the FMX) inside the moved
gate, and `in_band` says whether it is within the band.

### 3.2 PercentileOffset - "the 99th percentile of the negative, plus 0.3"

`GateRule::solve` on the **reference** values: the percentile by linear
interpolation between order statistics, plus `offset` in display units
(`threshold::percentile_offset`). For a `Below` rule the values are negated,
solved, and the answer negated back - so the percentile is counted from the
top. The gate's line is then moved to that coordinate
(`translate_edge_to`). Judged on the reference population. No band, so
"rule satisfied" always scores 1.

### 3.3 AboveTheNegative - "as far above the negative as on the reference"

Two steps: **calibrate** on the reference, **place** on the sample
(`AboveTheNegativeRule::calibrate` / `place`).

- Calibrate: find the reference's negative (centre `c_ref`, width
  `s_ref`), and read how many widths above it the gate's current line
  `x_ref` sits on the reference file: `widths = (x_ref - c_ref) / s_ref`.
- Place: find the sample's negative (`c`, `s`) and put the line at
  `c + widths * scale * s + nudge`.

The negative is found one of two ways (`find`):

- `NegativePeak` (`threshold::negative_peak`): a Gaussian KDE with
  Silverman's bandwidth over 512 points spanning the data; the centre is the
  **leftmost** local maximum at least 25% as tall as the tallest (falling
  back to the tallest point). Ignores the gate.
- `BelowTheGate` (`threshold::negative_below`, `refine_from`): the centre is
  the median of the events in the gate's shadow (below the line). For
  placing, this is refined iteratively from where the gate stands: up to 12
  passes; each pass moves the line at most one width; it stops when a pass
  does not move it, or as soon as a step is larger than the previous one
  (diverging).

In both, the width is a **left-flank sigma**: the distance from the centre
down to the value 31.74% (= 0.1587 / 0.5) of the way into the events below
the centre - so positives on the right flank cannot inflate it.

Consequences worth knowing:

- With `measured_on: Itself` and `scale = 1`, `nudge = 0`, the calibration
  and the placement read the same population and the gate stays exactly
  where it is. The rule only does something with a Partner/File reference,
  or a scale/nudge.
- The placed gate is judged on the **sample's** own population (not the
  reference), and its "distance moved" is not scored - moving is the point.
- The right side of the negative is never read to place the gate, but it is
  checked afterwards (`threshold::peak_sides`, `confidence::right_side`): how
  many right-side widths above the peak the gate landed - each side measured
  on a smoothed density, from the peak down to a quarter of its height -
  against the same on the reference. A gate much further out than on the
  reference means the negative's shape has changed (its right side pulled in,
  or its left side spread) and the mirror assumption no longer holds: likely
  too high. A gate closer in usually means positives smearing into the
  negative, which is often a stimulated sample rather than a wrong one, so
  that direction is only noted. See the table in section 4.

### 3.4 InTheValley - "in the dip, where it sits on the reference"

`ValleyRule::calibrate` / `place` over `threshold::first_valley`:

- KDE with Silverman's bandwidth times `smoothing`, 512 points over
  `[min, max]`.
- The negative is the leftmost local maximum at least 25% of the tallest.
- Walk right: descend to the bottom of a dip, climb to the summit on its far
  side. `depth = (min(left peak, right summit) - bottom) / min(...)`. The
  first dip with `depth >= 0.02` **and** a far-side summit at least 5% of the
  tallest is the valley. None -> refused with what the density looked like
  (one peak only / nothing deep enough).
- Calibrate: `offset = x_ref - bottom_ref` on the reference. Place: the line
  goes at `bottom + offset` on the sample.

Judged on the reference population; distance moved not scored. An extra
component compares the dip's depth with the reference's (below).

### 3.5 MatchThePhenotype

Does not move a line. It describes the cells inside the reference gate by a
robust z (median and MAD, with the far tail trimmed) of each chosen marker
against their own parent, finds the cells in the sample that match, and
either moves and resizes the drawn shape onto them (`KeepShape`) or traces a
new polygon round them (`DrawPolygon`). It must be measured on one named,
hand-gated file (`File(id)`). See `gate_rules/phenotype.rs` and
`position_by_phenotype`. It is **not replayable** (runs keep only the gate's
two axes, and it reads the whole marker panel).

## 4. Confidence

Each placement gets components, each scored 0 to 1 (clamped; unmeasurable
counts as 0). **The overall confidence is the minimum**, and the lowest
component is reported as "weakest" (`Confidence::from_components`). The
population the numbers are counted on is the one the placement is judged on
(3.1-3.4). Limits (`ConfidenceLimits`, can be changed per rule in the rules
file): `events_full = 10000`, `events_floor = 100`, `swing_half = 1`,
`displacement_limit = 0.5`.

| component | score |
|-----------|-------|
| parent event count | `ln(n / 100) / ln(10000 / 100)`, 0 at or below 100, 1 at or above 10000 |
| events in the gate | `1 - 1/sqrt(k)`, `k` = the smaller of admitted and excluded events |
| stability of the gate's contents | `1 / (1 + swing)`; the gate is nudged +-0.1 x the interquartile range (IQR) of the judged population; `swing = abs(held when nudged back - held when nudged forward) / held` |
| rule satisfied | 1 in the band or with no band; otherwise `1 - miss / band width` |
| distance moved from the reference | band and percentile rules only: `1 - (abs(to - from) / IQR) / 0.5`, where `from` is the sample's line **before the run** (not the reference file's line) |
| depth of the valley it sat in | valley rule only: sample dip depth / reference dip depth |
| the negative's right side against the reference | above-the-negative only, positive gates: `q` = (right-side widths the gate sits above the peak) / (the same on the reference). `q` up to 1.25 scores 1, falling to 0 at 2. Below 1, 1 down to 0.7 and 0.5 at 0.4 and below - never lower, because a smear widens the right side. A right side that never falls to a quarter of the peak before the data ends (merged with what is above) scores 0.5 |
| phenotype rule | events matching, purity, how much of the population is caught, one cloud, abundance against the reference (`confidence::assess_match`) |

The Gate Rules tab and the Review tab flag a placement below 0.30.

## 5. The review (what "looks wrong" means)

`review::assess` compares each placement with its **peers**: the other
samples of the same sample type whose placement was confident (>= 0.5) and in
band; fewer than 3 such peers and every sample of that type is used. Each
comparison is a robust z-score (median and MAD, the sample left out) - 2 is
notable, 3 or more is flagged - on:

- where the gate sits between the sample's own negative and positive peaks,
  as a fraction of the way from one to the other (when the sample and at
  least half of - and at least 3 - peers have two peaks), otherwise in IQRs
  from its median (weight 1);
- the fraction of the sample's events beyond the line, in log-odds
  (weight 0.75);
- the distribution: median shift (weight 0.6) and spread (weight 0.5);
- the rule's confidence (below 0.30 is flagged whatever the peers say), and
  whether it reached its band.

Each z has a floor on the peer spread, so near-identical peers do not make a
hair's difference look enormous (`Measure::floor`). The "typical peer" shown
beside a flagged sample is, of its peers that are not flagged themselves, the
one whose gate position measure is nearest the peers' median (`typical_of`).

A person then reviews the run: accepts it, moves gates by hand, or reports a
placement with a problem (`too_high`, `too_low`,
`cuts_through_a_population`, `wrong_population`, `too_tight`, `too_loose`,
`wrong_reference`, `should_not_have_moved`, `other`) and, when the workspace is saved, the fix. Marking the run
reviewed writes `review.json`, and copies it with the reports and the run's
events into the review library when one is set.

## 6. Replays

### What a run keeps

Every run keeps, per gate per file measured (`review/events.rs`,
`reviews/run_events.bin`):

- events of the gate's **parent population** - the events the plot of that
  gate shows, filtered by every gate above it - on the gate's two axes. Not
  a sample of the whole file: a rare parent is kept whole or nearly so.
  All of them up to 5,000; beyond that an even step through them (events
  are in acquisition order, so this is a fair sample) plus the extremes of
  each axis. A band or percentile rule reads a thin tail, which 5,000
  events leave only a handful of, so for those enough are kept that the
  thinner side of the rule holds about 100 events - 100 / 0.002 = 50,000 for
  a band starting at 0.2% - up to 50,000 (`kept_for`);
- the population's total count;
- the gate as it stood on that file before the run, shape and all;
- every file's metadata row.

Coordinates are stored as 16 bits across the range the events span.

A review library keeps each population **once**: in `events/` beside the
reviewed runs, named by a hash of its contents; each reviewed run's
`run_events.json` lists which gate and file each was read for. Rerunning
the rules leaves every population whose parent gates did not move exactly
as it was, so a rerun adds only its lists and the populations that changed.

### Replaying

A replay (`review/replay.rs`, `replay_run`) rebuilds those populations and
gates and runs the **real** solver (`measure_population` + `solve_all`) on
them - with the run's own rules (the *baseline*) and with proposed rule
changes. Each placement is judged against where the review says the gate
belongs:

| truth | right gate |
|-------|-----------|
| accepted as placed | where the rule put it |
| left alone, accepted | where it was |
| reported, fix saved | the gate the reviewer left, shape and all |
| moved by hand, no report | the gate the reviewer left, shape and all |
| reported, no fix | none known - only what was wrong |

Placements are compared on **what the gate holds**: the fraction of the
sample's kept events inside the gate, with its real shape on both axes,
counted by the same statistic the plot shows (`admitted_by`). Every rule
that moves a line slides the whole shape, unchanged, along its parameter,
so the run's, the baseline's and the replay's gates are the gate before
the run slid to each one's line. The reviewer's gate is used as they left
it - reshaped or not (reviews made before the shape was kept are judged as
the gate slid to the reviewer's line, and the case says so when the extents
show a reshape). Close enough: within 20% of the smaller of the right
fraction and what it leaves out, or 3 of the events kept, whichever is
larger. Each line's fraction past the line on the parameter alone
(`beyond`) is reported beside it, for reading.

Verdicts: `fixed` (run wrong, replay right), `broken` (run right, replay
wrong), `still_wrong`, `still_right`; with no right answer, `changed` or
`unchanged`; `not_reproduced` - the baseline replay did not reproduce what
the run did on the kept events, so no change can be judged on that case (a
band rule stopping at the first probe inside its band, near the band's edge,
is the usual reason); `not_replayed` - the replay could not place it (the
case says why).

### The same placement twice

Each case carries a fingerprint of its **input**: the run's rule; the
sample's population, metadata and gate shape; the file the rule read - its
population, metadata, and its gate with its position, since the rule is
calibrated on it; and where the gate started on the sample, for the rules
that read that (a band rule, which searches from it and keeps a gate already
in its band, and above-the-negative with the finder that refines from the
gate). A gate's id, label and the files it applies to are not part of it.
Two runs with the same input for a placement did the same thing on the same
data: the case is counted once, with the later run and its review, and the
earlier reviews are listed with it (`earlier_reviews`) - two reviews of one
placement disagreeing is worth knowing.

Runs applied before events were kept this way cannot be replayed; rerun the
rules to make them replayable.

### Shapes

Rectangles, polygons and line gates are positioned and replayed; the rule's
parameter must be one of the gate's two axes. The line is the shape's
extreme on that parameter (its leftmost vertex, for a positive gate), and
a rule moves the whole shape rigidly so its line lands where the rule says.
Where the rule reads the population against the gate - the negative below
it, the band it holds - it reads each event against the boundary **at that
event's height** (`boundary_at`), so a slanted polygon is read as slanted.
Ellipses and gates made of other gates (quadrants) cannot be slid along one
axis; a rule on one is refused and says so. Phenotype rules reshape the
gate, but read the whole marker panel, which runs do not keep, so they are
not replayed.

## 7. Known behaviours worth discussing

These are properties of the code as it stands, not settled choices:

1. **Band rules stop at the first in-band probe** (3.1), so where in the
   band a gate lands depends on the reference population's two most extreme
   events, and a gate already in the band is kept wherever in it it sits.
   Two similar samples can land at opposite edges of the band. `aim: Middle`
   is the steadier choice.
2. **The band rule's bracket mixes files**: it is built from the
   reference's values and the sample's current line, and the slide is
   applied to the gate on the reference file.
3. **`distance moved`** is measured from the sample's own line before the
   run, not from the reference sample's line, whatever its name says.
4. **AboveTheNegative with `Itself`** does nothing unless scale or nudge is
   set (3.3).
5. **AboveTheNegative multiplies a width**: a sample whose negative reads
   wider (merged populations, smeared positives) carries the gate further
   out in proportion - InTheValley exists for that case, but needs a real
   second population.
6. **Kept when in band** is judged on the gate as it stands on the
   reference file, so a gate kept as "met the rule" is never re-examined for
   position relative to its peers until the review.
7. **Confidence limits are uncalibrated guesses** except
   `displacement_limit`, which came from one workflow's hand moves.

## 8. The rules file

`rules/gate_rules.json` in the workspace. A rule entry:

```json
{
  "target": { "gate": "CD69+", "parent": "CD4+" },
  "rule": {
    "parameter": "CD69",
    "bound": "Above",
    "measured_on": { "Partner": "FMX" },
    "rule": { "kind": "TailFraction", "band": [0.002, 0.005] }
  }
}
```

Rule kinds and their fields:

- `{"kind": "TailFraction", "band": [low, high], "aim": "AnywhereInBand" | "Middle"}`
- `{"kind": "PercentileOffset", "percentile": 99.0, "offset": 0.3}`
- `{"kind": "AboveTheNegative", "scale": 1.0, "nudge": 0.0, "find": "BelowTheGate" | "NegativePeak"}`
- `{"kind": "InTheValley", "smoothing": 1.0}`
- `{"kind": "MatchThePhenotype", "markers": ["CD161"], "fit": "KeepShape" | "DrawPolygon", "keep": 0.95, "smoothing": 1.0, "vertices": 24}`
- `{"kind": "FromAnotherGate", "same_shape_as": {"gate": "CD4-CD8+", "parent": "..."}}`, or
  `{"kind": "FromAnotherGate", "edges": [{"anchor": {"gate": "CD19+CD14-", "parent": "CD45+"}, "parameter": "CD19", "side": "Upper" | "Lower", "anchor_side": "Lower" | "Upper", "gap": 0.0}]}` -
  the anchor is placed first (section 2); `parameter`, `bound` and `measured_on` are ignored.

`measured_on` is `"Itself"`, `{"Partner": "<sample type>"}` or
`{"File": "<file id>"}`. Every rule kind except MatchThePhenotype and
FromAnotherGate also takes
`"confidence": {"limits": {"events_full": 10000, "events_floor": 100,
"swing_half": 1.0, "displacement_limit": 0.5}}`.
