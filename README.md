# Git Stone

Save the boundary of each task while you work. Turn the boundaries into clean Git commits later.

![Without Git Stone, an agent leaves one pile of changes. With it, each task is a stone you review, fold with drop, and commit cleanly.](docs/demo.gif)

```text
$ git stone "add login" -d "Adds login(). Covers the /login route."
Stone 1 created: add login
  1 file changed, +1 -0
  Adds login(). Covers the /login route.

$ git stone "add route guard"
Stone 2 created: add route guard
  2 files changed, +2 -0

$ git stone commit-all
[feature/auth e06a1bc] add login
 1 file changed, 1 insertion(+)
Committed Stone 1 as e06a1bc: add login
[feature/auth 1d5c432] add route guard
 2 files changed, 2 insertions(+)
Committed Stone 2 as 1d5c432: add route guard
No stones are left.
```

## The problem

You (or an AI coding agent) work through many tasks on one branch without stopping. To end up with
atomic commits today, you have two choices:

- **Commit after every task.** That breaks your flow, and you must decide the commit message and
  contents while your head is still in the code.
- **Split everything at the end.** That is slow by hand, and an agent often deletes and rewrites its own
  code just to make the pieces come out atomic.

A **Stone** saves the state of your whole working tree at the moment a task ends. It costs one
command and about 100 ms. Your files, your staging area and your branch are not touched. Later you
turn the stones into ordinary commits, one by one, without rewriting any code.

## How it works in one picture

```text
HEAD ─ commit A ──────────────────────────────────────────────► working tree now
              │
              ├─ Stone 1  "add login"        (everything up to the end of task 1)
              ├─ Stone 2  "add route guard"  (everything up to the end of task 2)
              └─ Stone 3  "rate limit"       (everything up to the end of task 3)

Diff of a stone = the stone before it  →  this stone.
```

- A stone is a full snapshot of the working tree, kept as a Git tree object under `refs/stones/`.
- A stone's diff starts at the previous stone, so it shows only that task's changes.
- `git stone 1 commit` makes a real `git commit` whose content is exactly Stone 1.

**Four things to remember:**

- **A Stone is a boundary, not a branch.**
- **Dropping a Stone removes the boundary, not the code.**
- **Stones are sequential and may depend on earlier Stones.**
- **Final commits are ordinary Git commits.**

## Install

You need Git on your `PATH` (developed and tested with Git 2.53; `repair` needs 2.40 or newer).

**macOS and Linux, prebuilt binary:**

```sh
curl -fsSL https://raw.githubusercontent.com/HarshKothari88/git-stone/main/install.sh | sh
```

This puts `git-stone` in `~/.local/bin` (set `PREFIX` to change that) together with its man page. Git runs any
`git-<name>` program on your `PATH` as `git <name>`, so `git stone` works right away. Check with `git stone version`.

**Homebrew:** `brew install HarshKothari88/tap/git-stone`

**With Rust:** `cargo install --git https://github.com/HarshKothari88/git-stone`

