# clingate-mcp

clingate's tools for Claude, over the Model Context Protocol. Claude Desktop
starts this program and asks it questions about a workspace: the FCS files,
an Omiq metadata export, an Omiq scaling export and an Omiq gating file in one
folder. The files never leave the computer; only the answers - counts,
percentages, summaries - go to Claude.

## Build

From the repository root, on the machine that will run it (macOS or Windows):

    cargo build --release -p clingate-mcp

No system packages are needed: this builds `clingate-core` and the server,
not the desktop app. The program is `target/release/clingate-mcp`
(`clingate-mcp.exe` on Windows).

## Add it to Claude Desktop

Open Claude Desktop's configuration file - Settings, Developer, Edit Config -
which is at:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

and add the server under `mcpServers`, with the full path to the program:

```json
{
  "mcpServers": {
    "clingate": {
      "command": "/Users/you/clingate/target/release/clingate-mcp"
    }
  }
}
```

On Windows the path is written with doubled backslashes:
`"C:\\Users\\you\\clingate\\target\\release\\clingate-mcp.exe"`.

Restart Claude Desktop. The tools appear under the tools menu in a chat. An
organisation's Claude settings can restrict which local servers may run; if
they do not appear, that is the first thing to check.

To see what the server is doing, set `CLINGATE_LOG=debug` in an `env` block
beside `command`; it writes to Claude Desktop's MCP log, never to the
protocol.

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
`save_gating` and `export_gating` write to disk, and the recovery copy is
kept up to date as Claude edits. Claude is told to change, save or export
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
