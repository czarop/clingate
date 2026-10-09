# TODO

Open work. Items carry enough context to be picked up cold. The detail behind
each is in `docs/test-audit.md` (bugs found in clingate, by id) and
`docs/flow-review.md` (the review of the flow crates, by number).

## Tools for Claude over MCP

The aim: Claude Desktop drives clingate headless through MCP tools that
compute and act - understand what is happening in the data and, in time,
take action - built on the same code the app uses. First the restructuring
that makes the code usable as tools. Decided:

- Three crates in one Cargo workspace: `clingate-core` (everything that
  computes), `clingate` (the desktop app), later `clingate-mcp`.
- The core keeps `derive(Store)` and its store methods, so the app keeps its
  fine-grained reactivity, and depends on `dioxus-stores` only - the reactive
  core, no renderer, webview or GTK. Nothing in the core may use components,
  `rsx!`, `use_context` or other UI.

Each stage leaves behaviour unchanged and is checked with the full suite and in
the running app.

- [x] **Stage 1 - operations out of the UI files.** The rules run
      (`run_solve`, `measure_all`) out of `gate_rules_window.rs`; one events
      pipeline (open, compensate, scale, filter by the gate chain, index)
      shared by the editor, the gallery and the rules run, in place of three;
      workspace loading (metadata, scaling with gates carried across, gating)
      as plain functions the Workspace tab calls.
- [x] **Stage 2 - the core modules use only `dioxus-stores`.** And print
      nothing: a stdio tool server's protocol runs on stdout.
- [x] **Stage 3 - split the crates.** `crates/clingate-core` and the app at
      the root; `git mv`, so history is kept.
- [x] **Stage 4 - a headless `Session`** (`clingate_core::session`): opens a
      folder as the Workspace tab does, and answers by name - samples by any
      word of file name or metadata, populations by gate-path markers,
      parameters by marker or channel - strictly, with anything else a
      question carrying suggestions. First queries: overview, samples,
      populations, population stats, distribution; the Omiq compensation
      answer.
- [x] **Stage 5 - `clingate-mcp`** (`crates/clingate-mcp`): an `rmcp` stdio
      server over the session, seven tools at first; how to add it to Claude Desktop is
      in its README. Tested by running the binary over the protocol.
- [ ] **Try it in Claude Desktop** on macOS and Windows, and see what the
      questions actually asked need next.
