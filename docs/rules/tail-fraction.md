# Tail fraction

"The gate should hold 0.2% to 0.5% of the FMX."

## What it's for

A marker whose positives smear out of the negative with no dip to cut at,
where each specimen has a control - usually its FMX - that shows where the
negative ends. The FMX has no staining for the marker, so anything above the
line on it is background; fixing how much background the gate lets through
puts the line at the top of the negative, sample by sample.

It also works on the sample itself (`measured_on: "Itself"`) when a fixed
percentage of that sample is genuinely what is wanted - rarely right for a
biological population, whose frequency is the thing being measured.

## When not to use it

- No FMX, or FMXs of poor quality or low event count.
- A clear dip between negative and positive: in the valley reads it directly.
- A band so narrow for the event count that it holds a handful of cells:
  0.2% of 2,000 events is four.

## How it works

1. It reads the FMX (the file `measured_on` names) for the specimen.
2. If the gate as it stands already holds a fraction inside the band on the
   FMX, it is left exactly where it is (with `aim: Middle`, only if it holds
   within a tenth of the band's width of the middle).
3. Otherwise it slides the whole gate, shape unchanged, along the marker and
   asks what it holds - counted by the same statistic the plot shows, so a
   slanted polygon or a gate bounded on its other axis is counted as drawn.
4. It searches by halving, like a number-guessing game. The range runs from
   a bit below the FMX's dimmest event to a bit above its brightest (the
   population's width either side). It puts the gate in the middle of the
   range and counts. Too many events: it looks further up. Too few: further
   down. It halves the range and tries again, up to 48 times.
5. With `aim: AnywhereInBand` it stops at the first guess anywhere inside the
   band. With `aim: Middle` it carries on and keeps the guess nearest the
   band's middle fraction.
6. The position is written for the whole specimen, so the full stain gets
   the gate the FMX set.

## Settings

- `band` - the acceptable fractions of the parent, as fractions: 0.2% to
  0.5% is `[0.002, 0.005]`. In the form, typed as percentages.
- `aim` - `AnywhereInBand` (the default) or `Middle`.
  - `AnywhereInBand` lands wherever the guessing first hits the band. The
    guesses are fixed by the ends of the search range, which are set by the
    single dimmest and brightest event in the FMX - so two nearly identical
    FMXs can land at opposite edges of the band, 0.21% and 0.49%, and a gate
    already inside the band is kept wherever in it it sits.
  - `Middle` lands every sample at the band's middle fraction, as near as its
    events allow, whatever its extreme events. The steadier choice; the
    default is kept so existing rules behave as they did.
- `pool` - `Specimen` (the default) or `Run`: which files the band is
  counted on.
  - `Specimen` counts it on each specimen's own file - its own FMX - and
    gives each specimen its own line.
  - `Run` counts it on every file of that kind in the sample's run together -
    all of the run's FMX files, pooled - and gives every specimen in the run
    the same line. The run is the pairing's run column, which has to be set
    (`set_run_column`). For small populations: a band of 0.2-0.5% of an FMX
    of 700 events is one to three events, so a line per specimen is set by
    where a couple of stray events fall; pooled over a run's FMX files, it is
    set by dozens. It is what "per run in the first instance" means.
  The line is solved once for each run, from the gate as it stands on the
  run's first file of that kind, and placed on every specimen of the run -
  including one whose own FMX is missing.
- `confidence` - see the shared settings in the choosing guide.

## Traps

- **Where in the band depends on one event** with `AnywhereInBand` (above).
  If similar samples land at noticeably different places, switch to
  `Middle` and replay.
- **The fraction is of the FMX, not the full stain.** The percentage the
  full stain's gate shows is the biology; the band only fixes the FMX's.
- **A band is not a target.** A band of 0.1% to 1% is a tenfold range in
  background and can move the line a long way through a smear.
- **Few events decide it.** The confidence's "events in the gate" score is
  1 - 1/sqrt(k) of the smaller side: 4 events score 0.5, 100 score 0.9.
  Reading the run's files together (`pool`: `Run`) is the remedy when the
  specimens are alike enough to share a line.
- **A pooled line hides a specimen that differs.** One line for the run
  means a donor whose negative sits higher gets the run's line, not its
  own; check the run with gate_profile before pooling.

## What its confidence says

- *parent event count* - how many events the FMX's parent holds.
- *events in the gate* - how many events decided the placement (the smaller
  of held and excluded).
- *stability of the gate's contents* - how much a nudge of a tenth of the
  population's interquartile width changes what the gate holds.
- *rule satisfied* - whether any position held a fraction in the band;
  outside it, how far.
- *distance moved from the reference* - how far the gate moved from where it
  stood on this sample before the run, in interquartile widths; half a width
  scores 0.

## Checking it

Look at the FMX with the gate on it: the line should sit at the top of the
negative. Then at the full stain: positives above, negatives below. Across
specimens, the FMX's line should track the top of each FMX's negative.
