# Above the negative

"As far above the negative as the reference sits."

## What it's for

Positives that smear out of the negative with no dip, and no FMX - but one
sample gated by hand (a QC or template). The distance between that sample's
negative and its gate is the only statement of "a bit above the negative",
so the rule reads it there and repeats it on every sample, measured from each
sample's own negative in units of that negative's own width. A negative that
drifts or broadens from run to run carries the gate with it.

## When not to use it

- The negative changes shape between samples - its width, or how lopsided it
  is - rather than just its position. The rule measures one side and
  assumes the other; see the traps.
- A clear dip between negative and positive: valley or smear reads it
  directly and amplifies nothing, and cuts the samples that smear as on one
  gated by hand.
- Negatives that merge with the positives in some samples: the width read
  from a merged population is too wide and is multiplied (one real case put
  a gate six widths past where it belonged).

## How it works

1. **Calibrate on the reference** (the file `measured_on` names, normally a
   hand-gated QC with `{"File": ...}`). Find its negative's centre and width,
   and read how many widths above the centre its gate sits:
   `widths = (gate - centre) / width`.
2. **Place on each sample.** Find its negative's centre and width and put the
   line at `centre + widths x scale x width + nudge`. The whole shape slides.
3. **The width** is always measured on the **left** side of the negative - the
   distance from the centre down to the point 31.7% of the way into the
   events below it (one standard deviation, for a normal negative). The right
   side runs into the positives, so measuring it there would let the
   positives widen it. The rule assumes the right side mirrors the left.
4. **Finding the centre** - `find` chooses how:
   - `NegativePeak` (the peak finder): smooth the population into a density
     and take the leftmost peak at least a quarter as tall as the tallest -
     so a large positive population is never mistaken for the negative. It
     ignores the gate. For smears, where there is no valley to cut at.
   - `BelowTheGate` (the default): the median of the events below the gate,
     improved step by step from where the gate stands (up to 12 steps, each
     moving the line at most one width, stopping if it starts to diverge).
     No smoothing, no thresholds; it assumes the gate starts roughly right.
     About five times sharper where negative and positive are separated;
     where the positives smear, cutting at the gate swallows the smear and the
     centre drifts about three times as far as the peak finder's.

## Settings

- `find` - `NegativePeak` or `BelowTheGate` (above).
- `scale` - multiplies the calibrated distance: 1 is exactly as on the
  reference, 1.1 a tenth further out.
- `nudge` - added afterwards, in the plot's units, independent of width.
- `confidence` - see the shared settings in the choosing guide.

## Traps

- **The right side is assumed, not read.** When a negative's median moves up
  the arcsinh scale its right side is squeezed more than its left, and when
  its tail shortens the right side pulls in. The left side then overstates
  the right and the gate lands too high. The placement's right-side check
  (below) flags this.
- **Read on itself it does nothing** unless `scale` or `nudge` is set: the
  calibration and the placement read the same negative.

## What its confidence says

- *parent event count*, *events in the gate*, *stability* - as for the band
  rule, counted on the sample itself (not the reference).
- *rule satisfied* - always 1; there is no band.
- *the negative's right side against the reference* - the side the rule does
  not read, checked afterwards. Both sides are measured on a smoothed density
  from the peak down to a quarter of its height. The score compares how many
  right-side widths above the peak the gate sits, here and on the reference:
  - further out than on the reference - the negative's shape has changed,
    and positives cannot cause it (they only widen the right side): up to
    1.25 times the reference's scores 1, falling to 0 at twice;
  - closer in - the right side has widened, which is what a positive smear
    does: noted, never scored below 0.5 on its own;
  - a right side that never falls to a quarter of the peak before the data
    ends (merged with what is above it) scores 0.5.

## Checking it

On a flagged sample, compare its negative's shape with the reference's: a
negative that is lopsided differently, narrower on the right or with a
shorter tail, is placed on an assumption that no longer holds. Several
peaks where the reference had one means the leftmost peak may not be the
negative.
