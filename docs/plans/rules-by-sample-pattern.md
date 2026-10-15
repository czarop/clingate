# Plan: different rules for samples that look different

Status: planned, not built. Questions to settle first are at the end.

## The problem

A rule that places a gate well on most donors fails on a few, and a rule
that suits those few fails on the rest. Today the choice is one rule for
all or gating the few by hand. The aim: find what the failing samples have
in common, in the data, and justify a second rule for samples like them -
with the evidence to show it is a real difference and not a fit to noise.

## Telling which samples a rule fails on

From any of:

- hand gating - `score_rules`' agreement per sample (the strongest);
- the review - placements flagged, reported, moved by hand after the run;
- a rule report the user marks up.

## What is compared

For each sample, what the data under the gate looks like - all already
measured somewhere in clingate:

- the shape class of the parent on the rule's parameter (separate,
  shoulder, smear, merged, negative only - `gate_profile`);
- the negative's width, the dip's depth, the positives' separation;
- events in the parent; the share above the line;
- the FMX against the full stain (a shift between them);

and what the metadata says: day, donor, plate, stim, sample type.

## Finding the pattern

For one gate:

1. Split the samples into those the rule places well and badly.
2. For each property, the cleanest split it gives (a threshold for a number,
   a set of values for a category), scored by how well it separates good
   from bad. Only splits with enough samples on each side.
3. Guard against fitting noise: the split must hold when each donor is left
   out in turn, and must beat the same search on shuffled labels.
4. For the bad group, `pick_rule` restricted to those samples: the best rule
   for them. For the good group, the rule as it stands.
5. The proposal, with numbers: "samples where the parent is a smear on
   CD69 (9 of 36, all D29): the band on the FMX agrees 0.96, the valley rule
   0.71. Elsewhere the valley agrees 0.97. A second rule for smears: typical
   agreement 0.91 → 0.96, samples off 9 → 1."

If no split passes, it says so, and asks - see below.

## Writing it

A rule entry gets variants: "for samples where <condition>, this rule;
otherwise the main rule". A condition is either:

- **metadata**: a column's value (Day = D29) - simple, fixed per sample;
- **the data**: the parent's shape class on the parameter, or a measured
  property against a threshold - decided by the run, sample by sample, so a
  new donor is placed by what its data looks like.

Every run, report and review says which variant placed each sample, and why.
Variants are written only on the user's word, like any rule.

## Asking when stuck

When no split is clean enough, or the groups are too small, Claude asks a
few targeted questions instead of guessing, e.g.:

- "These 4 samples are off and share nothing measurable - is anything
  different about them (a different stim, a known bad tube)?"
- "On smears, should the gate follow the FMX, or stay where the hand gate
  puts it on the QC?"
- "Is a D29 difference expected biologically, or a batch effect?"

The answers go into the proposal's justification, kept with the rule.

## Questions to settle before building

1. **Conditions on metadata, on the data, or both?** Data conditions are
   more general (a new donor is placed by what it looks like) but harder to
   audit; metadata conditions are simple but only cover known groups.
2. **Where do the "good" and "bad" labels come from for you** - mostly hand
   gating (score_rules), or the review after a run?
3. **How many variants per rule** - two (one alternative), or more?
