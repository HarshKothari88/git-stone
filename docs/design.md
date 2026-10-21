# Design

This document explains how Git Stone works and why it is built this way. For the exact on-disk layout and recovery
recipes see [internals.md](internals.md). For what normal Git commands do to stones see
[git-interactions.md](git-interactions.md).

## Contents

1. [Purpose](#purpose)
2. [The model](#the-model)
3. [Invariants](#invariants)
4. [Storage](#storage)
5. [Snapshots](#snapshots)
6. [Commands](#commands)
7. [When the branch moves](#when-the-branch-moves)
8. [Concurrency and crash safety](#concurrency-and-crash-safety)
9. [Design decisions](#design-decisions)
10. [Performance](#performance)
11. [Limits](#limits)
12. [Testing](#testing)

## Purpose

A developer, or an AI agent, works through many tasks on one branch. Getting atomic commits out of that today means
either committing after every task (which breaks the flow) or splitting everything at the end (slow by hand, and an
agent often deletes and rewrites code just to make the pieces atomic).

Git Stone records the boundary at the moment a task ends. Commits are made later, directly from the recorded
snapshots, without rewriting any code.

Every choice below serves one goal: **making a boundary instant, safe, and possible from any state the repository is
in.** Git Stone refuses only when going ahead could lose or misattribute work.

Non-goals: replacing branches, stashing, or version control of the stones themselves. Stones are local working notes,
not shared history.

## The model

```text
branch (never moved by making a stone)
refs/heads/feature/auth ──► H0 (the anchor)

stone state (one ref per branch)
refs/stones/heads/feature/auth ──► state commit
                                     ├─ parent: H0
                                     └─ tree
                                         ├─ meta
                                         ├─ trees/  1 ─► tree of the repository at the end of task 1
                                         │          2 ─► tree ... task 2
                                         └─ desc/   1 ─► description of Stone 1
```

- A **stone** is a snapshot of the whole working tree, stored as a Git *tree* object, with an id, a name and an
  optional description.
- The stones of one branch form a **chain**, ordered by creation. Stone *k* depends on the ones before it: its diff
  runs from the previous stone (or, for the first stone, from the **anchor**: the branch commit the stones were made on).
- The whole chain is one **state commit** behind **one ref**. Every change builds a new state commit and moves that ref
  with a compare-and-swap.

Why one ref holds everything: a single ref update is atomic in Git, so a chain can never be half-updated and no
multi-ref transaction is needed. A crash leaves the old state or the new one.

## Invariants

These hold in every command and are checked by tests.

| | Invariant |
|---|---|
| I1 | Making, listing, diffing or dropping a stone never changes the branch ref, the working tree or the real index, byte for byte. |
| I2 | A stone's tree never changes after it is made. Only `repair --apply`, run on purpose, writes new trees, and it says so. |
| I3 | The diff of stone *k* runs from the tree of the previous surviving stone (the anchor's tree for the first) to the tree of *k*. |
| I4 | Dropping removes a boundary only. No file changes; the next stone's diff grows to cover the dropped one. |
| I5 | Only the first surviving stone can be committed, and the commit's tree equals that stone's tree exactly. |
| I6 | Every stone in a chain was made while `HEAD` was the anchor. If the branch moved, commands that change stones refuse (except the one provably safe case below). |
| I7 | Ids go up and are never reused within a chain, even when the chain becomes empty. |
| I8 | Every Git call is an argument list, never a shell string. Only `-z` or fixed machine formats are parsed. |
| I9 | A chain belongs to the place it started: one branch, or the detached HEAD of one worktree. Switching away and back never loses stones, because they live in refs. |

## Storage

**A stone is a tree, not a commit.** Diffs are tree-to-tree and commits are made later from the tree, so a tree is all
that is needed. It is one concept fewer.

**State commit.** Its tree holds `meta` (chain order, names, ids, anchor), `trees/<id>` (the snapshots) and an optional
`desc/<id>` per described stone. Its only parent is the anchor commit, which keeps the anchor reachable even if the
branch is rebased or reset. Repair needs it for a three-way merge.

**No link between state versions.** If each state commit had the previous one as parent, dropped stones could never be
garbage collected. History lives in the ref's reflog instead, which expires normally. A dropped stone is recoverable
until then, and removable by `git gc` after.

**Reading is strict.** A wrong header, a stone without a snapshot or the reverse, ids out of order, or a branch that does
not match the ref it was read from give `stone state is corrupt` (exit 2). Nothing is guessed.

**Names and descriptions** come from untrusted input. A name is one line of at most 200 bytes with no control characters;
everything else is allowed, including Unicode, `$`, `;` and a leading `-`. A description is up to 10 000 bytes of text, may
span lines, and never contains NUL. It is stored as its own blob because it can be long and `meta` has one record per line.
Descriptions are shown with terminal control characters removed. Both are immutable once made. The description is not used as
a commit message: a stone commit uses `-m` or the stone name.

**Refs.** `refs/stones/heads/<branch>` mirrors `refs/heads/<branch>`. Git never allows `a` and `a/b` as branches together,
so the mirrored names cannot clash either. Every write creates the reflog explicitly, because Git keeps none for custom
namespaces by default.

**Object formats.** SHA-1 and SHA-256 are supported. Id lengths, the empty tree and the "ref must not exist" zero id are
derived from the repository.

## Snapshots

A snapshot must equal what `git add -A && git write-tree` would give, without touching the real index:

1. Copy the real index to a temporary file inside the worktree's git directory (same filesystem, and a split index still
   finds its shared files).
2. Run `git add -A` and `git write-tree` against the copy.
3. Delete the copy, also on error.

Because Git does the work, `.gitignore`, attributes, clean filters (CRLF, LFS), file modes, symlinks, deletions, new files
and submodule pointers all behave as in a normal `git add`. The copy keeps the file stat cache, so only changed files are
hashed. Sparse checkouts, split indexes, linked worktrees and a user-set `GIT_INDEX_FILE` were all verified to work.

All Git calls set `GIT_OPTIONAL_LOCKS=0`, so a read command never refreshes the real index behind the user's back. The
only exceptions are the steps of `commit` that write the real index on purpose.

## Commands

### Making a stone

`git stone "name" [-d "details"]`

Refuses when: the name or description is invalid; a merge, rebase, cherry-pick, revert or bisect is in progress; files
have merge conflicts (a stone would store conflict markers); the branch moved; nothing changed since the previous stone
(`--allow-empty` overrides); or stones exist and the working tree equals the branch commit, which usually means a stash
(`--force` overrides). It warns, without refusing, when the stone adds over 1000 new paths, which usually means a missing
`.gitignore` entry.

Any `HEAD` is fine: a branch, a detached HEAD, or a branch with no commits yet (the anchor is then `none`). With no stones
in the chain, the anchor is simply moved to the current `HEAD`, since nothing can be misattributed.

### Reading

`status`, `list`, `diff`, `show` and `review` never refuse because the branch moved: tree-to-tree diffs stay true. They
print a warning. `diff` hands its output and extra arguments straight to `git diff`, so the user's pager and colours apply.
`review` costs one `git diff-tree --numstat` per stone, which scales with changed paths, not with repository size.

`status --porcelain` and `list --porcelain` give versioned, tab-separated, NUL-terminated records for scripts and agents
(see [ai-agents.md](ai-agents.md)).

### Committing a stone

`git stone 1 commit -m "feat: add auth"` runs a real `git commit` against a temporary index rather than building a commit
with `git commit-tree`. That gives hooks, signing, identity, templates, the editor and trailers for free; re-implementing
them would be fragile.

Let *H0* be the anchor (equal to `HEAD`) and *S* the stone's tree.

1. Refuse during a merge or conflicts, if the stone is not the first, or if `index.lock` exists (the real index is
   updated after the commit, so a lock would strand it).
2. **Dry run.** On a copy of the real index, run `git read-tree -m -i H0 S`. A clash between the user's staged changes and
   the stone fails here, before anything changes.
3. **Commit.** Make a temporary index from a copy of the real index moved onto *S* with `git read-tree -m -i S`, check that
   `git write-tree` on it equals *S*, then run `git commit` on it with the user's terminal attached. Allowed options are
   `-m`, `-F`, `-e`, `--no-verify`, `-S`, `--author`, `--date`, `-s`.
4. Check that `HEAD`'s parent is *H0*; otherwise another process moved the branch, so stop and explain.
5. **Real index.** Run `git read-tree -m -i H0 HEAD`. This moves the user's index forward and is idempotent.
6. If `HEAD^{tree}` differs from *S*, a hook changed the commit. Warn and list the paths; the next stone's base becomes the
   real `HEAD` tree.
7. Remove the stone from the chain, set the anchor to `HEAD`, and save with a compare-and-swap.

An unborn branch works with the empty tree as *H0*, and the first commit is a root commit.

`commit-all [--edit]` repeats this for every stone in order, using the stone name as the message. It stops at the first
failure; commits already made stay made and recorded.

### Dropping

Removes the stone from the chain and keeps `next-id`. Only the state ref changes, so it works during a merge or rebase.
Like every command that changes stones, it refuses when the branch moved.

### Adopting

`git stone adopt <branch|detached>` moves a chain to the current scope after a branch rename or after `git switch -c` from
a detached HEAD. The tool never moves chains by itself, because it cannot know the intent. The source must have stones and
the target none. Ids are kept when they cannot collide with ids the target already used; otherwise they are renumbered and
the mapping is printed. Both refs change in one `update-ref --stdin` transaction, so a failure leaves the chain in one place
or the other, never lost.

### Pruning

`git stone prune [--apply] [--chain <ref>]` finds chains nothing can reach any more: the branch was deleted, or the linked
worktree was removed. Without `--chain`, only empty chains and stale temporary indexes are removed. A chain that still holds
stones is listed and removed only when named. Empty chains of branches that still exist are left alone, because removing
them would restart ids at 1 (I7). Each removal deletes the ref only if it still holds the listed value, and the command
prints the `update-ref` that restores it (deleting a ref also deletes its reflog).

## When the branch moves

Every command that has stones compares `HEAD` with the anchor.

| Situation | Meaning | Action |
|---|---|---|
| `HEAD` equals the anchor | normal | proceed |
| The first-parent commits from the anchor to `HEAD` have trees equal to stones 1..*k*, in order, each directly on the previous | the stones were committed (by us, interrupted, or by hand) | **reconcile**: move the real index forward, drop those stones, set the anchor, one compare-and-swap |
| Anything else (ordinary commit, pull, rebase, reset, amend, recreated branch) | the branch moved | reading warns; changing refuses |

Reconcile is limited to a first-parent walk of at most *stones + 1* commits, so a long history can never be mistaken for a
match, and any mismatch means "moved". It is also how an interrupted `git stone commit` heals itself.

### Repair

`git stone repair` replays the stones onto the new `HEAD` using Git's three-way merge:

```text
old_base = tree(anchor)        new_base = tree(HEAD)
for each stone S in order:
    merged   = merge-tree(base = old_base, ours = new_base, theirs = S)
    old_base = S               new_base = merged
```

Without `--apply` it only runs the merges (which write objects, nothing referenced) and prints the plan. With `--apply` it
saves the merged trees as new snapshots with the same ids, names, descriptions and times, and moves the anchor to `HEAD`.
The old state stays in the ref's reflog. Any conflict changes nothing and names the first conflicting stone and its paths.

Why this is right in every case: a commit of part of the work makes that part "already there" for the merge, so the stone
shrinks to the rest; a rebase onto new upstream work gives the stones the upstream changes, so committing them never reverts
upstream; a reset backwards keeps the stones' content on the older base. Only `repair` ever writes new trees (I2).

## Concurrency and crash safety

- The only mutable thing is the state ref. Every change is *read the old oid, write objects, compare-and-swap*. Two
  processes at once: one wins, the other is told to run again. States are never merged.
- Objects written before a failed swap are unreachable garbage for `git gc`. Git Stone never deletes objects.
- Temporary indexes are deleted by a guard, also on error. After a hard kill they remain; commands that change stones remove
  any older than one hour, and `prune` lists them.
- There is no moment where a kill leaves a half state, except the window in `commit` between the Git commit and the state
  update. That window self-heals through reconcile.
- Failure points for tests: setting `GIT_STONE_FAILPOINT` to `create:before-cas`, `drop:before-cas`, `repair:before-cas`,
  `commit:after-git-commit` or `commit:after-index-update` makes the process exit with code 99 at that point.

Exit codes: 0 done, 1 refused (the message says why, nothing changed), 2 internal or Git failure.

## Design decisions

What was chosen, what else was possible, and why.

| Decision | Alternative | Reason |
|---|---|---|
| A stone is a tree. | A commit per stone. | Trees are all a diff or a later commit needs. |
| All stones of a branch in one state commit behind one ref. | One ref per stone. | One ref update is atomic; per-stone refs need multi-ref transactions and can be left inconsistent by a crash. |
| No link from a state commit to the previous one. | Parent chain of states. | A chain would keep dropped stones reachable forever. The reflog keeps recoverable history and expires. |
| Snapshot with `git add -A` on a copy of the index. | Walk the working tree ourselves. | Git already applies ignore rules, filters, modes and submodules, and the copy keeps its stat cache. |
| Commit with `git commit` on a temporary index. | `git commit-tree`. | Hooks, signing, identity, templates and the editor come for free. |
| The commit's index is a copy of the real one moved onto the stone with a single-tree merge, checked to equal the stone. | `git read-tree <stone>` into an empty index. | A fresh index has no file stat data, so `git commit` re-hashes every file: seconds on a large repository instead of milliseconds. The copy keeps the stat data and the check keeps the commit exact. |
| Every `read-tree -m` uses `-i`. | Plain `read-tree -m`. | Without `-i`, a two-tree merge also compares index entries with files on disk. Entries written by an earlier `read-tree` carry no file stamps, so editing a file after committing the stone before it fails with "not uptodate". Git Stone never touches the working tree, so it asks Git not to look at it. Real clashes with staged changes are still found. |
| Repair uses `git merge-tree --write-tree` on trees directly. | Wrapping the trees in commits first. | `merge-tree` accepts trees for the merge base and both sides, and the empty tree works as a base. No temporary commits are written. Needs Git 2.40 or newer, checked only by `repair`. |
| Detached chains live in `refs/stones/detached/...`, one shared ref per worktree. | `refs/worktree/...`, which is per worktree by design. | `git gc` run in a different worktree does not treat another worktree's `refs/worktree/*` as roots, so a detached chain whose reflog had expired was collected. Shared refs are roots for every worktree. |
| A moved branch blocks changes; `repair` is explicit. | Silently rebase stones. | A snapshot rebased without the user knowing could revert upstream work or attribute it wrongly. |
| Descriptions are separate immutable blobs. | A line in `meta`. | They are long, multi-line and untrusted. |
| `adopt` is explicit. | Follow branch renames automatically. | The tool cannot know whether a missing branch was renamed or deleted. |
| Dropping never touches files. | Dropping reverts code. | A boundary is only a label. Reverting code is the user's job. |
| No runtime dependencies; only the `git` binary, with argument lists. | libgit2 or a Rust Git library. | Git itself is the reference for every behaviour above, and one small static binary is easy to install and audit. |

## Performance

Cost is dominated by starting `git` processes (about 4 ms each) plus, for a stone, one `git add -A` on a copy of the index.
Measured on an Apple M5 laptop, macOS, Git 2.53.0, with 100 stones of 10 changed files each, medians (`bench/bench.sh`):

| Repository size | 10 files | 1 000 files | 5 000 files | 20 000 files |
|---|---:|---:|---:|---:|
| `git status` (for comparison) | 4.7 ms | 8.3 ms | 12.4 ms | 26.5 ms |
| make a stone | 77.0 ms | 119.8 ms | 127.1 ms | 143.7 ms |
| `status` | 57.9 ms | 96.9 ms | 125.2 ms | 135.8 ms |
| `list` | 32.8 ms | 32.3 ms | 38.1 ms | 32.0 ms |
| `review` (100 stones) | 497.4 ms | 575.0 ms | 595.7 ms | 607.7 ms |
| `diff current` | 43.4 ms | 85.2 ms | 103.6 ms | 116.9 ms |
| `drop` | 52.0 ms | 52.3 ms | 49.6 ms | 51.2 ms |
| `commit` | 85.8 ms | 118.3 ms | 165.9 ms | 155.4 ms |

- `list` reads one ref and one blob. `drop` and `list` do not depend on the repository size at all.
- `review` scales with the number of stones and changed paths, never with the number of files in the repository.
- Making a stone and `status` grow with the file count the way `git status` does, but with a constant of about 15 process
  starts on top, which dominates small repositories.
- A 100 MB binary that has not changed adds nothing, because the stat cache skips it (1 000 files plus one such file: 108 ms to
  make a stone).

## Limits

- **Stones are local.** `git push` and `git clone` do not transfer them (`git push --mirror` would).
- **What a snapshot contains is what `git add -A` stages.** Ignored files are absent; assume-unchanged and skip-worktree
  files keep their index version; a submodule is recorded at its checked-out commit and uncommitted changes inside it are
  not captured (`status` warns); renames are not stored, diffs detect them.
- **The `post-index-change` hook** can fire on the temporary index. Hooks that assume it is the real index should check
  `GIT_INDEX_FILE`.
- **A conflicting repair cannot keep that chain, but it can be dropped.** Typically an agent kept editing after the stone and
  then ran a plain `git commit`, so the branch already holds a later version of the same lines. `drop <id>` and `drop all`
  work on a moved branch (they only delete records, never a file or a commit), and the conflict message says so. The other way
  out is to move the branch back to the anchor, where the stones are usable again.
- **A removed worktree's detached chain stays** under `refs/stones/detached/linked/<name>` until pruned.
- **A conflicting `index.lock`** taken by another process in the instant between the lock check and the index update in
  `commit` leaves the commit made and the index behind; the error prints the exact `git read-tree -m -i <H0> HEAD` to finish,
  and the next command also heals it.
- **Disk full** is handled as any failed Git call: exit 2, nothing changed, no temporary index left. A write that fails halfway
  through a file is not reproduced by the tests.

## Testing

The suite runs the real binary against real repositories in temporary directories, with a fixed identity and no user or system
Git configuration. Properties checked:

- **Invariant I1** for every command that must not change anything: a fingerprint of the branch ref, the index bytes and every
  file (ignored and untracked too) is compared before and after.
- **Every row of the interaction table** in [git-interactions.md](git-interactions.md), each ending with `git fsck` clean and the
  stones loading.
- **Crashes**, with the failpoints above, compared against a repository where the command was not interrupted.
- **Hostile input**: names and descriptions with shell syntax, control characters and Unicode.
- **Concurrency**: parallel `create` runs never corrupt or duplicate the chain.
- **Failures**: an unwritable object store or ref, a damaged state ref, stale temporary indexes.
- **Git behaviours the design relies on** (sparse checkout, split index, linked worktrees, signing, `merge-tree` exit codes and
  output, `gc` roots), each as a test, so a Git upgrade that changes them fails loudly.
