# Match the phenotype

"The cells that look like the ones I gated on the QC, wherever they are."

## What it's for

Populations the other rules cannot reach, because they do not separate
along one axis: several clusters close together, a smear with no dip, a
population that moves in both axes between donors (MAIT cells are the case
that defeats every line rule). Instead of moving an edge, it identifies the
population by where its cells sit across chosen markers and fits the gate to
them.

## When not to use it

- A population that separates cleanly on one marker: a line rule is simpler
  and easier to check.
- Without a hand-gated reference sample that contains the population.
- Markers whose staining differs systematically between samples in a way
  that is not biology: each sample is read against its own parent, which
  absorbs shifts but not changes of shape.

## How it works

1. **Read each marker on each sample's own landmarks.** On the parent
   population: 0 at the negative's peak, 1 at the valley above it - where a
   person would put a positive gate. A marker with no valley on either the
   reference or the sample (all negative, or a smear) is read on both as a
   robust z instead: spreads from the parent's middle (median and MAD, with
   values more than 4 spreads out dropped first). Nothing is rescaled between
   samples: each is its own frame, so brightness may drift.
2. **Describe the reference population marker by marker.** For each marker,
   the range its cells inside the reference gate sit in - ranges that
   together hold 95% of them - and its middle and spread.
3. **Find them in each sample.** A cell matches only if it is one of the
   population on **every** marker; one marker out is no match - CD8 T cells
   that are CD4-positive are not CD8 T cells, however well the rest agree.
   What "one of them" means on a marker comes from the reference's range
   (each end widened by a tenth of how far out it sits, for the noise in the
   landmarks):
   - wholly above the valley: positive - any cell above the valley, however
     bright or dim;
   - wholly below it: negative - any cell below the valley;
   - across it, a dim population: within the range;
   - with no valley, read in spreads: a population above the parent's middle
     may be brighter but not dimmer than its range, one below it dimmer but
     not brighter, one across the middle within its range.

   Where the reference gate is drawn on the marker and leaves a side open -
   drawn past every cell, or with nothing beyond its edge but dust (fewer
   than 1% of the events in the gate, and no more than 20) - that side sets
   no limit: the person who drew it meant "everything beyond here", so a
   cell brighter than any on the reference is still one of them.
4. **Decide whether the population was found.** All four must hold, or the
   gate is left where it is and the run says which failed:
   - at least 50 cells match;
   - they are at least a fifth as common, as a share of the parent, as the
     reference's population is of its own;
   - they form one cloud on the plot: the largest holds at least 80% of them;
   - on every marker their middle is still one of the population - and,
     for a dim one, within the reference population's own spread of the
     reference's middle.

   A gate left where it is counts as not placed, so when it has ruled gates
   under it the run pauses for it to be placed by hand.
5. **Place the gate**, edge by edge. Each edge keeps its place in the gap
   between the population and the cells beyond it, as a person would put
   it: on the reference, how far it sits from the near boundary of the
   cells beyond it (their 5% nearest the population) to the population's
   own boundary (its 5% nearest the edge); on the sample, the same share of
   the way across the same gap. The population is the cells that match, on
   the reference as on the sample, and the cells beyond are those that do
   not, within the gate's span on the plot's other axis. On an axis the rule
   reads, the cells within that span are split instead at the valley among
   them, where the reference and the sample both have one beside the edge:
   the line a cell is matched by is read off the whole parent, and on a
   marker most of the parent smears along - CD161 across T cells, say - it
   can fall in the middle of the population. Where no more than
   dust lies beyond the population, on either, the edge moves as far as the
   population's boundary did. It is the gap, not the matched cells' middle,
   which moves with how many there are and how bright, nor the parent's
   negative and valley, which know nothing of the cells either side of the
   edge. Carried so, an edge on a side the gate leaves open is never pulled
   in.

   A marker in `pinned` is placed by its negative instead, on the side of
   the gate nearest it: on the reference, how many of the negative's widths
   that edge sits from the peak of the whole parent's negative on the
   marker; on the sample, as many of the sample's widths from its own peak.
   The negative is read as for the line rules: its peak, and its width from
   the side below the peak, which no positives reach.

   Then, by `fit`:
   - `KeepShape` - each edge as above, so the gate may grow or shrink with
     the gaps, and everything between them kept in proportion: a polygon
     keeps its shape, stretched between its new edges. A gate whose area
     would change by more than 30% either way slides instead, its size kept,
     and the run says so. The gate stays the kind it was.
   - `MoveOnly` - the gate slides as far as its closed edges move on
     average, its size and shape unchanged, open sides and all.

   A gate that slides - `MoveOnly`, or `KeepShape` past the area limit -
   slides as far as its pinned edge moves, so that edge stays pinned.
   - `DrawPolygon` - trace a new polygon round the matched cells on a
     smoothed density, holding `keep` of them, with about `vertices` points.
     If its area is more than 30% from the one drawn the same way round the
     reference's cells, the shape is kept instead (as `KeepShape`), and the
     run says so.
