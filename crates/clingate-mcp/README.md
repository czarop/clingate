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

All but the last are read-only; none writes to disk.

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
