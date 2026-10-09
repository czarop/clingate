# Choosing a rule for a gate

A rule decides where one gate goes on every sample, from the gate a person
drew on one sample. It moves one edge of the gate - the side it keeps events
from - along one marker, or (the phenotype rule) finds the cells themselves.
Each rule has its own guide; this one is about choosing between them.

Claude reads the samples' data for any of this only when the user asks it to.
Otherwise it asks the user what the plots look like, and offers to look.

## First: what does the plot look like?

Look at the gate's parent population on the rule's marker, on the samples the
rule will read (the FMX, the QC, the sample itself), across the dataset:

| what you see | rule to try first |
|--------------|-------------------|
| A negative and a clearly separate positive, with a dip between them - on every sample, or only on some, the rest smears | **Valley or smear** - reads the dip where there is one, and cuts a smear as on one gated by hand |
| A negative with positives smearing out of it, no dip, and a matching FMX for every sample | **Tail fraction** on the FMX - "0.2% to 0.5% of the FMX above the line" |
| A negative with positives smearing out of it, no dip, no FMX - but a hand-gated QC or template | **Valley or smear**, the QC its reference - "as far above the negative as on the QC" |
| A negative only (FMX, unstimulated), and you want the line a fixed step above its top | **Percentile offset** |
| A population that does not separate on either axis alone, several clusters close together, or one that moves in both axes | **Match the phenotype** |
| A gate the guide places by another gate: "the same position as", "aligned to the edge of" | **From another gate** - settle the other gate first |
| A gate "adjacent to" another on the same plot, of any shape: up against it, touching but not over it | **Next to another gate** - settle the other gate first |

Then check the harder questions:

- **Does the negative change shape between samples?** Shifts in its median
  are fine for every rule. Changes in its width or skew are not: above the
  negative paces out in widths of the left side and assumes the right side
  mirrors it; its right-side check flags when that stops holding.
- **Is there always a dip?** Valley or smear decides per sample: a sample
  with no dip is cut as on a smear gated by hand. In the valley, written
  before it, refuses such a sample.
- **Is the FMX good?** An FMX rule is only as good as the FMX. A poorly
  stained or low-count FMX gives a noisy tail; the band rule's confidence
  says how many events decided it.
- **How rare is the positive?** A band of 0.2% of 2,000 events is four
  cells. Check the parent's event count before trusting any tail rule.

## Which file the rule reads

`measured_on` says which file each sample's placement is read from:

- `{"Partner": "FMX"}` - each specimen's own FMX (or any sample type the
  pairing names). The line is set on the FMX and the same position applies
  to the specimen's full stain. The usual choice for band rules.
- `{"File": "<id>"}` - one named file for every specimen: a QC or template
  sample gated by hand. That file's gate is never moved; every other
  sample is calibrated against it. Required for valley or smear and match
  the phenotype; the usual choice for above the negative and in the valley.
- `"Itself"` - the sample being gated. For a band or percentile rule this is
  "that fraction of this sample's own population"; for the calibrated rules
  (above the negative, in the valley) it calibrates on the sample's own
  current gate, which does nothing unless a scale or nudge is set.

The sample types come from the pairing on the Gate Rules tab: typically FS
(full stain), FMX (fluorescence minus the marker) and QC. Unstained samples
("U"), where present, are not read by any rule.

The file actually gated is the specimen's file whose type comes last in the
pairing's display order - normally the full stain. The placement is written
for the whole specimen, so its FMX shows the same gate.

## Every rule, in a line

- **Tail fraction** - slide the gate until it holds a percentage of the
  file it reads, within a band.
- **Percentile offset** - put the line a fixed step above (or below) a
  percentile of the file it reads.
- **Valley or smear** - in the dip where a sample has one, as far from its
  bottom as on the reference; on a smear, as many negative-widths above the
  negative as on a smear gated by hand. The Gate Rules tab offers it in
  place of the two below.
- **Above the negative** - put the line as many negative-widths above each
  sample's negative as it sits on the reference. Rules written as this still
  run.
- **In the valley** - put the line in the dip between negative and
  positive, as far from its bottom as on the reference. Rules written as
  this still run.
- **Match the phenotype** - find the cells that look like the reference
  gate's across chosen markers, and fit the gate to them.
- **From another gate** - take another gate's shape, or set an edge against
  another gate's edge, on the same sample, once that gate is placed.
- **Next to another gate** - bring the gate up against another on the same
  plot, growing its side, following the other's outline, or sliding whole.

## Settings every rule shares

