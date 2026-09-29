# From another gate

"In the same position as that gate", or "against that gate's edge".

## What it's for

Gates a gating guide places by another gate rather than by the data:

- the MAIT CD4-CD8+ gate "in the same position as the main CD4-CD8+ gate";
- CD154 on MAIT or TCRgd cells "according to the same position in the CD4
  Th cell gate";
- a TCRgd CD45RA/CD197 quadrant "according to the position of the same
  gates on the CD4-CD8+ population";
- the CD19- gate "aligned to the left edge of the CD19+CD14- gate", and the
  same for CD3- and CD56-;
- the MAIT CD4-CD8- gate "adjacent to the left edge of the CD4+CD8- gate and
  the bottom of the CD4-CD8+ gate";
- one gate "adjacent to" another on the same plot, like 81b beside 81a.

Nothing is read from this gate's population to decide where it goes. The
position is the anchor's, on the same sample.

## When not to use it

- When the gate's own population should decide. Copying a gate from CD4 T
  cells to MAIT cells assumes the two are stained and resolve alike; where
  they do not, a rule reading the MAIT population is the honest choice.
- For a linked gate - one gate drawn under two parents. That is already one
  position: set it from one place and the copies follow. This rule is for
  two different gates.

## How it works

1. **The anchor comes first.** A run places gates in levels (see
   `explain_gate_positioning`, section 2): this gate waits for every gate it
   follows that a rule places, and for every ruled gate above it. An anchor
   placed by hand, or not moved by any rule, is read as it stands. The order
   the rules are listed in plays no part.
2. **On each sample**, the anchor is read as it is on that sample - its
   per-specimen position when a rule or a person gave it one.
3. **Same shape** (`same_shape_as`): this gate takes the anchor's shape
   whole. Both must be drawn on the same two parameters - either way round,
   the anchor is turned to match. A quadrant takes another quadrant's lines
   (its centre and arms), keeping its own corners' names; a quadrant and a
   plain gate cannot copy each other.
4. **Edges** (`edges`): each edge of this gate is set to an edge of an
   anchor on one parameter, plus a gap. A rectangle's edge moves on its own -
   the other three stay put, which is what "against that edge" means. Any
   other shape slides whole until that edge is there. Edges are set in the
   order given; each may name its own anchor.
5. **What it holds** on the sample is counted and reported, before and
   after, as for the phenotype rule. A gate already where the anchor puts it
   is reported as already in place.

## Settings

- `same_shape_as` - the gate to copy, named as a rule names a gate:
  `{"gate": "CD4-CD8+", "parent": "CD3+CD14- / NOT CD3+CD56+"}`. Give this
  or `edges`, not both.
- `edges` - a list of edges to set. Each has:
  - `anchor` - the gate whose edge it takes, named the same way;
  - `parameter` - the marker both gates are drawn on (a marker or a
    channel);
  - `side` - which edge of this gate moves: `Lower` (left on x, bottom on
    y) or `Upper`;
  - `anchor_side` - which edge of the anchor it goes to: `Lower` or
    `Upper`;
  - `gap` - added to the anchor's edge, in the plot's units (default 0: the
    edges touch).

The rule's own `parameter`, `bound` and `measured_on` mean nothing here and
are ignored: a gate that follows another reads its own sample.

Examples:

- CD19- against CD19+CD14-: `edges: [{anchor: {gate: "CD19+CD14-",
  parent: "CD45+"}, parameter: "CD19", side: "Upper", anchor_side:
  "Lower"}]`.
- MAIT CD4-CD8-: two edges - `Upper` on CD4 at the `Lower` of CD4+CD8-,
  and `Upper` on CD8 at the `Lower` of CD4-CD8+ (both under the MAIT
  parent).

## Traps

- **A loop** - A follows B and B follows A, directly or through other
  gates - has no gate to place first. It is refused when written and left
  alone by a run.
- **An anchor that names two gates.** "CD4+CD8-" is drawn under both the T
  cells and the MAIT cells: name the parent. A linked gate is one gate, and
  resolves.
- **An anchor with an open edge** on the parameter (a gate running to the
  end of the axis) has no edge to set against; that sample is refused.
- **Setting a rectangle's edge past its other edge** is refused rather than
  turning the rectangle inside out.
- **A replay** does not re-place a gate that follows another: its position
  is the anchor's, so replay the anchor's rule.

## Confidence

Nothing is estimated, so the score is 1 - "copied" - on every sample it
places. Doubt about the position belongs to the anchor: review the anchor.
