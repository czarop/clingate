# flow crates: review for clingate

A review of `flow_fcs`, `flow_gates` and `flow_plots` (`czarop/flow`, branch
`claude/fcs-errors-not-panics`) limited to the parts clingate uses, with
clingate's needs in mind. Each finding says where it is, what it costs
clingate, and what to change. Nothing here has been changed yet.

What clingate uses:

- **flow_fcs** - `Fcs::open` (every plot, gallery image and rules run),
  `apply_arcsinh_transforms`, `Header`/`Metadata::from_mmap` and the keyword
  getters (the workspace), `Parameter`/`ParameterMap`, `TransformType`
  (`Linear` and `Arcsinh`; `Biexponential` is never produced).
- **flow_gates** - `Gate`, `GateGeometry`, `GateNode`, the
  `create_*_geometry` constructors, `EventIndex` (percentages, autogate),
  `polygon::point_in_polygon` (the filter), and
  `transforms::{get_plotting_area, pixel_to_raw(_y), raw_to_pixel(_y)}` (the
  plot mapper).
- **flow_plots** - `DensityPlot::render` with `PlotType::Density`, built
  without the `raster` feature, so the plotters backend: every plot and every
  gallery/PDF image.

## Correctness

### 1. The density plot shows an arbitrary bin's colour on about a third of its pixels (flow_plots)

`density_calc::calculate_density_per_pixel_cpu` bins events on a grid the
size of the *whole image* (400 x 400, margins and label areas included) over
the axis range. `render_pixels` then draws those bins into the *plotting
area*, about 330 x 340. Several bins land on one screen pixel, and the last
one written wins, in `FxHashMap` iteration order, which is arbitrary. So a
dense pixel can show a sparse neighbour's colour, and the speckle changes
from run to run of the same data.

Also in the same function:

- Events outside the axis range are clamped into the edge bins, and those
  piles count towards the colour scale's maximum, so a large off-scale pile
  can wash out the real populations.
- A bin is mapped back to data at its lower-left corner, not its centre - a
  half-bin shift against the gate overlay, which is drawn at exact
  coordinates.

**Change:** bin directly at the plotting area's size (see 9 for where that
size should come from), skip events outside the range instead of clamping
them (or keep an edge pile but leave it out of the maximum), use bin centres,
and write the dense array straight into the image with no hash map in
between. The last also removes an allocation and makes the output
deterministic.

### 2. A keyword value containing the delimiter is cut short (flow_fcs)

`Metadata::from_mmap` splits the TEXT segment at every delimiter byte. The
FCS standard escapes a delimiter inside a keyword or value by doubling it
(`run//2024` for `run/2024` when the delimiter is `/`). Tried on a real file:
`$FIL/run//2024.fcs/` reads as `run`. Because an escape always adds two
splits, the keywords after it stay aligned - only the value is lost.

clingate reads `$PnN` (channel names) and `$PnS` (marker labels) through
this. A marker label with the delimiter in it - `CD45RA/RO` in a file whose
delimiter is `/` - would come back truncated, and the axis would be named,
and matched to the scaling, wrongly.

**Change:** tokenise properly - walk the delimiter positions and treat a
pair of adjacent delimiters as one literal delimiter inside the current
token. Two tests: an escaped value, and one ending in an escape.

### 3. `Fcs::get_guid` always fails (flow_fcs)

It looks up `"GUID"`, but every keyword is stored with its `$`, so it never
finds one - the same mistake `validate_guid` made (B-FCS-1). clingate's
`FcsSampleStub` has its own `get_guid` that looks up `$GUID`, so this does
not reach clingate today, but anything that uses `Fcs::get_guid` gets an
error for every file. `set_guid` works (the insert adds the `$`).

**Change:** look up `$GUID`.

### 4. The pixel-to-data functions panic (flow_gates)

`transforms::pixel_to_raw` and `pixel_to_raw_y` call `panic!` on a
non-finite pixel, a non-finite range, or an empty pixel range. These run on
the UI thread - the plot mapper calls them for every mouse move and drag.
`get_plotting_area` returns an empty range for any plot narrower or shorter
than 70 px (60 px of label area and margin on one side, 10 on the other). No
clingate plot is that small today (400 px in the editor, 512 in the export),
so this is latent - but a resized window, a zero size during layout or a
thumbnail view would take the app down rather than a worker thread.

**Change:** never panic here. For clingate, the simplest is item 7 below:
replace these with a small affine mapping that clingate owns.

## Performance

### 5. Diagnostic output on every call, in release builds (all three)

Unconditional `eprintln!`/`println!` in hot paths:

- `pixel_to_raw`/`pixel_to_raw_y`: four or more lines per call - every mouse
  move over a plot, every drag step.
- `DensityPlot::render` and the density calculation: about ten timing lines
  per plot rendered.
- `Metadata::validate_text_segment_keywords`: `Validating FCS file...` on
  every open; `Unable to parse keyword` for each keyword it does not model.
- `TransformType::inverse_transform` (Arcsinh): two lines per call in debug
  builds; `validate_coordinate_transformation` prints one line per point.

On a terminal that is noise; on Windows, where stderr is slow, the
per-mouse-move lines are measurable lag.

**Change:** every one of these through `tracing` (already a dependency of
flow_fcs) at `debug`/`trace`, so they cost nothing unless asked for.

### 6. Every view re-reads and re-transforms the whole file (clingate)

`plot_window` opens the FCS file with `Fcs::open` whenever it loads -
parsing every event - and applies the arcsinh transforms to every scaled
channel. Switching back to a sample does it again; the gallery does it once
per image; a rules run does it once per file per run. On a spectral panel
(30-60 channels, 1-5 million events) that is seconds of work and hundreds of
MB per view.