- [x] **More tools, round 2** - parameters, a gate's details (as drawn, or
      as positioned for one sample), comparing samples on a parameter, the
      rules listed, previewed, applied, and the gating file saved. Fourteen
      tools; the rules steps checked on Plate 10 (CD134+ of CD4+ against
      each specimen's FMX) with the saved file reopened.
- [ ] **More tools, as use shows** - candidates: editing a rule or a
      reference over MCP; the confidence behind a placement in more detail
      (once the confidence scores are reworked); undoing an applied preview
      without reopening the folder.
- [ ] **A rules run that pauses, over MCP.** In the app, a run stops at the
      end of a level when a gate with gates under it could not be placed, or
      scored under 0.2, on any sample, and asks the person to place it by hand
      before the level below is measured. Claude cannot do that: where a gate
      should go on a plot is not something a person can say in words, and
      Claude cannot see the plot. Work out what a paused run looks like over
      MCP - probably the run stops, reports which gates on which samples need
      a person, and Claude hands them back to the user in the app.
- [ ] **A cache of scaled events**, if questions over a plate get slow: each
      question reads its files afresh, as the editor does per plot. Keyed by
      path, file modification time, cofactors and compensation, so a change
      to any of them misses rather than serving stale events.

## Compensation

Built and unit-tested - groups as in Omiq, the matrix applied in Omiq taken
back out and the wanted one put in, export for Omiq and for the exported files,
and the tabbed panel on the Workspace tab (`clingate_core::compensation`,
`clingate_core::compensation::groups`, `src/gate_editor/compensation_panel.rs`). The
fixture tests pin the arithmetic against Omiq's own exports. What they cannot
show is that it holds up on real work, and that is the user's to do.

- [ ] **Test the whole workflow by hand, extensively.** On a real plate, with
      files exported from Omiq both with and without compensation applied:
      - The grouping: files land in the groups expected; Assign files (search,
        multi-select, shift-click, new group) moves them; groups and answers
        survive closing and reopening the workspace.
      - The Omiq question: "No compensation applied in Omiq" draws the files as
        they are; "Compensation applied in Omiq" with Omiq's matrix pasted
        draws them identically to before (nothing changes until an edit).
        Bad pastes are refused with a reason that makes sense.
      - An edit here moves only what it should, in the editor, the gallery and
        a rules run alike.
      - **The round trip:** edit here, Copy matrix for Omiq, paste into Omiq,
        export again from Omiq, load the new export, answer "applied" with the
        new matrix - the plots should look the same as they did here before
        the round trip.
      - "Save matrix for the exported files" gives the right correction in
        whatever other software would use it.
      - The 38-channel grid stays responsive to edit.
- [ ] **Omiq exports that carry a `$SPILLOVER`.** The Plate_10 exports carry a
      32-channel matrix in their header, though the earlier test exports carried
      none - probably the cytometer's matrix passed through by Omiq. They are
      grouped by that matrix (source "each file's own") and still asked the Omiq
      question. Confirm what that matrix is, and whether Omiq's own
      compensation of such a file is baked in on top of it, before trusting a
      group of them.
- [ ] **An on-plot nudge.** Discussed, not built: drag one spillover value
      while watching the two-channel plot it affects, as in FlowJo's
      compensation editor. Only worth doing once the grid has been used enough
      to know it is too slow for this.

## flow crates

All the review's fixes are on `czarop/flow` branch
`claude/fcs-errors-not-panics`, and clingate's `Cargo.toml` pins that branch's
commit (`b7a77c4`).

- [ ] **Merge the flow branch and repoint clingate.** Merge
      `claude/fcs-errors-not-panics` into flow's default branch, then point the
      three `flow-*` dependencies in `Cargo.toml` back at it (the comment above
      them says so). Until then clingate depends on an unmerged branch.
- [ ] **flow's own compensation is wrong for any asymmetric matrix.**
      `Fcs::apply_compensation` computes channel `i` as `Σⱼ S⁻¹[i][j]·oⱼ`; the
      `$SPILLOVER` orientation (row = fluorochrome, column = detector) needs
      `Σⱼ oⱼ·S⁻¹[j][i]`. They agree only for a symmetric matrix, and
      `apply_file_compensation` passes `$SPILLOVER` straight in. clingate does
      not call it - it compensates in `clingate_core::compensation` - but anyone else
      using flow gets wrong values. `$COMP` is also read as if it had
      `$SPILLOVER`'s channel names. Fix or remove both (flow-review 16).
- [ ] **GatingML import panics.** `gatingml_to_gates` is `todo!()` for
      polygon, rectangle and ellipse gates, so reading any GatingML file with a
      gate panics. clingate does not use it; it should return an error.
- [ ] **Integer data ignores the `$PnR` bit mask.** `$DATATYPE` I files that
      use fewer bits than they store should be masked, per the standard. Only
      matters for integer files from older instruments.
- [ ] **Biexponential is not logicle - needs a decision** (flow-review 11).
      `TransformType::Biexponential` claims to match FlowJo's logicle but is a
      scaled arcsinh that ignores `width`. clingate never produces it but has
      four `todo!()` arms for it (`axis_info.rs`, `skewed_quadrant_gate.rs`),
      and `read_axis_configs` skips an unsupported scaling type with a
      `println`, so such a channel silently has no axis - it should be a
      reported problem like the others. Either remove the variant or implement
      real logicle, if Omiq's logicle scalings will be needed.

## Fitting rules to hand gating

The scorer (`gate_rules::score`, the `score_rules` tool) compares a ruleset
with a workspace gated by hand, event by event; the search
(`gate_rules::fit`, the `fit_rule` tool) tries a gate's rule settings and
ranks them by it.

- [ ] **Try the search on a hand-gated workspace** and settle what ranks
      best: typical agreement with ties to the fewest off, or fewest off
      first - and whether the default grids, the tie of 0.02 and the split
      at eight specimens suit runs of 20 to 40 samples.

- [ ] **Quadrant gates in the score.** A quadrant gate is four gates in one;
      `admitted_by` returns nothing for it, so the score skips it. Score each
      quadrant on its own - which events each holds against the hand-drawn
      quadrants - and sum the gate up over the four.
- [x] **Flick between close candidates on the plots.** A search keeps its
      best, those tied with it and the rule as it stands in
      `rules/searches.json`; the Gallery tab draws each one's gate dashed
      over the gate as drawn, one at a time, and can take its rule.
- [ ] **Mark the samples a candidate is off on** in the gallery, and say
      which ones it could not place - the search knows both.
- [x] **Pick the best rule - kind and settings - for every gate.** The
      Gate Rules tab's "Pick the best rule for every gate" and the
      `pick_rule` tool: each kind tried once from the hand gating, then the
      best kind's settings searched, on one reading of the files; Use, or
      Use the best for every gate.
- [ ] **Pick for one gate, or a chosen few, from the app** - and with
      settings other than the defaults (the ranking, the tie, the split).
- [ ] **Pick for a gate with no rule yet.** A pick starts from the rule a
      gate has, which says the marker and the edge it moves; a gate without
      one would need them read from how it is drawn.
- [ ] **Time a pick on a real workspace.** What is left to speed up is one
      density per sample at each of the 5 smoothings tried, each summing the
      events near each of 512 points.

## clingate

- [ ] **A shared cache of transformed frames** (flow-review 6) - the largest
      speed-up available. Every plot view, every gallery image and every file
      of a rules run opens the FCS and re-applies the transforms (and now
      compensation). One cache keyed by path, modification time, cofactors and
      the compensation digest, evicted least-recently-used under a memory
      budget, read by all three. Not started.
- [ ] **B-AUTO-1 - decide how "above the negative" finds the negative.** With
      the default `NegativeFinder::BelowTheGate`, a negative that drifts past
      the reference's gate is read low and the gate lands inside it,
      confidently (69% admitted against 10% on the reference). The other
      finder, the density's leftmost peak, tracks any drift but is about five
      times noisier. Pinned as an ignored failing test in
      `gate_rules/run.rs`. Wants testing against hand gating on real
      samples; a hybrid - refine from the gate, fall back to the peak when it
      sticks - is the likely end state.
- [ ] **B-KDE-2 and B-KDE-3 - decisions about method** in
      `gate_move::kde_shift`, which the app does not call yet. B-KDE-2: the
      fixed 0.1 significance threshold is below the noise in a wide
      population's peak, so a widened negative reads as a shift. B-KDE-3: the
      smear score's entropy term depends on the KDE grid and never nears its
      documented ends. Both pinned as ignored failing tests; worth settling
      before `gate_move` is wired in.
