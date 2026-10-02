# In the valley

"In the dip between negative and positive, where it sits on the reference."

## What it's for

A negative and a positive that are separate populations with a dip between
them - the boundary a person reads off a contour plot. The rule finds the
dip on each sample and puts the gate the same distance from its bottom as
on the reference. Nothing is multiplied, so nothing is amplified: a negative
that broadens does not throw the gate out.

## When not to use it

- Positives that smear with no dip: there is nothing to find, and the rule
  refuses those samples (above the negative is for them) - unless it has a
  `fallback`.
- Datasets where some samples have a positive population and others only a
  smear: the smear samples will be refused, or placed by the `fallback`.
  Valley or smear places them as on a smear gated by hand; the Gate Rules
  tab offers it in place of this rule.

## How it works

1. **The density.** The population on the marker is smoothed into a density
   (a kernel estimate over 512 points, bandwidth by Silverman's rule times
   `smoothing`).
2. **The negative** is the leftmost peak at least a quarter as tall as the
   tallest.
3. **The dip.** Walking right from it: down into a dip, up to whatever is on
   the far side. The first dip that is at least 2% deep (against the lower of
   the two peaks either side) and whose far side reaches at least 5% of the
   tallest peak is the valley. A wobble in a sparse tail is not a valley,
   however deep it looks against a tiny far side.
   **A small negative.** If that finds nothing and the tallest peak stands
   above where the gate is now, the tallest peak is the positives - a
   stimulated sample that is almost all positive. The highest bump below it
   is then the negative, and the lowest point between them is the valley, if
   at least 1% of the events and at least 30 of them lie below it and the dip
   is at least 2% deep against the bump. Judged by events rather than height,
   so a thin negative counts and a few stray events do not.
4. **Calibrate** on the reference: the offset from the bottom of its dip to
   its gate. **Place** on each sample: the bottom of its dip plus the same
   offset. The whole shape slides.
5. No dip at all - on the reference or on this sample: refused, with what
   the density looked like (one peak only, or dips too shallow or in the
   tail). With a `fallback`, the gate's edge - the one the dip would have
   set - goes where the fallback gate's same edge is on this sample instead.

## Settings

- `smoothing` - scales the bandwidth. Below 1 finds shallower dips, and more
  noise; above 1 smooths shallow ones away. Which is wanted depends on the
  marker.
- `confidence` - see the shared settings in the choosing guide.
- `fallback` - a gate named as a rule names one, `{"gate": "IFNy+",
  "parent": "CD4+"}`: where this gate goes on a sample with no dip. Usually
  the same gate under another parent, where the positives do separate. It
  has to be drawn on the rule's parameter. When a rule places it, the run
  places it first, as for a rule from another gate.

## Traps

- **A shallow dip is placed, not refused,** and scored low: its depth
  against the reference's dip is part of the confidence, so a placement in a
  dip a twentieth as deep comes up for review.
- **The first dip wins.** A dim population between the negative and the
  bright positive gives an earlier dip.

## What its confidence says

- *parent event count*, *events in the gate*, *stability*, *rule
  satisfied* (always 1) - counted on the reference population.
- *depth of the valley it sat in* - this sample's dip depth against the
  reference's.
- Not scored on distance moved: finding each sample's own dip is the rule.
- Placed by the fallback: one component, *no valley, so placed from another
  gate*, at 0.25 - below the Review tab's 0.30, so every one comes up for
  review, and above the 0.2 at which a run pauses, so it does not stop the
  run.