**Change (in clingate):** one shared cache of transformed frames, keyed by
path, file modification time and the cofactors, evicting least-recently-used
beyond a memory budget. The plot, the gallery and the rules run all read
from it. This is the largest single speed-up available.

### 7. `Fcs::open` holds every event twice while parsing (flow_fcs)

`store_raw_data_as_dataframe` decodes the whole DATA segment into one
`Vec<f32>` (events x parameters) and then copies each parameter out with
`skip(i).step_by(p)` into its column. Peak memory is twice the data. The
"zero-copy" fast path is not: `bytemuck::try_cast_slice` is followed by
`.to_vec()`, and it falls back to a chunked copy whenever the data does not
start on a 4-byte boundary, which is most files.

**Change:** allocate the columns first and decode each event's values
straight into them - one pass over the bytes, one copy of the data.

### 8. Every gate vertex is a hash map (flow_gates)

`GateNode` keeps its coordinates in a `HashMap<Arc<str>, f32>` with the
default SipHash, and the constructors allocate a new `Arc<str>` of the
channel name for every coordinate of every vertex. clingate's gates are
always 2D and their channels are on `Gate::parameters`, so each
`get_coordinate` hashes a string to find one of two numbers. That runs in
every hit test, every redraw, every filter's preparation and every clone -
and a phenotype polygon can have hundreds of vertices.

**Change, if you are customising freely:** store `(x, y)` on the node and
keep the channel names on the gate. It touches every gate type in both
crates, so it is the largest change here. **A cheaper step first:**
`FxHashMap`, and share the two channel `Arc`s across a geometry's nodes
instead of allocating per node.

Also: `GateNode`'s doc says its coordinates are in raw data space; clingate
stores them in display (transformed) space. Worth correcting in the doc,
since the name `raw_to_pixel` and friends suggest the same.

### 9. The plot image is rebuilt from scratch, and JPEG-encoded, on every render (flow_plots)

- Axes, ticks and labels are drawn by plotters, rasterising text, on every
  render, though they only change with the axis settings. Caching the frame
  per (size, ranges, labels, transforms) and drawing only the density on top
  would remove most of a render's fixed cost.
- Progress chunks: every 1,000 pixels, up to 1,000 pixel structs are copied
  into a new `Vec` and passed to `report_progress` - even when no callback
  is set, which is clingate's case. Build the chunk only if there is a
  callback. (The last partial chunk also re-sends pixels from the chunk
  before it.)
- The image is JPEG at quality 85. A density plot is isolated coloured
  pixels on white - the worst case for JPEG, which smears a single event's
  colour into its neighbours. PNG is lossless, typically no larger for
  sparse plots, and the gallery's PDF would get exact colours. clingate then
  base64-encodes it into a data URL (+33%).

### 10. The gate overlay and the image agree only by convention (flow_gates / flow_plots)

`flow_gates::transforms` hard-codes `PLOT_MARGIN = 10` and label areas of
50 - "matching plotting/mod.rs" - while `flow_plots` takes them from
`BasePlotOptions` (default 10 and 50) and plotters computes the plotting
area itself. clingate's gate overlay uses the first; the image uses the
second. They agree only while nobody changes the options.

**Change:** have the render return the plotting area it actually used
(plotters' `get_pixel_range`) and build the mapper from that.

## Smaller things

11. **`TransformType::Biexponential` is not logicle.** The doc says it
    matches FlowJo's logicle; the formula is a scaled arcsinh and `width` is
    ignored. clingate never produces it, but carries five `todo!()` arms for
    it. In your fork, either remove the variant (the compiler then lists
    every place to tidy) or implement real logicle if you will need Omiq's
    logicle scalings - clingate currently skips an unsupported scaling type
    with a `println`, so such a channel silently has no axis; that should be
    a reported problem, like the others `read_axis_configs` collects.
12. **Slanted quadrant arms in linear space lose precision.** A skewed
    quadrant's arms run out to +-1e8 in linear space, where an `f32` has a
    resolution of about 8 units, so `point_in_polygon` can misplace events
    within a few units of a slanted arm. Axis-aligned arms and arcsinh axes
    are unaffected. Do the ray-cast arithmetic in `f64`, or project the arms
    to a bound nearer the data.
13. **`TransformType`'s `Hash` and `PartialEq` disagree** for `-0.0`/`0.0`
    (equal, different bits) and NaN. Harmless unless used as a hash-map key
    with such values.
14. **Counting without collecting.** `EventIndex::filter_by_gate` returns a
    `Vec<usize>` of every admitted event; the percentages only need the
    count. A `count_in_gate` would save an allocation per gate per render.
15. **Duplicate code in clingate:** `FcsSampleStub::generate_parameter_map`
    is a copy of `Fcs::generate_parameter_map` (only an error message
    differs). Call flow's.
16. **Compensation.** clingate never applies `$SPILLOVER`
    (`Fcs::apply_file_compensation` exists and is unused). That is right for
    files already unmixed - consistent with percentages matching Omiq - but
    a conventional-cytometer file with a spillover matrix that Omiq
    compensates would show uncompensated events here, and gates would not
    line up. Worth deciding explicitly, and saying so in the UI if such a
    file is opened.

## Suggested order

1. Logging to `tracing` (5) - small, mechanical, immediate.
2. The density plot's binning (1) - what every plot shows.
3. Keyword escapes and `get_guid` (2, 3) - small, correctness.
4. Frame cache in clingate (6) - the largest speed-up.
5. The plotting area from the render (10) with the mapper replacement (4).
6. Single-copy FCS parsing (7), render caching and PNG (9).
7. The 2D `GateNode` (8), when you next touch the gate types.
