# Valley or smear

"In the dip where there is one; where there is none, as far above the
negative as on a smear gated by hand."

## What it's for

A marker that is a clear population on some samples and a smear on others -
an activation marker, say, bright on stimulated samples and smearing out of
the negative on the rest. Each sample is read for a dip between its negative
and its positive, and the rule decides for that sample which it is:

- **A dip:** the gate goes in it, as far from its bottom as on the
  reference. Nothing is multiplied, so nothing is amplified: a negative that
  broadens does not throw the gate out.
- **No dip - a smear:** the gate goes as many negative-widths above the
  negative as it sits on a *smear example*, a smear gated by hand - as the
  above-the-negative rule places it, finding each negative by its peak.

It is always calibrated on a hand-gated reference (`measured_on` a `File`).

## When not to use it

- No hand-gated sample at all: use a band rule on the FMX.
- A smear that should be cut by the FMX, not by eye: a band rule.
- Dips that are not the negative/positive boundary (a dim population between
  them): the first dip wins.

## The smear example

A dip on the reference says nothing about where to cut a smear, so the rule
needs a smear gated by hand as well:

- **A reference that is itself a smear** is the example: every sample with no
  dip is placed from it, and so is every sample with a dip, there being no
  dip on the reference to place it from.
- **A reference with a dip**, and no example yet: the Gate Rules tab's run
  stops at the first smear the rule meets, before keeping anything of that
  level, and the editor shows it. Gate it by hand and Continue the run: it is
  saved into the rule as `smear_example`, and the level is run again, the
  example left where it was put and every other smear placed from it. Save
  the rules to keep it. Stopping the run instead takes it out of the rule
  again. The form shows the example, and Forget clears it, for the next run
  to stop at a smear again.
- Runs that never pause - the tools for Claude - leave a smear with no
  example unplaced, saying so. Gate one smear by hand and name it in the
  rule as `smear_example`. Which samples are smears is found by the run:
  there is no need to read the data beforehand to predict it.

The smear example is a reference, as the reference is: never moved by a run.

## How it works

1. **The dip**, on the reference and on the sample:
   - **The density.** The population on the marker is smoothed into a
     density (a kernel estimate over 512 points, bandwidth by Silverman's
     rule times `smoothing`).
   - **The negative** is the leftmost peak at least a quarter as tall as the
     tallest.
   - **The dip.** Walking right from it: down into a dip, up to whatever is
     on the far side. The first dip that is at least 2% deep (against the
     lower of the two peaks either side) and whose far side reaches at least
     5% of the tallest peak is the valley. A wobble in a sparse tail is not a
     valley, however deep it looks against a tiny far side.
   - **A small negative.** If that finds nothing and the tallest peak stands
     above where the gate is now, the tallest peak is the positives - a
     stimulated sample that is almost all positive. The highest bump below
     it is then the negative, and the lowest point between them is the
     valley, if at least 1% of the events and at least 30 of them lie below
     it and the dip is at least 2% deep against the bump. Judged by events
     rather than height, so a thin negative counts and a few stray events do
     not.
   - With `lowest_before`, the dip found moves to the lowest point of the
     density between the negative's peak and it; with `smallest_dip`, a dip
     shallower than it is no dip.
2. **Both have one:** the gate goes at the sample's dip bottom plus the
   reference's offset from its own.
3. **Otherwise**, with a `fallback`: the gate's edge - the one the dip would
   have set - goes where the fallback gate's same edge is on this sample.
4. **Otherwise**, from the example - the smear example, or the reference
   when the reference has no dip: its negative's peak and left-side width
   are read, and how many widths above the peak its gate sits; on the sample
   the line goes the same number of widths above its own negative's peak.
   The peak, not the events below the gate: on a smear, the events below the
   gate include the dim cells, and the more of them there are the higher it
   would read the negative.
5. **No example:** left unplaced - or, in a run that pauses, the run stops
   for one to be gated.

## Settings

- `smoothing` - scales the bandwidth the dip is looked for with. Below 1
  finds shallower dips, and more noise; above 1 smooths shallow ones away.
- `fallback` - optional, a gate named as a rule names one, `{"gate":
  "IFNy+", "parent": "CD4+"}`: on a smear, this gate's edge goes where that
  gate's is on the same sample, instead of as on the example. Usually the
  same gate under another parent, where the positives do separate. A run
  places it first.
- `smear_example` - the hand-gated smear, named as a rule names a file; set
  by a paused run, or by hand.
- `smallest_dip` - optional, a fraction: the shallowest dip, against the
  lower peak beside it, that counts. A shallower one is read as no dip, so
  the sample is a smear (or goes to the `fallback`). For positives that run
  straight off the negative as a plateau, where the rule would otherwise gate
  a 5% wobble in them. Read each placement's depth from its confidence
  ("the dip is x% as deep as the reference's") and set it between the
  wobbles and the real dips. It applies to the reference too: a reference
  shallower than it is a smear, and every sample is placed from it.
- `lowest_before` - `true` to gate a sample with a dip in the lowest point between the
  negative's peak and the dip found, rather than that dip. For positives
  spread thin: a few percent of the cells over a wide range stand lower
  beside the negative than the 5% a dip's far side needs, so the rule walks
  past the real dip and stops at a ripple inside the positives, where they
  pile up - a gate too high. The lowest point it walked past is the real dip.
  A sample whose first dip counted is placed as before, and a smear is still
  a smear. Off by default: on a marker with a third population above the
  positives, it can drop the gate to the dip below them. Choose it per rule,
  where a run puts some gates too high inside a thin positive population.
- `confidence` - see the shared settings in the choosing guide.

## Traps

- **What counts as a dip decides which way a sample goes.** A shallow dip
  is a dip: the sample is placed in it, scored on how deep it is against the
  reference's, so a placement in a dip a twentieth as deep comes up for
  review. Raise `smoothing` if shoulders on a smear are being read as dips.
- **The first dip wins.** A dim population between the negative and the
  bright positive gives an earlier dip.
- **One example for every smear.** A smear much brighter or dimmer than the
  example is cut as far above its negative, which may not be where a person
  would cut it.

## What its confidence says

- In a dip: *parent event count*, *events in the gate*, *stability*, *rule
  satisfied* (always 1) - counted on the reference population - and *depth
  of the valley it sat in*, this sample's dip depth against the reference's.
- On a smear: as above the negative - counts, events in the gate, stability,
  and the negative's right side against the example's.
- Placed by the fallback: one component, *no valley, so placed from another
  gate*, at 0.25 - below the Review tab's 0.30, so every one comes up for
  review, and above the 0.2 at which a run pauses, so it does not stop the
  run.
