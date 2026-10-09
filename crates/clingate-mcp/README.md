# clingate-mcp

clingate's tools for Claude, over the Model Context Protocol. Claude Desktop
starts this program and asks it questions about a workspace: the FCS files,
an Omiq metadata export, an Omiq scaling export and an Omiq gating file in one
folder. The files never leave the computer; only the answers - counts,
percentages, summaries - go to Claude.

## Build

The quick way: a script does everything below except installing the Xcode
tools and Rust on a Mac, and on Windows offers to install those too. Run it
again to update.

- macOS: `bash scripts/install-mcp-mac.sh`
- Windows: `powershell -ExecutionPolicy Bypass -File scripts\install-mcp-windows.ps1`

Either works on its own, before the repository is cloned: download it from
GitHub and run it from wherever it was saved. The rest of this section is
what they do, step by step. Both keep everything in one folder -
`~/programs/clingate` (`%USERPROFILE%\programs\clingate` on Windows) - with
the code in `source` and the program Claude Desktop runs in `bin`.

The server is built from source on the machine that will run it - a Mac, or a
Windows PC. Only `clingate-core` and the server are built, not the desktop
app, so no system libraries are needed beyond a compiler.

Both this repository and `czarop/flow`, which it depends on, are private.
The repository's `.cargo/config.toml` has cargo fetch `flow` through the
`git` command line, so the build can reach it whenever `git` itself can
reach GitHub over HTTPS with your account. Check that first; this should list
the branches rather than ask for a password or say "not found":

    git ls-remote https://github.com/czarop/flow

### macOS

1. The compiler and git: `xcode-select --install`.
2. Rust: from <https://rustup.rs>, `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`,
   then open a new terminal.
3. GitHub over HTTPS: install GitHub's `gh` (`brew install gh`) and run
   `gh auth login`, choosing HTTPS and letting it set git up with your
   credentials. (An SSH key alone is not enough: `flow` is fetched by its
   HTTPS address.) Then run the `git ls-remote` check above.
4. Get the code and build:

       mkdir -p ~/programs/clingate && cd ~/programs/clingate
       git clone https://github.com/czarop/clingate source
       cd source
       cargo build --release -p clingate-mcp

   The first build takes several minutes; later ones are quicker.
5. Put the program somewhere it will stay, so a rebuild or a
   `cargo clean` never pulls it out from under Claude Desktop:

       mkdir -p ~/programs/clingate/bin
       cp target/release/clingate-mcp ~/programs/clingate/bin/

### Windows