6. The reference file must be one named, hand-gated sample
   (`measured_on: {"File": ...}`); an FMX would describe cells with no signal
   in the marker being matched.

## Settings

- `markers` - the markers that say what the population is. Empty means
  every marker on the panel: a reasonable start, rarely the finish - every
  marker must match, so one that is silent about the population only loses
  cells to its noise.
- `fit` - `KeepShape`, `MoveOnly` or `DrawPolygon` (above).
- `keep` - the fraction of matched cells the gate should hold (0.95 by
  default); the last few percent are the ones the signature is least sure
  about.
- `smoothing` - for `DrawPolygon`, the outline's smoothness: below 1 follows
  the cells closely, above 1 smoother. Ignored when the shape is kept.
- `vertices` - for `DrawPolygon`, about how many points the polygon has
  (24 by default). Ignored when the shape is kept.
- `pinned` - markers, of the two the gate is drawn on and among those the
  rule reads, whose edge nearest the negative is pinned to it (above).
  Empty by default: every edge keeps its place in the gap. Not used by
  `DrawPolygon`, unless its shape is kept.

## Pinned or in the gap

Nothing in the data says which a person meant - an edge put in a gap, in a
dip, or a fixed distance above the negative look alike on one sample - so
it is a setting, and the person who drew the gate decides.

- **Pin** an edge drawn against the negative: just above it, or cutting its
  top, on a marker where the positives smear and differ between samples -
  CD8 against CD4, say. In the gap, such an edge follows the smear: into the
  negatives where the positives are dim, away from them where bright.
- **Leave it in the gap** where the edge sits between two populations, or
  between the population and something other than the negative.

A run says when the reference edge on a marker it reads lies within the
negative's own spread - within 1.645 of its widths of the peak, inside its
95th percentile - as a hint to pin it there. It is a hint: an edge in a
real dip a width above a wide negative reads the same.

## Traps

- **Replays cannot try it**: runs keep only the gate's two axes, and this
  rule reads the whole panel.
- **Purity is about the two plot axes.** A gate that holds other cells may be
  right about the population but unable to separate it on this plot.
- **An axis the rule does not read moves with the cells.** A gate drawn on
  CD3 against CD56, matched on CD56 alone, slides on CD3 as far as the
  matched cells' CD3 middle moved. Add the marker to the rule to tie the
  gate's CD3 edges to the parent's CD3 negative instead.
- **A pinned edge moves with the negative alone**, so where a sample's
  negative is misread the edge follows the misreading. Where a sample's
  parent has no negative to read on the marker at all, the edge is carried
  in the gap instead.
- **A population much rarer than on the reference is left alone**, not
  gated: under a fifth as common, most of what matches is near misses. Pick
  a reference where the population is typical, not unusually large.

## What its confidence says

- *events matching the phenotype* - how many cells matched (the scarcer of
  matched and not).
- *how much else the gate holds* - the share of the gate that is the
  population.
- *how much of the population the gate holds*.
- *whether the matched cells form one cloud* - two clouds cannot be one
  outline.
- *how common the population is, against the reference* - scored on the
  ratio: three times more or less common halves the score.
- *whether the edges agree between halves of the events* - the edges are
  placed twice more, once from each half of the sample's events (alternate
  events), and of the cells either placement holds, the share both hold is
  the score. An edge on a flat stretch of density - a smear with no real
  dip - lands wherever the noise puts it, and the halves disagree; an edge
  that moves through empty space between them costs no cells, so does not
  count against it. Not scored for a traced polygon, which places no edges.
