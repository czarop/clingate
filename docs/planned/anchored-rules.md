# Planned: linked gates for Claude, rules from another gate, per-run FMX

On hold at the user's request (2026-09-29). Came out of the first test of
Claude building rules for the ICS Th17 HUPD BMXA panel from its gating guide.

## 1. Linked gates visible to Claude; rules target by path

Linking already exists in core (`GateState::link_node_to_gate`,
`unlink_node`, `is_linked`), the Omiq import keeps Omiq's links, and a rule
that moves a linked gate moves it everywhere. The MCP tools never say so,
so Claude concluded there was no such thing.

- `gate_details` and `list_populations` name the other placements a gate
  is linked with.
- A rule on a linked gate reads events only at the placement it names -
  never at a copy under a subset (MAIT > CD4+CD8- > IFNy+). A rule that
  names no parent on a linked gate must not read both.
- Instructions: write the rule once on the top-level gate; copies follow.
- A rule's parent is the shortest unique path (`gate_paths::unique_names`,
  e.g. `CD161+Va7.2+ / CD4+CD8-`). Say so in the tool docs; show those
  names in `list_populations`.

## 2. A rule that positions a gate from another gate

What the guide asks for:

- Same position: MAIT CD4-CD8+ = main CD4-CD8+; MAIT and TCRgd CD154+ =
  CD4's CD154+; gate 47 quadrant = the CD8 one (plot 99).
- Aligned to an edge: CD19- to the left edge of CD19+CD14- (also CD3-,
  CD56-); MAIT CD4-CD8- to the left of CD4+CD8- and the bottom of
  CD4-CD8+; 81b next to 81a.

One rule kind, two modes: copy the anchor's whole shape (same axes), or set
this gate's edge on a marker to the anchor's edge on that marker, with an
optional gap. Per sample: it copies whatever the anchor has on that sample,
by rule or by hand.

Ordering is enforced by the run, not left to the caller: rules run in
stages from a dependency graph (anchor before follower; a gate whose parent
a rule moves after that parent). A loop is refused when the rule is
written. A follower whose anchor could not be placed on a sample is skipped
there with that reason. `try_rules` on a follower uses the anchor's
candidate position when both are in the trial, the current one otherwise.
Keep frames in memory between stages where they fit.

Tell Claude: settle the anchor first (rule written and reviewed, or gated
by hand), then write the rules that follow it. A rule-guide page; 
`update_rule` warns when the anchor's rule has not been reviewed.

## 3. Per-run FMX, one reference per run

- Tail-fraction option: read all of a run's FMX files together and set one
  line per run (MAIT/NKT FMX parents of ~700 events make 0.2-0.5% one to
  three events).
- One FS reference per value of a run column.
- Open questions for the user: are Plate 1 and Plate 2 separate runs, and
  which metadata column names the run.