1. The compiler: Visual Studio Build Tools
   (<https://visualstudio.microsoft.com/visual-cpp-build-tools/>), with the
   **Desktop development with C++** workload - the MSVC compiler and the
   Windows SDK.
2. Rust: `rustup-init.exe` from <https://rustup.rs>, with its defaults
   (the `msvc` toolchain).
3. Git: Git for Windows (<https://git-scm.com/download/win>). It includes
   Git Credential Manager, which signs in to GitHub in the browser the first
   time a private repository is fetched. Run the `git ls-remote` check above
   in a new terminal to get that done before building.
4. Get the code and build, in PowerShell:

       mkdir $env:USERPROFILE\programs\clingate -Force; cd $env:USERPROFILE\programs\clingate
       git clone https://github.com/czarop/clingate source
       cd source
       cargo build --release -p clingate-mcp

5. Put the program somewhere it will stay:

       mkdir $env:USERPROFILE\programs\clingate\bin -Force
       copy target\release\clingate-mcp.exe $env:USERPROFILE\programs\clingate\bin\

   This matters more on Windows: while Claude Desktop is running the server,
   Windows locks its file, and a build that tries to replace it fails with
   "Access is denied".

On a work laptop, things outside the code can stop this: a company proxy or
firewall that blocks `crates.io` or GitHub (the build fails fetching
crates), a policy that blocks programs you built yourself, or organisation
settings in Claude that do not allow local servers (the tools never appear).
Those are for your IT department.

### Updating

Quit Claude Desktop completely (from the menu bar on a Mac, the system tray
on Windows - closing the window leaves it running), then `git pull`, build
again, copy the program over the old one, and start Claude Desktop.

## Add it to Claude Desktop

In Claude Desktop, open Settings, Developer, Edit Config. That opens
`claude_desktop_config.json` - on a Mac in
`~/Library/Application Support/Claude/`, on Windows in `%APPDATA%\Claude\`.
Add the server under `mcpServers`, with the full path to the program - on a
Mac:

```json
{
  "mcpServers": {
    "clingate": {
      "command": "/Users/you/programs/clingate/bin/clingate-mcp"
    }
  }
}
```

and on Windows, with every backslash doubled:

```json
{
  "mcpServers": {
    "clingate": {
      "command": "C:\\Users\\you\\programs\\clingate\\bin\\clingate-mcp.exe"
    }
  }
}
```

If the file already has other servers, add `"clingate": {...}` beside them
inside the same `mcpServers`.

Quit and restart Claude Desktop. The tools appear under the tools menu in a
chat.

If they do not, or a tool fails, Claude Desktop's log for the server says
why: `~/Library/Logs/Claude/mcp-server-clingate.log` on a Mac,
`%APPDATA%\Claude\logs\mcp-server-clingate.log` on Windows. For more
detail, add `"env": { "CLINGATE_LOG": "debug" }` beside `command`; the server
writes its diagnostics to that log, never into the protocol.

On a Mac, if opening a workspace fails with a permission error for data in
Documents, Desktop, Downloads, iCloud Drive or an external disk, macOS is
keeping Claude Desktop out of that folder: allow it in System Settings,
Privacy & Security, Files and Folders (or Full Disk Access).

## The tools

| Tool | What it does |
|---|---|
| `open_workspace` | Opens the workspace in a folder and answers with an overview. Claude is told always to ask which folder. |
| `workspace_overview` | What loaded, the metadata columns and their values, the compensation groups, and anything that must be asked first. |
| `find_samples` | The samples a query names, with their metadata. |
| `list_populations` | The gating tree's populations, each with the shortest name that names only it. |
| `population_stats` | A population's events, parent events and percent of parent in each sample named, with min, median and max. |
| `distribution` | A population's parent on one parameter in one sample: percentiles, a histogram, and where the gate's edges sit. |
| `answer_omiq_compensation` | Records the user's answer about compensation applied in Omiq, for Omiq-exported files. Only with the user's own answer. |
| `list_parameters` | Every parameter's marker, channel, scale and axis range, or the one a query names. |
| `gate_details` | A population's gate: its parameters, its extent on each, its shape - and for one sample, the position that applies to it and whether it was set for that sample, for a group, or is the gate as drawn. |
| `compare_samples` | A population's parent on one parameter across samples: percentiles, how far each sample's median is from the others' (in typical interquartile ranges), its spread against theirs, and where the gate sits. For finding the sample distributed unlike the rest. |
| `list_rules` | The workspace's gate rules (`rules/gate_rules.json`, where the Gate Rules tab saves them), each in words. |
| `preview_rules` | Runs every rule and says what it would move, from where to where, with what confidence and which want review. Moves nothing. |
| `apply_rule_placements` | Applies the last preview to the working copy - one undo step, as a run is in the app. Refused if the gates changed since. |
| `undo` | Steps the working copy back one edit. |
| `redo` | Steps forward again after an undo. |
| `revert_to_saved` | Puts the working copy back to the last save; `undo` brings the changes back. |
| `save_gating` | Saves the working copy into the workspace folder as `clingate_gating.omiqgt` with `clingate_scaling.csv` - the app's Save. The workspace opens on it next time, in the app too. |
| `export_gating` | Writes the saved copy as an Omiq gating file in the workspace folder, under a name the user chose - the app's Export. Never replaces a file unless told to. |
| `restore_unsaved_changes` | Takes back unsaved changes an earlier session - the app's or Claude's - left in the folder. |
| `discard_unsaved_changes` | Throws those away. |
| `assess_run` | How many of the last applied run's placements are in each pile of the Review tab, the ones needing a look - the gate 2 or more of its parent's IQRs from where its peers put theirs, or the rule unsure - with reasons, and each gate across the run in a line. Reads no files. |
| `compare_to_peers` | One sample's placement of a gate beside its peers', in numbers: percentiles, peaks, the line, where it sits between the peaks and what it lets through. |
| `mark_looks_right` | Clears a placement's flag - the user judged it right - or takes that back, as the Review tab's Looks right does. Only on the user's word. |
| `report_placement` | Reports a gate the rules placed badly on one sample, with the user's reason - as the app's Report... button does. Only on the user's word. |
| `mark_run_reviewed` | Marks the last applied rules run reviewed: placements not reported count as accepted, and the review is copied into the review library. Only on the user's word. |
| `rule_guide` | A guide to the gate rules: how to choose one by what the data looks like, and for each rule what it suits, how it works step by step, every setting, its traps and what its confidence says (`docs/rules`). Needs no workspace. |
| `score_rules` | Scores the rules against the gating drawn by hand: every rule, or one population's, run on the files with each gate under its parent as drawn. Per gate and sample, the events both gates hold - their agreement, how much of the hand gate the rule catches, how much it holds beyond it - with how far the rule's gate sits from the hand gate, and for a one-edge rule how far it moved the edge. A sample is off below `off_below` (default 0.8), and a sample of few events may fall up to `noise_widths` / sqrt(events) further. Each gate summed up in a line: typical and lowest agreement, and which samples are off. Moves nothing. |
| `fit_rule` | Searches one population's rule settings for those closest to the gating drawn by hand: the rule as it stands, any candidates given and - by default when none are - every combination of a few values of each setting that matters for its kind, each scored as `score_rules` scores it. With eight specimens or more, ranked on half of them and checked on the other half. Ranked by typical agreement with near-ties (`tie_within`) going to the fewest samples off, or by fewest off (`rank_by: "off"`); each candidate's place both ways and on the check half, and the best and those tied with it marked. Those and the rule as it stands are kept in the workspace for the app's Gallery tab, which draws each one's gate over the user's. Moves no gate. |
| `pick_rule` | Picks a rule for one population's gate, or every ruled gate, trying the kinds in the user's order of preference - the band on the FMX at the range they accept, above the negative, the valley or a smear, a band on the sample, the phenotype last - and taking the first that passes their cut-off (no more than a share of samples agreeing less than an agreement with the hand gate; the settings chosen in the app unless given). The first that passes has its settings searched; when none does, each kind's are searched in turn, and the closest of all is shown, flagged. Per gate: the rule beside the rule as it stands, whether it passed, how each kind did, and the others tied with it - kept for the Gallery tab. Changes no rule. |
| `try_rules` | Tries one to four candidate rules for one gate on the files as they are, moving nothing: what the gate would hold on every sample, summed up by sample type, beside what it holds now, and what each rule was unsure of or could not place. Each file is read once for all of them. |
| `gate_profile` | What a gate's populations look like across a spread of specimens, on each of its markers, by sample type: shape classes (separate, shoulder, smear, merged, negative only, several peaks), how the negative shifts and changes shape, where the gate sits against it and what it holds, and the signal above each full stain's FMX. With no population, every gate in a line. |
| `gate_picture` | One picture of a gate drawn on several samples with its outline in - the ones named, or the reference and each sample type's least, most and most typical - with captions saying which is which. |
| `explain_gate_positioning` | Exactly how the rules position gates, in words: which file is gated and which read, each rule step by step with its constants, confidence, review, replays, and the rules file's format. Needs no workspace. |
| `read_positioning_code` | The source that positions, judges and replays gates - by file and line range, or found by a search. Needs no workspace. |
| `replay_rules` | Replays reviewed runs (the workspace's and the review library's) on the events each kept, with the rules they ran with and with proposed changes: what each change fixes, breaks or leaves wrong, per gate and per placement. Changes nothing. |
| `replay_case` | One replayed placement in full: histograms of the sample and the file the rule read, where the gate started, and where the run, the replay and the reviewer put it. |
| `update_rule` | Writes one rule into the workspace's rules file. Only on the user's word, after the replay. |

## The working copy

The tools edit the same working copy the app does, through the same code:
the gates Claude changes are a working copy, every change is one step that
`undo` and `redo` walk, `save_gating` writes it where the app's Save does,
and `export_gating` writes the saved copy, as the app's Export does. While
there are unsaved changes a hidden recovery copy is kept in the folder, so
the app offers Claude's unsaved changes back when it opens the folder, and
the other way round. Tests in the app (`src/gate_editor/parity_tests.rs`)
do the same things through the app's own buttons' code and through these
tools, and fail if the two ever come out differently.

`answer_omiq_compensation` and the edits change only the open session.
`save_gating` and `export_gating` write to disk, `fit_rule` and `pick_rule`
keep their closest candidates in `rules/searches.json` for the Gallery tab,
and the recovery copy is kept up to date as Claude edits. Claude is told to change, save or export
only when the user says to, and to ask the user before restoring or
discarding earlier unsaved changes.

## The saved workspace

When the app has saved the workspace in the folder (`clingate_workspace.json`),
`open_workspace` opens it as the app left it: the same FCS files, the
compensation groups and the answers already given to the Omiq question, and
the metadata columns that group and sort the samples. Without one, the folder
is searched as the app would for a new workspace. The tools never write it.

## Naming things

Samples, populations and parameters are named the way a person names them:

- **Samples** by any word of their file name or metadata values - `fmx`,
  `wk1 fmx` (both words), `SampleType=FS` (that column only), or `all`.
- **Populations** by the markers of their gate names, with `>` for steps
  down the tree - `CD4+`, `CD4+ > CD279+`, `cd45ra- cd279+`.
- **Parameters** by marker or channel - `CD4`, `BUV395-A`, `BUV395`.

Matching is strict: `qc` does not find `QC4`, and `cd4` does not find `CD4+`.
Anything that does not match exactly comes back as `needs_clarification`,
with what might have been meant, and Claude is told to ask rather than pick.

## Try

Once it is added, in a new chat:

- "Open my clingate workspace." - Claude asks for the folder.
- "What's the %CD4+ in the FMX samples?"
- "How is CD4 spread in the WK1 FMX sample, and where does the CD4+ gate sit?"
- "Which populations are called CD279+?"
- "Is any sample's CD134 distribution in CD4+ unlike the others?"
- "What would the rules do to this plate?" - then, if the moves look right,
  "apply them and save", or "undo that".
