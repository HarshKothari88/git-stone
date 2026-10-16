# Git Stone and normal Git

Stones are trees stored in refs. They are not in your working tree or in your index, and they do not move when your
branch moves. This page lists what each Git action does to them, with an example.

The one rule behind all of it: **a stone only means something while its branch is still where the stones were
made** (the *anchor*). If the branch moved, `git stone` reads freely but refuses to change or commit anything until
you run `git stone repair`.

## At a glance

| You do | Stones | `git stone` does |
|---|---|---|
| `git add`, `restore`, `restore --staged` | untouched | `status` recomputes the current changes |
| `git stash`, `stash pop` | untouched | when stones exist, refuses a stone whose tree equals the branch commit |
| `git commit` that equals the first stone | untouched | marks that stone as committed |
| Any other `git commit`, `amend`, `reset`, `rebase`, `pull`, `merge`, `cherry-pick`, `am` | untouched | the branch *moved*: changes refused, reads warn, `repair` |
| Merge, rebase or cherry-pick in progress, conflicts | untouched | `create`, `commit`, `repair --apply` refuse; reads and `drop` work |
| `git reset --hard`, `git clean` | untouched (your files are lost, the stones keep them) | restore from a stone |
| `git switch other` and back | each branch keeps its own chain | never shows another branch's stones |
| `git branch -m old new` | stays under the old name | warns; `adopt old` |
| Delete a branch and recreate it elsewhere | stays | treated as moved |
| Detached HEAD | own chain, per worktree | fully supported; `adopt detached` after `git switch -c` |
| Branch with no commits yet | anchor `none` | fully supported; the first stone commit is a root commit |
| `git worktree add` | per branch; detached chains per worktree | warns if one branch is checked out twice |
| `git gc` | live stones kept; dropped ones collected after the reflog expires | nothing |
| Git hooks | only `git stone commit` runs them | `--no-verify` is passed through |
| `git push`, `git clone` | not transferred | stones are local |

## Staging and restoring

```text
$ git add src/auth.ts
$ git restore --staged src/auth.ts
```

Neither touches a stone. `git stone status` always compares the working tree with the latest stone, so it reflects the
current files whatever is staged. Making a stone also ignores what is staged: a stone is the working tree as `git add -A`
would stage it.

## Stash

```text
$ git stash -u
$ git stone "after stash"
Your working tree matches the branch commit, not Stone 2. Did you stash? Use --force to stone it anyway.
```

A stash empties the working tree, so a stone made then would record "everything was removed". (This guard applies when stones already exist.) Git Stone refuses that by
default. `git stash pop` brings the work back and nothing was lost. `--force` records the stone anyway.

## You commit by hand

If the commits between the anchor and `HEAD` are exactly the first stones, in order, with trees equal to those stones,
the next `git stone` command that changes things marks them as committed. This also heals an interrupted
`git stone commit`.

```text
$ git add src/auth.ts && git commit -m "login"       # the same content as Stone 1
$ git stone "next task"
Note: Stone 1 matches a commit already on the branch; marked as committed.
Stone 2 created: next task
```

`git stone status` and `list` show a warning first. Anything that is not an exact match is a moved branch (below).

## The branch moves

An ordinary commit, `pull`, `rebase`, `reset`, `amend`, `merge`, `cherry-pick`, `am`, or a branch deleted and recreated
elsewhere. Reading still works, because diffs between trees stay true. Changing refuses:

```text
$ git stone "next"
Git Stone: branch `main` moved since the stones were made.
  Stones are based on commit b22c7ad.
  The branch is now at   65ba100.
Nothing was changed. Run `git stone repair` to see how to fix this.
```

`git stone repair` replays each stone onto the new `HEAD` with Git's own three-way merge. Without `--apply` it only checks:

```text
$ git stone repair
Git Stone: branch `main` moved since the stones were made.
  Stones are based on commit b22c7ad.
  The branch is now at   65ba100.

Repair puts the stones back on top of the branch's current commit, using Git's own 3-way merge.
The check found no conflicts.
This makes new snapshots for Stone 1. The old ones stay in the reflog of refs/stones/heads/main.
No files are changed.

Run `git stone repair --apply` to do it.
```

Why replaying is right in each case:

- **Someone committed part of your work:** the merge sees that part as already there, and the stone's diff shrinks to the rest.
- **You rebased onto new upstream work:** the stones gain the upstream changes, so committing them never reverts upstream.
- **You reset backwards:** the stones keep their own work on top of the older commit.

If a stone cannot be replayed, nothing changes. The first conflicting stone and its paths are named. Files are never touched
by `repair`, only the stone snapshots and the anchor.

## Merge, rebase, cherry-pick in progress

`create`, `commit`, `commit-all` and `repair --apply` refuse, because a snapshot taken mid-merge would hold conflict
markers and a commit would lose the merge parent. `status`, `list`, `show`, `diff`, `review` and `drop` keep working
(`drop` and `adopt` only edit stone refs). After `--abort` or `--continue` everything is as before.

## `reset --hard` and `clean`

Your files are gone; the stones are not.

```text
$ git reset --hard && git clean -fd
$ git restore --source=refs/stones/heads/feature/auth:trees/2 --worktree -- .
```

## Switching branches

Every branch has its own chain. Switching away never shows another branch's stones, and switching back finds yours.
Uncommitted files follow Git's normal rules for a switch; the stones are safe either way.

## Renaming a branch

`git branch -m old new` leaves the chain under the old name. `git stone status` warns, and `adopt` moves it:

```text
$ git stone adopt old
Moved 1 stone from branch `old` to branch `new`.
No files were changed.
```

## Detached HEAD

Stones work on a detached HEAD and stay with that worktree. If you then create a branch from it:

```text
$ git switch -c feat
$ git stone adopt detached
Moved 1 stone from detached HEAD to branch `feat`.
No files were changed.
```

`adopt` needs stones at the source and none at the target. Stone ids are kept when they cannot collide with ids the
target used before; otherwise they are renumbered and the mapping is printed (`Stone 1 is now Stone 5.`).

`git worktree remove` does not delete the worktree's detached chain; it stays under `refs/stones/detached/linked/<name>`
until `git stone prune` (or you) remove it.

## Several worktrees

Each worktree has its own index and files, so stones never mix. A branch's chain is shared by every worktree that has the
branch checked out (Git normally allows only one; `--force` allows more, and `status` warns).

## Garbage collection

Live stones are reachable from their ref, so `git gc` keeps them. A dropped stone stays reachable through the ref's reflog
until the entry expires, then `git gc` removes it. `git stone` never deletes objects itself.

## Hooks

Making a snapshot runs `git add` on a temporary index and runs no hooks, except `post-index-change`, which can fire on
that temporary index. `git stone commit` runs a normal `git commit`, so `pre-commit`, `commit-msg`, `prepare-commit-msg` and
`post-commit` run, and signing, identity and templates work. If a hook changes what is committed (a formatter that re-adds
files), you get a warning listing the paths; the next stone starts from the real `HEAD`.

## Staged changes during `git stone commit`

Your staged work is kept. Before committing, Git Stone checks on a copy of your index that your staged changes do not clash
with the stone; if they do, nothing happens and you are told which paths to unstage. The commit itself contains exactly the
stone, never your staged work.

## Interrupted commands

A `git stone commit` killed between the Git commit and the state update leaves a branch commit equal to the first stone. The
next command that changes things recognises it (see "You commit by hand") and finishes the job. Any other crash leaves either
the old state or the new one, because every change is one ref update.
