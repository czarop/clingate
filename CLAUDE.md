# clingate

A Rust workspace: the `clingate` app (Dioxus, `src/`), `crates/clingate-core` (everything that computes, headless) and `crates/clingate-mcp` (the MCP server over `Session`).

## Writing code

- As few comments as possible; none where the code can say it. Keep only a short "why" the code cannot express. No history, no narrating the change. One-line doc comments on public items.
- Small functions that do one thing, with names that say what they do. Well-named variables; no abbreviations.
- No duplication: look for an existing helper before writing one. Build nothing before it is needed.
- When a change alters behaviour, update every comment, doc (`docs/`, `CLAUDE.md`, `.claude/`), MCP instruction or tool description, and UI hint that describes the old behaviour.

## Tests

- Every new behaviour and edge case gets a test that fails without the change; check by breaking the code and running the test.
- Expected values come from an independent route - by hand, or another path - never from the code under test.
- New features get integration tests through `Session` (`crates/clingate-core/tests/session.rs`), the MCP protocol (`crates/clingate-mcp/tests/protocol.rs`) and the app where they reach it, including how they combine with existing features.
- Run: `cargo test -p clingate-core`, `cargo test -p clingate-mcp`, `cargo test -p clingate --no-default-features`.

## Pull requests

- Before opening one, run the `review` skill once: the reviewer over the branch's diff, and `cargo mutants` over the changed code.
- Tell the user when the branch is ready for a pull request: when a piece of work is finished and tested, and before starting the next one - small pull requests, one piece of work each.
