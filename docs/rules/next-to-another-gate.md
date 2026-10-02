# Next to another gate

"Up against that gate": as close to it as it can be without overlapping it.

## What it's for

Two gates side by side on one plot where only one is set from the data:

- CD19- beside CD19+CD14-: a rule places CD19+CD14-, and CD19- is grown up
  to its left side, wherever that ends up on each sample;
- a negative gate brought up to a positive one above it.

The other gate is placed first - by its own rule, or where it is drawn -
and this one is brought up to it on each sample. It works for any shapes:
the two meet where they first touch, on the plot as drawn.

## When not to use it

- For a gate whose edge should sit at the other's edge as drawn, on
  rectangles: "from another gate" with an edge says that exactly.
- When the gap between the two populations is itself what decides the
  edge: a rule reading this gate's own population is the honest choice.
- For ellipses and quadrants: it brings up rectangles and polygons only.

## How it works

1. The other gate is found on the same sample, after any rule placing it.
2. Along `parameter`, the gate moves towards it from the `side` it is on,
   until the two touch - less `gap`:
   - `GrowSide` (the default): the gate's side facing the other - the points
     past its middle - moves, every point alike; the side away stays where
     it is. A gate that already overlaps is shrunk back. A rectangle stays a
     rectangle.
   - `FollowOutline`: where the gate lies alongside the other, its facing
     side takes the other's outline, so there is no gap anywhere along it,
     with steps where that stretch begins and ends; the rest of the gate is
     as it was. Only the other's side facing it is followed, not its top or
     bottom - an edge running more along the axis than across - so the gate
     never wraps round it. Polygons only - a rectangle grows its side.
   - `Slide`: the whole gate slides, its shape as it is.
3. A gate that never comes level with the other along that axis - the other
   lies wholly above it, say, for a gate moving across - is left where it
   is, and the run says why.
4. Like every rule, it is never placed over a third gate on the plot: if it
   would be, it is left where it was and the run says why.

## Settings

- `anchor` - the other gate, named as a rule names a gate:
  `{"gate": "CD19+CD14-", "parent": "CD45+"}`. It must be on the same plot:
  under the same parent, on the same two parameters.
- `parameter` - which of the gate's two parameters it moves along.
- `side` - which side of the other gate it sits on: `Lower` is to its left,
  or below it; `Upper` to its right, or above it.
- `meet` - `GrowSide`, `FollowOutline` or `Slide`, as above.
- `gap` - left between the two, in the plot's units; 0 is touching.

The rule's own `parameter`, `bound` and `measured_on` are not read: the
position is the other gate's, on the gate's own sample.

## What its confidence says

- *copied*, at 1: nothing is estimated - the review of the other gate is
  where doubt about the position belongs.
