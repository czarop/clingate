# Handover

Context a fresh session needs before touching this repository. `TODO.md` is the
task list; this is the background that makes those tasks make sense.

## What the project is

clingate is a Rust/Dioxus desktop gate editor for flow cytometry FCS files. It
imports gating data exported from **Omiq** - gate positions, sample metadata,
axis scaling - alongside local copies of the FCS files.

The goal is **autogating**: gate one QC sample, then position gates on every
other sample per defined rulesets, and export the resulting positions back into
an Omiq-compliant gating file. **Autogating is not started.** Everything so far
is the import/export/editing substrate it needs.

## State

- Branch `claude/funny-bardeen-bleqcn`, 25 commits ahead of `273645d`.
- 492 unit tests, 12 doctests. `cargo check` clean with the desktop feature.
- Import and export round-trip a real 658 KB Omiq file: 250 nodes, 299
  containers, unchanged.

## Building and testing

    cargo test --lib --no-default-features   # no system packages needed
    cargo test                               # needs the GTK packages below

Every test lives in the library, so `--no-default-features` runs them all: it
drops dioxus's `desktop` feature, which pulls in `gdk-sys` and probes
pkg-config for `gdk-3.0`. Without those packages a plain `cargo test` fails at
that probe before running anything.

    libgtk-3-dev libwebkit2gtk-4.1-dev libxdo-dev
    libayatana-appindicator3-dev librsvg2-dev

`cargo check` (no flags) is worth running too: it is the only thing that
compiles `main.rs` and the desktop-only code paths.

### The real-file tests

Three tests are gated on an environment variable and silently skip when it is
unset:

    OMIQ_GATING_FILE=/path/to/export.omiqgt cargo test --lib --no-default-features a_real_

They are the highest-value tests in the suite - they run against what Omiq
actually writes rather than against fixtures built from our own understanding
of the format. Ask the user for a gating file if you do not have one.

### Handling the user's Omiq files - important

Real gating files contain a workflow URL, dataset/task/workflow ids, donor
metadata column names and sample names. **Never commit one.** The fixtures in
`tests/fixtures/` are anonymised derivatives (url -> `example.invalid`,
workflowId -> 1, datasetId -> 2, taskId -> 1, date -> 2020-01-01, sample ids ->
`sample1`/`sample2`). Any new fixture must be anonymised the same way. Real
files are used only through `OMIQ_GATING_FILE`.

## The Omiq gating format, as established from real files

These were derived by reading actual exports; they are not in any spec we have.

- A gating file has `tree.nodes` (`GatingNode`) and `tree.filterContainers`
  (Atomic or Compound). **The node is the tree position; the container is the
  gate.** A node has `id`, `parentId`, `filterContainerId`, `ord`, `collapsed`
  and **no name** - the name lives on the container.
- **Linked gates**: one container can be referenced by several nodes, which is
  how Omiq applies one gate at several points in the tree. A real export had
  250 nodes over 146 containers; 38 containers were linked, one at nine points.
- **Composites** (quadrant, skewed quadrant, bisector) are *not* a container.
  Each corner is its own container, tied to its siblings by a `groupId` with a
  positional suffix: `{group}_QUAD0..3`, `{group}_SPLIT0..1`,
  `{group}_SKEWEDQUAD0..3`. The group id itself is not a container.
- Omiq links composites too, as a group: in a real export a skewed quadrant had
  all four corners placed under the same three parents, each corner keeping its
  own `ord`.
- Composites are all-or-nothing to the user: they cannot delete one corner.
- **Ghost containers**: containers with no node. Omiq leaves these behind when
  a gate is deleted while a boolean still references it, and the boolean goes on
  working. Confirmed by a controlled experiment the user ran: building a boolean
  off a quadrant and then deleting the quadrant leaves the boolean alive and its
  operand container present but nodeless. We keep them verbatim and write them
  back untouched.

  Two different things share the name. Containers that were *already* nodeless
  in the file Omiq wrote live in `omiq_rebuild.ghost_containers` as raw JSON and
  are never touched - we cannot assume Omiq does not need them, so a round trip
  returns the document it was given. A gate that *this session* leaves nodeless
  is a registered gate, and it is kept only while something reaches it:
  `collect_stranded_ghosts` runs after every delete and drops the rest.
- `1e16` is the sentinel for an unbounded edge. This is unambiguous within the
  files seen - 1e16 on an arcsinh axis whose real coordinates span about +/-4
  cannot be a measurement - but one dataset cannot prove Omiq always uses it.
  clingate's own convention is different (`get_infinite_bounds`: 1e8 linear,
  `asinh(1e8/cofactor)+5` which is about 15.41 on an arcsinh axis), so the
  exporter snaps unbounded edges back to `1e16`.
