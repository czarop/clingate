# Plan: a compensation and unmixing check

Status: planned, not built. Questions to settle first are at the end.

## What it is for

A gate that sits wrong on some samples is often not the rule's fault: the
data under it is distorted. Two distortions look alike on a plot - a
population leaning diagonally, a negative smeared upward - and are fixed in
different places:

- **Spillover (compensation) error.** Too much or too little of fluorochrome
  A is taken out of channel B. Fixed in the compensation matrix (clingate's
  compensation panel, then back to Omiq).
- **Unextracted autofluorescence (AF).** In spectral data, cells' own
  fluorescence is a spectrum of its own; if it is not unmixed out as a
  separate "fluorochrome", it lands in whichever fluors' spectra resemble it.
  Fixed in the unmixing (SpectroFlo or Omiq), by extracting AF - not by
  editing a compensation matrix, which can only move the error around.

The check reads the data, says which of the two (or neither) each problem
looks like, with the numbers behind it and a picture, and says what to do.

## How the two are told apart

They leave different fingerprints:

| | Spillover error | Unextracted AF |
|---|---|---|
| Depends on | how bright marker A is | how autofluorescent the cell is |
| In the unstained sample | absent - nothing to spill | **present**: unstained cells sit off zero in the affected channels |
| Across cell types | the same lean in every population positive for A | strongest in large, granular cells (high SSC: monocytes, granulocytes), weak in lymphocytes |
| Shape | B rises linearly with A among A+ cells | B is shifted, or spread, in A- cells too; several channels move together |
| Across samples | the same lean on every sample compensated with the same matrix | varies with the donor's AF, and with cell composition |

A third pattern is worth naming, because it is neither: a lean that is
linear like spillover but **varies from sample to sample** points at tandem
dye breakdown (PE-Cy5, APC-Fire 810, ...) in particular tubes - fixed by
neither the matrix nor AF extraction, and worth knowing before trusting a
rule on those samples.

## The measurements

For a workspace, a population to look in (default: the live single cells),
and the channels of the panel:

1. **Pairwise lean (spillover).** For each pair (A, B) with enough cells
   positive for A and negative for B (by the existing negative finder): the
   robust slope of B against A among them, against the slope among A-
   cells. A slope well away from zero, in units of B's negative spread per
   unit of A, is a lean. Its size gives the matrix change that would remove
   it - the same arithmetic the compensation panel already does.
2. **Consistency.** The lean per sample, and its spread across samples
   compensated with the same matrix. Consistent: the matrix. Varying: the
   samples (tandems).
3. **Unstained offsets (AF).** In each donor's unstained (U) file: each
   channel's median and robust spread against zero, and how the channels'
   deviations move together - the first principal component of the
   unstained cells across channels is the AF residual's shape.
4. **AF in stained samples.** Among cells negative for everything, B's
   median in high-SSC cells against low-SSC cells. AF residual shows as a
   gap that the unstained sample also shows; spillover does not.
5. **Where to look for AF in the unstained.** The detector (or, without raw
   detectors, the unmixed channel) where the unstained cells are brightest
   against their spread - read from the data, not assumed - with the cell
   population it is brightest in. On a five-laser Aurora, cellular AF
   usually peaks in the violet and blue detectors (V3 to V7, B1 to B3), but
   the check reports what this data shows. That is the channel to set the AF
   gate on when extracting AF in the unstained, and the population to draw
   it on (e.g. "the high-SSC cells in V7").

## What it gives

- A table per channel pair or channel: verdict (spillover, AF residual,
  sample-specific, fine), the numbers above, and what to do: "raise
  S[BV421 → BV510] by 0.8%" or "extract AF: in the unstained, gate the
  high-SSC cells and use V7".
- A picture per finding: A against B on a typical sample with the lean
  drawn, and the unstained beside it - Claude reads these as you do, so it
  can say when a verdict looks wrong.
- In the app: a "Check compensation and unmixing" panel on the Workspace
  tab, findings listed worst first, each opening the plot. For Claude:
  `check_spillover`, read-only.
- A section in the test report.

## Questions to settle before building

1. **Are the unstained (U) files available to clingate?** Your metadata has
   a U file per donor and day, but the workspace I saw holds only FS and FMX
   (71 files). The AF test needs U.
2. **Do your exported FCS files carry the raw detector channels** (V1-V16,
   B1-B14, ...), or only the unmixed fluors? Without raw detectors the check
   can still say *that* AF is unextracted and which unmixed channels it
   lands in, but can only suggest a detector from the fluors' peaks.
3. **Where is the unmixing done** - SpectroFlo, or Omiq - and was AF
   extracted (one AF, or several)? That decides whether the advice is
   "extract AF" or "add an AF spectrum for this cell type".
4. **Instrument configuration** - five-laser Aurora (UV, V, B, YG, R)?
