---
name: review
description: Review a branch before opening a pull request (`/review`): one pass by the reviewer agent over the diff, and a mutation run over the changed code to check the tests catch breakage.
---

Once per pull request, not per commit.

1. **Tests first.** Run the suites for the crates the branch touches (see `CLAUDE.md`). On a full disk, run one test target at a time and delete each test binary after it.
2. **The reviewer.** `git fetch origin main`, then run the `reviewer` agent once with `git diff origin/main...HEAD` and one paragraph on what the branch is for. If it is not available by name, run a `general-purpose` agent with `model: sonnet` told to read `.claude/agents/reviewer.md` and follow it. For a diff over about 3,000 lines, give it the diff one area at a time (`-- crates/clingate-core`, `-- src`, ...).
3. **Mutations.** For each Rust crate the branch changes:
   `git diff origin/main...HEAD > /tmp/branch.diff && cargo mutants --in-place --in-diff /tmp/branch.diff -p <crate> --cargo-arg=--lib`
   `--in-place` because a copy of the tree would rebuild everything on a disk that cannot hold it, and `--cargo-arg=--lib` because building every integration-test binary at once does not fit either. The working tree is restored after each mutant, so check `git status` is clean afterwards, and commit nothing while it runs - the tree holds a broken line. A mutant that is *missed* is a change to the code no test noticed: add a test, or say why it cannot matter.
4. **Act on it.** Fix every `must-fix` and every missed mutant that matters, run the tests again, and commit. For each `consider`, fix it or give a one-line reason. Do not run the reviewer again.
5. **Tell the user** in a few lines what the review and the mutation run found, what was changed, and anything left open; put the same in the pull request's description.