- `parameter` - the marker the rule positions along. Never an axis name: the
  same marker can be on either axis of a plot.
- `bound` - `Above` keeps events above the line (a positive gate), `Below`
  keeps those below (a negative gate). A `Below` band or percentile rule is
  solved on the mirror image, so "0.5% below" works as you would expect.
- `confidence.limits` (optional, per rule) - when the confidence score
  starts to complain: `events_full` (10,000: parent events at which count is
  no worry), `events_floor` (100: below this a placement is worth nothing),
  `swing_half` (1: how unstable a nudge may make the contents before the
  stability score halves). These were set by judgement, and are worth
  tuning against reviewed runs. A `displacement_limit` in an older rules
  file is ignored: how far a gate moved is not held against it.

## How to check a choice

A choice is a guess until it has been tried. Try the candidates on the
current files without applying them (`try_rules`), compare with the
hand-gated reference and with what should be consistent (QC samples across
plates), and - once runs have been reviewed - replay them (`replay_rules`)
to see what each fixes and breaks. On a workspace gated by hand, score the
rules against it (`score_rules`): each gate under its parent as drawn, and
the events the rule's gate shares with the hand-drawn one - how much of
yours it catches, how much it holds beyond it, and their agreement, with a
sample of few cells allowed further below the line that calls it off. A
sample the rule cannot place scores as a gate holding nothing: it would be
gated by hand.

To find the settings that come closest, search them (`fit_rule`): the rule
as it stands, any candidates given, and by default every combination of a
few values of each setting that matters for its kind - smoothing, smallest
dip and lowest-point-before for a valley; the band's width and aim; the
distance above the negative and how the negative is found; the percentile
and offset; a phenotype's fit and share kept. What a rule reads - its
parameter, markers, reference and fallback - is left as it is. Each
candidate is scored as `score_rules` scores it. With eight specimens or
more they are dealt alternately into two halves, ranked on one and checked
on the other, so a setting tuned to a couple of samples shows as one that
falls down the ranking on the half it was not chosen on. The ranking is by
typical agreement, a tie (within `tie_within`, 0.02) going to the fewest
samples off, or by the fewest samples off first; each candidate says where
it stands both ways, and the best and those tied with it are marked, to be
looked at side by side before choosing: they and the rule as it stands are
kept in the workspace (`rules/searches.json`, the latest search of each
rule), and the Gallery tab, on the gate's population, draws each one's gate
dashed over the gate as drawn, one candidate at a time, with a button to
take its rule.

To pick the kind of rule as well as its settings, pick it (`pick_rule`, or
"Pick the best rule for every gate" on the Gate Rules tab). The kinds are
tried in order of preference, and the first that passes is taken:

1. A band read on each specimen's FMX, at the range the user accepts -
   tried as it is, its settings never searched.
2. Above the negative.
3. The valley, or a smear.
4. A band read on each sample, around what the hand gates hold there.
5. The phenotype on the plot's two axes - the last resort.

The negative, the valley and the phenotype are calibrated, as the Gate
Rules tab makes them, on one sample gated by hand: the one the rule as it
stands is calibrated on, or else the specimen whose hand gate holds the
middle share of its parent. That sample is not scored. Read on each sample
itself they would only find each sample's own hand gate. A valley or smear
rule that meets a smear with no example takes the first smear, in order,
as its example, as a run would ask for one.

A rule passes when no more than a share of its samples - a tenth, unless
set - agree less than an agreement - 0.95, unless set - with the gate drawn
by hand; a sample of few events may fall a counting-noise width below it.
The band, the agreement and the share are asked for at the start of a pick
on the Gate Rules tab and kept with the workspace
(`rules/pick_settings.json`); the tools for Claude use them unless told
otherwise, and `pick_settings` shows them. A gate whose rule reads no
partner, in a workspace whose pairing shows one sample type, has no FMX to
read the band on: the pick says so and goes on without it.

Every kind but the phenotype is first tried as the hand gating starts it,
all together, and the first in order that passes has its settings searched; the best of its
settings that also pass is the pick. When none passes as started, each
kind's settings are searched in turn, in order, until one passes. When none
does, the closest of everything tried is shown, flagged: that gate may be
better gated by hand. Each gate's pick is listed beside the rule as it
stands with how each kind did, the flagged gates first. The files are read
once, and again only for the phenotype, for the gates that come to it.
Nothing changes until a pick is taken - Use, or Use the best for every gate
whose rule it differs from - and saved. A gate whose rule is a phenotype
is tried only as a phenotype, its settings searched; a gate placed from
another gate is left as it is.
