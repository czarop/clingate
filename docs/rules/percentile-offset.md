# Percentile offset

"The line sits 0.3 above the 99th percentile of the FMX."

## What it's for

Setting a line a fixed visual step off the top of a negative: take a high
percentile of the file the rule reads - on an FMX, that is the top of the
negative - and step off it. For a shoulder with no positive population to
aim at, where a person would put the line "just clear of the negative".

## When not to use it

- The file it reads has positives in it: the percentile then reads the
  positives, not the negative. Read an FMX, not the full stain.
- The negative's width changes a lot between samples: a fixed step is too
  much for a tight negative and too little for a wide one. Above the
  negative scales with the width.

## How it works

1. It reads the file `measured_on` names.
2. It takes the chosen percentile of the parent population on the marker,
   interpolating between the two events either side.
3. It adds `offset`, in the plot's own units (after the arcsinh or other
   scaling), and moves the gate's leading edge to that line - the whole
   shape slides, unchanged.
4. For a `Below` gate the values are mirrored, so the percentile is counted
   from the top: the 99th percentile of a `Below` rule is the 1st of the
   values, and the offset steps down from it.
5. It is always re-solved; no band means no "already right".

## Settings

- `percentile` - 0 to 100.
- `offset` - in the plot's display units. The same offset means very
  different distances either side of an arcsinh transform, which is why it
  is visual rather than in raw channel values.
- `confidence` - see the shared settings in the choosing guide.

## Traps

- **High percentiles are decided by a few events.** The 99.9th percentile of
  5,000 events is the 5th brightest. Low-count controls give jumpy lines.
- **Display units.** An offset tuned on one scaling is wrong after the
  cofactor changes.

## What its confidence says

As tail fraction's, except *rule satisfied* is always 1 (there is no band).
*Distance moved from the reference* compares with where the gate stood on
this sample before the run.
