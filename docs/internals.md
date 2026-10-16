# Internals

Where Git Stone keeps its data, and how to read, recover or remove it with plain Git. Nothing here needs
`git-stone` itself. Every recipe was run against a real repository.

Replace `<branch>` with the branch name, for example `feature/auth`.

## Refs

| Ref | Holds |
|---|---|
| `refs/stones/heads/<branch>` | The stones of a branch. |
| `refs/stones/detached/main` | The stones made on a detached HEAD in the main worktree. |
| `refs/stones/detached/linked/<name>` | The same for a linked worktree. `<name>` is its directory under `.git/worktrees/`; every character outside `A-Za-z0-9_-` is written as `%XX`. |

The name mirrors the branch ref (`refs/heads/a/b` becomes `refs/stones/heads/a/b`). Git never allows `a` and
`a/b` as branches at the same time, so the mirrored names cannot clash either.

Each ref points at a **state commit**. Every change to a branch's stones writes a new state commit and moves the
ref with a compare-and-swap (`git update-ref <ref> <new> <old>`), so two processes can never both win.

## The state commit

```text
state commit
├─ parent: the anchor commit (the branch commit the stones were made on)
└─ tree
   ├─ meta            text blob
   ├─ trees/
   │  ├─ 1            tree: the whole repository at the end of Stone 1
   │  └─ 3            tree: ... Stone 3 (ids are never reused, so gaps are normal)
   └─ desc/           only when some stone has a description
      └─ 1            blob: the description of Stone 1, raw UTF-8
```

- The only parent is the anchor. This keeps the anchor reachable if the branch is rebased or reset.
- There is **no** link to the previous state commit. History lives in the ref's reflog instead, so dropped stones
  can be garbage collected once their reflog entries expire.
- The author and committer are always `git-stone <git-stone@localhost>`, so saving works with no Git identity
  configured. The message is `git-stone: <operation>`, for example `git-stone: create 2`.
- State commits are never signed.

### `meta`

```text
git-stone 1
branch refs/heads/feature/auth
anchor 1d5c4320357fe735c1a9e3d21a355e0550c1874a
next-id 5
stone 3 1792183987 add login
stone 4 1792183987 add route guard
```

| Line | Meaning |
|---|---|
| `git-stone 1` | Format version. |
| `branch` | The full branch ref, or the word `detached`. Must match the ref it was read from. |
| `anchor` | Full object id of the commit the stones are based on, or `none` on a branch with no commits yet. |
| `next-id` | The id the next stone gets. It only goes up, and survives an empty chain. |
| `stone <id> <unix seconds> <name>` | One per stone, in chain order. The name is the rest of the line. |

Reading is strict. A wrong header, a stone without a `trees/` entry (or the reverse), ids that are not increasing
or not below `next-id`, a description for an unknown stone, a branch that does not match: all give
`stone state is corrupt` and exit code 2. Nothing is guessed.

### The anchor tree

A stone's diff starts at the previous stone, or for the first stone at the **anchor tree**: the tree of the anchor
commit, or Git's empty tree when the anchor is `none` (for SHA-256 repositories the empty tree has a different id,
so it is always asked from Git).

## Reading it with plain Git

```sh
# every chain in the repository
git for-each-ref refs/stones/

# the chain's meta file
git cat-file -p refs/stones/heads/<branch>:meta

# the snapshot trees (one entry per stone)
git ls-tree refs/stones/heads/<branch>:trees

# the description of Stone 1, and which stones have one
git cat-file -p refs/stones/heads/<branch>:desc/1
git ls-tree refs/stones/heads/<branch>:desc

# what Stone 2 changed compared with Stone 1
git diff refs/stones/heads/<branch>:trees/1 refs/stones/heads/<branch>:trees/2

# history of the chain: one reflog entry per change
git reflog refs/stones/heads/<branch>

# the state commit itself
git cat-file -p refs/stones/heads/<branch>
```

## Recovering work

A stone is a complete copy of the working tree at that moment, so lost work comes back from it:

```sh
# after `git reset --hard` and `git clean -fd`: put Stone 2's files back
git restore --source=refs/stones/heads/<branch>:trees/2 --worktree -- .
```

Restored files appear as ordinary untracked or modified files. The index is not changed. To get a single file:

```sh
git restore --source=refs/stones/heads/<branch>:trees/2 --worktree -- path/to/file
```

## Undoing a drop, or an earlier state

Every change is in the reflog. To go back to how the stones were before the last operation:

```sh
git reflog refs/stones/heads/<branch>                       # find the entry
git update-ref refs/stones/heads/<branch> <oid-from-reflog>
```

Reflog entries expire like any other (90 days by default; unreachable ones after 30 days). After that, `git gc`
removes a dropped stone for good.

## Removing stone data

```sh
# all stone data of one branch
git update-ref -d refs/stones/heads/<branch>
```

The snapshot objects are removed by the next `git gc` after the reflog expires. `git stone prune` does this for
chains whose branch or worktree no longer exists.

## Temporary files

A snapshot copies the real index to `stone-index-<pid>-<nanos>-<counter>` inside the worktree's git directory,
runs `git add -A` and `git write-tree` on the copy, and deletes it. The real index is never written. A process
killed in between can leave the file behind; the next command that changes stones removes any older than one hour,
and `git stone prune` lists them.

## Object formats

SHA-1 and SHA-256 repositories are supported. Object id lengths and the all-zero id used for "this ref must not
exist yet" are derived from the repository, never hardcoded.