**Windows:** download `git-stone-x86_64-pc-windows-msvc.zip` from the
[releases page](https://github.com/HarshKothari88/git-stone/releases) and put `git-stone.exe` on your `PATH`.
Windows builds are not tested yet.

**Let your AI agent use it** (Claude Code, Codex). One command adds a short, marked block to the agent's memory file and keeps
everything else in it as it is:

```sh
git stone setup --agent claude            # this project: CLAUDE.md
git stone setup --agent both --global     # every project: ~/.claude/CLAUDE.md and ~/.codex/AGENTS.md
```

Details and other tools: [docs/ai-agents.md](docs/ai-agents.md).

## Two-minute walkthrough

You are on a branch and just finished the first task.

```text
$ echo "export function login() {}" > src/auth.ts
$ git stone "add login" -d "Adds login(). Covers the /login route. Tests come in the next stone."
Stone 1 created: add login
  1 file changed, +1 -0
  Adds login(). Covers the /login route. Tests come in the next stone.
```

The second task builds on the first. Nothing special is needed.

```text
$ echo "export function guard() {}" > src/guard.ts
$ echo "// guard" >> src/auth.ts
$ git stone "add route guard"
Stone 2 created: add route guard
  2 files changed, +2 -0
```

You keep working. Where do things stand?

```text
$ echo "// wip" >> src/app.ts
$ git stone
Branch: feature/auth

Stones:
  1  add login
  2  add route guard

Current changes since Stone 2:
  M src/app.ts

Unstoned changes: 1 file
```

Review what a stone holds, or all of them at once:

```text
$ git stone review
Stone 1  add login
    Adds login(). Covers the /login route. Tests come in the next stone.
    1 file changed, +1 -0
    +1 -0  src/auth.ts

Stone 2  add route guard
    2 files changed, +2 -0
    +1 -0  src/auth.ts
    +1 -0  src/guard.ts

CURRENT
    1 file changed, +1 -0
    +1 -0  src/app.ts
```

Commit the first stone. Only that stone's content goes in; your unstoned work stays where it is.

```text
$ git stone 1 commit -m "feat: add login"
[feature/auth e06a1bc] feat: add login
 1 file changed, 1 insertion(+)
Committed Stone 1 as e06a1bc: feat: add login
Stone 2 is now next.
```

Or commit everything in order, using the stone names as messages:

```text
$ git stone commit-all
```

`git status` now shows only `src/app.ts`, the work you had not stoned.

Instead of committing, you can change your mind about a boundary and drop it. No file changes; the next stone just grows to cover it.

```text
$ git stone 2 drop
Dropped Stone 2: add route guard

No files were changed.
Your unstoned changes now include all changes from the old Stone 2 boundary.
```

## Commands

| Command | What it does |
|---|---|
| `git stone <name> [-d <text>]` | Make a stone. `-d` says in detail what the task was and why. |
| `git stone <name> -x <pattern> [--remember]` | Leave matching paths out of this stone. Globs work (`'*.log'`, `scratch/`). `--remember` keeps the rule for every later stone in this repository, on every branch. |
| `git stone exclude [--remove <pattern> \| --clear]` | Show the remembered rules, or remove one or all of them. |
| `git stone -- <name>` | Same, for a name that looks like a command, a number, or starts with `-`. |
| `git stone` / `git stone status [--porcelain]` | Stones, current changes, warnings. |
| `git stone list [--porcelain]` | One line per stone. |
| `git stone review` | Per stone: description, files, +/- counts; then the current changes. |
| `git stone diff <id\|current> [git diff args]` | A stone's diff, or the unstoned changes. Extra arguments go to `git diff`. |
| `git stone show <id>` | Header, description, stat and diff of one stone. |
| `git stone <id> commit [options]` | Commit the first stone. Accepts `-m`, `-F`, `-e`, `--no-verify`, `-S`, `--author`, `--date`, `-s`. |
| `git stone commit-all [--edit]` | Commit every stone in order, with the stone name as the message. |
| `git stone <id> drop` | Remove a boundary. No file changes. |
| `git stone repair [--apply]` | After the branch moved: show (or do) the replay of the stones onto the new HEAD. |
| `git stone adopt <branch\|detached>` | Move a chain to the branch you are on (after a rename, or after `git switch -c`). |
| `git stone prune [--apply] [--chain <ref>]` | List (or remove) chains of deleted branches and removed worktrees. |
| `git stone setup --agent <claude\|codex\|both> [--global]` | Add the AI-agent block to `CLAUDE.md` / `AGENTS.md` (this project, or with `--global` your own agent folder). |
| `git stone setup --update` | Refresh the block in every `CLAUDE.md` / `AGENTS.md` that already has it (this project, `~/.claude`, `~/.codex`). Creates nothing. `git stone status` tells you when a block is older than your install. |
| `git stone agents` | Print that block, to paste into any other tool. |

`create` also takes `--allow-empty` and `--force`. `git stone <id> <verb>` and `git stone <verb> <id>` both work.

## What happens if I use normal Git?

Stones live in refs, not in your working tree, so most Git commands do not affect them.

| You do | Result |
|---|---|
| `git add`, `restore`, `stash`, `reset --hard`, `clean` | Stones are untouched. After a `reset --hard`, bring work back with `git restore --source=refs/stones/heads/<branch>:trees/<id> --worktree -- .` |
| Make the commit yourself that equals the first stone | Recognised. The stone is marked as committed. |
| Any other commit, `pull`, `rebase`, `reset`, `amend`, `merge` | The branch moved. Commands that change stones refuse; reading still works. `git stone repair` puts the stones back on the new HEAD. |
| Merge or rebase in progress, or conflicts | `create`, `commit`, `repair --apply` refuse. Reading and `drop` work. |
| `git switch other` and back | Every branch has its own stones. Nothing is lost. |
| `git branch -m old new` | `git stone` warns; `git stone adopt old` moves the stones. |
| Detached HEAD | Works. Stones stay with that worktree. `git stone adopt detached` after `git switch -c`. |
| `git gc` | Live stones are kept. Dropped stones are collected once their reflog expires. |

Details and examples: [docs/git-interactions.md](docs/git-interactions.md).

## Limits

- **Stones are local.** `git push` and `git clone` do not carry them (`git push --mirror` would).
- **A snapshot is what `git add -A` would stage.** Ignored files are not in it. Files marked assume-unchanged or
  skip-worktree keep their index version. A submodule is recorded at its checked-out commit; uncommitted changes
  inside it are not captured (`git stone status` warns). Renames are not stored; diffs detect them.
- **Excluded paths are not in the stone** (`-x`, `--remember`). A tracked file that matches stays at its version from
  the previous stone, so your edits to it are not captured and `git reset --hard` cannot restore them from a stone.
  Rules live in this clone's `.git/config` (`stone.exclude`): they apply on every branch, and they are not pushed.
  Every stone prints what was left out, so a rule that matches nothing (`0 files`) is easy to spot.
- **The `post-index-change` hook** can fire on the temporary index used for a snapshot.
- **A big unignored folder** (for example `node_modules`) ends up in the snapshot. Making a stone warns when it adds
  more than 1000 new paths.
- **Only the first stone can be committed.** Commit it, or drop it to fold it into the next one.
- Stone names are one line (up to 200 bytes); descriptions up to 10000 bytes. Both are immutable once made.

## Speed

Measured on an Apple M5 laptop, one stone per task of 10 changed files, median times
(see [docs/design.md](docs/design.md#performance) and `bench/bench.sh`):

| Files in the repo | 10 | 1 000 | 5 000 | 20 000 |
|---|---:|---:|---:|---:|
| `git status` (for comparison) | 5 ms | 8 ms | 12 ms | 27 ms |
| `git stone <name>` | 77 ms | 120 ms | 127 ms | 144 ms |
| `git stone status` | 58 ms | 97 ms | 125 ms | 136 ms |
| `git stone list` | 33 ms | 32 ms | 38 ms | 32 ms |
| `git stone review` (100 stones) | 497 ms | 575 ms | 596 ms | 608 ms |
| `git stone 1 commit` | 86 ms | 118 ms | 166 ms | 155 ms |

Each command starts a handful of Git processes (about 4 ms each), which is most of the time in small repositories.
A stone costs about the same as `git add -A` on a copy of your index, so it grows with the number of files like
`git status` does, and an unchanged 100 MB binary adds nothing.

## Documentation

- [docs/design.md](docs/design.md): the design and the reasons behind it.
- [docs/internals.md](docs/internals.md): the exact ref and object layout, and how to read or recover stones with plain Git.
- [docs/git-interactions.md](docs/git-interactions.md): what every normal Git action does to your stones.
- [docs/ai-agents.md](docs/ai-agents.md): a short block to paste into an AI agent's memory (global or per project), so it makes stones on its own.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
bench/bench.sh        # needs a release build; see the script header
```

Git Stone has no runtime dependencies. All Git work is done by running the `git` binary with argument lists, and
only machine-readable (`-z` or fixed-format) output is parsed.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