- Coordinates are **per axis**: fluorescence channels are arcsinh, scatter and
  Time are linear. A gate can be arcsinh on one axis and linear on the other.

## Architecture decisions made this session

### Gate identity is split from tree placement

This is the big one, and most of the codebase now depends on it.

    GateId  - the gate: geometry, name, per-file positions.   Omiq's container.
    NodeId  - one appearance of that gate in the tree.        Omiq's node.

`GateState` holds `placements: FxHashMap<NodeId, GatePlacement>` (which gate a
node shows) and `nodes_by_gate` (the reverse index). Both are maintained through
exactly two private methods, `record_placement` and `forget_placement`, called
from every point that writes the hierarchy - so the tree and the table cannot
disagree about what exists.

`is_linked`, `placement_count` and `is_ghost` are **derived** from the table,
never stored, so there is no second source of truth.

`GateHierarchy` keeps its `Arc<str>` signatures. It is a general tree over
opaque ids - a deliberate fork of a reusable flow-gates structure - and the
gate/node distinction is domain meaning that belongs in the store. The ids it
holds are node ids.

Consequences worth knowing:
- The **view index** `gate_ids_by_view` is keyed by the *parent node*, because a
  plot is a tree position: the same gate at two points sits on two different
  populations.
- **Gating chains are node-scoped** (`gate_chain_for_node`). Taking a chain from
  a gate alone was wrong for a linked gate and gave it the statistics of
  whichever placement survived the import.
- The **exporter writes one node per placement**, read from the tree. It used to
  read a snapshot captured at import, which could not carry an edit.
- `as_parent_node` bridges callers that still pass a bare gate id, resolving to
  its first placement. Ambiguous for a linked gate by construction; the UI
  passes nodes.

### Decisions the user made

- Linked gates **share one name**, matching Omiq, which stores the name on the
  container. Per-placement names are not representable in a gating file.
- Linking **discards** the source position's own geometry. The discarded gate is
  kept as a ghost rather than deleted, since a boolean may reference it.
- Linking across different parameter pairs is **refused**.
- A new gate is assumed to be acceptable to Omiq under any unique UUID. Whether
  Omiq really accepts a clingate-written file is untested - see `TODO.md`.

## Traps

### Lost Dioxus subscriptions - three bugs this session

The store refactor moved plain-data logic off the Dioxus lenses so it could be
tested without a runtime. Wherever a Store wrapper or a memo then reads through
`peek`, it **subscribes to nothing**, and anything built on it silently stops
updating. This has bitten three times:

1. The axis selectors showed the wrong channel because the index memo peeked the
   axis store and never recomputed after the scaling export loaded.
2. `get_current_sample` peeked, so the gate resolver was built once. Dragging a
   gate wrote the new position and redrew the old one; a new gate never
   appeared at all.
3. The filtered dataframe peeked the resolver, so moving a parent gate redrew
   its outline while every plot below it kept the old population.

**None of these were caught by 492 tests**, because the store is deliberately
tested without a runtime. If a user reports "my edit does not show up", look for
a `peek` before looking anywhere else. The deliberate peeks are commented as
such (two latches, and `match_gates_to_plot`, which depends on gate parameters
rather than positions).

### Tests that pass vacuously

Several near-misses this session. The suite caught one (`assert_ne!` guard
firing on a comparison that was identical either way); review caught others
(`filter_map` in an assertion silently skipping the case being tested). When a
test asserts something is correct, check it would fail if the code were wrong -
the fastest way is to revert the fix and re-run.

The pre-existing `flow_tests` modules in `gate_move` print results and pass
unconditionally, including on `Err`. They are exploratory harnesses, not tests.
The 41 asserting `gate_move` tests are separate.

### Composite gates are registered under several keys

A composite is in the registry under **its own id and each corner's id**, all
aliased to one `Arc`. That is what lets a corner id resolve to the whole gate at
filter time. Code that iterates the registry sees a composite several times;
code that deletes one must remove every key.

Note `is_ghost` currently returns true for every composite, because a composite
has no node of its own - only its corners do. Not reachable today, but it would
break the ghost sweep. See `TODO.md`.

## Working practice the user asked for

Show the proposed code edits and wait for an explicit OK before committing.
Implement and verify first, then present the diff - do not present speculative
code. Commit only when they approve.

A stop hook complains about uncommitted changes during those review pauses. The
user has been told; it can be gated on branch if it becomes annoying.

Write tests for new code as a matter of course - a standing instruction from the
user.
