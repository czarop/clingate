# Plan: a compensation and unmixing check

Status: first part built - `crates/clingate-core/src/spillover.rs`, the
Workspace tab's "Check compensation and unmixing", `check_spillover` for
Claude, and a test report section. Built: the pairwise lean with its
consistency across samples (1, 2), AF in stained samples by side scatter
(4), confirmed on the unstained files where the metadata marks any U or
Unstained, and the unmixed channel to look in (5, without raw detectors).
Not yet: the unstained's channels moving together (3), pictures per
finding, the nearest unstained for a donor without one, and the raw
unstained file. Questions still open are at the end.

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

## Settled

- **Unstained files are optional.** Not every donor has one. Where a donor
  has none, the AF test uses the nearest unstained - same plate, then same
  day - and says which it used; with none at all, it relies on the stained
  samples' high-SSC against low-SSC test alone, and says the verdict is weaker.
- **Unmixed files only, plus one optional raw unstained file per
  experiment.** The workspace's FCS files carry only unmixed fluors. The
  detector layout does not change between experiments, so the workspace can
  name one raw unstained file (detector channels, not in the gating) that the
  check reads to name the exact detector where AF is brightest, and the cells
  it is brightest in. Without it, the detector is suggested from the fluors'
  peaks.

## Still open

- Where the unmixing is done (SpectroFlo or Omiq), and whether AF is already
  extracted - one AF, or several: this decides whether the advice is "extract
  AF" or "add an AF spectrum for this cell type".
- The instrument configuration - a five-laser Aurora (UV, V, B, YG, R)?
