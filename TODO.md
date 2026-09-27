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

- [ ] **Stage 1 - operations out of the UI files.** The rules run
      (`run_solve`, `measure_all`) out of `gate_rules_window.rs`; one events
      pipeline (open, compensate, scale, filter by the gate chain, index)
      shared by the editor, the gallery and the rules run, in place of three;
      workspace loading (metadata, scaling with gates carried across, gating)
      as plain functions the Workspace tab calls.
- [ ] **Stage 2 - the core modules use only `dioxus-stores`.**
- [ ] **Stage 3 - split the crates.** `git mv`, so history is kept.
- [ ] **Stage 4 - a headless `Session`** holding workspace, gates, metadata,
      scaling, compensation and rules, with load and save, and serialisable
      operations with stable ids (samples by program name, populations by
      gate path, parameters by channel). The first data-understanding
      operations go here.
- [ ] **Stage 5 - `clingate-mcp`**, an `rmcp` stdio server over the session,
      built for Windows and macOS.

## Compensation

Built and unit-tested - groups as in Omiq, the matrix applied in Omiq taken
back out and the wanted one put in, export for Omiq and for the exported files,
and the tabbed panel on the Workspace tab (`src/compensation.rs`,
`src/compensation/groups.rs`, `src/gate_editor/compensation_panel.rs`). The
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
      not call it - it compensates in `src/compensation.rs` - but anyone else
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
      `gate_rules_window.rs`. Wants testing against hand gating on real
      samples; a hybrid - refine from the gate, fall back to the peak when it
      sticks - is the likely end state.
- [ ] **B-KDE-2 and B-KDE-3 - decisions about method** in
      `gate_move::kde_shift`, which the app does not call yet. B-KDE-2: the
      fixed 0.1 significance threshold is below the noise in a wide
      population's peak, so a widened negative reads as a shift. B-KDE-3: the
      smear score's entropy term depends on the KDE grid and never nears its
      documented ends. Both pinned as ignored failing tests; worth settling
      before `gate_move` is wired in.
