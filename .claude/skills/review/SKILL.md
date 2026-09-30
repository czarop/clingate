---
name: review
description: A quick review of a change before committing or opening a pull request, when asked (`/review`, or `/review pr`). One small reviewer, one pass, about 10,000 tokens.
---

1. Pick the diff command: `git diff HEAD` for a commit (list any untracked files too, and tell the reviewer to read them with `git diff --no-index /dev/null <file>`), or `git diff origin/main...HEAD` for `/review pr`. If the diff is over about 1,500 lines, review it in parts by path, or tell the user it is too big for one quick pass.
2. Run the `reviewer` agent once, giving it the diff command and one sentence on what the change is for. If it is not available by name, run a `general-purpose` agent with `model: haiku`, told to read `.claude/agents/reviewer.md` and follow it.
3. Fix every `must-fix`. For each `consider`, fix it or give a one-line reason not to. Do not run the reviewer again; run the tests instead.
4. Tell the user in a few lines what it found and what you changed.
