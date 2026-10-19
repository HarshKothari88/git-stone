## Git Stone (task boundaries)

`git stone` is installed. Use it to save a boundary after each finished task, so the human can turn the
work into atomic commits later. Never rewrite or delete code to make commits atomic.

When to make a stone: after each finished logical task, tests passing, before starting the next one.
How: `git stone "<short label>" -d "<what the task was, why, which files or routes it touches>"`.
Always add `-d`: the human reads it later to understand the work. If the label looks like a command, a
number, or starts with `-`, put `--` before it. No secrets in labels or descriptions.

Leaving files out: only when the human asked, or a file must never be committed (secrets, local config).
Add `-x '<path or glob>'` to the stone command; it leaves matching files out of that stone only. The output
lists `Left out:`; check it. Excluded files are not in the stone, so they cannot be restored from it.

Workflow: do task -> `git stone "label" -d "details"` -> next task. At hand-over run `git stone review`
and show it to the human. Check state with `git stone status --porcelain` or `git stone list --porcelain`
(NUL-separated records, tab-separated fields; the first record is `git-stone-porcelain<TAB>1<TAB><command>`).

Never, unless the human asks:
- `git stone <id> drop` to undo code. It removes a boundary only; files stay as they are.
- `git stone commit`, `commit-all`, `repair --apply`, `adopt`, `prune --apply`. Committing is the human's call.
- `--force` or `--allow-empty` on a stone.
- `--remember` or `git stone exclude --remove|--clear`. They change a rule for the whole repository, on every branch.
- a plain `git commit` between stones (it moves the branch; stones then need `git stone repair`).
If a command is refused (exit 1), read its message and tell the human. Do not work around it.
Exit codes: 0 done, 1 refused with a reason, 2 failure (report it).
