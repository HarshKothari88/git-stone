# Git Stone for AI coding agents

Give your coding agent this once, in its memory file, and it will use `git stone` on its own. No skill file is needed.
The block below is short on purpose, because memory files are read in every session.

## One command (Claude Code and Codex)

```sh
git stone setup --agent claude             # this project: CLAUDE.md at the top of the repository
git stone setup --agent codex              # this project: AGENTS.md
git stone setup --agent both --global      # every project: ~/.claude/CLAUDE.md and ~/.codex/AGENTS.md
```

- Without `--global` it writes in the project you are in; with it, in the agent's own folder. (`CLAUDE_CONFIG_DIR` and
  `CODEX_HOME` are honoured if set.)
- **Your own instructions are never changed.** The block is added at the end of the file between two marker lines
  (`<!-- git-stone:begin ... -->` and `<!-- git-stone:end -->`). A file that does not exist is created.
- Running it again is safe: an up-to-date block changes nothing, and an older one is replaced in place, so rerun it after
  upgrading Git Stone. To remove the block, delete the lines between and including the markers.
- After upgrading, `git stone setup --update` refreshes the block in every file that already has one (this project's
  `CLAUDE.md` and `AGENTS.md`, `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`). It creates nothing and skips files without our
  markers. `git stone status` warns (`agent-instructions-outdated`) when a block is older than the installed version.
  A block pasted by hand has no markers, so replace that one yourself.
- A symlinked `CLAUDE.md` or `AGENTS.md` stays a symlink. A file with a damaged marker is refused and nothing is changed.
- The agent reads the file when it starts a new session.

## For any other tool

```sh
git stone agents            # prints the block for the installed version
```

## Where to paste it by hand

Paste the block into the file your tool reads. **Global** applies to every project, **project** only to one repository
(commit it, and the whole team's agents follow it).

| Tool | Global | Project |
|---|---|---|
| Claude Code | `~/.claude/CLAUDE.md` | `CLAUDE.md` |
| Codex CLI | `~/.codex/AGENTS.md` | `AGENTS.md` |
| Gemini CLI | `~/.gemini/GEMINI.md` | `GEMINI.md` |
| Cursor | Settings → Rules → User Rules | `.cursor/rules/git-stone.mdc` (set `alwaysApply: true`) |
| GitHub Copilot | | `.github/copilot-instructions.md` |
| Windsurf | `~/.codeium/windsurf/memories/global_rules.md` | `.windsurf/rules/git-stone.md` |
| Any tool that reads `AGENTS.md` | the tool's global file | `AGENTS.md` |

Tools change these locations from time to time; check your tool's documentation if a path does not work.

## The block

````markdown
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
````

## Details the block leaves out

An agent that needs more can read these on demand; they do not belong in always-loaded memory.

- `git stone status --porcelain` adds records: `scope<TAB><ref|detached>`, `head<TAB><oid>`, `anchor<TAB><oid>` (different
  means the branch moved), `change<TAB><code><TAB><path>`, and `warning<TAB><code><TAB><text>`. Warning codes:
  `branch-moved`, `stones-already-committed`, `operation-in-progress`, `unmerged-paths`, `matches-branch-commit`,
  `dirty-submodule`, `orphaned-chain`, `detached-chain`, `branch-in-two-worktrees`, `agent-instructions-outdated`.
- A `stone` record is `stone<TAB><id><TAB><created unix seconds><TAB><tree><TAB><name><TAB><description>`. The description
  may contain tabs and newlines but never NUL, so split on NUL first, then on tab with at most 6 fields.
- A stone is refused while a merge, rebase or cherry-pick is in progress, while files have conflicts, when nothing changed
  since the previous stone, and when the branch moved since the stones were made.
- What each normal Git action does to stones: [git-interactions.md](git-interactions.md).
