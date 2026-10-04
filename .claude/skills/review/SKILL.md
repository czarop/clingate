---
name: review
description: Review a branch before opening a pull request (`/review`): the tests, then one pass by the reviewer agent over the diff.
---

Once per pull request, not per commit.

1. **Tests first.** Run the suites for the crates the branch touches (see `CLAUDE.md`). On a full disk, run one test target at a time and delete each test binary after it.
2. **The reviewer.** `git fetch origin main`, then run the `reviewer` agent once with `git diff origin/main...HEAD` and one paragraph on what the branch is for. If it is not available by name, run a `general-purpose` agent with `model: sonnet` told to read `.claude/agents/reviewer.md` and follow it. For a diff over about 3,000 lines, give it the diff one area at a time (`-- crates/clingate-core`, `-- src`, ...).
3. **Act on it.** Fix every `must-fix`, run the tests again, and commit. For each `consider`, fix it or give a one-line reason. Do not run the reviewer again.
4. **Tell the user** in a few lines what the review found, what was changed, and anything left open; put the same in the pull request's description.
