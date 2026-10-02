# Valley or smear

"In the dip where there is one; where there is none, as far above the
negative as on a smear gated by hand."

## What it's for

A marker that is a clear population on some samples and a smear on others -
an activation marker, say, bright on stimulated samples and smearing out of
the negative on the rest. Each sample is read for a dip between its negative
and its positive, and the rule decides for that sample which it is:

- **A dip:** the gate goes in it, as far from its bottom as on the
  reference - exactly as the in-the-valley rule places it.
- **No dip - a smear:** the gate goes as many negative-widths above the
  negative as it sits on a *smear example*, a smear gated by hand - as the
  above-the-negative rule places it, finding each negative by its peak.

It is always calibrated on a hand-gated reference (`measured_on` a `File`).
It replaces in the valley and above the negative in the Gate Rules tab;
rules of those kinds still load and run.

## When not to use it

- No hand-gated sample at all: use a band rule on the FMX.
- A smear that should be cut by the FMX, not by eye: a band rule.
- Dips that are not the negative/positive boundary (a dim population between
  them): the first dip wins, as in the valley.

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

1. **The dip**, on the reference and on the sample, exactly as in the valley
   finds it (see that guide): a density smoothed by `smoothing`, the
   leftmost peak, and the first dip deep enough to count.
2. **Both have one:** the gate goes at the sample's dip bottom plus the
   reference's offset from its own.
3. **Otherwise**, with a `fallback`: the gate's edge goes where the fallback
   gate's is on this sample, as in the valley's fallback.
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
- `confidence` - see the shared settings in the choosing guide.

## Traps

- **What counts as a dip decides which way a sample goes.** A shallow dip
  is a dip: the sample is placed in it, scored on how deep it is against the
  reference's. Raise `smoothing` if shoulders on a smear are being read as
  dips.
- **One example for every smear.** A smear much brighter or dimmer than the
  example is cut as far above its negative, which may not be where a person
  would cut it.

## What its confidence says

- In a dip: as in the valley - counts, events in the gate, stability, and
  the depth of the dip against the reference's.
- On a smear: as above the negative - counts, events in the gate, stability,
  and the negative's right side against the example's.
- Placed by the fallback: one component, at 0.25, so every one comes up for
  review.
