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

1. **Describe the reference population.** The cells inside the reference
   gate, on the chosen markers, each expressed as a robust z against the
   reference's own parent population: how many spreads from the parent's
   middle, with the median and MAD taken after dropping values more than 4
   spreads out (so the population itself does not set the scale). Their
   centre and covariance are the signature.
2. **The cut.** The distance (Mahalanobis, allowing for markers that move
   together) that holds 95% of the reference's own cells - or what the
   statistics imply for that many markers, whichever is larger.
3. **Find them in each sample.** Each event of the sample's parent, in z
   against that sample's own parent, is matched if it is within the cut.
   Nothing is rescaled between samples: each is its own frame.
4. **Fit the gate** to the matched cells on the plot's two axes:
   - `fit: KeepShape` - move and resize the drawn shape onto them (stretched
     at most 4 times either way; clamped beyond, and said). The gate stays
     the kind it was.
   - `fit: DrawPolygon` - trace a new polygon round them on a smoothed
     density, holding `keep` of them, with about `vertices` points.
5. The reference file must be one named, hand-gated sample
   (`measured_on: {"File": ...}`); an FMX would describe cells with no signal
   in the marker being matched.

## Settings

- `markers` - the markers that say what the population is. Empty means
  every marker on the panel: a reasonable start, rarely the finish - a
  marker that is silent about the population adds noise to the distance.
- `fit` - `KeepShape` or `DrawPolygon` (above).
- `keep` - the fraction of matched cells the gate should hold (0.95 by
  default); the last few percent are the ones the signature is least sure
  about.
- `smoothing` - for `DrawPolygon`, the outline's smoothness: below 1 follows
  the cells closely, above 1 smoother. Ignored when the shape is kept.
- `vertices` - for `DrawPolygon`, about how many points the polygon has
  (24 by default). Ignored when the shape is kept.

## Traps

- **Replays cannot try it**: runs keep only the gate's two axes, and this
  rule reads the whole panel.
- **Purity is about the two plot axes.** A gate that holds other cells may be
  right about the population but unable to separate it on this plot.

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
